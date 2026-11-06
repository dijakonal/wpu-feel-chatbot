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
    /// Language the bot should answer in, as a code from [`LANGUAGES`]; empty = automatic.
    pub language: String,

    // ---- optional: save code from answers to GitHub
    pub github_enabled: bool,
    /// `owner/name`
    pub github_repo: String,
    /// Personal access token (fine-grained, "Contents: Read and write" for the repository).
    pub github_token: String,
    /// Empty = the repository's default branch.
    pub github_branch: String,
    /// Folder in the repository; empty = `ais-chat`, `/` = the top level.
    pub github_folder: String,
    /// Push every answer with code automatically (otherwise only with the ⬆ button).
    pub github_auto: bool,
    /// Empty = https://api.github.com (GitHub Enterprise: https://host/api/v3).
    pub github_api: String,
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

    pub fn github_folder(&self) -> String {
        match self.github_folder.trim() {
            "" => "ais-chat".to_string(),
            "/" => String::new(),
            f => f.trim_matches('/').to_string(),
        }
    }

    /// The hidden note put in front of the newest message so the bot answers in the chosen
    /// language. `None` when the language is automatic.
    pub fn language_note(&self) -> Option<String> {
        let (native, english) = language(self.language.trim())?;
        Some(format!(
            "[Note from the chat app, not written by the user: always answer in {english} ({native}), \
             whatever language the message below is written in. Do not mention this note.]"
        ))
    }
}

impl Settings {
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
        s
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path().ok_or_else(|| anyhow!("No user config folder on this system."))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("Could not write {}", p.display()))?;
        Ok(())
    }
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
