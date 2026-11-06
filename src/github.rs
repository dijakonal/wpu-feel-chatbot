//! Optional: save code from answers to a GitHub repository.
//!
//! Uses the GitHub REST API with a personal access token (a fine-grained token with
//! "Contents: Read and write" for one repository is enough). All files of one push become one
//! commit (Git Data API: tree → commit → move the branch). An empty repository has no commit to
//! build on, so there the files are created one by one with the Contents API instead.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde_json::{json, Value};

use crate::markdown::CodeBlockInfo;

const DEFAULT_API: &str = "https://api.github.com";

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub token: String,
    /// `owner/name`
    pub repo: String,
    /// Empty = the repository's default branch.
    pub branch: String,
    /// Empty = https://api.github.com (other values for GitHub Enterprise or tests).
    pub api: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileOut {
    pub path: String,
    pub content: String,
}

struct Api {
    http: Client,
    base: String,
    token: String,
    repo: String,
}

impl Api {
    fn new(cfg: &Config) -> Result<Self> {
        let repo = normalize_repo(&cfg.repo).ok_or_else(|| anyhow!("Enter the repository as owner/name, e.g. lena/schulprojekt."))?;
        if cfg.token.trim().is_empty() {
            bail!("Enter a GitHub token in Settings.");
        }
        let base = if cfg.api.trim().is_empty() { DEFAULT_API.to_string() } else { cfg.api.trim().trim_end_matches('/').to_string() };
        let http = Client::builder()
            .user_agent("ais-chat-client")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { http, base, token: cfg.token.trim().to_string(), repo })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}/repos/{}{}", self.base, self.repo, path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    fn get(&self, path: &str) -> Result<(u16, Value)> {
        read(self.req(reqwest::Method::GET, path).send())
    }

    fn send(&self, method: reqwest::Method, path: &str, body: Value) -> Result<(u16, Value)> {
        read(self.req(method, path).header("Content-Type", "application/json").body(body.to_string()).send())
    }

    /// `(default branch, can write)`
    fn repo_info(&self) -> Result<(String, bool)> {
        let (status, v) = self.get("")?;
        match status {
            200 => Ok((
                v["default_branch"].as_str().unwrap_or("main").to_string(),
                v["permissions"]["push"].as_bool().unwrap_or(false),
            )),
            401 => bail!("GitHub does not accept the token (expired or mistyped?)."),
            403 => bail!("The token is not allowed to read {} ({}).", self.repo, message(&v)),
            404 => bail!("Repository {} not found – check the name, and that the token has access to it.", self.repo),
            _ => bail!("GitHub answered HTTP {status}: {}", message(&v)),
        }
    }

    /// Current commit of `branch`, `None` if the branch does not exist, error 409 = empty repo.
    fn branch_head(&self, branch: &str) -> Result<Result<Option<String>, ()>> {
        let (status, v) = self.get(&format!("/git/ref/heads/{branch}"))?;
        match status {
            200 => Ok(Ok(v["object"]["sha"].as_str().map(str::to_string))),
            404 => Ok(Ok(None)),
            409 => Ok(Err(())), // "Git Repository is empty."
            _ => bail!("Could not read branch {branch} (HTTP {status}: {}).", message(&v)),
        }
    }
}

fn read(r: reqwest::Result<Response>) -> Result<(u16, Value)> {
    let resp = r.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            anyhow!("Could not reach GitHub. Check the internet connection.")
        } else {
            anyhow!(e)
        }
    })?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn message(v: &Value) -> String {
    v["message"].as_str().unwrap_or("no details").to_string()
}

/// Accepts `owner/name`, `https://github.com/owner/name(.git)` and `github.com/owner/name/`.
pub fn normalize_repo(input: &str) -> Option<String> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.split("github.com/").last().unwrap_or(s);
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match parts.as_slice() {
        [owner, name] if ok(owner) && ok(name) => Some(format!("{owner}/{name}")),
        _ => None,
    }
}

/// Check the settings: repository reachable and the token may write to it.
pub fn check(cfg: &Config) -> Result<String> {
    let api = Api::new(cfg)?;
    let (default_branch, can_write) = api.repo_info()?;
    if !can_write {
        bail!("The token can read {} but not write to it. Give it \"Contents: Read and write\".", api.repo);
    }
    let branch = if cfg.branch.trim().is_empty() { default_branch } else { cfg.branch.trim().to_string() };
    Ok(format!("Ready: code goes to {} on branch {branch}.", api.repo))
}

/// Push `files` as one commit. Returns the commit's web address.
pub fn push(cfg: &Config, files: &[FileOut], commit_message: &str) -> Result<String> {
    if files.is_empty() {
        bail!("Nothing to push.");
    }
    let api = Api::new(cfg)?;
    let (default_branch, _) = api.repo_info()?;
    let branch = if cfg.branch.trim().is_empty() { default_branch.clone() } else { cfg.branch.trim().to_string() };

    let head = match api.branch_head(&branch)? {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            // the chosen branch does not exist yet: start it from the default branch
            let base = match api.branch_head(&default_branch)? {
                Ok(Some(sha)) => sha,
                _ => return push_to_empty_repo(&api, files, commit_message, &branch),
            };
            let (status, v) = api.send(reqwest::Method::POST, "/git/refs", json!({"ref": format!("refs/heads/{branch}"), "sha": base}))?;
            if status != 201 {
                bail!("Could not create branch {branch} (HTTP {status}: {}).", message(&v));
            }
            base
        }
        Err(()) => return push_to_empty_repo(&api, files, commit_message, &branch),
    };

    let (status, v) = api.get(&format!("/git/commits/{head}"))?;
    let base_tree = v["tree"]["sha"].as_str().filter(|_| status == 200).ok_or_else(|| anyhow!("Could not read the latest commit (HTTP {status})."))?;

    let tree: Vec<Value> = files.iter().map(|f| json!({"path": f.path, "mode": "100644", "type": "blob", "content": f.content})).collect();
    let (status, v) = api.send(reqwest::Method::POST, "/git/trees", json!({"base_tree": base_tree, "tree": tree}))?;
    let tree_sha = v["sha"].as_str().filter(|_| status == 201).ok_or_else(|| write_error("save the files", status, &v))?;

    let (status, v) = api.send(reqwest::Method::POST, "/git/commits", json!({"message": commit_message, "tree": tree_sha, "parents": [head]}))?;
    let commit = v["sha"].as_str().filter(|_| status == 201).ok_or_else(|| write_error("create the commit", status, &v))?.to_string();

    let (status, v) = api.send(reqwest::Method::PATCH, &format!("/git/refs/heads/{branch}"), json!({"sha": commit, "force": false}))?;
    if status != 200 {
        return Err(write_error(&format!("move branch {branch}"), status, &v));
    }
    Ok(commit_url(&api, &commit))
}

/// An empty repository has nothing to build a tree on; the Contents API can create the first
/// files (one commit per file).
fn push_to_empty_repo(api: &Api, files: &[FileOut], commit_message: &str, branch: &str) -> Result<String> {
    let mut last = String::new();
    for f in files {
        let path: Vec<String> = f.path.split('/').map(|p| url::form_urlencoded::byte_serialize(p.as_bytes()).collect()).collect();
        let body = json!({"message": commit_message, "content": base64(f.content.as_bytes()), "branch": branch});
        let (status, v) = api.send(reqwest::Method::PUT, &format!("/contents/{}", path.join("/")), body)?;
        if status != 201 && status != 200 {
            return Err(write_error(&format!("create {}", f.path), status, &v));
        }
        last = v["commit"]["sha"].as_str().unwrap_or_default().to_string();
    }
    Ok(commit_url(api, &last))
}

fn commit_url(api: &Api, sha: &str) -> String {
    let web = if api.base == DEFAULT_API { "https://github.com".to_string() } else { api.base.replace("/api/v3", "") };
    format!("{web}/{}/commit/{sha}", api.repo)
}

fn write_error(what: &str, status: u16, v: &Value) -> anyhow::Error {
    match status {
        403 | 404 => anyhow!("GitHub refused to {what}: the token needs \"Contents: Read and write\" for this repository."),
        409 | 422 => anyhow!("GitHub could not {what} ({}). The branch may have changed at the same moment – try again.", message(v)),
        _ => anyhow!("GitHub could not {what} (HTTP {status}: {}).", message(v)),
    }
}

/// Standard base64 (for the Contents API).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ------------------------------------------------------------------ file names

/// File extension for a code block's language tag.
pub fn extension(lang: &str) -> &'static str {
    match lang.trim().to_lowercase().as_str() {
        "python" | "py" | "python3" => "py",
        "rust" | "rs" => "rs",
        "javascript" | "js" | "node" => "js",
        "typescript" | "ts" => "ts",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "java" => "java",
        "kotlin" | "kt" => "kt",
        "c" => "c",
        "cpp" | "c++" | "cc" => "cpp",
        "csharp" | "cs" | "c#" => "cs",
        "go" | "golang" => "go",
        "php" => "php",
        "ruby" | "rb" => "rb",
        "swift" => "swift",
        "lua" => "lua",
        "sql" => "sql",
        "bash" | "sh" | "shell" | "zsh" => "sh",
        "powershell" | "ps1" => "ps1",
        "batch" | "bat" | "cmd" => "bat",
        "json" => "json",
        "yaml" | "yml" => "yml",
        "toml" => "toml",
        "xml" => "xml",
        "markdown" | "md" => "md",
        "arduino" | "ino" => "ino",
        "processing" | "pde" => "pde",
        "gdscript" | "gd" => "gd",
        "r" => "r",
        _ => "txt",
    }
}

/// A file name like `main.py` or `src/app.js` inside `text`, if there is one.
fn find_file_name(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '*' | '"' | '\'' | '(' | ')' | ',' | ':' | ';' | '<' | '>' | '[' | ']'))
        .map(|t| t.trim_end_matches(['.', '!', '?']))
        .filter(|t| looks_like_file(t))
        .last()
        .map(str::to_string)
}

fn looks_like_file(t: &str) -> bool {
    let Some((stem, ext)) = t.rsplit_once('.') else { return false };
    let ext_ok = (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) && ext.chars().any(|c| c.is_ascii_alphabetic());
    let stem_ok = !stem.is_empty()
        && !stem.ends_with('/')
        && stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
        && !stem.contains("..")
        && !stem.starts_with('/')
        && !t.contains("://");
    // "z.B." / "e.g." style abbreviations are not files
    ext_ok && stem_ok && stem.len() + ext.len() >= 4 && !matches!(ext.to_lowercase().as_str(), "b" | "g" | "e" | "a" | "com" | "org" | "de")
}

/// Name for a code block: from a comment on its first line (`# main.py`), else from the text
/// right before it ("Save this as `main.py`"), else `code-<n>.<ext>`.
pub fn file_name(block: &CodeBlockInfo, n: usize) -> String {
    let ext = extension(&block.lang);
    let first_line = block.code.lines().next().unwrap_or("");
    let comment = first_line
        .trim()
        .trim_start_matches(['#', '/', '*', '-', '<', '!', ';', '%'])
        .trim_end_matches(['*', '/', '-', '>'])
        .trim();
    let from_comment = if first_line.trim() != comment && comment.split_whitespace().count() <= 3 { find_file_name(comment) } else { None };
    from_comment.or_else(|| find_file_name(&block.before)).unwrap_or_else(|| format!("code-{}.{ext}", n + 1))
}

/// Folder name for a chat: `Wie baue ich ein Quiz?` → `wie-baue-ich-ein-quiz`.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        let mapped = match c {
            'ä' => "ae".to_string(),
            'ö' => "oe".to_string(),
            'ü' => "ue".to_string(),
            'ß' => "ss".to_string(),
            c if c.is_ascii_alphanumeric() => c.to_string(),
            _ => "-".to_string(),
        };
        if mapped == "-" && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push_str(&mapped);
    }
    let mut out: String = out.trim_matches('-').chars().take(40).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "chat".to_string() } else { out }
}

/// Files for some code blocks of one answer, under `folder/chat_dir/`. Names that occur twice
/// get `-2`, `-3`, … .
pub fn files_for(blocks: &[(usize, &CodeBlockInfo)], folder: &str, chat_dir: &str) -> Vec<FileOut> {
    let folder = folder.trim().trim_matches('/');
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (n, b) in blocks {
        let mut name = file_name(b, *n);
        if seen.contains(&name) {
            let (stem, ext) = name.rsplit_once('.').map(|(s, e)| (s.to_string(), e.to_string())).unwrap_or((name.clone(), String::new()));
            let mut k = 2;
            while seen.contains(&format!("{stem}-{k}.{ext}")) {
                k += 1;
            }
            name = format!("{stem}-{k}.{ext}");
        }
        seen.push(name.clone());
        let dir = [folder, chat_dir].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/");
        let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
        let mut content = b.code.clone();
        if !content.ends_with('\n') {
            content.push('\n');
        }
        out.push(FileOut { path, content });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(lang: &str, code: &str, before: &str) -> CodeBlockInfo {
        CodeBlockInfo { lang: lang.into(), code: code.into(), before: before.into() }
    }

    #[test]
    fn reads_repo_names() {
        assert_eq!(normalize_repo("lena/projekt").as_deref(), Some("lena/projekt"));
        assert_eq!(normalize_repo(" https://github.com/lena/projekt.git ").as_deref(), Some("lena/projekt"));
        assert_eq!(normalize_repo("github.com/lena/my_app-2/").as_deref(), Some("lena/my_app-2"));
        assert_eq!(normalize_repo("projekt"), None);
        assert_eq!(normalize_repo("a/b/c"), None);
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("print('hi')\nä".as_bytes()), "cHJpbnQoJ2hpJykKw6Q=");
    }

    #[test]
    fn names_files() {
        assert_eq!(file_name(&block("python", "# main.py\nprint(1)", ""), 0), "main.py");
        assert_eq!(file_name(&block("js", "// src/app.js\nx()", ""), 0), "src/app.js");
        assert_eq!(file_name(&block("html", "<!-- index.html -->\n<p>", ""), 0), "index.html");
        assert_eq!(file_name(&block("python", "print(1)", "Speichere das als spiel.py:"), 0), "spiel.py");
        assert_eq!(file_name(&block("python", "# Ein Kommentar über das Programm\nx", "Hier ist der Code, z.B. so:"), 2), "code-3.py");
        assert_eq!(file_name(&block("", "x", "Siehe https://example.com/a.html"), 0), "code-1.txt");
        assert_eq!(file_name(&block("python", "x", "Datei ../../etc/passwd.py"), 0), "code-1.py");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Wie baue ich ein Quiz? Für die Schule!"), "wie-baue-ich-ein-quiz-fuer-die-schule");
        assert_eq!(slug("???"), "chat");
    }

    /// A tiny stand-in for the GitHub API: answers each request from `routes` (method, path
    /// → status, body) and records what it received.
    fn mock(routes: Vec<(&'static str, &'static str, u16, &'static str)>) -> (String, std::sync::mpsc::Receiver<(String, String, String)>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() || first.is_empty() {
                    continue;
                }
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let mut parts = first.split_whitespace();
                let (method, path) = (parts.next().unwrap().to_string(), parts.next().unwrap().to_string());
                let (status, reply) = routes
                    .iter()
                    .find(|(m, p, _, _)| *m == method && *p == path)
                    .map(|(_, _, s, b)| (*s, *b))
                    .unwrap_or((404, r#"{"message":"Not Found"}"#));
                let _ = tx.send((method, path, String::from_utf8_lossy(&body).to_string()));
                let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (addr, rx)
    }

    fn cfg(api: &str, branch: &str) -> Config {
        Config { token: "t".into(), repo: "o/r".into(), branch: branch.into(), api: api.into() }
    }

    #[test]
    fn pushes_all_files_in_one_commit() {
        let (api, rx) = mock(vec![
            ("GET", "/repos/o/r", 200, r#"{"default_branch":"main","permissions":{"push":true}}"#),
            ("GET", "/repos/o/r/git/ref/heads/main", 200, r#"{"object":{"sha":"c1"}}"#),
            ("GET", "/repos/o/r/git/commits/c1", 200, r#"{"tree":{"sha":"t1"}}"#),
            ("POST", "/repos/o/r/git/trees", 201, r#"{"sha":"t2"}"#),
            ("POST", "/repos/o/r/git/commits", 201, r#"{"sha":"c2"}"#),
            ("PATCH", "/repos/o/r/git/refs/heads/main", 200, r#"{}"#),
        ]);
        let files = vec![FileOut { path: "ais-chat/q/main.py".into(), content: "print(1)\n".into() }, FileOut { path: "ais-chat/q/a.css".into(), content: "x\n".into() }];
        let url = push(&cfg(&api, ""), &files, "msg").unwrap();
        assert!(url.ends_with("/o/r/commit/c2"), "{url}");
        let log: Vec<(String, String, String)> = rx.try_iter().collect();
        assert_eq!(log.len(), 6);
        let tree: Value = serde_json::from_str(&log[3].2).unwrap();
        assert_eq!(tree["base_tree"], "t1");
        assert_eq!(tree["tree"][0]["path"], "ais-chat/q/main.py");
        assert_eq!(tree["tree"][1]["content"], "x\n");
        let commit: Value = serde_json::from_str(&log[4].2).unwrap();
        assert_eq!(commit["parents"], json!(["c1"]));
        assert_eq!(commit["tree"], "t2");
        let r#ref: Value = serde_json::from_str(&log[5].2).unwrap();
        assert_eq!(r#ref["sha"], "c2");
        assert_eq!(r#ref["force"], false);
    }

    #[test]
    fn creates_files_in_an_empty_repository() {
        let (api, rx) = mock(vec![
            ("GET", "/repos/o/r", 200, r#"{"default_branch":"main","permissions":{"push":true}}"#),
            ("GET", "/repos/o/r/git/ref/heads/main", 409, r#"{"message":"Git Repository is empty."}"#),
            ("PUT", "/repos/o/r/contents/ais-chat/main.py", 201, r#"{"commit":{"sha":"c9"}}"#),
        ]);
        let files = vec![FileOut { path: "ais-chat/main.py".into(), content: "foo".into() }];
        let url = push(&cfg(&api, ""), &files, "msg").unwrap();
        assert!(url.ends_with("/o/r/commit/c9"), "{url}");
        let log: Vec<(String, String, String)> = rx.try_iter().collect();
        let put: Value = serde_json::from_str(&log[2].2).unwrap();
        assert_eq!(put["content"], "Zm9v");
        assert_eq!(put["branch"], "main");
    }

    #[test]
    fn starts_a_missing_branch_from_the_default_branch() {
        let (api, rx) = mock(vec![
            ("GET", "/repos/o/r", 200, r#"{"default_branch":"main","permissions":{"push":true}}"#),
            ("GET", "/repos/o/r/git/ref/heads/main", 200, r#"{"object":{"sha":"c1"}}"#),
            ("POST", "/repos/o/r/git/refs", 201, r#"{}"#),
            ("GET", "/repos/o/r/git/commits/c1", 200, r#"{"tree":{"sha":"t1"}}"#),
            ("POST", "/repos/o/r/git/trees", 201, r#"{"sha":"t2"}"#),
            ("POST", "/repos/o/r/git/commits", 201, r#"{"sha":"c2"}"#),
            ("PATCH", "/repos/o/r/git/refs/heads/schule", 200, r#"{}"#),
        ]);
        let files = vec![FileOut { path: "x.py".into(), content: "x".into() }];
        push(&cfg(&api, "schule"), &files, "msg").unwrap();
        let log: Vec<(String, String, String)> = rx.try_iter().collect();
        let created: Value = serde_json::from_str(&log.iter().find(|l| l.1 == "/repos/o/r/git/refs").unwrap().2).unwrap();
        assert_eq!(created["ref"], "refs/heads/schule");
        assert_eq!(created["sha"], "c1");
    }

    #[test]
    fn explains_bad_tokens() {
        let (api, _rx) = mock(vec![("GET", "/repos/o/r", 200, r#"{"default_branch":"main","permissions":{"push":false}}"#)]);
        let e = check(&cfg(&api, "")).unwrap_err().to_string();
        assert!(e.contains("not write"), "{e}");
        let (api, _rx) = mock(vec![("GET", "/repos/o/r", 401, r#"{"message":"Bad credentials"}"#)]);
        assert!(check(&cfg(&api, "")).unwrap_err().to_string().contains("token"));
        let (api, _rx) = mock(vec![]);
        assert!(check(&cfg(&api, "")).unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn builds_paths_and_avoids_duplicates() {
        let a = block("python", "print(1)", "");
        let b = block("python", "print(2)", "");
        let c = block("css", "# style.css\nbody{}", "");
        let files = files_for(&[(0, &a), (1, &b), (2, &c)], "/ais-chat/", "quiz");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["ais-chat/quiz/code-1.py", "ais-chat/quiz/code-2.py", "ais-chat/quiz/style.css"]);
        assert_eq!(files[0].content, "print(1)\n");
        let same = block("python", "# main.py\nx", "");
        let dup = files_for(&[(0, &same), (1, &same)], "", "q");
        assert_eq!(dup[1].path, "q/main-2.py");
    }
}
