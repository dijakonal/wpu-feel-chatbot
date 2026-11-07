//! Free models via GitHub Models (https://models.github.ai), as an alternative to AIS.chat.
//!
//! The API is OpenAI-compatible: `POST /inference/chat/completions` with `stream: true` answers
//! with Server-Sent Events (`data: {"choices":[{"delta":{"content":"…"}}]}` … `data: [DONE]`).
//! A GitHub token with the "Models: Read" permission is needed; free use has daily limits.

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};

const DEFAULT_API: &str = "https://models.github.ai";

/// A chat model from the GitHub Models catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// e.g. `openai/gpt-4.1-mini` – what the API expects
    pub id: String,
    /// e.g. `OpenAI GPT-4.1-mini`
    pub name: String,
    /// "low" / "high" / "custom": how many requests per day are free (low = the most)
    pub tier: String,
}

/// Used when the catalog cannot be read: models that are good at code.
pub fn fallback_models() -> Vec<Model> {
    [
        ("openai/gpt-4.1-mini", "OpenAI GPT-4.1 mini", "low"),
        ("openai/gpt-4.1", "OpenAI GPT-4.1", "high"),
        ("openai/gpt-4o-mini", "OpenAI GPT-4o mini", "low"),
        ("mistral-ai/codestral-2501", "Codestral 25.01", "low"),
        ("deepseek/deepseek-v3-0324", "DeepSeek-V3", "high"),
        ("meta/llama-3.3-70b-instruct", "Llama 3.3 70B Instruct", "high"),
        ("microsoft/phi-4", "Phi-4", "low"),
    ]
    .iter()
    .map(|(id, name, tier)| Model { id: id.to_string(), name: name.to_string(), tier: tier.to_string() })
    .collect()
}

fn base(api: &str) -> String {
    let a = api.trim().trim_end_matches('/');
    if a.is_empty() { DEFAULT_API.to_string() } else { a.to_string() }
}

fn client(timeout: u64) -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout))
        .build()?)
}

/// The chat models of the catalog (text in, text out), sorted by name.
pub fn fetch_catalog(api: &str, token: &str) -> Result<Vec<Model>> {
    let mut req = client(30)?.get(format!("{}/catalog/models", base(api))).header("Accept", "application/vnd.github+json");
    if !token.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", token.trim()));
    }
    let text = req.send()?.error_for_status()?.text()?;
    parse_catalog(&text).ok_or_else(|| anyhow!("unexpected answer from the model catalog"))
}

pub fn parse_catalog(text: &str) -> Option<Vec<Model>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let has = |m: &Value, key: &str, what: &str| m[key].as_array().map(|a| a.iter().any(|x| x == what)).unwrap_or(false);
    let mut models: Vec<Model> = v
        .as_array()?
        .iter()
        .filter(|m| has(m, "supported_input_modalities", "text") && has(m, "supported_output_modalities", "text"))
        .filter(|m| !m["id"].as_str().unwrap_or("").contains("embedding"))
        .filter_map(|m| {
            Some(Model {
                id: m["id"].as_str()?.to_string(),
                name: m["name"].as_str().unwrap_or(m["id"].as_str()?).to_string(),
                tier: m["rate_limit_tier"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect();
    models.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    (!models.is_empty()).then_some(models)
}

/// Readable message for an error answer of the API.
fn describe_error(status: u16, body: &str, model: &str) -> anyhow::Error {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("").to_string();
    match status {
        401 => anyhow!("GitHub does not accept the token. Check it in Settings → Free models (GitHub)."),
        403 => anyhow!("The GitHub token may not use GitHub Models – give it the permission \"Models: Read\"."),
        404 | 400 if detail.to_lowercase().contains("unknown model") || status == 404 => {
            anyhow!("GitHub Models does not know the model {model}. Pick another one in the list.")
        }
        413 => anyhow!("The message is too long for {model}. Attach fewer files or start a new chat."),
        429 => anyhow!("The free limit for {model} is used up for now. Pick another model in the list, or try again later."),
        _ if detail.to_lowercase().contains("token") && detail.to_lowercase().contains("max") => {
            anyhow!("The conversation is too long for {model}. Start a new chat or pick a model with more room.")
        }
        _ => anyhow!("GitHub Models answered HTTP {status}. {}", detail.chars().take(200).collect::<String>()),
    }
}

/// Decoder for Server-Sent Events: collects the text pieces of `data:` lines.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    pub done: bool,
}

impl SseParser {
    /// Feed bytes; returns the text pieces found in complete lines.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<String>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(payload) = line.strip_prefix("data:").map(str::trim) else { continue };
            if payload == "[DONE]" {
                self.done = true;
                continue;
            }
            let v: Value = match serde_json::from_str(payload) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(e) = v["error"]["message"].as_str() {
                bail!("GitHub Models: {e}");
            }
            if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                if !t.is_empty() {
                    out.push(t.to_string());
                }
            }
        }
        Ok(out)
    }
}

/// Send `messages` (`(role, content)`, roles "system"/"user"/"assistant") and stream the answer.
pub fn stream_chat(api: &str, token: &str, model: &str, messages: &[(String, String)], mut on_text: impl FnMut(&str)) -> Result<()> {
    if token.trim().is_empty() {
        bail!("Free models need a GitHub token: add one in Settings → Free models (GitHub).");
    }
    let msgs: Vec<Value> = messages.iter().map(|(role, content)| json!({"role": role, "content": content})).collect();
    let body = json!({"model": model, "messages": msgs, "stream": true});
    let mut resp = client(300)?
        .post(format!("{}/inference/chat/completions", base(api)))
        .header("Authorization", format!("Bearer {}", token.trim()))
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .body(body.to_string())
        .send()
        .map_err(|e| if e.is_connect() || e.is_timeout() { anyhow!("Could not reach GitHub Models.") } else { anyhow!(e) })?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().unwrap_or_default();
        return Err(describe_error(status, &text, model));
    }
    let mut parser = SseParser::default();
    let mut buf = [0u8; 4096];
    let mut got_text = false;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for t in parser.feed(&buf[..n])? {
            got_text = true;
            on_text(&t);
        }
        if parser.done {
            break;
        }
    }
    // a final line without a line break
    for t in parser.feed(b"\n")? {
        got_text = true;
        on_text(&t);
    }
    if !got_text {
        bail!("{model} sent an empty answer. Try again or pick another model.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_sse_in_any_chunking() {
        let stream = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\"Hallo\"}}]}\n\n\
                      : keep-alive\n\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\", 🐍 Welt\"}}]}\n\n\
                      data: {\"choices\":[],\"usage\":{\"total_tokens\":9}}\n\n\
                      data: [DONE]\n\n";
        for size in [1, 2, 7, 4096] {
            let mut p = SseParser::default();
            let mut text = String::new();
            for chunk in stream.as_bytes().chunks(size) {
                for t in p.feed(chunk).unwrap() {
                    text.push_str(&t);
                }
            }
            assert_eq!(text, "Hallo, 🐍 Welt", "chunk size {size}");
            assert!(p.done);
        }
    }

    #[test]
    fn stream_errors_are_reported() {
        let mut p = SseParser::default();
        let e = p.feed(b"data: {\"error\":{\"message\":\"Rate limit reached\"}}\n").unwrap_err();
        assert!(e.to_string().contains("Rate limit"));
    }

    #[test]
    fn reads_the_catalog() {
        let json = r#"[
            {"id":"openai/gpt-4.1-mini","name":"OpenAI GPT-4.1-mini","rate_limit_tier":"low",
             "supported_input_modalities":["text","image"],"supported_output_modalities":["text"]},
            {"id":"openai/text-embedding-3-small","name":"Embedding","rate_limit_tier":"embeddings",
             "supported_input_modalities":["text"],"supported_output_modalities":["embeddings"]},
            {"id":"mistral-ai/codestral-2501","name":"Codestral 25.01","rate_limit_tier":"low",
             "supported_input_modalities":["text"],"supported_output_modalities":["text"]}
        ]"#;
        let models = parse_catalog(json).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["mistral-ai/codestral-2501", "openai/gpt-4.1-mini"], "chat models only, sorted by name");
        assert!(parse_catalog("OK").is_none());
    }

    #[test]
    fn explains_errors() {
        assert!(describe_error(429, "{}", "m").to_string().contains("limit"));
        assert!(describe_error(403, "{}", "m").to_string().contains("Models: Read"));
        assert!(describe_error(401, "{}", "m").to_string().contains("token"));
        assert!(describe_error(404, r#"{"error":{"message":"Unknown model: x"}}"#, "x").to_string().contains("does not know"));
    }

    #[test]
    fn fallback_list_is_not_empty() {
        assert!(fallback_models().iter().any(|m| m.id == "openai/gpt-4.1-mini"));
    }
}
