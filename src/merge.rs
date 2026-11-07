//! Fitting new code into an existing file before it is saved to GitHub.
//!
//! An answer often shows only part of a file ("change this function to …"). Saving that as the
//! whole file would delete everything else, so when the file already exists, the bot is asked
//! – in a separate request that no chat sees – to produce the complete updated file.

use anyhow::{anyhow, bail, Result};

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

/// The request that asks where a code block without a file name belongs.
pub fn placement_request(repo_files: &[String], lang: &str, before: &str, code: &str) -> String {
    let f = fence(&[code]);
    let list = if repo_files.is_empty() { "(the repository is empty)".to_string() } else { repo_files.join("\n") };
    format!(
        "A code block from an earlier answer has to be saved in a Git repository, but the answer did not say \
         which file it is.\n\
         If the code changes or extends one of the files listed below, reply with that file's path. If it is \
         a new file, reply with a short, descriptive path for it that fits the project's layout (lowercase, no \
         spaces, the right extension).\n\
         Reply with the path only – nothing else.\n\n\
         FILES IN THE REPOSITORY:\n{list}\n\n\
         TEXT BEFORE THE CODE BLOCK:\n{before}\n\n\
         CODE BLOCK ({lang}):\n{f}\n{code}\n{f}"
    )
}

/// The path from the bot's reply, if it is a usable one.
pub fn placement_from_reply(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let path = line.trim_matches(|c| matches!(c, '`' | '"' | '\'' | '*' | ' ')).trim_start_matches("./").trim_end_matches('.');
    crate::github::valid_path(path).then(|| path.to_string())
}

/// Ask the bot where a code block without a file name belongs: an existing file of the
/// repository (it will be merged into it) or a new path.
pub fn place(s: &Settings, repo_files: &[String], lang: &str, before: &str, code: &str) -> Result<String> {
    let reply = crate::backend::ask(s, &placement_request(repo_files, lang, before, code))?;
    placement_from_reply(&reply).ok_or_else(|| anyhow!("the bot did not name a usable file"))
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
    let reply = crate::backend::ask(s, &merge_request(path, current, new_code)).map_err(|e| anyhow!("merging {path} failed: {e}"))?;
    merged_file(&reply).map_err(|e| anyhow!("merging {path} failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::{self, ChatMessage, Event};

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
    fn reads_suggested_paths() {
        assert_eq!(placement_from_reply("`src/settings.rs`").as_deref(), Some("src/settings.rs"));
        assert_eq!(placement_from_reply("\n  ./README.md\n").as_deref(), Some("README.md"));
        assert_eq!(placement_from_reply("It belongs to src/settings.rs because …"), None);
        assert_eq!(placement_from_reply("../../x.rs"), None);
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

    /// `cargo test -- --ignored live_` : snippets without a file name find their file.
    #[test]
    #[ignore]
    fn live_placement() {
        let mut s = Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        ais::refresh(&mut s).expect("setup");
        let files: Vec<String> =
            ["Cargo.toml", "README.md", "CHANGELOG.md", "src/main.rs", "src/app.rs", "src/settings.rs", "src/github.rs", "src/secret.rs"]
                .iter()
                .map(|f| f.to_string())
                .collect();
        let field = "    /// Personal access token. Kept in the OS keystore.\n    #[serde(skip_serializing)]\n    pub github_token: String,";
        let p1 = place(&s, &files, "rust", "Change the token field in the settings struct:", field).expect("place");
        println!("field -> {p1}");
        assert_eq!(p1, "src/settings.rs");
        let text = "The token is stored in your system's credential store, **not** in `settings.json`.";
        let p2 = place(&s, &files, "markdown", "Update the documentation:", text).expect("place");
        println!("docs -> {p2}");
        assert_eq!(p2, "README.md");
        let new = "pub fn shuffle<T>(items: &mut [T]) { /* … */ }";
        let p3 = place(&s, &files, "rust", "A new helper module for shuffling quiz questions:", new).expect("place");
        println!("new -> {p3}");
        assert!(!files.contains(&p3) && p3.ends_with(".rs"), "{p3}");
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
