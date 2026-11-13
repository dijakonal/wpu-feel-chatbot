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

/// How the app was unlocked on the start screen ("moodle", "moin" or "none"). Sent with every
/// request, so the server can keep an app without a school login out of school accounts.
static APP_LOGIN: std::sync::RwLock<&'static str> = std::sync::RwLock::new("");

pub fn set_app_login(method: &'static str) {
    if let Ok(mut m) = APP_LOGIN.write() {
        *m = method;
    }
}

fn client() -> Result<Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    let login = APP_LOGIN.read().map(|m| *m).unwrap_or("");
    if !login.is_empty() {
        headers.insert("x-app-login", reqwest::header::HeaderValue::from_static(login));
    }
    Ok(Client::builder()
        .default_headers(headers)
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// The server address with `http://` or `https://` and without a trailing slash.
///
/// Without one, a computer in the same network (`localhost`, `192.168.…`, `10.…`,
/// `laptop.local`, a name without dots) gets `http://` – that is how the server runs when
/// started on a laptop. Anything else (`sync.school.example`) gets `https://`, because on the
/// internet the server belongs behind https.
pub fn normalize_server(input: &str) -> Result<String> {
    let t = input.trim().trim_end_matches('/');
    if t.is_empty() {
        bail!("Enter the address of the sync server.");
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        let host = url::Url::parse(&format!("http://{t}")).ok().and_then(|u| u.host().map(|h| h.to_owned()));
        match host {
            Some(h) if is_local(&h) => format!("http://{t}"),
            _ => format!("https://{t}"),
        }
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| anyhow!("That is not a server address."))?;
    if url.host_str().is_none() {
        bail!("That is not a server address.");
    }
    Ok(with_scheme)
}

/// A computer in the same (home or school) network, or this one.
fn is_local(host: &url::Host) -> bool {
    match host {
        url::Host::Ipv4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        // fc00::/7 = addresses that only exist inside a network
        url::Host::Ipv6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        url::Host::Domain(name) => {
            let name = name.to_lowercase();
            name == "localhost" || !name.contains('.') || [".local", ".lan", ".home", ".internal", ".fritz.box"].iter().any(|e| name.ends_with(e))
        }
    }
}

/// Plain http to another computer sends the password readable for everyone on the way.
pub fn is_insecure(server: &str) -> bool {
    let Ok(url) = url::Url::parse(server) else { return false };
    url.scheme() == "http" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// The error text of a server answer, or a fitting one for the status.
fn failure(status: u16, body: &str) -> anyhow::Error {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
    let pending = serde_json::from_str::<Value>(body).ok().is_some_and(|v| v["pending"] == true);
    let school_account = serde_json::from_str::<Value>(body).ok().is_some_and(|v| v["school_account"] == true);
    match (status, msg) {
        (401, _) => anyhow!(Unauthorized),
        (403, _) if school_account => anyhow!(SchoolAccount),
        (403, _) if pending => anyhow!(Pending),
        (_, Some(m)) => anyhow!(m),
        (404, None) => anyhow!("This address is not a sync server (404)."),
        (s, None) => anyhow!("The sync server answered with error {s}."),
    }
}

/// The saved login belongs to an account made with a school login, but the app now runs
/// without one – it cannot use that account.
#[derive(Debug)]
pub struct SchoolAccount;

impl std::fmt::Display for SchoolAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", crate::i18n::t("This account belongs to a school login. Without a school login you can only use accounts made without one."))
    }
}

impl std::error::Error for SchoolAccount {}

/// The account exists, but the person who runs the server has not approved it yet.
#[derive(Debug)]
pub struct Pending;

impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", crate::i18n::t("Your account is waiting for approval by the person who runs the sync server."))
    }
}

impl std::error::Error for Pending {}

/// The server no longer accepts the login token (logged out elsewhere, password changed, …).
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "The login has expired – log in again.")
    }
}

impl std::error::Error for Unauthorized {}

/// Why the server could not be reached, in words that say what to do.
fn unreachable(e: reqwest::Error) -> anyhow::Error {
    let https = e.url().is_some_and(|u| u.scheme() == "https");
    let address = e.url().map(|u| format!("{}:{}", u.host_str().unwrap_or(""), u.port_or_known_default().unwrap_or(0))).unwrap_or_default();
    anyhow!(explain_unreachable(&format!("{e:?}"), e.is_timeout(), https, &address))
}

fn explain_unreachable(details: &str, timeout: bool, https: bool, address: &str) -> String {
    let d = details.to_lowercase();
    if d.contains("dns error") {
        format!("The address {address} was not found. Check how it is spelled.")
    } else if d.contains("connectionrefused") || d.contains("connection refused") {
        format!(
            "Nothing answers at {address}. Is the sync server running there, and is the port right? \
             (The server prints its address when it starts.)"
        )
    } else if https && (d.contains("connectionreset") || d.contains("corrupt message") || d.contains("invalidcontenttype") || d.contains("handshake")) {
        "This server does not use https. Write the address with http:// in front, e.g. http://192.168.1.20:8787.".to_string()
    } else if timeout || d.contains("timed out") {
        format!(
            "The sync server at {address} does not answer. If it runs on another computer, its firewall may block the \
             port – allow the server there, or check that both computers are in the same network."
        )
    } else if d.contains("certificate") {
        "The server's https certificate is not valid.".to_string()
    } else {
        "Could not reach the sync server. Check the address and the internet connection.".to_string()
    }
}

fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let os = std::env::consts::OS;
    if host.is_empty() { format!("AIS.chat Client · {os}") } else { format!("AIS.chat Client · {os} · {host}") }
}

/// What logging in gave: the token, the name the server stored, and whether the account still
/// waits for approval.
#[derive(Debug)]
pub struct LoggedIn {
    pub token: String,
    pub username: String,
    pub pending: bool,
}

/// Log in (or create the account first). `method` tells a new account's server how the person
/// logged in to the app ("moodle", "moin" or "none"); the server shows it when approving.
pub fn login(server: &str, user: &str, password: &str, create: bool, method: &str) -> Result<LoggedIn> {
    let path = if create { "register" } else { "login" };
    let body = json!({ "username": user.trim(), "password": password, "device": device_name(), "method": method });
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
    Ok(LoggedIn {
        token: token.to_string(),
        username: v["username"].as_str().unwrap_or(user).to_string(),
        pending: v["pending"].as_bool().unwrap_or(false),
    })
}

/// Log in with the school account: the school user name and the code made from the school
/// password (`moodle_login::sync_code`). The account is created on first use.
pub fn login_with_school_account(server: &str, school_user: &str, code: &str, method: &str) -> Result<LoggedIn> {
    let user = school_user.trim().to_lowercase();
    let usable = (3..=32).contains(&user.chars().count())
        && user.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !usable {
        bail!("Your school user name cannot be used on the sync server. Log in there with your own name and password.");
    }
    match login(server, &user, code, false, method) {
        Ok(ok) => Ok(ok),
        Err(_) => match login(server, &user, code, true, method) {
            Ok(ok) => Ok(ok),
            Err(e) if e.to_string().contains("taken") => bail!(
                "The name {user} is already used on the sync server with another password (maybe your school password changed). \
                 Ask the person who runs the server to delete that account in the dashboard."
            ),
            Err(e) => Err(e),
        },
    }
}

/// A login for the server's dashboard (for an account the owner gave permissions to). The
/// app opens `<server>/admin#login=<token>` in the browser.
pub fn dashboard_login(server: &str, token: &str) -> Result<String> {
    let resp = client()?.post(format!("{server}/api/v1/dashboard")).bearer_auth(token).send().map_err(unreachable)?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(failure(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("Unexpected answer from the sync server."))?;
    // newer servers say where the dashboard is (its https address); older ones serve it here
    if let Some(url) = v["url"].as_str() {
        return Ok(url.to_string());
    }
    let login = v["token"].as_str().ok_or_else(|| anyhow!("Unexpected answer from the sync server."))?;
    Ok(format!("{server}/admin#login={login}"))
}

/// Make the token useless on the server (errors do not matter: the app forgets it anyway).
pub fn logout(server: &str, token: &str) {
    if let Ok(c) = client() {
        let _ = c.post(format!("{server}/api/v1/logout")).bearer_auth(token).send();
    }
}

/// Send the changes; returns the new revision and the changes from elsewhere.
/// Returns the new revision, the changes from elsewhere, the credits, and whether the account
/// may open the dashboard.
pub fn sync(server: &str, token: &str, out: &Outgoing) -> Result<(i64, Vec<Item>, Option<Credits>, bool)> {
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
        /// (servers before the credit system do not send this)
        credits: Option<Credits>,
        #[serde(default)]
        dashboard: bool,
    }
    let a: Answer = serde_json::from_str(&text).map_err(|_| anyhow!("Unexpected answer from the sync server."))?;
    Ok((a.rev, a.changes, a.credits, a.dashboard))
}

// ------------------------------------------------------------------ credits

/// Where the account's credits stand (one credit = one message to the bot). The person who
/// runs the server sets the limit in its dashboard.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Credits {
    /// Spent in the current period.
    pub used: i64,
    /// Credits per period; `None` = no limit.
    pub limit: Option<i64>,
    /// Unix time of the next refill, if there is one.
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub blocked: bool,
}

/// The server's answer to "may I send a message?".
#[derive(Debug, PartialEq)]
pub enum Spend {
    /// One credit was spent.
    Spent(Credits),
    /// The limit is used up (or the account is blocked): do not send.
    Refused(Credits),
    /// A server from before the credit system: nothing to check.
    NoCredits,
    /// The account still waits for approval.
    Pending,
}

/// Ask the server to spend one credit. The app calls this before each message to the bot.
pub fn spend_credit(server: &str, token: &str) -> Result<Spend> {
    let resp = client()?.post(format!("{server}/api/v1/credits/spend")).bearer_auth(token).send().map_err(unreachable)?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    read_spend(status, &text)
}

fn read_spend(status: u16, text: &str) -> Result<Spend> {
    let credits = || serde_json::from_str::<Value>(text).ok().and_then(|v| serde_json::from_value::<Credits>(v["credits"].clone()).ok());
    let pending = serde_json::from_str::<Value>(text).ok().is_some_and(|v| v["pending"] == true);
    match (status, credits()) {
        (403, _) if pending => Ok(Spend::Pending),
        (200, Some(c)) => Ok(Spend::Spent(c)),
        (402 | 403, Some(c)) => Ok(Spend::Refused(c)),
        (404, _) => Ok(Spend::NoCredits),
        _ => Err(failure(status, text)),
    }
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
        // computers in the same network: plain http, as the server runs there
        for (typed, expected) in [
            ("192.168.1.20:8787", "http://192.168.1.20:8787"),
            ("10.0.0.5:8787", "http://10.0.0.5:8787"),
            ("172.20.1.4:8787", "http://172.20.1.4:8787"),
            ("laptop:8787", "http://laptop:8787"),
            ("lenas-laptop.local:8787", "http://lenas-laptop.local:8787"),
            ("[::1]:8787", "http://[::1]:8787"),
            ("8.8.8.8:8787", "https://8.8.8.8:8787"),
            ("https://192.168.1.20:8787", "https://192.168.1.20:8787"),
        ] {
            assert_eq!(normalize_server(typed).unwrap(), expected, "{typed}");
        }
        assert!(normalize_server("").is_err());
        assert!(is_insecure("http://10.0.0.5:8787"));
        assert!(!is_insecure("http://localhost:8787"));
        assert!(!is_insecure("https://sync.example.org"));
    }

    #[test]
    fn reads_credit_answers() {
        let left = r#"{"credits":{"used":3,"limit":50,"period":"day","resets_at":1794438000,"blocked":false}}"#;
        assert_eq!(read_spend(200, left).unwrap(), Spend::Spent(Credits { used: 3, limit: Some(50), resets_at: Some(1794438000), blocked: false }));
        let out = r#"{"error":"No credits left.","credits":{"used":50,"limit":50,"period":"never","resets_at":null,"blocked":false}}"#;
        assert!(matches!(read_spend(402, out).unwrap(), Spend::Refused(c) if c.used == 50 && c.resets_at.is_none()));
        let blocked = r#"{"error":"blocked","credits":{"used":1,"limit":null,"period":"day","resets_at":1,"blocked":true}}"#;
        assert!(matches!(read_spend(403, blocked).unwrap(), Spend::Refused(c) if c.blocked));
        assert_eq!(read_spend(404, "").unwrap(), Spend::NoCredits, "an older server");
        assert_eq!(read_spend(403, r#"{"error":"waiting","pending":true}"#).unwrap(), Spend::Pending);
        assert!(failure(403, r#"{"error":"waiting","pending":true}"#).downcast_ref::<Pending>().is_some());
        assert!(failure(403, r#"{"error":"school","school_account":true}"#).downcast_ref::<SchoolAccount>().is_some());
        assert!(read_spend(401, "").unwrap_err().downcast_ref::<Unauthorized>().is_some());
        assert!(read_spend(500, "").is_err());
    }

    #[test]
    fn explains_why_the_server_is_unreachable() {
        let reset = r#"source: hyper_util::client::legacy::Error(Connect, Os { code: 104, kind: ConnectionReset, message: "Connection reset by peer" })"#;
        assert!(explain_unreachable(reset, false, true, "192.168.1.20:8787").contains("http://"), "https to an http server");
        let refused = r#"ConnectError("tcp connect error", 127.0.0.1:9, Os { code: 111, kind: ConnectionRefused })"#;
        assert!(explain_unreachable(refused, false, false, "127.0.0.1:9").starts_with("Nothing answers at 127.0.0.1:9"));
        let dns = r#"ConnectError("dns error", Custom { error: "Name or service not known" })"#;
        assert!(explain_unreachable(dns, false, false, "sync.exampel.org:443").contains("was not found"));
        assert!(explain_unreachable("operation timed out", true, false, "192.168.1.20:8787").contains("firewall"));
    }

    #[test]
    fn explains_server_errors() {
        assert!(failure(401, "").downcast_ref::<Unauthorized>().is_some());
        assert_eq!(failure(422, r#"{"error":"At most 1000 items"}"#).to_string(), "At most 1000 items");
        assert!(failure(404, "<html>").to_string().contains("not a sync server"));
    }

    /// Against a running server: `SYNC_TEST_SERVER=http://localhost:8787 cargo test -- --ignored school_account`
    #[test]
    #[ignore]
    fn school_account_login() {
        let server = std::env::var("SYNC_TEST_SERVER").expect("SYNC_TEST_SERVER");
        let name = format!("Schueler{}", crate::conversations::now() % 1_000_000);
        let code = crate::moodle_login::sync_code(&name, "school-password");
        // first computer: the account does not exist yet and is created
        let first = login_with_school_account(&server, &name, &code, "moodle").expect("create");
        assert_eq!(first.username, name.to_lowercase());
        // second computer, same school login: same account
        let second = login_with_school_account(&server, &name, &crate::moodle_login::sync_code(&name, "school-password"), "moodle").expect("login");
        assert_ne!(first.token, second.token);
        assert_eq!(first.pending, second.pending);
        // the school password itself does not work as the sync password
        assert!(login(&server, &name, "school-password", false, "moodle").is_err());
        // a changed school password gives another code: refused with an explanation
        let err = login_with_school_account(&server, &name, &crate::moodle_login::sync_code(&name, "new-password"), "moodle").unwrap_err();
        assert!(err.to_string().contains("already used"), "{err}");
        assert!(login_with_school_account(&server, "max@school", &code, "moodle").unwrap_err().to_string().contains("cannot be used"));
    }

    /// Against a running server: `SYNC_TEST_SERVER=http://localhost:8787 cargo test -- --ignored sync_server`
    #[test]
    #[ignore]
    fn sync_server_round_trip() {
        let server = std::env::var("SYNC_TEST_SERVER").expect("SYNC_TEST_SERVER");
        let name = format!("test{}", crate::conversations::now() % 1_000_000);
        let first = login(&server, &name, "test-password", true, "none").expect("register");
        assert_eq!(first.username, name);
        let (laptop, phone) = (first.token, login(&server, &name, "test-password", false, "none").expect("login").token);

        let mut here = vec![chat("a", 10, "from the laptop")];
        let mut state_here = SyncState::default();
        let out = outgoing(&here, &state_here);
        let (rev, changes, _, _) = sync(&server, &laptop, &out).expect("sync");
        apply(&mut here, &mut state_here, &out, rev, changes);

        let mut there = vec![chat("b", 20, "from the phone")];
        let mut state_there = SyncState::default();
        let out = outgoing(&there, &state_there);
        let (rev, changes, _, _) = sync(&server, &phone, &out).expect("sync");
        apply(&mut there, &mut state_there, &out, rev, changes);
        assert_eq!(there.len(), 2, "the phone got the laptop's chat");

        let out = outgoing(&here, &state_here);
        let (rev, changes, _, _) = sync(&server, &laptop, &out).expect("sync");
        apply(&mut here, &mut state_here, &out, rev, changes);
        assert_eq!(here.len(), 2, "and the other way round");

        // credits: every spend counts up; the sync answer reports the same number
        let Spend::Spent(first) = spend_credit(&server, &laptop).expect("spend") else { panic!("not spent") };
        let Spend::Spent(second) = spend_credit(&server, &phone).expect("spend") else { panic!("not spent") };
        assert_eq!(second.used, first.used + 1, "both computers share the account's credits");
        let (_, _, credits, _) = sync(&server, &laptop, &outgoing(&here, &state_here)).expect("sync");
        assert_eq!(credits.map(|c| c.used), Some(second.used));

        logout(&server, &phone);
        let err = sync(&server, &phone, &outgoing(&there, &state_there)).unwrap_err();
        assert!(err.downcast_ref::<Unauthorized>().is_some());
    }
}
