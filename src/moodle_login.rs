//! # Checking the school login (Moodle)
//!
//! Before the chat can be used, the app asks for the user name and password of the
//! school's Moodle (https://moodle.ohg-goe.net) and checks them there – exactly like
//! logging in on the Moodle website in a browser.
//!
//! ## What this file does with your password – and what it does NOT do
//!
//! - It sends the user name and password to ONE place only: the Moodle login page of the
//!   school, `https://moodle.ohg-goe.net/login/index.php`. The address is written once,
//!   below, in `MOODLE_ADDRESS`, and the code refuses to talk to any other address.
//! - The connection is always encrypted (https). Plain http is refused.
//! - The password is NOT saved: not in a file, not in the settings, not in the system
//!   password store, not in a log. It only exists in memory while the check runs.
//! - The password is NOT sent to the chat bot, to AIS.chat, to GitHub, to the sync
//!   server or anywhere else.
//! - After a successful check, the app logs out of Moodle again right away, so no
//!   Moodle session stays open.
//! - Nothing from your Moodle account is read: no courses, no grades, no messages, no
//!   name. The only thing the app learns is "the login worked" or "it did not".
//!
//! A plain-language explanation (English and German) is in `docs/MOODLE-LOGIN.md`.
//!
//! ## The steps (the same ones a browser does)
//!
//! 1. Open the login page. Moodle answers with the page and a "session cookie" (a
//!    random number that lets Moodle recognise the next request). The page contains a
//!    hidden one-time code, the "login token", which Moodle wants back with the form.
//! 2. Send the form: user name, password and the login token.
//! 3. Moodle answers with a forwarding ("redirect"). We follow it – but only to pages
//!    of the same Moodle.
//! 4. If the page we end up on has a "Log out" link, the login worked. Otherwise the
//!    user name or password was wrong.
//! 5. Click "Log out" (we call the log-out address), so the session ends.

use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};

/// The school's Moodle. This is the ONLY address the password is ever sent to.
pub const MOODLE_ADDRESS: &str = "https://moodle.ohg-goe.net";

/// The page with the login form.
const LOGIN_PAGE: &str = "https://moodle.ohg-goe.net/login/index.php";

/// The name of the cookie Moodle uses to recognise a session.
const SESSION_COOKIE_NAME: &str = "MoodleSession";

/// What came out of the check.
#[derive(Debug, PartialEq)]
pub enum LoginResult {
    /// Moodle accepted the user name and password.
    Accepted,
    /// Moodle did not accept them.
    WrongNameOrPassword,
    /// The check could not be done (no internet, Moodle down, …). The text says why.
    CouldNotCheck(&'static str),
}

/// Check the user name and password at the school's Moodle.
///
/// This takes a few seconds (it talks to Moodle over the internet), so the app calls it
/// in the background, not while drawing the window.
pub fn check_login(username: &str, password: &str) -> LoginResult {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return LoginResult::WrongNameOrPassword;
    }
    // Moodle has a public "guest" account that everybody can use. It does not prove
    // that someone belongs to the school, so it is not accepted here.
    if username.eq_ignore_ascii_case("guest") {
        return LoginResult::WrongNameOrPassword;
    }
    match try_login(username, password) {
        Ok(true) => LoginResult::Accepted,
        Ok(false) => LoginResult::WrongNameOrPassword,
        Err(problem) => LoginResult::CouldNotCheck(problem),
    }
}

/// The actual steps. `Ok(true)` = logged in, `Ok(false)` = not accepted, `Err` = could
/// not check.
fn try_login(username: &str, password: &str) -> Result<bool, &'static str> {
    // A connection that only speaks https and never follows forwardings on its own
    // (we follow them ourselves, below, so we can check where they lead).
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("ais-chat-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Could not start the connection.")?;

    // Step 1: open the login page; remember the session cookie and the login token.
    let page = client.get(LOGIN_PAGE).send().map_err(|_| could_not_reach())?;
    let mut session_cookie = session_cookie_from(&page).ok_or_else(unexpected_answer)?;
    let page_text = page.text().map_err(|_| could_not_reach())?;
    let login_token = login_token_from(&page_text).ok_or_else(unexpected_answer)?;

    // Step 2: send the login form – to the login page of the school's Moodle, nowhere else.
    let form = [("anchor", ""), ("logintoken", login_token.as_str()), ("username", username), ("password", password)];
    let mut answer = client
        .post(LOGIN_PAGE)
        .header(COOKIE, format!("{SESSION_COOKIE_NAME}={session_cookie}"))
        .form(&form)
        .send()
        .map_err(|_| could_not_reach())?;

    // Step 3: follow Moodle's forwardings (at most 5), but only within the same Moodle.
    // The password is NOT sent again; only the session cookie is.
    for _ in 0..5 {
        if let Some(new_cookie) = session_cookie_from(&answer) {
            session_cookie = new_cookie; // Moodle gives a new session number after logging in
        }
        let Some(next_address) = forwarding_address(&answer) else { break };
        if !next_address.starts_with(&format!("{MOODLE_ADDRESS}/")) {
            return Err(unexpected_answer()); // never leave the school's Moodle
        }
        answer = client
            .get(&next_address)
            .header(COOKIE, format!("{SESSION_COOKIE_NAME}={session_cookie}"))
            .send()
            .map_err(|_| could_not_reach())?;
    }
    let final_page = answer.text().map_err(|_| could_not_reach())?;

    // Step 4: a "Log out" link means we are logged in.
    let Some(logout_code) = logout_code_from(&final_page) else {
        return Ok(false);
    };

    // Step 5: log out again, so no session stays open. (If this fails, Moodle ends the
    // session by itself after a while; the check has worked either way.)
    let logout_address = format!("{MOODLE_ADDRESS}/login/logout.php?sesskey={logout_code}");
    let _ = client.get(&logout_address).header(COOKIE, format!("{SESSION_COOKIE_NAME}={session_cookie}")).send();

    Ok(true)
}

fn could_not_reach() -> &'static str {
    "Could not reach the school's Moodle. Check the internet connection."
}

fn unexpected_answer() -> &'static str {
    "The school's Moodle answered in an unexpected way. Try again later."
}

// ------------------------------------------------------------------ small helpers
// They only read text that Moodle sent back. None of them sends anything.

/// The value of the `MoodleSession` cookie in an answer, if Moodle set one.
fn session_cookie_from(answer: &Response) -> Option<String> {
    for header in answer.headers().get_all(SET_COOKIE) {
        let text = header.to_str().ok()?;
        // e.g. "MoodleSession=abc123; path=/; secure"
        if let Some(rest) = text.strip_prefix(&format!("{SESSION_COOKIE_NAME}=")) {
            let value = rest.split(';').next().unwrap_or("");
            if !value.is_empty() && value != "deleted" {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Where Moodle forwards us to (the `Location` of a redirect), as a full address.
fn forwarding_address(answer: &Response) -> Option<String> {
    if !answer.status().is_redirection() {
        return None;
    }
    let location = answer.headers().get(LOCATION)?.to_str().ok()?;
    if location.starts_with("https://") || location.starts_with("http://") {
        Some(location.to_string())
    } else {
        // a forwarding within the site, e.g. "/my/"
        Some(format!("{MOODLE_ADDRESS}/{}", location.trim_start_matches('/')))
    }
}

/// The hidden one-time code of the login form:
/// `<input type="hidden" name="logintoken" value="THIS">`
fn login_token_from(page: &str) -> Option<String> {
    let after_name = &page[page.find("name=\"logintoken\"")?..];
    let after_value = &after_name[after_name.find("value=\"")? + "value=\"".len()..];
    let token = &after_value[..after_value.find('"')?];
    (!token.is_empty()).then(|| token.to_string())
}

/// The code in the "Log out" link of a page for logged-in users:
/// `…/login/logout.php?sesskey=THIS`
fn logout_code_from(page: &str) -> Option<String> {
    let after = &page[page.find("logout.php?sesskey=")? + "logout.php?sesskey=".len()..];
    let code: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (!code.is_empty()).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_login_token() {
        let page = r#"<form id="login"><input id="anchor" type="hidden" name="anchor" value="">
            <input type="hidden" name="logintoken" value="Xy12ab"><input name="username"></form>"#;
        assert_eq!(login_token_from(page).as_deref(), Some("Xy12ab"));
        assert_eq!(login_token_from("<p>no form</p>"), None);
    }

    #[test]
    fn recognises_a_logged_in_page() {
        let logged_in = r#"<a class="dropdown-item" href="https://moodle.ohg-goe.net/login/logout.php?sesskey=AbC123xyz">Logout</a>"#;
        assert_eq!(logout_code_from(logged_in).as_deref(), Some("AbC123xyz"));
        let login_form = r#"<form class="login-form" action="https://moodle.ohg-goe.net/login/index.php">"#;
        assert_eq!(logout_code_from(login_form), None);
    }

    #[test]
    fn refuses_empty_and_guest_logins_without_asking_moodle() {
        assert_eq!(check_login("", "secret"), LoginResult::WrongNameOrPassword);
        assert_eq!(check_login("lena", ""), LoginResult::WrongNameOrPassword);
        assert_eq!(check_login("Guest", "guest"), LoginResult::WrongNameOrPassword);
    }

    /// Talks to the real Moodle once, with a made-up account:
    /// `cargo test -- --ignored real_moodle`
    /// With `MOODLE_TEST_USER` and `MOODLE_TEST_PASSWORD` set, it checks that a real
    /// account is accepted instead.
    #[test]
    #[ignore]
    fn real_moodle() {
        match (std::env::var("MOODLE_TEST_USER"), std::env::var("MOODLE_TEST_PASSWORD")) {
            (Ok(user), Ok(password)) => assert_eq!(check_login(&user, &password), LoginResult::Accepted),
            _ => assert_eq!(check_login("no-such-user-ais-chat-client-test", "not-a-password"), LoginResult::WrongNameOrPassword),
        }
    }

    #[test]
    fn talks_only_to_the_school_moodle_over_https() {
        assert!(MOODLE_ADDRESS.starts_with("https://"));
        assert!(LOGIN_PAGE.starts_with(MOODLE_ADDRESS));
        // the password is used in exactly one place in this file: the login form
        let code = include_str!("moodle_login.rs");
        let code = &code[..code.find("#[cfg(test)]").unwrap()];
        let uses = code.matches("(\"password\", password)").count();
        assert_eq!(uses, 1);
    }
}
