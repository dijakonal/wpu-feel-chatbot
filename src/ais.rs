//! Talking to AIS.chat's shared-character dialog endpoint.
//!
//! The site is a Next.js app. Sending a message is a "server action" POST whose body is a
//! JSON array and whose answer is a React "Flight" stream (`text/x-component`). This module
//! builds the request and decodes the streamed answer. It is an unofficial client: the values
//! `next-action` / `x-deployment-id` change whenever AIS.chat is redeployed.

use std::{io::Read, time::Duration};

use anyhow::{anyhow, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};

use crate::settings::Settings;

// ------------------------------------------------------------------ chat model

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    User,
    Assistant,
}

/// What an attached file turned into.
#[derive(Clone, Debug)]
pub enum AttachmentData {
    /// Text file whose contents were pasted into the message (works without any upload endpoint).
    Inline(String),
    /// File uploaded to AIS.chat; this is the id the server gave back (sent in `fileIds`).
    Uploaded(String),
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub data: AttachmentData,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub role: Role,
    /// Exactly what is sent to the server (including pasted file contents).
    pub content: String,
    /// What to show in the window instead of `content` (hides pasted file contents).
    pub display: Option<String>,
    pub attachments: Vec<Attachment>,
    /// ISO-8601 UTC timestamp, e.g. 2026-11-06T11:10:09.517Z
    pub created_at: String,
    /// When the message was created in this window (drives the slide-in effect).
    pub born: std::time::Instant,
    /// The fixed greeting shown at the top of every chat (sent as "initial-message").
    pub initial: bool,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl ChatMessage {
    pub fn greeting(text: &str) -> Self {
        Self { id: "initial-message".into(), role: Role::Assistant, content: text.into(), display: None, attachments: Vec::new(), created_at: now_iso(), born: std::time::Instant::now(), initial: true }
    }
    #[allow(dead_code)]
    pub fn user(text: &str) -> Self {
        Self::user_with(text, Vec::new())
    }
    /// A user message with attached files. Text files are pasted below the message text;
    /// uploaded files travel as ids in the request's `fileIds`.
    pub fn user_with(text: &str, attachments: Vec<Attachment>) -> Self {
        let mut content = text.to_string();
        let mut any_inline = false;
        for a in &attachments {
            if let AttachmentData::Inline(body) = &a.data {
                any_inline = true;
                content.push_str(&inline_block(&a.name, body));
            }
        }
        let mut display = if any_inline { Some(text.to_string()) } else { None };
        if content.trim().is_empty() && !attachments.is_empty() {
            content = "Please look at the attached file(s).".to_string();
            display = None;
        }
        let content = content.trim_start().to_string();
        Self { id: new_id(), role: Role::User, content, display, attachments, created_at: now_iso(), born: std::time::Instant::now(), initial: false }
    }
    pub fn assistant_placeholder() -> Self {
        Self { id: new_id(), role: Role::Assistant, content: String::new(), display: None, attachments: Vec::new(), created_at: now_iso(), born: std::time::Instant::now(), initial: false }
    }
    pub fn touch(&mut self) {
        self.created_at = now_iso();
    }
}

pub fn new_session_id() -> String {
    new_id()
}

/// The text block that carries a pasted file. The fence is longer than any run of backticks
/// inside the file, so the file cannot break out of it.
fn inline_block(name: &str, body: &str) -> String {
    let (mut longest, mut run) = (0usize, 0usize);
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    let lang = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    format!("\n\n[Attached file: {name}]\n{fence}{lang}\n{body}\n{fence}")
}

// ------------------------------------------------------------------ request

/// React's reply encoding escapes strings that start with `$`.
fn esc(s: &str) -> String {
    if s.starts_with('$') { format!("${s}") } else { s.to_string() }
}

pub fn build_body(s: &Settings, msgs: &[ChatMessage], session_id: &str) -> String {
    let messages: Vec<Value> = msgs
        .iter()
        .map(|m| match (m.role, m.initial) {
            (Role::Assistant, true) => json!({
                "id": "initial-message",
                "role": "assistant",
                "content": esc(&m.content),
                "activitySteps": "$undefined",
            }),
            (Role::User, _) => json!({
                "id": m.id,
                "role": "user",
                "content": esc(&m.content),
                "createdAt": format!("$D{}", m.created_at),
            }),
            (Role::Assistant, false) => json!({
                "id": m.id,
                "role": "assistant",
                "content": esc(&m.content),
                "createdAt": format!("$D{}", m.created_at),
                "modelName": "$undefined",
                "activitySteps": "$undefined",
            }),
        })
        .collect();

    // Every uploaded file of this chat is listed again with each request, because the history is
    // re-sent every time as well.
    let file_ids: Vec<&str> = msgs
        .iter()
        .flat_map(|m| m.attachments.iter())
        .filter_map(|a| match &a.data {
            AttachmentData::Uploaded(id) => Some(id.as_str()),
            AttachmentData::Inline(_) => None,
        })
        .collect();

    let body = json!([{
        "characterId": s.character_id.trim(),
        "inviteCode": s.invite_code.trim(),
        "messages": messages,
        "fileIds": file_ids,
        "sharedSessionId": session_id,
    }]);
    body.to_string()
}

fn router_state_tree(character_id: &str) -> String {
    let tree = json!(["", {"children": ["(unauth)", {"children": ["ua", {"children": ["characters",
        {"children": [["characterId", character_id, "d", null],
            {"children": ["dialog", {"children": ["__PAGE__", {}, null, null, 0]}, null, null, 0]},
            null, null, 0]}, null, null, 0]}, null, null, 0]}, null, null, 0]}, null, null, 16]);
    url::form_urlencoded::byte_serialize(tree.to_string().as_bytes()).collect()
}

pub(crate) fn http_client(s: &Settings) -> Result<Client> {
    let ua = if s.user_agent.trim().is_empty() { "ais-chat-client/0.1" } else { s.user_agent.trim() };
    Ok(Client::builder()
        .user_agent(ua)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

fn describe_http_error(status: reqwest::StatusCode, body: &str) -> String {
    let snippet: String = body.chars().take(200).collect();
    if status == reqwest::StatusCode::NOT_FOUND || body.contains("Server Action") {
        "AIS.chat does not recognise the saved request values. It was probably updated. \
         Open Settings and import a fresh cURL (or try Auto-detect)."
            .to_string()
    } else {
        format!("The chat service answered HTTP {status}. {snippet}")
    }
}

/// Send the conversation and stream the answer. `on_text` is called for every piece of text.
/// Returns the server's id for the new assistant message, if it sent one.
pub fn stream_reply(s: &Settings, body: String, mut on_text: impl FnMut(&str)) -> Result<Option<String>> {
    let http = http_client(s)?;
    let url = s.dialog_url();

    let mut req = http
        .post(&url)
        .header("Accept", "text/x-component")
        .header("Content-Type", "text/plain;charset=UTF-8")
        .header("next-action", s.next_action.trim())
        .header("next-router-state-tree", router_state_tree(s.character_id.trim()))
        .header("Origin", s.base())
        .header("Referer", &url);
    if !s.deployment_id.trim().is_empty() {
        req = req.header("x-deployment-id", s.deployment_id.trim());
    }
    let mut resp = req.body(body).send()?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!(describe_http_error(status, &text)));
    }

    let mut parser = FlightParser::default();
    let mut buf = [0u8; 4096];
    let mut got_text = false;
    let mut errors: Vec<String> = Vec::new();
    loop {
        let n = resp.read(&mut buf)?;
        let events = if n == 0 { parser.finish() } else { parser.feed(&buf[..n]) };
        for ev in events {
            match ev {
                Event::Text(t) => {
                    got_text = true;
                    on_text(&t);
                }
                Event::Error(e) => errors.push(e),
                Event::Done => {}
            }
        }
        if n == 0 {
            break;
        }
    }
    if !got_text {
        return Err(anyhow!(match errors.first() {
            Some(e) => format!("The chat service returned an error: {e}"),
            None => "Empty reply from the chat service. If this keeps happening, AIS.chat may have \
                     been updated: re-import a fresh cURL in Settings."
                .to_string(),
        }));
    }
    Ok(parser.message_id)
}

// ------------------------------------------------------------------ streamed reply decoding

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    Done,
    Error(String),
}

/// Decoder for the React Flight rows AIS.chat sends:
///
/// ```text
/// 0:{...}\n            JSON row
/// 2:R\n                a text stream with id 2 starts
/// 1:{"stream":"$2","messageId":"..."}\n
/// 2:T6,Great,2:T2, a   text chunks: `<id>:T<hex byte length>,<exactly that many bytes>`
/// 2:C                  stream closed
/// ```
#[derive(Default)]
pub struct FlightParser {
    buf: Vec<u8>,
    stream_id: Option<String>,
    pub message_id: Option<String>,
}

impl FlightParser {
    pub fn feed(&mut self, data: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(data);
        self.drain(false)
    }

    pub fn finish(&mut self) -> Vec<Event> {
        self.drain(true)
    }

    fn is_stream(&self, id: &str) -> bool {
        match &self.stream_id {
            Some(s) => s == id,
            None => id != "0" && id != "1",
        }
    }

    fn drain(&mut self, eof: bool) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            // skip stray line breaks between rows
            let skip = self.buf.iter().take_while(|&&b| b == b'\n' || b == b'\r').count();
            if skip > 0 {
                self.buf.drain(..skip);
            }
            if self.buf.is_empty() {
                break;
            }

            let colon = match self.buf.iter().take(16).position(|&b| b == b':') {
                Some(c) => c,
                None => {
                    if self.buf.len() >= 16 || eof {
                        self.buf.clear();
                    }
                    break;
                }
            };
            let id = String::from_utf8_lossy(&self.buf[..colon]).to_string();
            if self.buf.len() <= colon + 1 {
                if eof {
                    self.buf.clear();
                }
                break;
            }

            if self.buf[colon + 1] == b'T' {
                let start = colon + 2;
                let comma = match self.buf[start..].iter().take(17).position(|&b| b == b',') {
                    Some(r) => start + r,
                    None => {
                        if eof {
                            self.buf.clear();
                        }
                        break;
                    }
                };
                let len = match usize::from_str_radix(&String::from_utf8_lossy(&self.buf[start..comma]), 16) {
                    Ok(n) => n,
                    Err(_) => {
                        events.push(Event::Error("Malformed text row in the reply".into()));
                        self.buf.clear();
                        break;
                    }
                };
                let end = comma + 1 + len;
                if self.buf.len() < end {
                    if eof {
                        // truncated reply: keep what arrived
                        let text = String::from_utf8_lossy(&self.buf[comma + 1..]).to_string();
                        if self.is_stream(&id) && !text.is_empty() {
                            events.push(Event::Text(text));
                        }
                        self.buf.clear();
                    }
                    break;
                }
                let text = String::from_utf8_lossy(&self.buf[comma + 1..end]).to_string();
                self.buf.drain(..end);
                if self.is_stream(&id) {
                    events.push(Event::Text(text));
                }
                continue;
            }

            // a normal row that ends with a line break
            let nl = self.buf.iter().position(|&b| b == b'\n');
            let (line_end, consume) = match nl {
                Some(p) => (p, p + 1),
                None => {
                    if !eof {
                        break;
                    }
                    (self.buf.len(), self.buf.len())
                }
            };
            let content = String::from_utf8_lossy(&self.buf[colon + 1..line_end]).to_string();
            self.buf.drain(..consume);
            self.handle_row(&id, &content, &mut events);
        }
        events
    }

    fn handle_row(&mut self, id: &str, content: &str, events: &mut Vec<Event>) {
        if content == "C" {
            if self.is_stream(id) {
                events.push(Event::Done);
            }
        } else if content == "R" {
            // text stream starts
        } else if let Some(rest) = content.strip_prefix('E') {
            events.push(Event::Error(rest.chars().take(300).collect()));
        } else if content.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(content) {
                if let Some(s) = v.get("stream").and_then(Value::as_str) {
                    self.stream_id = Some(s.trim_start_matches('$').to_string());
                }
                if let Some(m) = v.get("messageId").and_then(Value::as_str) {
                    self.message_id = Some(m.to_string());
                }
                if let Some(e) = v.get("error").and_then(Value::as_str) {
                    events.push(Event::Error(e.to_string()));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ keeping up with site updates

pub struct Detected {
    pub deployment_id: Option<String>,
    /// (server action id, function name if the bundle reveals it)
    pub actions: Vec<(String, Option<String>)>,
}

/// The page's `<meta name="baggage">` carries `sentry-release=<id>`, which equals the deployment id.
pub fn deployment_from_html(html: &str) -> Option<String> {
    let key = "sentry-release=";
    let i = html.find(key)?;
    let id: String = html[i + key.len()..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if id.len() >= 7 { Some(id) } else { None }
}

pub fn fetch_deployment_id(s: &Settings) -> Result<Option<String>> {
    let html = http_client(s)?.get(s.dialog_url()).send()?.error_for_status()?.text()?;
    Ok(deployment_from_html(&html))
}

/// Best effort: reads the page and its JavaScript chunks to find the current deployment id and
/// server action id(s).
pub fn detect(s: &Settings) -> Result<Detected> {
    let client = http_client(s)?;
    let html = client.get(s.dialog_url()).send()?.error_for_status()?.text()?;
    let deployment_id = deployment_from_html(&html);

    let mut actions: Vec<(String, Option<String>)> = Vec::new();
    for url in chunk_urls(&html, &s.base()).into_iter().take(60) {
        let Ok(resp) = client.get(&url).send() else { continue };
        let Ok(js) = resp.text() else { continue };
        for found in scan_server_references(&js) {
            if !actions.iter().any(|(id, _)| *id == found.0) {
                actions.push(found);
            }
        }
    }
    Ok(Detected { deployment_id, actions })
}

/// Pick the server action that looks like "send a chat message".
pub fn choose_action(actions: &[(String, Option<String>)]) -> Option<String> {
    if actions.len() == 1 {
        return Some(actions[0].0.clone());
    }
    let hints = ["chat", "dialog", "message", "send", "stream", "generate", "reply"];
    actions
        .iter()
        .find(|(_, name)| {
            name.as_ref()
                .map(|n| {
                    let l = n.to_lowercase();
                    hints.iter().any(|h| l.contains(h))
                })
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
}

fn chunk_urls(html: &str, base: &str) -> Vec<String> {
    let key = "static/chunks/";
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(i) = html[from..].find(key) {
        let start = from + i;
        let rest = &html[start..];
        let end = rest
            .find(|c: char| matches!(c, '"' | '\'' | '\\' | '?' | ' ' | '<' | ')'))
            .unwrap_or(rest.len());
        let path = &rest[..end];
        if path.ends_with(".js") {
            let url = format!("{base}/_next/{path}");
            if !out.contains(&url) {
                out.push(url);
            }
        }
        from = start + key.len();
    }
    out
}

fn slice_from(s: &str, start: usize, max: usize) -> &str {
    let mut end = (start + max).min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[start..end]
}

fn scan_server_references(js: &str) -> Vec<(String, Option<String>)> {
    let key = "createServerReference";
    let marker = "findSourceMapURL,\"";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = js[from..].find(key) {
        let start = from + i + key.len();
        from = start;
        let w = slice_from(js, start, 300);
        let Some(q1) = w.find('"') else { continue };
        let rest = &w[q1 + 1..];
        let Some(q2) = rest.find('"') else { continue };
        let id = &rest[..q2];
        if id.len() >= 40 && id.chars().all(|c| c.is_ascii_hexdigit()) {
            let name = rest[q2..].find(marker).map(|p| {
                rest[q2 + p + marker.len()..].chars().take_while(|c| *c != '"').collect::<String>()
            });
            out.push((id.to_string(), name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "0:{\"a\":\"$@1\",\"q\":\"?inviteCode=X\",\"i\":false}\n2:R\n1:{\"stream\":\"$2\",\"messageId\":\"abc\"}\n2:T6,Great,2:T2, a2:T1e, Python script is a good start2:T2,! 2:T8,🐍\n\nTo2:C";
    const EXPECTED: &str = "Great, a Python script is a good start! 🐍\n\nTo";

    fn decode(chunk: usize) -> (String, Vec<Event>, Option<String>) {
        let mut p = FlightParser::default();
        let mut text = String::new();
        let mut all = Vec::new();
        for part in SAMPLE.as_bytes().chunks(chunk) {
            for ev in p.feed(part) {
                if let Event::Text(t) = &ev {
                    text.push_str(t);
                }
                all.push(ev);
            }
        }
        for ev in p.finish() {
            all.push(ev);
        }
        (text, all, p.message_id.clone())
    }

    #[test]
    fn decodes_real_stream_in_any_chunking() {
        for chunk in [1, 2, 3, 5, 7, 64, 4096] {
            let (text, events, mid) = decode(chunk);
            assert_eq!(text, EXPECTED, "chunk size {chunk}");
            assert_eq!(events.last(), Some(&Event::Done));
            assert_eq!(mid.as_deref(), Some("abc"));
        }
    }

    #[test]
    fn body_has_expected_shape() {
        let s = Settings {
            character_id: "cid".into(),
            invite_code: "code".into(),
            ..Default::default()
        };
        let msgs = vec![ChatMessage::greeting("Hi"), ChatMessage::user("$5 please")];
        let body = build_body(&s, &msgs, "sess");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v[0]["characterId"], "cid");
        assert_eq!(v[0]["sharedSessionId"], "sess");
        assert_eq!(v[0]["messages"][0]["id"], "initial-message");
        assert_eq!(v[0]["messages"][1]["content"], "$$5 please");
        assert!(v[0]["messages"][1]["createdAt"].as_str().unwrap().starts_with("$D"));
    }

    #[test]
    fn inline_file_is_pasted_and_hidden_in_display() {
        let a = Attachment { name: "main.py".into(), size: 9, data: AttachmentData::Inline("print(1)\n".into()) };
        let m = ChatMessage::user_with("Why?", vec![a]);
        assert!(m.content.starts_with("Why?"));
        assert!(m.content.contains("[Attached file: main.py]\n```py\nprint(1)\n\n```"));
        assert_eq!(m.display.as_deref(), Some("Why?"));
    }

    #[test]
    fn fence_grows_around_backticks_in_file() {
        let block = inline_block("a.md", "x\n```\ny\n```");
        assert!(block.contains("````md\n"));
        assert!(block.ends_with("\n````"));
    }

    #[test]
    fn attachment_only_message_gets_default_text() {
        let a = Attachment { name: "x.pdf".into(), size: 1, data: AttachmentData::Uploaded("id1".into()) };
        let m = ChatMessage::user_with("", vec![a]);
        assert!(!m.content.is_empty());
        assert!(m.display.is_none());
    }

    #[test]
    fn uploaded_files_go_into_file_ids() {
        let s = Settings { character_id: "cid".into(), invite_code: "code".into(), ..Default::default() };
        let up = Attachment { name: "a.pdf".into(), size: 1, data: AttachmentData::Uploaded("file-1".into()) };
        let inl = Attachment { name: "b.txt".into(), size: 1, data: AttachmentData::Inline("hi".into()) };
        let msgs = vec![ChatMessage::greeting("Hi"), ChatMessage::user_with("see", vec![up, inl])];
        let v: Value = serde_json::from_str(&build_body(&s, &msgs, "sess")).unwrap();
        assert_eq!(v[0]["fileIds"], json!(["file-1"]));
    }

    #[test]
    fn finds_server_references() {
        let js = r#"x=(0,n.createServerReference)("40468826f02e7c5404dc45e562ad6c0df1e8874951",n.callServer,void 0,n.findSourceMapURL,"sendMessage");"#;
        let found = scan_server_references(js);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.as_deref(), Some("sendMessage"));
        assert_eq!(choose_action(&found).as_deref(), Some("40468826f02e7c5404dc45e562ad6c0df1e8874951"));
    }

    #[test]
    fn finds_deployment_id() {
        let html = r#"<meta name="baggage" content="sentry-environment=production,sentry-release=72251121fc13034b,sentry-public_key=x">"#;
        assert_eq!(deployment_from_html(html).as_deref(), Some("72251121fc13034b"));
    }
}
