//! Fitting new code into an existing file before it is saved to GitHub.
//!
//! An answer often shows only part of a file ("change this function to …"). Saving that as the
//! whole file would delete everything else, so when the file already exists, the bot is asked
//! – in a separate request that no chat sees – to produce the complete updated file.

use anyhow::{anyhow, bail, Result};

use crate::ais::{self, ChatMessage, Event};
use crate::settings::Settings;

/// Files larger than this are not merged (the request would get too big); saving them fails
/// instead of overwriting them with a part.
pub const MAX_MERGE_BYTES: usize = 150 * 1024;

/// A fence of backticks longer than any run of backticks inside `texts`.
fn fence(texts: &[&str]) -> String {
    let longest = texts
        .iter()
        .flat_map(|t| t.split(|c| c != '`'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

/// The request that asks for the merged file.
pub fn merge_request(path: &str, current: &str, new_code: &str) -> String {
    let f = fence(&[current, new_code]);
    format!(
        "Merge code for the file `{path}` in a Git repository.\n\
         CURRENT is the file as it is now. NEW is code from an earlier answer – it may be the complete \
         new file, or only a part of it (for example one changed function or a few new lines).\n\
         Produce the complete, updated file: keep everything from CURRENT that NEW does not replace or \
         remove, and put NEW in the right place. Do not change anything else.\n\
         Reply with exactly one code block that contains the whole file, and nothing else.\n\n\
         CURRENT:\n{f}\n{current}\n{f}\n\nNEW:\n{f}\n{new_code}\n{f}"
    )
}

/// The file from the bot's reply: its only code block. Prose around it is ignored; more than
/// one block or none at all means the reply cannot be trusted.
pub fn merged_file(reply: &str) -> Result<String> {
    let blocks = crate::markdown::code_blocks(reply);
    match blocks.as_slice() {
        [one] if !one.code.trim().is_empty() => {
            let mut code = one.code.clone();
            if !code.ends_with('\n') {
                code.push('\n');
            }
            Ok(code)
        }
        [] => bail!("the bot did not send the merged file"),
        _ => bail!("the bot sent several code blocks instead of one merged file"),
    }
}

/// Merge `new_code` into `current` (the file `path` as it is in the repository).
pub fn merge(s: &Settings, path: &str, current: &str, new_code: &str) -> Result<String> {
    if current.trim() == new_code.trim() {
        return Ok(current.to_string());
    }
    if current.len() > MAX_MERGE_BYTES {
        bail!("{path} is too large to merge automatically ({} KB)", current.len() / 1024);
    }
    // a plain request: no answer-language or GitHub note (they could change the code)
    let mut plain = s.clone();
    plain.language.clear();
    plain.github_enabled = false;
    let msgs = vec![ChatMessage::user(&merge_request(path, current, new_code))];
    let mut reply = String::new();
    ais::stream_reply(&plain, ais::build_body(&plain, &msgs, &ais::new_session_id()), |ev| {
        if let Event::Text(t) = ev {
            reply.push_str(t);
        }
    })
    .map_err(|e| anyhow!("merging {path} failed: {e}"))?;
    merged_file(&reply).map_err(|e| anyhow!("merging {path} failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_fences_cannot_be_broken() {
        let r = merge_request("README.md", "text\n```\ncode\n```", "new");
        assert!(r.contains("````\ntext"), "{r}");
        assert!(r.contains("`README.md`"));
    }

    #[test]
    fn takes_exactly_one_code_block() {
        assert_eq!(merged_file("Here you go:\n\n```rust\nfn a() {}\nfn b() {}\n```\n").unwrap(), "fn a() {}\nfn b() {}\n");
        assert!(merged_file("Sorry, I can't.").is_err());
        assert!(merged_file("```\na\n```\n\n```\nb\n```").is_err());
        assert!(merged_file("```\n\n```").is_err());
    }

    #[test]
    fn same_code_needs_no_merge() {
        let s = Settings::default();
        assert_eq!(merge(&s, "a.rs", "fn a() {}\n", "fn a() {}").unwrap(), "fn a() {}\n");
    }

    /// `cargo test -- --ignored live_` : with GitHub on, a small change comes back as the whole file.
    #[test]
    #[ignore]
    fn live_full_file_note() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        ais::refresh(&mut s).expect("setup");
        s.github_enabled = true;
        s.github_repo = "lena/schulprojekt".into();
        s.github_token = "x".into();
        let file = "fn greet(name: &str) {\n    println!(\"Hi {name}\");\n}\n\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    greet(\"Lena\");\n    println!(\"{}\", add(2, 3));\n}\n";
        let msg = format!("This is src/main.rs:\n\n```rust\n{file}```\n\nChange greet so it says \"Welcome back\".");
        let msgs = vec![ChatMessage::user(&msg)];
        let mut reply = String::new();
        ais::stream_reply(&s, ais::build_body(&s, &msgs, &ais::new_session_id()), |ev| {
            if let Event::Text(t) = ev {
                reply.push_str(t)
            }
        })
        .expect("reply");
        println!("{reply}");
        let blocks = crate::markdown::code_blocks(&reply);
        let whole = blocks.iter().find(|b| b.code.contains("Welcome back")).expect("changed code");
        assert!(whole.code.contains("fn add") && whole.code.contains("fn main"), "whole file shown");
        assert_eq!(crate::github::named_file(whole).as_deref(), Some("src/main.rs"), "path named");
    }

    /// `cargo test -- --ignored live_` : a partial snippet is merged into the whole file.
    #[test]
    #[ignore]
    fn live_merge_keeps_the_rest_of_the_file() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        ais::refresh(&mut s).expect("setup");
        let current = "use std::io;\n\nfn greet(name: &str) {\n    println!(\"Hi {name}\");\n}\n\nfn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    greet(\"Lena\");\n    println!(\"{}\", add(2, 3));\n}\n";
        let snippet = "fn greet(name: &str) {\n    println!(\"Hello, {name}! Welcome back.\");\n}";
        let merged = merge(&s, "src/main.rs", current, snippet).expect("merge");
        println!("{merged}");
        assert!(merged.contains("Welcome back"), "new code is in");
        assert!(merged.contains("fn add(a: i32, b: i32)") && merged.contains("fn main()") && merged.contains("use std::io"), "rest kept");
        assert!(!merged.contains("println!(\"Hi {name}\")"), "old version replaced");
    }
}
