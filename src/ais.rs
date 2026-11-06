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

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Role {
    User,
    Assistant,
}

/// What an attached file turned into.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum AttachmentData {
    /// Text file whose contents were pasted into the message (works without any upload endpoint).
    Inline(String),
    /// File uploaded to AIS.chat; this is the id the server gave back (sent in `fileIds`).
    Uploaded(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub data: AttachmentData,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    /// When the message was created in this window (drives the slide-in effect). Not saved:
    /// messages loaded from disk count as old, so they do not fade in.
    #[serde(skip, default = "long_ago")]
    pub born: std::time::Instant,
    /// The fixed greeting shown at the top of every chat (sent as "initial-message").
    pub initial: bool,
    /// Part of an earlier answer this message replies to (shown above the text; sent as a
    /// Markdown quote at the start of `content`).
    #[serde(default)]
    pub quote: Option<String>,
}

fn long_ago() -> std::time::Instant {
    let now = std::time::Instant::now();
    now.checked_sub(std::time::Duration::from_secs(5)).unwrap_or(now)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl ChatMessage {
    pub fn greeting(text: &str) -> Self {
        Self { id: "initial-message".into(), role: Role::Assistant, content: text.into(), display: None, attachments: Vec::new(), created_at: now_iso(), born: std::time::Instant::now(), initial: true, quote: None }
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
        Self { id: new_id(), role: Role::User, content, display, attachments, created_at: now_iso(), born: std::time::Instant::now(), initial: false, quote: None }
    }
    /// Reply to part of an earlier answer: the quote goes in front of the message as Markdown
    /// (`> …`) so the bot sees what is meant; the window shows it separately above the text.
    pub fn with_quote(mut self, quote: &str) -> Self {
        let q = quote.trim();
        if q.is_empty() {
            return self;
        }
        let shown = self.display.clone().unwrap_or_else(|| self.content.clone());
        let quoted: Vec<String> = q.lines().map(|l| if l.trim().is_empty() { ">".to_string() } else { format!("> {l}") }).collect();
        self.content = format!("{}\n\n{}", quoted.join("\n"), self.content);
        self.display = Some(shown);
        self.quote = Some(q.to_string());
        self
    }
    pub fn assistant_placeholder() -> Self {
        Self { id: new_id(), role: Role::Assistant, content: String::new(), display: None, attachments: Vec::new(), created_at: now_iso(), born: std::time::Instant::now(), initial: false, quote: None }
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
    // The language note travels only with the newest user message and is never stored in the
    // chat, so changing the language takes effect with the next message.
    let last_user = msgs.iter().rposition(|m| m.role == Role::User);
    let note = s.language_note();
    let messages: Vec<Value> = msgs
        .iter()
        .enumerate()
        .map(|(i, m)| match (m.role, m.initial) {
            (Role::Assistant, true) => json!({
                "id": "initial-message",
                "role": "assistant",
                "content": esc(&m.content),
                "activitySteps": "$undefined",
            }),
            (Role::User, _) => json!({
                "id": m.id,
                "role": "user",
                "content": esc(&match (&note, Some(i) == last_user) {
                    (Some(n), true) => format!("{n}\n\n{}", m.content),
                    _ => m.content.clone(),
                }),
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
    let default_ua = concat!("ais-chat-client/", env!("CARGO_PKG_VERSION"));
    let ua = if s.user_agent.trim().is_empty() { default_ua } else { s.user_agent.trim() };
    Ok(Client::builder()
        .user_agent(ua)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// AIS.chat was updated and the saved `next-action` / deployment id no longer work. The app
/// reacts to this by reading fresh values from the site and sending again.
#[derive(Debug)]
pub struct SiteChanged(pub String);

impl std::fmt::Display for SiteChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SiteChanged {}

pub fn is_site_changed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SiteChanged>().is_some()
}

fn describe_http_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    let snippet: String = body.chars().take(200).collect();
    if status == reqwest::StatusCode::NOT_FOUND || body.contains("Server Action") {
        anyhow::Error::new(SiteChanged(
            "AIS.chat does not recognise the saved request values (the site was probably updated).".into(),
        ))
    } else if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED {
        anyhow!("AIS.chat refused the request (HTTP {status}). Is the invite link still valid? Check it in Settings.")
    } else if status.as_u16() == 429 {
        anyhow!("AIS.chat says there are too many requests right now. Wait a moment and try again.")
    } else {
        anyhow!("The chat service answered HTTP {status}. {snippet}")
    }
}

/// Send the conversation and stream the answer. `on_event` gets every piece of text
/// ([`Event::Text`]) and every status update ([`Event::Activity`]).
/// Returns the server's id for the new assistant message, if it sent one.
pub fn stream_reply(s: &Settings, body: String, mut on_event: impl FnMut(&Event)) -> Result<Option<String>> {
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
    if crate::settings::language(s.language.trim()).is_some() {
        req = req.header("Accept-Language", s.language.trim());
    }
    if !s.deployment_id.trim().is_empty() {
        req = req.header("x-deployment-id", s.deployment_id.trim());
    }
    let mut resp = req.body(body).send().map_err(friendly_network_error)?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(describe_http_error(status, &text));
    }

    let mut parser = FlightParser::default();
    let mut buf = [0u8; 4096];
    let mut got_text = false;
    let mut errors: Vec<String> = Vec::new();
    loop {
        let n = resp.read(&mut buf)?;
        let events = if n == 0 { parser.finish() } else { parser.feed(&buf[..n]) };
        for ev in events {
            match &ev {
                Event::Text(_) => got_text = true,
                Event::Error(e) => errors.push(e.clone()),
                Event::Activity(_) | Event::Done => {}
            }
            on_event(&ev);
        }
        if n == 0 {
            break;
        }
    }
    if !got_text {
        return Err(match errors.first() {
            Some(e) => anyhow!("The chat service returned an error: {e}"),
            None => anyhow::Error::new(SiteChanged("Empty reply from the chat service.".into())),
        });
    }
    Ok(parser.message_id)
}

fn friendly_network_error(e: reqwest::Error) -> anyhow::Error {
    if e.is_connect() || e.is_timeout() {
        anyhow!("Could not reach AIS.chat. Check the internet connection and try again.")
    } else {
        anyhow::Error::new(e)
    }
}

// ------------------------------------------------------------------ file upload

/// Largest file the app will upload.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// Upload one file the way the website's paperclip button does and return the file id that goes
/// into the chat request's `fileIds`. `session_id` must be the chat's `sharedSessionId`.
pub fn upload_file(s: &Settings, name: &str, bytes: Vec<u8>, session_id: &str) -> Result<String> {
    use reqwest::blocking::multipart::{Form, Part};

    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(anyhow!("{name} is larger than {} MB.", MAX_UPLOAD_BYTES / 1024 / 1024));
    }
    let part = Part::bytes(bytes).file_name(name.to_string()).mime_str(mime_for(name))?;
    let form = Form::new()
        .part("file", part)
        .text("inviteCode", s.invite_code.trim().to_string())
        .text("entityType", "character")
        .text("entityId", s.character_id.trim().to_string())
        .text("sharedSessionId", session_id.to_string());

    let resp = http_client(s)?
        .post(s.upload_endpoint())
        .header("Origin", s.base())
        .header("Referer", s.dialog_url())
        .multipart(form)
        .send()
        .map_err(friendly_network_error)?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if status.as_u16() == 413 {
        return Err(anyhow!("AIS.chat says {name} is too large."));
    }
    if !status.is_success() {
        let snippet: String = text.chars().take(160).collect();
        return Err(anyhow!("Uploading {name} failed (HTTP {status}). {snippet}"));
    }
    parse_upload_reply(&text).ok_or_else(|| anyhow!("Uploading {name}: AIS.chat sent an answer without a file id."))
}

/// The upload answers `{"body":"{\"fileId\":\"file_…\"}","status":200}`. Also accepts the id
/// directly in the object and the `file_id` spelling.
pub fn parse_upload_reply(text: &str) -> Option<String> {
    fn id_in(v: &Value) -> Option<String> {
        for k in ["fileId", "file_id", "id"] {
            if let Some(id) = v.get(k).and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
        None
    }
    let v: Value = serde_json::from_str(text).ok()?;
    if let Some(inner) = v.get("body") {
        let inner = match inner {
            Value::String(s) => serde_json::from_str::<Value>(s).ok()?,
            other => other.clone(),
        };
        if let Some(id) = id_in(&inner) {
            return Some(id);
        }
    }
    id_in(&v)
}

/// Content type for an upload, from the file extension.
pub fn mime_for(name: &str) -> &'static str {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "odt" => "application/vnd.oasis.opendocument.text",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        _ if is_text_name(name) => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Files that can be pasted into the message as text if uploading is not possible.
pub fn is_text_name(name: &str) -> bool {
    const TEXT: &[&str] = &[
        "txt", "md", "markdown", "csv", "tsv", "json", "xml", "yaml", "yml", "toml", "ini", "cfg", "log", "py", "rs",
        "js", "ts", "jsx", "tsx", "java", "kt", "c", "h", "cpp", "hpp", "cs", "go", "rb", "php", "swift", "html", "htm",
        "css", "scss", "sql", "sh", "bat", "ps1", "lua", "r", "m", "ino", "pde", "gd", "vue", "svelte",
    ];
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    TEXT.contains(&ext.as_str())
}

// ------------------------------------------------------------------ streamed reply decoding

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    /// What the bot is doing right now (e.g. "Reading the attached file…"); empty = finished.
    Activity(String),
    Done,
    Error(String),
}

/// AIS.chat mixes status rows like `{"type":"ai_activity","steps":[…]}` into the text stream.
/// Returns `Some(status)` for those (empty when all steps are done) and `None` for real text.
fn activity_status(chunk: &str) -> Option<String> {
    // the rows start with an ASCII record separator (0x1E), which `trim` does not remove
    let t = chunk.trim_matches(|c: char| c.is_whitespace() || c == '\u{1e}');
    if !t.starts_with('{') || !t.contains("\"ai_activity\"") {
        return None;
    }
    let v: Value = serde_json::from_str(t).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("ai_activity") {
        return None;
    }
    let steps = v.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    if steps.iter().any(|s| s.get("kind").and_then(Value::as_str) == Some("done")) {
        return Some(String::new());
    }
    let tool = steps.iter().rev().find_map(|s| s.get("tool").and_then(Value::as_str)).unwrap_or("");
    let l = tool.to_lowercase();
    Some(
        if l.contains("file") {
            "Reading the attached file…".to_string()
        } else if l.contains("search") || l.contains("web") {
            "Searching the web…".to_string()
        } else if l.contains("image") {
            "Looking at the image…".to_string()
        } else if tool.is_empty() {
            "Thinking…".to_string()
        } else {
            format!("Working ({tool})…")
        },
    )
}

fn text_event(text: String) -> Event {
    match activity_status(&text) {
        Some(status) => Event::Activity(status),
        None => Event::Text(text),
    }
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
                            events.push(text_event(text));
                        }
                        self.buf.clear();
                    }
                    break;
                }
                let text = String::from_utf8_lossy(&self.buf[comma + 1..end]).to_string();
                self.buf.drain(..end);
                if self.is_stream(&id) {
                    events.push(text_event(text));
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
    pub character_name: Option<String>,
    pub greeting: Option<String>,
}

/// The page's `<meta name="baggage">` carries `sentry-release=<id>`, which equals the deployment id.
pub fn deployment_from_html(html: &str) -> Option<String> {
    let key = "sentry-release=";
    let i = html.find(key)?;
    let id: String = html[i + key.len()..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if id.len() >= 7 { Some(id) } else { None }
}

/// Best effort: reads the page and its JavaScript chunks to find the current deployment id and
/// server action id(s).
pub fn detect(s: &Settings) -> Result<Detected> {
    let client = http_client(s)?;
    let html = client.get(s.dialog_url()).send()?.error_for_status()?.text()?;
    let deployment_id = deployment_from_html(&html);
    let character_name = character_name_from_html(&html);
    let greeting = page_string_field(&html, "initialMessage");

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
    Ok(Detected { deployment_id, actions, character_name, greeting })
}

/// Read everything needed from the site and store it in `s`. Returns a short summary for the
/// status line. Used on first start, from Settings, and automatically when the site was updated.
pub fn refresh(s: &mut Settings) -> Result<String> {
    if !s.is_linked() {
        return Err(anyhow!("Paste the invite link in Settings first."));
    }
    let d = detect(s).map_err(|e| match e.downcast_ref::<reqwest::Error>() {
        Some(r) if r.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
            anyhow!("AIS.chat does not know this invite link. Check it in Settings.")
        }
        Some(r) if r.is_connect() || r.is_timeout() => anyhow!("Could not reach AIS.chat. Check the internet connection."),
        _ => e,
    })?;
    let action = choose_action(&d.actions).ok_or_else(|| {
        anyhow!("Could not find the chat function on the AIS.chat page. Use \"Import cURL\" under Settings → Advanced.")
    })?;
    s.next_action = action;
    if let Some(dep) = d.deployment_id {
        s.deployment_id = dep;
    }
    if let Some(n) = d.character_name {
        s.character_name = n;
    }
    if let Some(g) = d.greeting {
        s.greeting = g;
    }
    Ok(format!("Connected to {}.", s.display_name()))
}

/// Pick the server action that sends a message to a shared character. The page also contains
/// actions for other chat kinds, so "character" wins over the general hints.
pub fn choose_action(actions: &[(String, Option<String>)]) -> Option<String> {
    if actions.len() == 1 {
        return Some(actions[0].0.clone());
    }
    let named = |pred: &dyn Fn(&str) -> bool| {
        actions
            .iter()
            .find(|(_, name)| name.as_deref().map(|n| pred(&n.to_lowercase())).unwrap_or(false))
            .map(|(id, _)| id.clone())
    };
    let hints = ["chat", "dialog", "message", "send", "stream", "generate", "reply"];
    named(&|n| n.contains("character") && n.contains("message"))
        .or_else(|| named(&|n| n.contains("send") && n.contains("message") && !n.contains("learning")))
        .or_else(|| named(&|n| hints.iter().any(|h| n.contains(h))))
}

/// The page embeds the character as escaped JSON inside a script tag; its `name` is the last
/// one before `instructions`.
pub fn character_name_from_html(html: &str) -> Option<String> {
    let anchor = html.find("\\\"instructions\\\":")?;
    let key = "\\\"name\\\":\\\"";
    let start = html[..anchor].rfind(key)? + key.len();
    decode_page_string(&html[start..]).filter(|n| !n.trim().is_empty())
}

/// Value of a string field (`\"field\":\"…\"`) in the page's embedded data.
pub fn page_string_field(html: &str, field: &str) -> Option<String> {
    let key = format!("\\\"{field}\\\":\\\"");
    let start = html.find(&key)? + key.len();
    decode_page_string(&html[start..]).filter(|g| !g.trim().is_empty())
}

/// Decode a JSON string that is itself inside a JS string literal, starting right after its
/// opening quote. Undoes both escape levels.
fn decode_page_string(s: &str) -> Option<String> {
    let mut level1 = String::new();
    let mut chars = s.chars();
    let mut escaped = false;
    while let Some(c) = chars.next() {
        // undo the outer (JS literal) escaping
        let c = if c == '\\' {
            match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                }
                other => other,
            }
        } else {
            c
        };
        // inner JSON string
        if c == '"' && !escaped {
            return serde_json::from_str::<String>(&format!("\"{level1}\"")).ok();
        }
        escaped = c == '\\' && !escaped;
        level1.push(c);
        if level1.len() > 20_000 {
            return None;
        }
    }
    None
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
    fn language_note_goes_only_in_front_of_the_newest_message() {
        let mut s = Settings { character_id: "cid".into(), invite_code: "code".into(), ..Default::default() };
        let msgs = vec![
            ChatMessage::greeting("Hallo"),
            ChatMessage::user("first"),
            ChatMessage::assistant_placeholder(),
            ChatMessage::user("$second"),
        ];
        let v: Value = serde_json::from_str(&build_body(&s, &msgs, "sess")).unwrap();
        assert_eq!(v[0]["messages"][3]["content"], "$$second", "automatic: unchanged");

        s.language = "en".into();
        let v: Value = serde_json::from_str(&build_body(&s, &msgs, "sess")).unwrap();
        assert_eq!(v[0]["messages"][1]["content"], "first");
        let last = v[0]["messages"][3]["content"].as_str().unwrap();
        assert!(last.starts_with("[Note from the chat app"), "{last}");
        assert!(last.contains("English (English)"));
        assert!(last.ends_with("\n\n$second"));
        assert_eq!(msgs[3].content, "$second", "the stored chat stays clean");
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
    fn quote_is_sent_as_markdown_and_hidden_in_display() {
        let m = ChatMessage::user("Why this?").with_quote("x = 1\n\ny = 2");
        assert_eq!(m.content, "> x = 1\n>\n> y = 2\n\nWhy this?");
        assert_eq!(m.display.as_deref(), Some("Why this?"));
        assert_eq!(m.quote.as_deref(), Some("x = 1\n\ny = 2"));
        let plain = ChatMessage::user("Hi").with_quote("  ");
        assert_eq!(plain.content, "Hi");
        assert!(plain.quote.is_none());
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
    fn activity_rows_are_not_text() {
        // real capture of a reply to a message with an uploaded file (byte lengths must stay exact)
        let sample = "0:{\"a\":\"$@1\",\"q\":\"?inviteCode=RVHN1H1B\",\"i\":false}\n2:R\n1:{\"stream\":\"$2\",\"messageId\":\"904e5bcd-f867-44ba-8c4e-77de85c51aa7\"}\n2:T7a,\u{1e}{\"type\":\"ai_activity\",\"steps\":[{\"kind\":\"tool\",\"id\":\"toolu_vrtx_01M1sEp5z5RsAhUcYYwcFQuE\",\"tool\":\"retrieve_entire_file\"}]}2:T11,The file says: \"h2:T17,ello from upload test\".2:T8a,\u{1e}{\"type\":\"ai_activity\",\"steps\":[{\"kind\":\"tool\",\"id\":\"toolu_vrtx_01M1sEp5z5RsAhUcYYwcFQuE\",\"tool\":\"retrieve_entire_file\"},{\"kind\":\"done\"}]}2:C\n";
        for chunk in [1, 3, 4096] {
            let mut p = FlightParser::default();
            let mut events = Vec::new();
            for part in sample.as_bytes().chunks(chunk) {
                events.extend(p.feed(part));
            }
            events.extend(p.finish());
            let text: String = events.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
            assert_eq!(text, "The file says: \"hello from upload test\".", "chunk size {chunk}");
            assert_eq!(events[0], Event::Activity("Reading the attached file…".into()));
            assert!(events.contains(&Event::Activity(String::new())));
        }
    }

    #[test]
    fn reads_upload_reply() {
        let real = r#"{"body":"{\"fileId\":\"file_Gv0E7W11BnzTcvc5s457OBcN\"}","status":200}"#;
        assert_eq!(parse_upload_reply(real).as_deref(), Some("file_Gv0E7W11BnzTcvc5s457OBcN"));
        assert_eq!(parse_upload_reply(r#"{"body":"{\"file_id\":\"f2\"}"}"#).as_deref(), Some("f2"));
        assert_eq!(parse_upload_reply(r#"{"fileId":"f3"}"#).as_deref(), Some("f3"));
        assert_eq!(parse_upload_reply("<html>"), None);
    }

    #[test]
    fn prefers_character_action() {
        let actions = vec![
            ("a".repeat(42), Some("sendChatMessageAction".to_string())),
            ("b".repeat(42), Some("sendLearningScenarioMessageAction".to_string())),
            ("c".repeat(42), Some("sendCharacterMessageAction".to_string())),
            ("d".repeat(42), Some("updateUserSettingsAction".to_string())),
        ];
        assert_eq!(choose_action(&actions), Some("c".repeat(42)));
    }

    #[test]
    fn reads_name_and_greeting_from_page() {
        let html = r#"x\"id\":\"6d16\",\"name\":\"VibeCoding-Assitent für WPU-FEEL\",\"description\":\"d\",\"instructions\":\"ROLLE\\nDu\",\"initialMessage\":\"Hallo, \\\"Coder\\\"!\\nWie geht's?\",\"x\":1"#;
        assert_eq!(character_name_from_html(html).as_deref(), Some("VibeCoding-Assitent für WPU-FEEL"));
        assert_eq!(page_string_field(html, "initialMessage").as_deref(), Some("Hallo, \"Coder\"!\nWie geht's?"));
    }

    #[test]
    fn outdated_action_is_detected() {
        // what AIS.chat really answers when next-action is out of date
        let e = describe_http_error(reqwest::StatusCode::BAD_REQUEST, "Invalid Server Action request.");
        assert!(is_site_changed(&e));
        assert!(is_site_changed(&describe_http_error(reqwest::StatusCode::NOT_FOUND, "")));
        assert!(!is_site_changed(&describe_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom")));
    }

    /// Talks to the real site: `cargo test -- --ignored live_`
    #[test]
    #[ignore]
    fn live_setup_upload_and_reply() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        let summary = refresh(&mut s).expect("setup from the invite link");
        println!("{summary} action={} deployment={} greeting={:?}", s.next_action, s.deployment_id, s.greeting);
        assert!(s.is_ready());

        let session = new_session_id();
        let id = upload_file(&s, "note.txt", b"The secret word is pineapple.\n".to_vec(), &session).expect("upload");
        let att = Attachment { name: "note.txt".into(), size: 30, data: AttachmentData::Uploaded(id) };
        let msgs = vec![
            ChatMessage::greeting(&s.greeting),
            ChatMessage::user_with("What is the secret word in the attached file? Answer with one word.", vec![att]),
        ];
        let mut text = String::new();
        let mut activity = Vec::new();
        stream_reply(&s, build_body(&s, &msgs, &session), |ev| match ev {
            Event::Text(t) => text.push_str(t),
            Event::Activity(a) => activity.push(a.clone()),
            _ => {}
        })
        .expect("reply");
        println!("reply: {text:?} activity: {activity:?}");
        assert!(text.to_lowercase().contains("pineapple"));
        assert!(!text.contains("ai_activity"));
    }

    /// `cargo test -- --ignored live_` : the bot follows the language note.
    #[test]
    #[ignore]
    fn live_language_note() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        refresh(&mut s).expect("setup");
        s.language = "fr".into();
        let msgs = vec![ChatMessage::greeting(&s.greeting), ChatMessage::user("Was ist eine Variable? Antworte in einem Satz.")];
        let mut text = String::new();
        stream_reply(&s, build_body(&s, &msgs, &new_session_id()), |ev| {
            if let Event::Text(t) = ev {
                text.push_str(t)
            }
        })
        .expect("reply");
        println!("reply: {text}");
        let l = text.to_lowercase();
        assert!(l.contains(" une ") || l.contains(" est ") || l.contains(" des "), "not French: {text}");
        assert!(!l.contains("note"), "mentions the note: {text}");
    }

    /// `cargo test -- --ignored live_` : the bot translates an unknown greeting cleanly.
    #[test]
    #[ignore]
    fn live_greeting_translation() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        refresh(&mut s).expect("setup");
        s.language = "fr".into();
        let source = "Hallo! Ich bin dein Geschichts-Assistent. Was möchtest du heute lernen?";
        let msgs = vec![ChatMessage::user(&crate::i18n::translation_request(source, "French"))];
        let mut reply = String::new();
        stream_reply(&s, build_body(&s, &msgs, &new_session_id()), |ev| {
            if let Event::Text(t) = ev {
                reply.push_str(t)
            }
        })
        .expect("reply");
        let text = crate::i18n::clean_translation(&reply, source);
        println!("translation: {text:?}");
        let t = text.expect("usable translation").to_lowercase();
        assert!(t.contains("bonjour") || t.contains("salut"), "{t}");
    }

    #[test]
    fn finds_deployment_id() {
        let html = r#"<meta name="baggage" content="sentry-environment=production,sentry-release=72251121fc13034b,sentry-public_key=x">"#;
        assert_eq!(deployment_from_html(html).as_deref(), Some("72251121fc13034b"));
    }
}
