//! Everything the app remembers between runs.
//!
//! The only value a person ever has to provide is the **invite link**. Everything else
//! (deployment id, server action id, upload address, character name, greeting) is read from the
//! site automatically and refreshed whenever AIS.chat is updated.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// The class's shared character. Used when no other invite link has been saved yet, so the app
/// works straight after the first start.
pub const DEFAULT_INVITE_LINK: &str =
    "https://app.ais-chat.schule/ua/characters/6d16c5dd-1555-435f-ac22-3ab521a692b1/dialog?inviteCode=RVHN1H1B";

const DEFAULT_BASE: &str = "https://app.ais-chat.schule";

/// Name of the GitHub token in the system password store.
const TOKEN_NAME: &str = "github-token";

/// Name of a provider's API key in the system password store.
fn ai_key_name(provider: &str) -> String {
    format!("ai-key-{provider}")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// The invite link exactly as the person pasted it.
    pub invite_link: String,
    /// `https://host` of the site (taken from the invite link).
    pub base_url: String,
    pub character_id: String,
    pub invite_code: String,

    // ---- found automatically (can be overridden under "Advanced")
    /// Server action id of "send a character message". Changes with every site update.
    pub next_action: String,
    /// Next.js deployment id. Changes with every site update.
    pub deployment_id: String,
    /// Where files are uploaded. Empty = automatic (`<site>/api/v1/shared-chat/files`).
    pub upload_url: String,
    /// Empty = a built-in browser-like name.
    pub user_agent: String,

    // ---- shown in the window
    pub character_name: String,
    pub greeting: String,
    /// "system" (default), "light" or "dark".
    pub appearance: String,
    /// Language of the menus and of the bot's answers, as a code from [`LANGUAGES`];
    /// empty = automatic (menus in the system language, answers in the language of the message).
    pub language: String,

    // ---- optional: save code from answers to GitHub
    pub github_enabled: bool,
    /// `owner/name`
    pub github_repo: String,
    /// Personal access token (fine-grained, "Contents: Read and write" for the repository).
    /// Kept in the system password store; written to settings.json only on systems that have
    /// none (and read from it to move tokens of older versions into the store).
    pub github_token: String,
    /// Empty = the repository's default branch.
    pub github_branch: String,
    /// Folder in the repository the files go into; empty (or `/`) = the top level, so a file
    /// the bot calls `src/main.rs` updates the repository's `src/main.rs`.
    pub github_folder: String,
    /// Push every answer with code automatically (otherwise only with the ⬆ button).
    pub github_auto: bool,
    /// Empty = https://api.github.com (GitHub Enterprise: https://host/api/v3).
    pub github_api: String,

    /// Which model answers: empty = the AIS.chat class bot, otherwise a model of the AI
    /// provider (`ai_provider`), e.g. `llama-3.3-70b-versatile`.
    pub model: String,
    /// The provider of the free AI models: "groq" (default), "openrouter", "mistral",
    /// "gemini" or "custom".
    pub ai_provider: String,
    /// API address for the provider "custom" (OpenAI-compatible, e.g. https://host/v1).
    pub ai_base_url: String,
    /// API key per provider – its own, not the GitHub token. Kept in the system password
    /// store; written to settings.json only on systems that have none.
    pub ai_keys: std::collections::BTreeMap<String, String>,
    /// The class bot's instructions from its AIS.chat page – given to the free models so they
    /// behave like the class bot.
    pub character_instructions: String,

    /// Greetings the bot translated (for bots whose greeting the app does not know):
    /// language code → [original greeting, translation].
    pub greeting_translations: std::collections::BTreeMap<String, [String; 2]>,

    /// Set for one-off questions (titles, translations, merging): no hidden note at all.
    #[serde(skip)]
    pub plain_request: bool,
}

/// The system's language if the app has menus for it, otherwise English.
pub fn system_language() -> &'static str {
    static FOUND: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| {
        let locale = sys_locale::get_locale().unwrap_or_default().to_lowercase();
        let code = locale.split(['-', '_', '.']).next().unwrap_or("");
        LANGUAGES.iter().map(|(c, _, _)| *c).find(|c| *c == code).unwrap_or("en")
    })
}

/// Answer languages: (code, name in the language itself, English name).
pub const LANGUAGES: &[(&str, &str, &str)] = &[
    ("de", "Deutsch", "German"),
    ("en", "English", "English"),
    ("fr", "Français", "French"),
    ("es", "Español", "Spanish"),
    ("it", "Italiano", "Italian"),
    ("nl", "Nederlands", "Dutch"),
    ("pl", "Polski", "Polish"),
    ("tr", "Türkçe", "Turkish"),
    ("uk", "Українська", "Ukrainian"),
    ("ru", "Русский", "Russian"),
];

/// `(native name, English name)` of a language code, if it is one of [`LANGUAGES`].
pub fn language(code: &str) -> Option<(&'static str, &'static str)> {
    LANGUAGES.iter().find(|(c, _, _)| *c == code).map(|(_, native, english)| (*native, *english))
}

impl Settings {
    pub fn github_config(&self) -> crate::github::Config {
        crate::github::Config {
            token: self.github_token.clone(),
            repo: self.github_repo.clone(),
            branch: self.github_branch.clone(),
            api: self.github_api.clone(),
        }
    }

    /// GitHub is switched on and has a repository and a token.
    pub fn github_ready(&self) -> bool {
        self.github_enabled && crate::github::normalize_repo(&self.github_repo).is_some() && !self.github_token.trim().is_empty()
    }

    /// A free model of the AI provider answers instead of the AIS.chat class bot.
    pub fn uses_ai_models(&self) -> bool {
        !self.model.trim().is_empty()
    }

    pub fn ai_provider(&self) -> &'static crate::models::Provider {
        crate::models::provider(self.ai_provider.trim())
    }

    /// The provider's API address.
    pub fn ai_base(&self) -> String {
        let p = self.ai_provider();
        if p.id == "custom" { self.ai_base_url.trim().to_string() } else { p.base.to_string() }
    }

    /// The API key for the current provider.
    pub fn ai_key(&self) -> String {
        self.ai_keys.get(self.ai_provider().id).cloned().unwrap_or_default()
    }

    /// Every secret with its name in the password store: the GitHub token and the AI keys.
    fn secrets(&self) -> Vec<(String, String)> {
        let mut out = vec![(TOKEN_NAME.to_string(), self.github_token.trim().to_string())];
        out.extend(self.ai_keys.iter().map(|(p, k)| (ai_key_name(p), k.trim().to_string())));
        out
    }

    /// Remove the secret `name` from these settings if it still has `value`.
    fn forget_secret(&mut self, name: &str, value: &str) {
        if name == TOKEN_NAME {
            if self.github_token.trim() == value {
                self.github_token.clear();
            }
        } else if let Some(p) = name.strip_prefix("ai-key-") {
            if self.ai_keys.get(p).map(|k| k.trim()) == Some(value) {
                self.ai_keys.remove(p);
            }
        }
    }

    pub fn github_folder(&self) -> String {
        self.github_folder.trim().trim_matches('/').to_string()
    }

    /// The greeting shown at the start of a chat, in the answer language if one is chosen and
    /// a translation is known; otherwise the bot's own greeting. Empty if the bot has none.
    pub fn displayed_greeting(&self) -> String {
        // the class bot's greeting is known in every language, so it follows the menus
        if let Some(g) = crate::i18n::known_greeting(&self.greeting, &self.ui_language()) {
            return g.to_string();
        }
        let lang = self.language.trim();
        if language(lang).is_some() {
            if let Some([source, translated]) = self.greeting_translations.get(lang) {
                if *source == self.greeting {
                    return translated.clone();
                }
            }
        }
        self.greeting.clone()
    }

    /// The bot has to be asked for a translation of its greeting.
    pub fn greeting_needs_translation(&self) -> bool {
        let lang = self.language.trim();
        language(lang).is_some()
            && !self.greeting.trim().is_empty()
            && crate::i18n::known_greeting(&self.greeting, lang).is_none()
            && self.greeting_translations.get(lang).map_or(true, |[source, _]| *source != self.greeting)
    }

    /// The hidden note put in front of the newest message: the answer language, and – while
    /// GitHub saving is on – that files are always shown whole and with their path, so they can
    /// be saved as they are. `None` when there is nothing to say.
    pub fn app_note(&self) -> Option<String> {
        if self.plain_request {
            return None;
        }
        let mut parts = Vec::new();
        if let Some((native, english)) = language(self.language.trim()) {
            parts.push(format!(
                "always answer in {english} ({native}), whatever language the message below or your own instructions \
                 are written in."
            ));
        } else {
            // the class bot's instructions are German; without this it answers English questions in German
            parts.push(
                "answer in the language the message below is written in, even if your own instructions are in another \
                 language."
                    .to_string(),
            );
        }
        if self.github_ready() {
            parts.push(
                "Whenever you write or change a file, show the complete file (not only the changed part) in one code \
                 block, and name its path (for example `src/main.rs`) in the sentence right before the block."
                    .to_string(),
            );
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!("[Note from the chat app, not written by the user: {} Do not mention this note.]", parts.join(" ")))
    }
}

impl Settings {
    /// Put after the newest message: a chosen answer language is repeated there, because the
    /// bot's own instructions (German for the class bot) otherwise tend to win.
    pub fn language_reminder(&self) -> Option<String> {
        if self.plain_request {
            return None;
        }
        language(self.language.trim()).map(|(_, english)| format!("[Reply in {english}.]"))
    }

    /// The language of the menus: the chosen one, or the system's.
    pub fn ui_language(&self) -> String {
        match language(self.language.trim()) {
            Some(_) => self.language.trim().to_string(),
            None => system_language().to_string(),
        }
    }

    /// `https://app.ais-chat.schule`
    pub fn base(&self) -> String {
        let b = self.base_url.trim().trim_end_matches('/');
        if b.is_empty() { DEFAULT_BASE.to_string() } else { b.to_string() }
    }

    /// The page and endpoint of the shared character's chat.
    pub fn dialog_url(&self) -> String {
        let code: String = url::form_urlencoded::byte_serialize(self.invite_code.trim().as_bytes()).collect();
        format!("{}/ua/characters/{}/dialog?inviteCode={}", self.base(), self.character_id.trim(), code)
    }

    /// Where attached files are sent.
    pub fn upload_endpoint(&self) -> String {
        let u = self.upload_url.trim();
        if u.is_empty() { format!("{}/api/v1/shared-chat/files", self.base()) } else { u.to_string() }
    }

    /// An invite link has been read successfully.
    pub fn is_linked(&self) -> bool {
        !self.character_id.trim().is_empty() && !self.invite_code.trim().is_empty()
    }

    /// Everything needed to send a message is known.
    pub fn is_ready(&self) -> bool {
        self.is_linked() && !self.next_action.trim().is_empty()
    }

    /// Short name for the window title / header.
    pub fn display_name(&self) -> String {
        let n = self.character_name.trim();
        if n.is_empty() { "AIS.chat".to_string() } else { n.to_string() }
    }

    /// Read an invite link (`…/ua/characters/<id>/dialog?inviteCode=…`). If it points at a
    /// different character than before, the values found for the old one are forgotten.
    pub fn apply_invite_link(&mut self, link: &str) -> Result<()> {
        let (base, id, code) = parse_invite_link(link)?;
        let changed = id != self.character_id || code != self.invite_code || base != self.base();
        self.invite_link = link.trim().to_string();
        self.base_url = base;
        self.character_id = id;
        self.invite_code = code;
        if changed {
            self.next_action.clear();
            self.deployment_id.clear();
            self.character_name.clear();
            self.greeting.clear();
        }
        Ok(())
    }

    /// Read the link, `next-action` and `x-deployment-id` out of a "Copy as cURL" capture
    /// (bash or Windows cmd flavour). Nothing else of it is kept. Returns a short summary.
    pub fn import_curl(&mut self, curl: &str) -> Result<String> {
        let link = find_url(curl).ok_or_else(|| anyhow!("No address found in the pasted cURL."))?;
        self.apply_invite_link(&link)?;
        let mut found = Vec::new();
        if let Some(a) = header_value(curl, "next-action") {
            self.next_action = a;
            found.push("next-action");
        }
        if let Some(d) = header_value(curl, "x-deployment-id") {
            self.deployment_id = d;
            found.push("deployment id");
        }
        Ok(if found.is_empty() {
            "Read the invite link from the cURL (no next-action header in it).".to_string()
        } else {
            format!("Read the invite link and {} from the cURL.", found.join(" + "))
        })
    }

    fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("ais-chat-client").join("settings.json"))
    }

    pub fn path_display() -> String {
        Self::path().map(|p| p.display().to_string()).unwrap_or_else(|| "(no config folder)".into())
    }

    /// Saved settings, or the class's default invite link on first start.
    pub fn load() -> Self {
        let mut s: Settings = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if !s.is_linked() {
            let link = if s.invite_link.trim().is_empty() { DEFAULT_INVITE_LINK.to_string() } else { s.invite_link.clone() };
            if s.apply_invite_link(&link).is_err() {
                let _ = s.apply_invite_link(DEFAULT_INVITE_LINK);
            }
        }
        // a model id from the retired GitHub Models (before providers existed): back to the class bot
        if s.ai_provider.trim().is_empty() && !s.model.trim().is_empty() {
            s.model.clear();
        }
        // secrets in the password store are read in the background (see `load_secrets`);
        // secrets still in the file (older version, or no store last time) are moved by `save`
        if s.secrets().iter().any(|(_, v)| !v.is_empty()) {
            let _ = s.save();
        }
        s
    }

    /// The GitHub token and the AI keys from the system password store, as `(name, value)`.
    /// Can block for a long time – call it on a background thread.
    pub fn load_secrets() -> Vec<(String, String)> {
        let mut names = vec![TOKEN_NAME.to_string()];
        names.extend(crate::models::PROVIDERS.iter().map(|p| ai_key_name(p.id)));
        names.into_iter().filter_map(|n| crate::secret::load(&n).map(|v| (n, v))).collect()
    }

    /// Put a secret read from the password store into these settings (if not set already).
    pub fn set_secret(&mut self, name: &str, value: String) {
        if name == TOKEN_NAME {
            if self.github_token.trim().is_empty() {
                self.github_token = value;
            }
        } else if let Some(p) = name.strip_prefix("ai-key-") {
            let slot = self.ai_keys.entry(p.to_string()).or_default();
            if slot.trim().is_empty() {
                *slot = value;
            }
        }
    }

    /// Write settings.json – right away, without waiting for the password store. The GitHub
    /// token stays in the file until the store (in the background) has taken it; then it is
    /// removed from the file. Without a store it stays there, so it is never lost.
    pub fn save(&self) -> Result<()> {
        let p = Self::path().ok_or_else(|| anyhow!("No user config folder on this system."))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
        }
        let mut on_disk = self.clone();
        let mut to_store = Vec::new();
        for (name, value) in self.secrets() {
            match crate::secret::status(&name, &value) {
                crate::secret::Status::Stored => on_disk.forget_secret(&name, &value),
                crate::secret::Status::Unknown => to_store.push((name, value)),
                crate::secret::Status::Unavailable => {}
            }
        }
        write_settings_file(&p, &on_disk)?;
        if !to_store.is_empty() {
            std::thread::spawn(move || {
                for (name, value) in to_store {
                    if crate::secret::store(&name, &value).is_ok() && !value.is_empty() {
                        // stored: take it out of the file (if the file still holds this value)
                        if let Ok(text) = std::fs::read_to_string(&p) {
                            if let Ok(mut s) = serde_json::from_str::<Settings>(&text) {
                                s.forget_secret(&name, &value);
                                let _ = write_settings_file(&p, &s);
                            }
                        }
                    }
                }
            });
        }
        Ok(())
    }

    /// Where the GitHub token is kept, as far as known (never blocks).
    pub fn token_status(&self) -> crate::secret::Status {
        crate::secret::status(TOKEN_NAME, self.github_token.trim())
    }

    /// Where the current provider's API key is kept, as far as known (never blocks).
    pub fn ai_key_status(&self) -> crate::secret::Status {
        crate::secret::status(&ai_key_name(self.ai_provider().id), self.ai_key().trim())
    }
}

/// Settings are written from the window and from the background token saver.
fn write_settings_file(p: &std::path::Path, s: &Settings) -> Result<()> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITING.lock();
    std::fs::write(p, serde_json::to_string_pretty(s)?).with_context(|| format!("Could not write {}", p.display()))
}

/// `(https://host, character id, invite code)`
pub fn parse_invite_link(link: &str) -> Result<(String, String, String)> {
    let link = link.trim().trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    if link.is_empty() {
        return Err(anyhow!("Paste the invite link first."));
    }
    let with_scheme = if link.contains("://") { link.to_string() } else { format!("https://{link}") };
    let u = url::Url::parse(&with_scheme).map_err(|_| anyhow!("That does not look like a web address."))?;
    let host = u.host_str().ok_or_else(|| anyhow!("That does not look like a web address."))?;
    let base = match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    };
    let segs: Vec<&str> = u.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let id = segs
        .iter()
        .position(|s| *s == "characters")
        .and_then(|i| segs.get(i + 1))
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("The link has no /characters/<id>/ part. Use the invite link of the shared character."))?;
    let code = u
        .query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case("inviteCode"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("The link has no ?inviteCode=… part. Use the full invite link."))?;
    Ok((base, id, code))
}

/// First http(s) address in a cURL command.
fn find_url(curl: &str) -> Option<String> {
    let i = curl.find("http")?;
    let rest = &curl[i..];
    let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '^')).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// The value of header `name` in a cURL command (`-H 'name: value'` / `-H ^"name: value^"`).
fn header_value(curl: &str, name: &str) -> Option<String> {
    let lower = curl.to_lowercase();
    let key = format!("{name}:");
    let i = lower.find(&key)?;
    let v: String = curl[i + key.len()..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if v.is_empty() { None } else { Some(v) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_follows_the_language() {
        let mut s = Settings { greeting: crate::i18n::CLASS_GREETING.into(), ..Default::default() };
        let system = crate::i18n::known_greeting(crate::i18n::CLASS_GREETING, system_language()).unwrap();
        assert_eq!(s.displayed_greeting(), system, "automatic = the system's language");
        s.language = "de".into();
        assert_eq!(s.displayed_greeting(), crate::i18n::CLASS_GREETING);
        s.language = "en".into();
        assert_eq!(s.displayed_greeting(), "Hello, how can I help you with coding?");
        assert!(!s.greeting_needs_translation());

        s.greeting = "Hallo, ich bin der Geschichts-Bot.".into();
        assert_eq!(s.displayed_greeting(), "Hallo, ich bin der Geschichts-Bot.", "unknown until translated");
        assert!(s.greeting_needs_translation());
        s.greeting_translations.insert("en".into(), [s.greeting.clone(), "Hi, I am the history bot.".into()]);
        assert_eq!(s.displayed_greeting(), "Hi, I am the history bot.");
        assert!(!s.greeting_needs_translation());
        s.greeting = "Neuer Gruß".into();
        assert_eq!(s.displayed_greeting(), "Neuer Gruß", "a translation of an old greeting is not used");
    }

    #[test]
    fn separate_secrets_for_github_and_each_provider() {
        let mut s = Settings { github_token: "ghp".into(), ..Default::default() };
        s.ai_keys.insert("groq".into(), "gsk".into());
        s.ai_keys.insert("mistral".into(), "mst".into());
        let names: Vec<String> = s.secrets().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["github-token", "ai-key-groq", "ai-key-mistral"]);
        assert_eq!(s.ai_key(), "gsk", "Groq is the default provider");
        s.ai_provider = "mistral".into();
        assert_eq!(s.ai_key(), "mst");
        assert_eq!(s.ai_base(), "https://api.mistral.ai/v1");
        s.forget_secret("ai-key-mistral", "mst");
        s.forget_secret("github-token", "other");
        assert!(!s.ai_keys.contains_key("mistral") && s.github_token == "ghp");
        let mut fresh = Settings::default();
        fresh.set_secret("ai-key-groq", "gsk2".into());
        fresh.set_secret("github-token", "ghp2".into());
        assert_eq!((fresh.ai_key(), fresh.github_token.as_str()), ("gsk2".to_string(), "ghp2"));
    }

    #[test]
    fn custom_provider_uses_its_address() {
        let s = Settings { ai_provider: "custom".into(), ai_base_url: " http://localhost:11434/v1 ".into(), ..Default::default() };
        assert_eq!(s.ai_base(), "http://localhost:11434/v1");
    }

    #[test]
    fn reads_invite_link() {
        let (base, id, code) = parse_invite_link(DEFAULT_INVITE_LINK).unwrap();
        assert_eq!(base, "https://app.ais-chat.schule");
        assert_eq!(id, "6d16c5dd-1555-435f-ac22-3ab521a692b1");
        assert_eq!(code, "RVHN1H1B");
    }

    #[test]
    fn reads_link_without_scheme_and_with_spaces() {
        let (_, id, code) = parse_invite_link("  app.ais-chat.schule/ua/characters/abc/dialog?inviteCode=XY  ").unwrap();
        assert_eq!((id.as_str(), code.as_str()), ("abc", "XY"));
    }

    #[test]
    fn rejects_link_without_code() {
        assert!(parse_invite_link("https://app.ais-chat.schule/ua/characters/abc/dialog").is_err());
    }

    #[test]
    fn upload_address_is_automatic() {
        let mut s = Settings::default();
        s.apply_invite_link(DEFAULT_INVITE_LINK).unwrap();
        assert_eq!(s.upload_endpoint(), "https://app.ais-chat.schule/api/v1/shared-chat/files");
        s.upload_url = "https://example.org/up".into();
        assert_eq!(s.upload_endpoint(), "https://example.org/up");
    }

    #[test]
    fn new_character_forgets_old_values() {
        let mut s = Settings::default();
        s.apply_invite_link(DEFAULT_INVITE_LINK).unwrap();
        s.next_action = "abc".into();
        s.apply_invite_link(DEFAULT_INVITE_LINK).unwrap();
        assert_eq!(s.next_action, "abc", "same link keeps values");
        s.apply_invite_link("https://app.ais-chat.schule/ua/characters/other/dialog?inviteCode=Z").unwrap();
        assert!(s.next_action.is_empty());
    }

    #[test]
    fn imports_bash_and_cmd_curl() {
        let bash = "curl 'https://app.ais-chat.schule/ua/characters/cid/dialog?inviteCode=CODE' \\\n  -H 'accept: text/x-component' \\\n  -H 'next-action: 40468826f02e7c5404dc45e562ad6c0df1e8874951' \\\n  -H 'x-deployment-id: 72251121fc13' --data-raw '[]'";
        let mut s = Settings::default();
        s.import_curl(bash).unwrap();
        assert_eq!(s.character_id, "cid");
        assert_eq!(s.invite_code, "CODE");
        assert_eq!(s.next_action, "40468826f02e7c5404dc45e562ad6c0df1e8874951");
        assert_eq!(s.deployment_id, "72251121fc13");

        let cmd = "curl ^\"https://app.ais-chat.schule/ua/characters/cid/dialog?inviteCode=CODE^\" ^\n  -H ^\"next-action: 40aa^\"";
        let mut s = Settings::default();
        s.import_curl(cmd).unwrap();
        assert_eq!(s.invite_code, "CODE");
        assert_eq!(s.next_action, "40aa");
    }
}
