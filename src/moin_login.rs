//! # Checking the school login (moin.schule)
//!
//! The second way to log in: with the account of **moin.schule**, the school login of Lower
//! Saxony. The app checks the user name and password there – exactly like logging in on the
//! moin.schule website in a browser. It works like `moodle_login.rs`; read that file first.
//!
//! ## What this file does with your password – and what it does NOT do
//!
//! - It sends the user name and password to ONE place only: the login page of moin.schule at
//!   `https://auth.moin.schule`. The address is written once, below, in `LOGIN_SERVER`, and the
//!   code refuses to talk to any other address.
//! - The connection is always encrypted (https).
//! - The password is NOT saved, and NOT sent to the chat bot, AIS.chat, GitHub or anywhere
//!   else. (For the sync server the app makes a separate code from it – see `sync_code` in
//!   `moodle_login.rs`.)
//! - Nothing from your moin.schule account is read. After a successful login moin.schule hands
//!   out a "ticket"; the app does not look inside it and only uses it to log out again.
//!
//! ## The steps (the same ones the moin.schule website does)
//!
//! 1. Open the login page, the way moin.schule's own website does when you click "Anmelden".
//!    moin.schule answers with the page and a few cookies (random numbers that let it
//!    recognise the next request). The page says where to send the form.
//! 2. Send the form there: user name and password.
//! 3. A correct login is answered with a forwarding back to the moin.schule website with a
//!    one-time `code`. A wrong one is answered with the login page again.
//! 4. Log out again: swap the one-time code for the ticket (that is what the website does
//!    too) and hand the ticket back with "log out", so no session stays open.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};

use crate::moodle_login::LoginResult;

/// The login server of moin.schule. This is the ONLY address the password is ever sent to.
pub const LOGIN_SERVER: &str = "https://auth.moin.schule";

/// moin.schule's login area ("realm") and the name its website uses there ("client").
const REALM: &str = "moins";
const WEBSITE: &str = "sanis-frontend";
/// Where moin.schule sends the browser back to after logging in (its own website).
const BACK_TO: &str = "https://moin.schule/";

/// Check the user name and password at moin.schule. Takes a few seconds; the app calls it in
/// the background.
pub fn check_login(username: &str, password: &str) -> LoginResult {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return LoginResult::WrongNameOrPassword;
    }
    match try_login(username, password) {
        Ok(true) => LoginResult::Accepted,
        Ok(false) => LoginResult::WrongNameOrPassword,
        Err(problem) => LoginResult::CouldNotCheck(problem),
    }
}

fn try_login(username: &str, password: &str) -> Result<bool, &'static str> {
    // only https, and no following of forwardings on its own (we check where they lead)
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Could not start the connection.")?;
    let mut cookies = Cookies::default();

    // Step 1: open the login page like moin.schule's website does. The two random values
    // (`verifier` and its fingerprint `challenge`) are part of every such login: they make
    // sure only the one who started the login can finish it.
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = base64url(&sha2_256(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().simple().to_string();
    let open = format!(
        "{LOGIN_SERVER}/realms/{REALM}/protocol/openid-connect/auth?client_id={WEBSITE}&redirect_uri={}\
         &response_type=code&scope=openid&state={state}&code_challenge={challenge}&code_challenge_method=S256",
        encode(BACK_TO)
    );
    let page = client.get(&open).send().map_err(|_| could_not_reach())?;
    cookies.take_from(&page);
    let page_text = page.text().map_err(|_| could_not_reach())?;
    let form_address = form_address_from(&page_text).ok_or_else(unexpected_answer)?;
    if !form_address.starts_with(&format!("{LOGIN_SERVER}/")) {
        return Err(unexpected_answer()); // never send the password anywhere else
    }

    // Step 2: send the form – to moin.schule's login server, nowhere else.
    let answer = client
        .post(&form_address)
        .header(COOKIE, cookies.header())
        .form(&[("username", username), ("password", password)])
        .send()
        .map_err(|_| could_not_reach())?;

    // Step 3: a forwarding back to the website with a one-time code = logged in.
    let forwarded_to = answer.headers().get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if answer.status().is_redirection() {
        let Some(code) = code_from(&forwarded_to, &state) else {
            return Err(unexpected_answer());
        };
        log_out(&client, &code, &verifier); // step 4
        return Ok(true);
    }
    let answer_text = answer.text().map_err(|_| could_not_reach())?;
    match page_kind(&answer_text).as_deref() {
        Some("login") => Ok(false), // the login page again: wrong name or password
        // another page (a second step like a code from an app, or "change your password")
        _ => Err("moin.schule wants another step for this account. Log in on moin.schule in the browser once, then try again."),
    }
}

/// Step 4: swap the one-time code for the ticket and hand it back with "log out". Errors do
/// not matter: the check has worked, and moin.schule ends unused sessions by itself.
fn log_out(client: &Client, code: &str, verifier: &str) {
    let tokens = client
        .post(format!("{LOGIN_SERVER}/realms/{REALM}/protocol/openid-connect/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", WEBSITE),
            ("redirect_uri", BACK_TO),
            ("code", code),
            ("code_verifier", verifier),
        ])
        .send()
        .and_then(|r| r.text());
    // only the ticket for logging out is taken from the answer – nothing else is looked at
    let ticket = tokens.ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["refresh_token"].as_str().map(str::to_string));
    let Some(ticket) = ticket else { return };
    let _ = client
        .post(format!("{LOGIN_SERVER}/realms/{REALM}/protocol/openid-connect/logout"))
        .form(&[("client_id", WEBSITE), ("refresh_token", ticket.as_str())])
        .send();
}

fn could_not_reach() -> &'static str {
    "Could not reach moin.schule. Check the internet connection."
}

fn unexpected_answer() -> &'static str {
    "moin.schule answered in an unexpected way. Try again later."
}

// ------------------------------------------------------------------ small helpers
// They only read text that moin.schule sent back, or prepare values. None of them sends
// anything.

/// The cookies moin.schule set, sent back with the next request (like a browser does).
#[derive(Default)]
struct Cookies(BTreeMap<String, String>);

impl Cookies {
    fn take_from(&mut self, answer: &Response) {
        for header in answer.headers().get_all(SET_COOKIE) {
            let Ok(text) = header.to_str() else { continue };
            // e.g. "AUTH_SESSION_ID=abc.def; Path=/realms/moins/; Secure; HttpOnly"
            if let Some((name, rest)) = text.split_once('=') {
                self.0.insert(name.trim().to_string(), rest.split(';').next().unwrap_or("").to_string());
            }
        }
    }

    fn header(&self) -> String {
        self.0.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ")
    }
}

/// Where the login form is sent to. The page holds it as `"loginAction": "https:\/\/…"`.
fn form_address_from(page: &str) -> Option<String> {
    let after = &page[page.find("\"loginAction\"")? + "\"loginAction\"".len()..];
    let start = after.find('"')? + 1;
    let end = start + after[start..].find('"')?;
    Some(after[start..end].replace("\\/", "/").replace("&amp;", "&"))
}

/// Which page moin.schule showed: `"pageId": "login"` is the login form.
fn page_kind(page: &str) -> Option<String> {
    let after = &page[page.find("\"pageId\"")? + "\"pageId\"".len()..];
    let start = after.find('"')? + 1;
    let end = start + after[start..].find('"')?;
    Some(after[start..end].to_string())
}

/// The one-time code in the forwarding back to the website – only if it really goes back to
/// moin.schule's website and belongs to this login (`state`).
fn code_from(forwarded_to: &str, state: &str) -> Option<String> {
    let url = url::Url::parse(forwarded_to).ok()?;
    if !forwarded_to.starts_with(BACK_TO) {
        return None;
    }
    let value = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.to_string());
    if value("state").as_deref() != Some(state) || value("error").is_some() {
        return None;
    }
    value("code").filter(|c| !c.is_empty())
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn sha2_256(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}

/// Base64 for web addresses (`-` and `_`, no `=` at the end).
fn base64url(data: &[u8]) -> String {
    crate::github::base64(data).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_where_the_form_goes() {
        let page = r#"const kcContext = { "url": { "loginAction": "https:\/\/auth.moin.schule\/realms\/moins\/login-actions\/authenticate?session_code=abc&amp;execution=1", }, "pageId": "login" };"#;
        assert_eq!(form_address_from(page).unwrap(), "https://auth.moin.schule/realms/moins/login-actions/authenticate?session_code=abc&execution=1");
        assert_eq!(page_kind(page).as_deref(), Some("login"));
        assert_eq!(form_address_from("<p>nothing</p>"), None);
    }

    #[test]
    fn takes_the_code_only_from_the_right_forwarding() {
        assert_eq!(code_from("https://moin.schule/?state=s1&session_state=x&code=c0de", "s1").as_deref(), Some("c0de"));
        assert_eq!(code_from("https://moin.schule/?state=other&code=c0de", "s1"), None, "another login's code");
        assert_eq!(code_from("https://evil.example/?state=s1&code=c0de", "s1"), None, "another website");
        assert_eq!(code_from("https://moin.schule/?state=s1&error=access_denied", "s1"), None);
    }

    #[test]
    fn challenge_is_the_standard_one() {
        // the example from RFC 7636, appendix B
        assert_eq!(base64url(&sha2_256(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn talks_only_to_moin_schule_over_https() {
        assert!(LOGIN_SERVER.starts_with("https://") && BACK_TO.starts_with("https://"));
        let code = include_str!("moin_login.rs");
        let code = &code[..code.find("#[cfg(test)]").unwrap()];
        assert_eq!(code.matches("(\"password\", password)").count(), 1, "the password goes into one form only");
    }

    /// Talks to the real moin.schule once, with a made-up account:
    /// `cargo test -- --ignored real_moin`. With `MOIN_TEST_USER` and `MOIN_TEST_PASSWORD` set,
    /// it checks that a real account is accepted instead.
    #[test]
    #[ignore]
    fn real_moin() {
        match (std::env::var("MOIN_TEST_USER"), std::env::var("MOIN_TEST_PASSWORD")) {
            (Ok(user), Ok(password)) => assert_eq!(check_login(&user, &password), LoginResult::Accepted),
            _ => assert_eq!(check_login("no-such-user-ais-chat-client-test", "not-a-password"), LoginResult::WrongNameOrPassword),
        }
    }
}
