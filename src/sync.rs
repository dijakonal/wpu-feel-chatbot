//! Account and chat sync with the sync server (repository `wpu-feel-chatbot-backend`).
//!
//! Every chat is sent as a whole JSON document with its `updated` time. The server keeps the
//! newest version of each chat and a revision number per account; a sync sends the chats that
//! changed here and gets back everything that changed elsewhere since the last sync.
//! Deleted chats are sent as "deleted" markers so other computers remove them too.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::conversations::Conversation;

// ------------------------------------------------------------------ what is remembered between syncs

/// Saved in `sync.json` next to the chats.
#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct SyncState {
    /// Server address and account this state belongs to (another account starts from zero).
    pub account: String,
    /// Revision of the last sync.
    pub rev: i64,
    /// For each chat: a fingerprint of the version the server has, so unchanged chats are not
    /// sent again.
    pub known: BTreeMap<String, u64>,
    /// Chats deleted here that the server does not know about yet: id → time of deleting.
    pub deleted: BTreeMap<String, i64>,
    /// Unix time of the last successful sync.
    pub last_sync: i64,
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ais-chat-client").join("sync.json"))
}

impl SyncState {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string(self) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    /// Start over for another account (or after logging in again).
    pub fn reset(&mut self, account: &str) {
        *self = SyncState { account: account.to_string(), ..Default::default() };
    }
}

/// A stable fingerprint of a chat's saved form (FNV-1a over its JSON).
pub fn fingerprint(c: &Conversation) -> u64 {
    let text = serde_json::to_string(c).unwrap_or_default();
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

// ------------------------------------------------------------------ one sync

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: String,
    pub id: String,
    pub updated: i64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// What one sync sends, and what to remember once the server took it.
#[derive(Clone)]
pub struct Outgoing {
    pub since: i64,
    pub items: Vec<Item>,
    /// Fingerprints of the chats sent.
    pub sent: Vec<(String, u64)>,
    /// Deleted markers sent.
    pub sent_deleted: Vec<String>,
}

/// The chats that changed since the server last saw them (not those still being answered or
/// still empty), and the deletions it does not know yet.
pub fn outgoing(convs: &[Conversation], state: &SyncState) -> Outgoing {
    let mut items = Vec::new();
    let mut sent = Vec::new();
    for c in convs.iter().filter(|c| !c.busy && !c.is_empty()) {
        let fp = fingerprint(c);
        if state.known.get(&c.id) == Some(&fp) {
            continue;
        }
        let Ok(data) = serde_json::to_value(c) else { continue };
        items.push(Item { kind: "chat".into(), id: c.id.clone(), updated: c.updated, deleted: false, data: Some(data) });
        sent.push((c.id.clone(), fp));
    }
    let mut sent_deleted = Vec::new();
    for (id, at) in &state.deleted {
        // a chat deleted here but still (or again) in the list is not deleted
        if convs.iter().any(|c| &c.id == id) {
            continue;
        }
        items.push(Item { kind: "chat".into(), id: id.clone(), updated: *at, deleted: true, data: None });
        sent_deleted.push(id.clone());
    }
    Outgoing { since: state.rev, items, sent, sent_deleted }
}

/// What changed in the chat list after a sync.
#[derive(Default, Debug, PartialEq)]
pub struct Applied {
    pub added: usize,
    pub updated: usize,
    pub removed: Vec<String>,
}

impl Applied {
    pub fn anything(&self) -> bool {
        self.added + self.updated + self.removed.len() > 0
    }
}

/// Note what the server accepted and merge what came from other computers into `convs`.
/// A change here that is newer than the one from the server is kept (and sent next time).
pub fn apply(convs: &mut Vec<Conversation>, state: &mut SyncState, out: &Outgoing, rev: i64, changes: Vec<Item>) -> Applied {
    for (id, fp) in &out.sent {
        state.known.insert(id.clone(), *fp);
    }
    for id in &out.sent_deleted {
        state.deleted.remove(id);
        state.known.remove(id);
    }
    let mut applied = Applied::default();
    for item in changes.into_iter().filter(|i| i.kind == "chat") {
        let local = convs.iter().position(|c| c.id == item.id);
        if item.deleted {
            state.known.remove(&item.id);
            if let Some(i) = local {
                let c = &convs[i];
                if !c.busy && c.updated <= item.updated {
                    applied.removed.push(c.id.clone());
                    convs.remove(i);
                }
            }
            continue;
        }
        let Some(remote) = item.data.and_then(|d| serde_json::from_value::<Conversation>(d).ok()) else { continue };
        if remote.id != item.id {
            continue;
        }
        match local {
            None => {
                state.known.insert(remote.id.clone(), fingerprint(&remote));
                // a chat this computer deleted is brought back by a newer change elsewhere
                state.deleted.remove(&remote.id);
                convs.push(remote);
                applied.added += 1;
            }
            Some(i) => {
                let c = &mut convs[i];
                if c.busy || c.updated > remote.updated {
                    continue; // ours is newer: it goes to the server next time
                }
                state.known.insert(remote.id.clone(), fingerprint(&remote));
                let changed = fingerprint(c) != fingerprint(&remote);
                // keep what only exists while the app runs (files being uploaded, the quote, …)
                let pending = std::mem::take(&mut c.pending);
                let quote = c.quote.take();
                let gen = c.gen;
                *c = remote;
                c.pending = pending;
                c.quote = quote;
                c.gen = gen;
                if changed {
                    applied.updated += 1;
                }
            }
        }
    }
    state.rev = rev;
    state.last_sync = crate::conversations::now();
    applied
}

// ------------------------------------------------------------------ the server

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// `https://host` without a trailing slash; `host:port` alone means https (http for
/// localhost).
pub fn normalize_server(input: &str) -> Result<String> {
    let t = input.trim().trim_end_matches('/');
    if t.is_empty() {
        bail!("Enter the address of the sync server.");
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else if t.starts_with("localhost") || t.starts_with("127.0.0.1") {
        format!("http://{t}")
    } else {
        format!("https://{t}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| anyhow!("That is not a server address."))?;
    if url.host_str().is_none() {
        bail!("That is not a server address.");
    }
    Ok(with_scheme)
}

/// Plain http to another computer sends the password readable for everyone on the way.
pub fn is_insecure(server: &str) -> bool {
    let Ok(url) = url::Url::parse(server) else { return false };
    url.scheme() == "http" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// The error text of a server answer, or a fitting one for the status.
fn failure(status: u16, body: &str) -> anyhow::Error {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
    match (status, msg) {
        (401, _) => anyhow!(Unauthorized),
        (_, Some(m)) => anyhow!(m),
        (404, None) => anyhow!("This address is not a sync server (404)."),
        (s, None) => anyhow!("The sync server answered with error {s}."),
    }
}

/// The server no longer accepts the login token (logged out elsewhere, password changed, …).
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "The login has expired – log in again.")
    }
}

impl std::error::Error for Unauthorized {}

fn unreachable(e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        anyhow!("The sync server does not answer.")
    } else {
        anyhow!("Could not reach the sync server. Check the address and the internet connection.")
    }
}

fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let os = std::env::consts::OS;
    if host.is_empty() { format!("AIS.chat Client · {os}") } else { format!("AIS.chat Client · {os} · {host}") }
}

/// Log in (or create the account first). Returns the login token and the stored user name.
pub fn login(server: &str, user: &str, password: &str, create: bool) -> Result<(String, String)> {
    let path = if create { "register" } else { "login" };
    let body = json!({ "username": user.trim(), "password": password, "device": device_name() });
    let resp = client()?.post(format!("{server}/api/v1/{path}")).header("content-type", "application/json").body(body.to_string()).send().map_err(unreachable)?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        if status == 401 {
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string));
            bail!(msg.unwrap_or_else(|| "Wrong user name or password.".into()));
        }
        return Err(failure(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("This address is not a sync server."))?;
    let token = v["token"].as_str().ok_or_else(|| anyhow!("This address is not a sync server."))?;
    Ok((token.to_string(), v["username"].as_str().unwrap_or(user).to_string()))
}

/// Make the token useless on the server (errors do not matter: the app forgets it anyway).
pub fn logout(server: &str, token: &str) {
    if let Ok(c) = client() {
        let _ = c.post(format!("{server}/api/v1/logout")).bearer_auth(token).send();
    }
}

/// Send the changes; returns the new revision and the changes from elsewhere.
pub fn sync(server: &str, token: &str, out: &Outgoing) -> Result<(i64, Vec<Item>)> {
    let body = json!({ "since": out.since, "changes": out.items });
    let resp = client()?.post(format!("{server}/api/v1/sync")).bearer_auth(token).header("content-type", "application/json").body(body.to_string()).send().map_err(unreachable)?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(failure(status, &text));
    }
    #[derive(Deserialize)]
    struct Answer {
        rev: i64,
        changes: Vec<Item>,
    }
    let a: Answer = serde_json::from_str(&text).map_err(|_| anyhow!("Unexpected answer from the sync server."))?;
    Ok((a.rev, a.changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::ChatMessage;

    fn chat(id: &str, updated: i64, text: &str) -> Conversation {
        let mut c = Conversation::new("Hi");
        c.id = id.into();
        c.messages.push(ChatMessage::user(text));
        c.updated = updated;
        c
    }

    fn item(c: &Conversation) -> Item {
        Item { kind: "chat".into(), id: c.id.clone(), updated: c.updated, deleted: false, data: Some(serde_json::to_value(c).unwrap()) }
    }

    #[test]
    fn sends_only_changed_chats() {
        let mut convs = vec![chat("a", 10, "one"), chat("b", 10, "two"), Conversation::new("Hi")];
        let mut state = SyncState::default();
        let out = outgoing(&convs, &state);
        assert_eq!(out.items.len(), 2, "the empty chat is not sent");
        apply(&mut convs, &mut state, &out, 2, vec![]);
        assert_eq!(state.rev, 2);
        assert!(outgoing(&convs, &state).items.is_empty(), "nothing changed since");

        convs[0].custom_title = Some("Renamed".into());
        let out = outgoing(&convs, &state);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].id, "a");
        assert_eq!(out.since, 2);

        convs[1].busy = true;
        convs[1].messages.push(ChatMessage::user("more"));
        assert_eq!(outgoing(&convs, &state).items.len(), 1, "a chat being answered waits");
    }

    #[test]
    fn merges_chats_from_elsewhere() {
        let mut convs = vec![chat("a", 10, "mine"), chat("b", 50, "newer here")];
        let mut state = SyncState::default();
        let out = outgoing(&[], &state);
        let from_server = vec![
            item(&chat("a", 20, "changed elsewhere")),
            item(&chat("b", 40, "older there")),
            item(&chat("c", 30, "new elsewhere")),
        ];
        convs[0].quote = Some("kept".into());
        let applied = apply(&mut convs, &mut state, &out, 7, from_server);
        assert_eq!(applied, Applied { added: 1, updated: 1, removed: vec![] });
        assert_eq!(convs.len(), 3);
        assert_eq!(convs[0].messages[1].content, "changed elsewhere");
        assert_eq!(convs[0].quote.as_deref(), Some("kept"), "what only exists while running stays");
        assert_eq!(convs[1].messages[1].content, "newer here", "the newer change wins");
        assert_eq!(convs[2].id, "c");
        // "b" is newer here and goes to the server next time; "a" and "c" do not go back
        let next = outgoing(&convs, &state);
        assert_eq!(next.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn deletions_travel_both_ways() {
        let mut convs = vec![chat("a", 10, "x"), chat("b", 10, "y")];
        let mut state = SyncState::default();
        let out = outgoing(&convs, &state);
        apply(&mut convs, &mut state, &out, 2, vec![]);

        // deleted here
        convs.retain(|c| c.id != "a");
        state.deleted.insert("a".into(), 20);
        let out = outgoing(&convs, &state);
        assert_eq!(out.items.len(), 1);
        assert!(out.items[0].deleted && out.items[0].data.is_none());
        apply(&mut convs, &mut state, &out, 3, vec![]);
        assert!(state.deleted.is_empty());

        // deleted elsewhere
        let gone = Item { kind: "chat".into(), id: "b".into(), updated: 30, deleted: true, data: None };
        let out = outgoing(&convs, &state);
        let applied = apply(&mut convs, &mut state, &out, 4, vec![gone.clone()]);
        assert_eq!(applied.removed, ["b"]);
        assert!(convs.is_empty());

        // … but a chat changed here after that stays
        let mut convs = vec![chat("b", 40, "still needed")];
        let out = outgoing(&convs, &state);
        let applied = apply(&mut convs, &mut state, &out, 5, vec![gone]);
        assert!(applied.removed.is_empty());
        assert_eq!(convs.len(), 1);
    }

    #[test]
    fn checks_server_addresses() {
        assert_eq!(normalize_server(" sync.example.org/ ").unwrap(), "https://sync.example.org");
        assert_eq!(normalize_server("localhost:8787").unwrap(), "http://localhost:8787");
        assert_eq!(normalize_server("http://10.0.0.5:8787").unwrap(), "http://10.0.0.5:8787");
        assert!(normalize_server("").is_err());
        assert!(is_insecure("http://10.0.0.5:8787"));
        assert!(!is_insecure("http://localhost:8787"));
        assert!(!is_insecure("https://sync.example.org"));
    }

    #[test]
    fn explains_server_errors() {
        assert!(failure(401, "").downcast_ref::<Unauthorized>().is_some());
        assert_eq!(failure(422, r#"{"error":"At most 1000 items"}"#).to_string(), "At most 1000 items");
        assert!(failure(404, "<html>").to_string().contains("not a sync server"));
    }

    /// Against a running server: `SYNC_TEST_SERVER=http://localhost:8787 cargo test -- --ignored sync_server`
    #[test]
    #[ignore]
    fn sync_server_round_trip() {
        let server = std::env::var("SYNC_TEST_SERVER").expect("SYNC_TEST_SERVER");
        let name = format!("test{}", crate::conversations::now() % 1_000_000);
        let (token, user) = login(&server, &name, "test-password", true).expect("register");
        assert_eq!(user, name);
        let (laptop, phone) = (token, login(&server, &name, "test-password", false).expect("login").0);

        let mut here = vec![chat("a", 10, "from the laptop")];
        let mut state_here = SyncState::default();
        let out = outgoing(&here, &state_here);
        let (rev, changes) = sync(&server, &laptop, &out).expect("sync");
        apply(&mut here, &mut state_here, &out, rev, changes);

        let mut there = vec![chat("b", 20, "from the phone")];
        let mut state_there = SyncState::default();
        let out = outgoing(&there, &state_there);
        let (rev, changes) = sync(&server, &phone, &out).expect("sync");
        apply(&mut there, &mut state_there, &out, rev, changes);
        assert_eq!(there.len(), 2, "the phone got the laptop's chat");

        let out = outgoing(&here, &state_here);
        let (rev, changes) = sync(&server, &laptop, &out).expect("sync");
        apply(&mut here, &mut state_here, &out, rev, changes);
        assert_eq!(here.len(), 2, "and the other way round");

        logout(&server, &phone);
        let err = sync(&server, &phone, &outgoing(&there, &state_there)).unwrap_err();
        assert!(err.downcast_ref::<Unauthorized>().is_some());
    }
}
