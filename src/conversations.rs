//! Several chats, each with its own context: its own AIS.chat session (`sharedSessionId`),
//! history, uploads, draft and quote. Chats are saved to the user config folder
//! (`conversations.json`) and come back after a restart.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ais::{self, AttachmentData, ChatMessage, Role};

/// At most this many chats are kept on disk (the oldest are dropped).
const MAX_SAVED: usize = 200;

pub enum FileState {
    Uploading,
    Ready(AttachmentData),
    Failed(String),
}

pub struct PendingFile {
    pub id: u64,
    pub name: String,
    pub size: usize,
    pub state: FileState,
}

#[derive(Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    /// Set by the user with Rename; otherwise the title comes from the first message.
    #[serde(default)]
    pub custom_title: Option<String>,
    /// A short summary of the first message, written by the bot (used when not renamed).
    #[serde(default)]
    pub summary: Option<String>,
    /// The answer language (code, "" = automatic) the summary was written for.
    #[serde(default)]
    pub summary_lang: String,
    pub messages: Vec<ChatMessage>,
    /// AIS.chat's `sharedSessionId` – uploads belong to it, so it stays with the chat.
    pub session_id: String,
    /// Unix seconds of the last change (sorts the list).
    pub updated: i64,
    /// Unsent text in the message box.
    #[serde(default)]
    pub input: String,

    // ---- only while the app runs
    #[serde(skip)]
    pub pending: Vec<PendingFile>,
    /// Bumped on Stop so late answers from an old request are ignored.
    #[serde(skip)]
    pub gen: u64,
    #[serde(skip)]
    pub busy: bool,
    #[serde(skip)]
    pub activity: String,
    #[serde(skip)]
    pub error: Option<String>,
    /// Part of an answer the next message replies to.
    #[serde(skip)]
    pub quote: Option<String>,
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Conversation {
    pub fn new(greeting: &str) -> Self {
        Self {
            id: ais::new_session_id(),
            custom_title: None,
            summary: None,
            summary_lang: String::new(),
            messages: vec![ChatMessage::greeting(greeting)],
            session_id: ais::new_session_id(),
            updated: now(),
            input: String::new(),
            pending: Vec::new(),
            gen: 0,
            busy: false,
            activity: String::new(),
            error: None,
            quote: None,
        }
    }

    /// Nothing but the greeting yet.
    pub fn is_empty(&self) -> bool {
        self.messages.iter().all(|m| m.initial)
    }

    /// Rename wins; then the bot's summary; otherwise the start of the first message;
    /// otherwise "New chat".
    pub fn title(&self) -> String {
        let named = [&self.custom_title, &self.summary].into_iter().flatten().map(|t| t.trim()).find(|t| !t.is_empty());
        if let Some(t) = named {
            return t.to_string();
        }
        let first = self.messages.iter().find(|m| m.role == Role::User);
        let text = first.map(|m| m.display.as_deref().unwrap_or(&m.content)).unwrap_or("");
        let line = text.lines().map(str::trim).find(|l| !l.is_empty());
        match line {
            Some(l) if l.chars().count() > 42 => format!("{}…", l.chars().take(40).collect::<String>().trim_end()),
            Some(l) => l.to_string(),
            None => match first.and_then(|m| m.attachments.first()) {
                Some(a) => a.name.clone(),
                None => crate::i18n::t("New chat").to_string(),
            },
        }
    }

    /// The start of the last message (without Markdown marks), for the "Recent chats" list.
    pub fn preview(&self) -> String {
        let last = self.messages.iter().rev().find(|m| !m.initial && !m.content.trim().is_empty());
        let text = last.map(|m| m.display.as_deref().unwrap_or(&m.content)).unwrap_or("");
        let line = text
            .lines()
            .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).trim())
            .find(|l| !l.is_empty() && !l.starts_with("```"))
            .unwrap_or("");
        let clean: String = line.chars().filter(|c| !matches!(c, '*' | '`' | '_')).collect();
        if clean.chars().count() > 90 {
            format!("{}…", clean.chars().take(88).collect::<String>().trim_end())
        } else {
            clean
        }
    }

    /// The first message as the bot should summarise it, if the chat still needs a title in
    /// the answer language `lang` (none yet, or one in another language).
    pub fn needs_summary(&self, lang: &str) -> Option<String> {
        if self.custom_title.is_some() || (self.summary.is_some() && self.summary_lang == lang) {
            return None;
        }
        let first = self.messages.iter().find(|m| m.role == Role::User)?;
        let mut text = first.display.as_deref().unwrap_or(&first.content).trim().to_string();
        let files: Vec<&str> = first.attachments.iter().map(|a| a.name.as_str()).collect();
        if !files.is_empty() {
            text.push_str(&format!("\n[Attached: {}]", files.join(", ")));
        }
        if text.trim().is_empty() {
            return None;
        }
        Some(text.chars().take(1500).collect())
    }

    pub fn touch(&mut self) {
        self.updated = now();
    }

    /// Replace the greeting while the chat is still empty (e.g. after connecting to another bot).
    pub fn set_greeting(&mut self, greeting: &str) {
        if self.is_empty() && !self.busy {
            self.messages = vec![ChatMessage::greeting(greeting)];
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ais-chat-client").join("conversations.json"))
}

/// Saved chats, newest first. Missing or unreadable file = no chats.
pub fn load() -> Vec<Conversation> {
    let mut list: Vec<Conversation> = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    list.retain(|c| !c.is_empty());
    list.sort_by(|a, b| b.updated.cmp(&a.updated));
    list
}

/// Save every chat that has messages (empty ones are not worth keeping).
pub fn save(list: &[Conversation]) -> anyhow::Result<()> {
    let p = path().ok_or_else(|| anyhow::anyhow!("No user config folder on this system."))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut keep: Vec<&Conversation> = list.iter().filter(|c| !c.is_empty()).collect();
    keep.sort_by(|a, b| b.updated.cmp(&a.updated));
    keep.truncate(MAX_SAVED);
    // write to a temporary file first so a crash cannot leave a half-written file behind
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&keep)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// The heading a chat is listed under in the side list.
pub fn day_group(unix: i64) -> &'static str {
    day_group_at(unix, chrono::Local::now().date_naive())
}

fn day_group_at(unix: i64, today: chrono::NaiveDate) -> &'static str {
    use chrono::{Local, TimeZone};
    let Some(day) = Local.timestamp_opt(unix, 0).single().map(|t| t.date_naive()) else { return "Older" };
    match (today - day).num_days() {
        d if d <= 0 => "Today",
        1 => "Yesterday",
        2..=6 => "Last 7 days",
        _ => "Older",
    }
}

/// The question that asks the bot for a chat title, in `language` (English name; `None`:
/// the language of the message).
pub fn summary_request(first_message: &str, language: Option<&str>) -> String {
    let lang = match language {
        Some(l) => format!("Write the title in {l}, even if the message is in another language."),
        None => "Use the language of the message.".to_string(),
    };
    format!(
        "Write a very short title (2 to 6 words) that sums up what this chat message is about. {lang} \
         Reply with the title only – no quotes, no full stop, no explanation.\n\n\
         Message:\n\"\"\"\n{first_message}\n\"\"\""
    )
}

/// Take the title out of the bot's answer; `None` if the answer does not look like a title.
pub fn clean_summary(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```"))?;
    let mut t: String = line.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '#')).collect();
    for prefix in ["Title:", "Titel:", "title:", "Titre :", "Titre:", "Título:"] {
        if let Some(rest) = t.trim().strip_prefix(prefix) {
            t = rest.to_string();
        }
    }
    let t = t
        .trim()
        .trim_matches(|c| matches!(c, '"' | '„' | '“' | '”' | '«' | '»' | '\'' | '‚' | '‘' | '’'))
        .trim_end_matches(['.', '!', ':', ';', ','])
        .trim();
    let words = t.split_whitespace().count();
    if t.is_empty() || words > 10 || t.chars().count() > 70 {
        return None;
    }
    Some(t.to_string())
}

/// A moment with its time: "14:05" for today, otherwise "12.11. 00:00".
pub fn when_exact(unix: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(unix, 0).single() else { return String::new() };
    if t.date_naive() == Local::now().date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%d.%m. %H:%M").to_string()
    }
}

/// "14:05" for today, "Yesterday", or "06.11.2026".
pub fn when(unix: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(unix, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    if day == today {
        t.format("%H:%M").to_string()
    } else if today.pred_opt() == Some(day) {
        crate::i18n::t("Yesterday").to_string()
    } else {
        t.format("%d.%m.%Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_comes_from_first_message() {
        let mut c = Conversation::new("Hi");
        assert_eq!(c.title(), "New chat");
        assert!(c.is_empty());
        c.messages.push(ChatMessage::user("\n  How do loops work in Python and what is the difference to while?"));
        assert!(!c.is_empty());
        assert_eq!(c.title(), "How do loops work in Python and what is…");
        c.custom_title = Some("Loops".into());
        assert_eq!(c.title(), "Loops");
    }

    #[test]
    fn quoted_message_title_uses_the_question() {
        let mut c = Conversation::new("Hi");
        c.messages.push(ChatMessage::user("Why?").with_quote("some quote"));
        assert_eq!(c.title(), "Why?");
    }

    #[test]
    fn preview_is_the_start_of_the_last_message() {
        let mut c = Conversation::new("Hi");
        assert_eq!(c.preview(), "");
        c.messages.push(ChatMessage::user("How do lists work?"));
        let mut a = ChatMessage::assistant_placeholder();
        a.content = "## Lists in **Python**\n\n```python\nx = []\n```".into();
        c.messages.push(a);
        assert_eq!(c.preview(), "Lists in Python");
    }

    #[test]
    fn summary_wins_over_first_message_but_not_rename() {
        let mut c = Conversation::new("Hi");
        assert_eq!(c.needs_summary(""), None, "nothing to summarise yet");
        c.messages.push(ChatMessage::user("can u help me my python loop prints the same number every time"));
        assert!(c.needs_summary("").unwrap().starts_with("can u help"));
        c.summary = Some("Python loop repeats a number".into());
        assert_eq!(c.title(), "Python loop repeats a number");
        assert_eq!(c.needs_summary(""), None);
        assert!(c.needs_summary("de").is_some(), "a new answer language needs a new title");
        assert!(summary_request("x", Some("German")).contains("in German"));
        c.custom_title = Some("Homework".into());
        assert_eq!(c.title(), "Homework");
    }

    #[test]
    fn cleans_summaries() {
        assert_eq!(clean_summary("\"Python loop prints same number.\"\n").as_deref(), Some("Python loop prints same number"));
        assert_eq!(clean_summary("**Titel:** Schleifen in Python").as_deref(), Some("Schleifen in Python"));
        assert_eq!(clean_summary("\n\n  Rust Hello World  \nSure, here you go!").as_deref(), Some("Rust Hello World"));
        assert_eq!(clean_summary(""), None);
        assert_eq!(clean_summary("Sure! Here is a short title that sums up what your message is about in a few words"), None);
    }

    #[test]
    fn groups_by_day() {
        use chrono::{Local, TimeZone};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let at = |d: u32| Local.with_ymd_and_hms(2026, 11, d, 12, 0, 0).unwrap().timestamp();
        assert_eq!(day_group_at(at(8), today), "Today");
        assert_eq!(day_group_at(at(7), today), "Yesterday");
        assert_eq!(day_group_at(at(3), today), "Last 7 days");
        assert_eq!(day_group_at(at(1), today), "Older");
    }

    #[test]
    fn round_trips_through_json() {
        let mut c = Conversation::new("Hi");
        c.messages.push(ChatMessage::user("question").with_quote("q"));
        c.input = "draft".into();
        let json = serde_json::to_string(&vec![&c]).unwrap();
        let back: Vec<Conversation> = serde_json::from_str(&json).unwrap();
        assert_eq!(back[0].session_id, c.session_id);
        assert_eq!(back[0].messages.len(), 2);
        assert_eq!(back[0].messages[1].quote.as_deref(), Some("q"));
        assert_eq!(back[0].input, "draft");
        assert!(!back[0].busy);
    }
}
