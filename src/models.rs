//! Free AI models from a provider with an OpenAI-compatible API (Groq, OpenRouter, Mistral,
//! Google Gemini, or any other), as an alternative to the AIS.chat class bot.
//!
//! `POST {base}/chat/completions` with `stream: true` answers with Server-Sent Events
//! (`data: {"choices":[{"delta":{"content":"…"}}]}` … `data: [DONE]`); `GET {base}/models` lists
//! the models. Each provider needs its own API key (free sign-up, daily limits).

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};

/// A provider with an OpenAI-compatible API.
#[derive(Debug, PartialEq)]
pub struct Provider {
    /// stored in the settings
    pub id: &'static str,
    pub name: &'static str,
    /// API base (empty for "other": the user enters it)
    pub base: &'static str,
    /// where to get a free key
    pub key_url: &'static str,
    /// shown when the model list cannot be read
    pub suggested: &'static [&'static str],
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "groq",
        name: "Groq",
        base: "https://api.groq.com/openai/v1",
        key_url: "https://console.groq.com/keys",
        suggested: &["llama-3.3-70b-versatile", "llama-3.1-8b-instant"],
    },
    Provider {
        id: "openrouter",
        name: "OpenRouter",
        base: "https://openrouter.ai/api/v1",
        key_url: "https://openrouter.ai/settings/keys",
        suggested: &["meta-llama/llama-3.3-70b-instruct:free"],
    },
    Provider {
        id: "mistral",
        name: "Mistral",
        base: "https://api.mistral.ai/v1",
        key_url: "https://console.mistral.ai/api-keys",
        suggested: &["codestral-latest", "mistral-small-latest"],
    },
    Provider {
        id: "gemini",
        name: "Google Gemini",
        base: "https://generativelanguage.googleapis.com/v1beta/openai",
        key_url: "https://aistudio.google.com/apikey",
        suggested: &["gemini-2.5-flash", "gemini-2.0-flash"],
    },
    Provider { id: "custom", name: "Other (OpenAI-compatible)", base: "", key_url: "", suggested: &[] },
];

/// The provider with this id; Groq if unknown or empty.
pub fn provider(id: &str) -> &'static Provider {
    PROVIDERS.iter().find(|p| p.id == id).unwrap_or(&PROVIDERS[0])
}

/// A chat model of the provider.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// what the API expects, e.g. `llama-3.3-70b-versatile`
    pub id: String,
}

/// A readable name for a model id: `meta-llama/llama-3.3-70b-instruct:free` →
/// `Llama 3.3 70B Instruct`, `gemini-2.5-flash` → `Gemini 2.5 Flash`.
pub fn pretty_name(id: &str) -> String {
    let base = id.rsplit('/').next().unwrap_or(id);
    let base = base.split(':').next().unwrap_or(base);
    base.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let l = w.to_lowercase();
            let is_size = l.len() > 1 && l.ends_with('b') && l[..l.len() - 1].chars().all(|c| c.is_ascii_digit() || c == '.');
            match l.as_str() {
                _ if is_size => l.to_uppercase(),
                "gpt" | "oss" | "it" | "vl" => l.to_uppercase(),
                _ => {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                }
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Short labels for the model menu: "free", "code" (made for programming), "fast".
pub fn model_tags(id: &str) -> Vec<&'static str> {
    let l = id.to_lowercase();
    let mut tags = Vec::new();
    if l.ends_with(":free") {
        tags.push("free");
    }
    if ["coder", "codestral", "devstral", "code-"].iter().any(|w| l.contains(w)) {
        tags.push("code");
    }
    if ["instant", "flash", "mini", "-8b", "small"].iter().any(|w| l.contains(w)) {
        tags.push("fast");
    }
    tags
}

pub fn suggested_models(p: &Provider) -> Vec<Model> {
    p.suggested.iter().map(|id| Model { id: id.to_string() }).collect()
}

fn client(timeout: u64) -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout))
        .build()?)
}

fn unreachable(e: reqwest::Error, provider: &str) -> anyhow::Error {
    if e.is_connect() || e.is_timeout() {
        anyhow!("Could not reach {provider}. Check the internet connection (and the address in Settings).")
    } else {
        anyhow!(e)
    }
}

/// The provider's chat models (`GET {base}/models`).
pub fn fetch_models(base: &str, key: &str, provider_id: &str) -> Result<Vec<Model>> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("Enter the provider's address in Settings.");
    }
    let mut req = client(30)?.get(format!("{base}/models"));
    if !key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", key.trim()));
    }
    let resp = req.send().map_err(|e| unreachable(e, provider(provider_id).name))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(describe_error(status, &text, "the model list", provider(provider_id).name));
    }
    parse_models(&text, provider_id).ok_or_else(|| anyhow!("unexpected answer from the model list"))
}

/// Chat models from a `/models` answer (`{"data":[{"id":…}]}` or a plain list): without
/// speech, image, embedding and safety models; at OpenRouter only the free ones.
pub fn parse_models(text: &str, provider_id: &str) -> Option<Vec<Model>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let list = v["data"].as_array().or(v["models"].as_array()).or(v.as_array())?;
    const NOT_CHAT: &[&str] = &["embed", "whisper", "tts", "guard", "moderation", "rerank", "transcribe", "image", "imagen", "veo", "aqa", "orpheus", "playai"];
    let mut models: Vec<Model> = list
        .iter()
        .filter_map(|m| m["id"].as_str().or(m["name"].as_str()).or(m.as_str()))
        .map(|id| id.trim_start_matches("models/").to_string())
        .filter(|id| {
            let l = id.to_lowercase();
            !NOT_CHAT.iter().any(|w| l.contains(w))
        })
        .filter(|id| provider_id != "openrouter" || id.ends_with(":free"))
        .map(|id| Model { id })
        .collect();
    models.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    models.dedup();
    (!models.is_empty()).then_some(models)
}

/// Readable message for an error answer of the API.
fn describe_error(status: u16, body: &str, model: &str, provider: &str) -> anyhow::Error {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = v["error"]["message"].as_str().or(v["message"].as_str()).or(v[0]["error"]["message"].as_str()).unwrap_or("").to_string();
    let lower = detail.to_lowercase();
    match status {
        401 => anyhow!("{provider} does not accept the API key. Check it in Settings → Free AI models."),
        403 => anyhow!("{provider} refused the request with this key ({}).", detail.chars().take(120).collect::<String>()),
        404 => anyhow!("{provider} does not know {model}. Pick another model in the list."),
        413 => anyhow!("The message is too long for {model}. Attach fewer files or start a new chat."),
        429 => anyhow!("The free limit at {provider} is used up for now. Pick another model in the list, or try again later."),
        _ if lower.contains("context") || (lower.contains("token") && lower.contains("max")) => {
            anyhow!("The conversation is too long for {model}. Start a new chat or pick a model with more room.")
        }
        _ => anyhow!("{provider} answered HTTP {status}. {}", detail.chars().take(200).collect::<String>()),
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
                bail!("{e}");
            }
            let t = text_of(&v);
            if !t.is_empty() {
                out.push(t);
            }
        }
        Ok(out)
    }
}

/// Where and how to send: the provider's API base, key and the model.
pub struct Target<'a> {
    pub base: &'a str,
    pub key: &'a str,
    pub model: &'a str,
    /// for messages, e.g. "Groq"
    pub provider: &'a str,
}

/// Send `messages` (`(role, content)`, roles "system"/"user"/"assistant") and stream the answer.
/// If the streamed answer brings no text (some models do not stream), it is asked once more
/// without streaming; if that brings none either, the error says what came back.
pub fn stream_chat(t: &Target, messages: &[(String, String)], mut on_text: impl FnMut(&str)) -> Result<()> {
    if t.key.trim().is_empty() {
        bail!("{} needs an API key: add one in Settings → Free AI models.", t.provider);
    }
    if t.base.trim().is_empty() {
        bail!("Enter the provider's address in Settings → Free AI models.");
    }
    if request(t, messages, true, &mut on_text)?.got_text {
        return Ok(());
    }
    let second = request(t, messages, false, &mut on_text)?;
    if second.got_text {
        return Ok(());
    }
    bail!(
        "{} sent no text (answer: {}, \"{}\"). Try again or pick another model.",
        t.model,
        second.content_type,
        String::from_utf8_lossy(&second.raw).chars().filter(|c| !c.is_control()).take(160).collect::<String>()
    )
}

struct Outcome {
    got_text: bool,
    content_type: String,
    raw: Vec<u8>,
}

fn request(t: &Target, messages: &[(String, String)], stream: bool, on_text: &mut impl FnMut(&str)) -> Result<Outcome> {
    let msgs: Vec<Value> = messages.iter().map(|(role, content)| json!({"role": role, "content": content})).collect();
    let body = json!({"model": t.model, "messages": msgs, "stream": stream});
    let mut resp = client(300)?
        .post(format!("{}/chat/completions", t.base.trim().trim_end_matches('/')))
        .header("Authorization", format!("Bearer {}", t.key.trim()))
        .header("Accept", if stream { "text/event-stream" } else { "application/json" })
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| unreachable(e, t.provider))?;
    let status = resp.status().as_u16();
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("no type").to_string();
    if !(200..300).contains(&status) {
        let text = resp.text().unwrap_or_default();
        return Err(describe_error(status, &text, t.model, t.provider));
    }
    let mut parser = SseParser::default();
    let mut buf = [0u8; 4096];
    let mut got_text = false;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for piece in parser.feed(&buf[..n]).map_err(|e| anyhow!("{}: {e}", t.provider))? {
            got_text = true;
            on_text(&piece);
        }
        if parser.done {
            break;
        }
    }
    // a final line without a line break
    for piece in parser.feed(b"\n").map_err(|e| anyhow!("{}: {e}", t.provider))? {
        got_text = true;
        on_text(&piece);
    }
    // not a stream: the whole answer as one JSON object
    if !got_text && !parser.saw_data {
        if let Ok(v) = serde_json::from_slice::<Value>(&parser.raw) {
            if let Some(e) = v["error"]["message"].as_str() {
                bail!("{}: {e}", t.provider);
            }
            let piece = text_of(&v);
            if !piece.is_empty() {
                got_text = true;
                on_text(&piece);
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
        let whole: Value = serde_json::from_str(r#"{"choices":[{"message":{"role":"assistant","content":"Hallo"}}]}"#).unwrap();
        assert_eq!(text_of(&whole), "Hallo");
        let parts: Value = serde_json::from_str(r#"{"choices":[{"delta":{"content":[{"type":"text","text":"Hal"},{"type":"text","text":"lo"}]}}]}"#).unwrap();
        assert_eq!(text_of(&parts), "Hallo");
        let none: Value = serde_json::from_str(r#"{"choices":[],"prompt_filter_results":[]}"#).unwrap();
        assert_eq!(text_of(&none), "");
        let mut p = SseParser::default();
        let t = p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\r\n\r\ndata: [DONE]\r\n").unwrap();
        assert_eq!(t, ["ok"]);
        assert!(p.done && p.saw_data);
    }

    #[test]
    fn stream_errors_are_reported() {
        let mut p = SseParser::default();
        let e = p.feed(b"data: {\"error\":{\"message\":\"Rate limit reached\"}}\n").unwrap_err();
        assert!(e.to_string().contains("Rate limit"));
    }

    #[test]
    fn reads_model_lists() {
        let groq = r#"{"object":"list","data":[
            {"id":"llama-3.3-70b-versatile","object":"model"},
            {"id":"whisper-large-v3","object":"model"},
            {"id":"meta-llama/llama-guard-4-12b","object":"model"},
            {"id":"playai-tts","object":"model"},
            {"id":"llama-3.1-8b-instant","object":"model"}]}"#;
        let ids: Vec<String> = parse_models(groq, "groq").unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["llama-3.1-8b-instant", "llama-3.3-70b-versatile"], "chat models only");

        let openrouter = r#"{"data":[{"id":"openai/gpt-5"},{"id":"meta-llama/llama-3.3-70b-instruct:free"}]}"#;
        let ids: Vec<String> = parse_models(openrouter, "openrouter").unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["meta-llama/llama-3.3-70b-instruct:free"], "only free models at OpenRouter");

        let gemini = r#"{"data":[{"id":"models/gemini-2.5-flash"},{"id":"models/text-embedding-004"}]}"#;
        let ids: Vec<String> = parse_models(gemini, "gemini").unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["gemini-2.5-flash"]);

        assert!(parse_models("OK", "groq").is_none());
    }

    #[test]
    fn explains_errors() {
        assert!(describe_error(429, "{}", "m", "Groq").to_string().contains("limit"));
        assert!(describe_error(401, "{}", "m", "Groq").to_string().contains("API key"));
        assert!(describe_error(404, "{}", "x", "Groq").to_string().contains("does not know x"));
        assert!(describe_error(400, r#"{"error":{"message":"context length exceeded"}}"#, "m", "Groq").to_string().contains("too long"));
    }

    #[test]
    fn readable_names_and_tags() {
        assert_eq!(pretty_name("llama-3.3-70b-versatile"), "Llama 3.3 70B Versatile");
        assert_eq!(pretty_name("meta-llama/llama-3.3-70b-instruct:free"), "Llama 3.3 70B Instruct");
        assert_eq!(pretty_name("qwen/qwen3-32b"), "Qwen3 32B");
        assert_eq!(pretty_name("gemini-2.5-flash"), "Gemini 2.5 Flash");
        assert_eq!(pretty_name("openai/gpt-oss-120b"), "GPT OSS 120B");
        assert_eq!(pretty_name("gemma2-9b-it"), "Gemma2 9B IT");
        assert_eq!(model_tags("qwen/qwen-2.5-coder-32b-instruct:free"), ["free", "code"]);
        assert_eq!(model_tags("llama-3.1-8b-instant"), ["fast"]);
        assert_eq!(model_tags("codestral-latest"), ["code"]);
        assert!(model_tags("llama-3.3-70b-versatile").is_empty());
    }

    #[test]
    fn providers() {
        assert_eq!(provider("").id, "groq", "Groq is the default");
        assert_eq!(provider("mistral").base, "https://api.mistral.ai/v1");
        assert!(PROVIDERS.iter().all(|p| p.id == "custom" || p.base.starts_with("https://")));
    }

    /// Needs the stand-in server: `cargo test -- --ignored live_models_shapes`
    #[test]
    #[ignore]
    fn live_models_shapes() {
        let msgs = vec![("user".to_string(), "Hallo?".to_string())];
        for model in ["openai/gpt-4.1-mini", "mistral-ai/codestral-2501", "meta/llama-3.3-70b-instruct"] {
            let mut text = String::new();
            let t = Target { base: "http://127.0.0.1:8777/inference", key: "k", model, provider: "Test" };
            stream_chat(&t, &msgs, |p| text.push_str(p)).unwrap();
            assert!(text.contains("Hallo?"), "{model}: {text}");
        }
    }
}
