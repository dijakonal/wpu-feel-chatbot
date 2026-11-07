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

/// The text in one answer object: `choices[0].delta.content` (stream) or
/// `choices[0].message.content` (whole answer); `content` may also be a list of parts
/// (`[{"type":"text","text":"…"}]`).
pub fn text_of(v: &Value) -> String {
    let choice = &v["choices"][0];
    let content = [&choice["delta"]["content"], &choice["message"]["content"], &choice["text"]]
        .into_iter()
        .find(|c| !c.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    match content {
        Value::String(t) => t,
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str().or(p.as_str())).collect(),
        _ => String::new(),
    }
}

/// Decoder for Server-Sent Events: collects the text pieces of `data:` lines.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    pub done: bool,
    /// Whether any `data:` line came at all (if not, the answer was plain JSON).
    pub saw_data: bool,
    /// The raw answer (up to 1 MB) – parsed as plain JSON if it was no stream, and shown
    /// shortened if no text could be found at all.
    pub raw: Vec<u8>,
}

impl SseParser {
    /// Feed bytes; returns the text pieces found in complete lines.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<String>> {
        if self.raw.len() < 1 << 20 {
            self.raw.extend_from_slice(data);
        }
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(payload) = line.strip_prefix("data:").map(str::trim) else { continue };
            self.saw_data = true;
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
            let t = text_of(&v);
            if !t.is_empty() {
                out.push(t);
            }
        }
        Ok(out)
    }
}

/// Send `messages` (`(role, content)`, roles "system"/"user"/"assistant") and stream the answer.
/// If the streamed answer brings no text (some models do not stream), it is asked once more
/// without streaming.
pub fn stream_chat(api: &str, token: &str, model: &str, messages: &[(String, String)], mut on_text: impl FnMut(&str)) -> Result<()> {
    if token.trim().is_empty() {
        bail!("Free models need a GitHub token: add one in Settings → Free models (GitHub).");
    }
    if request(api, token, model, messages, true, &mut on_text)?.got_text {
        return Ok(());
    }
    let second = request(api, token, model, messages, false, &mut on_text)?;
    if second.got_text {
        return Ok(());
    }
    bail!(
        "{model} sent no text (answer: {}, \"{}\"). Try again or pick another model.",
        second.content_type,
        String::from_utf8_lossy(&second.raw).chars().filter(|c| !c.is_control()).take(160).collect::<String>()
    )
}

struct Outcome {
    got_text: bool,
    content_type: String,
    raw: Vec<u8>,
}

fn request(api: &str, token: &str, model: &str, messages: &[(String, String)], stream: bool, on_text: &mut impl FnMut(&str)) -> Result<Outcome> {
    let msgs: Vec<Value> = messages.iter().map(|(role, content)| json!({"role": role, "content": content})).collect();
    let body = json!({"model": model, "messages": msgs, "stream": stream});
    let mut resp = client(300)?
        .post(format!("{}/inference/chat/completions", base(api)))
        .header("Authorization", format!("Bearer {}", token.trim()))
        // as in GitHub's documentation, also for streaming
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .body(body.to_string())
        .send()
        .map_err(|e| if e.is_connect() || e.is_timeout() { anyhow!("Could not reach GitHub Models.") } else { anyhow!(e) })?;
    let status = resp.status().as_u16();
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("no type").to_string();
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
    // not a stream: the whole answer as one JSON object
    if !got_text && !parser.saw_data {
        if let Ok(v) = serde_json::from_slice::<Value>(&parser.raw) {
            if let Some(e) = v["error"]["message"].as_str() {
                bail!("GitHub Models: {e}");
            }
            let t = text_of(&v);
            if !t.is_empty() {
                got_text = true;
                on_text(&t);
            }
        }
    }
    Ok(Outcome { got_text, content_type, raw: parser.raw })
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
    fn reads_every_answer_shape() {
        // whole answer (no stream), content as text
        let whole: Value = serde_json::from_str(r#"{"choices":[{"message":{"role":"assistant","content":"Hallo"}}]}"#).unwrap();
        assert_eq!(text_of(&whole), "Hallo");
        // content as a list of parts
        let parts: Value = serde_json::from_str(r#"{"choices":[{"delta":{"content":[{"type":"text","text":"Hal"},{"type":"text","text":"lo"}]}}]}"#).unwrap();
        assert_eq!(text_of(&parts), "Hallo");
        // filter results / usage only
        let none: Value = serde_json::from_str(r#"{"choices":[],"prompt_filter_results":[]}"#).unwrap();
        assert_eq!(text_of(&none), "");
        // Windows line endings
        let mut p = SseParser::default();
        let t = p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\r\n\r\ndata: [DONE]\r\n").unwrap();
        assert_eq!(t, ["ok"]);
        assert!(p.done && p.saw_data);
    }

    /// Needs the stand-in server: `cargo test -- --ignored live_models_shapes`
    #[test]
    #[ignore]
    fn live_models_shapes() {
        let api = "http://127.0.0.1:8777";
        let msgs = vec![("user".to_string(), "Hallo?".to_string())];
        for model in ["openai/gpt-4.1-mini", "mistral-ai/codestral-2501", "meta/llama-3.3-70b-instruct"] {
            let mut text = String::new();
            stream_chat(api, "t", model, &msgs, |t| text.push_str(t)).unwrap();
            println!("{model}: {text}");
            assert!(text.contains("Hallo?"), "{model}: {text}");
        }
        let e = stream_chat(api, "t", "microsoft/phi-4", &msgs, |_| {}).unwrap_err().to_string();
        println!("phi-4: {e}");
        assert!(e.contains("sent no text") && e.contains("application/json"), "{e}");
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
