# Working on this repository

The AIS.chat Client: a Rust (egui) desktop chat app. Its sync server is
https://github.com/dijakonal/wpu-feel-chatbot-backend.

## Every commit: new version + changelog entry

1. Raise the **last number** of `version` in `Cargo.toml` by one (1.2.1 → 1.2.2), and run
   `cargo build` so `Cargo.lock` has the same version. Commit both.
2. Add a section at the **top** of `CHANGELOG.md` for that version, with today's date:
   `## 1.2.2 – YYYY-MM-DD`. List **everything** the commit adds, changes or fixes, one
   bullet each, in plain words for students and teachers (what they notice, not which
   function changed). No "Unreleased" section.
3. Bigger version steps (1.2.x → 1.3.0) only when the owner asks for one. A new version
   number does not publish downloads; releases are made separately (see
   `.github/workflows/build.yml`).

## Other rules from the owner

- Commit straight to `main` and push. No branches, no pull requests.
- Run `cargo test` before committing; do not commit failing tests.
- Every text shown in the window goes through `t()` / `tf()` and needs a row in
  `src/ui_text.rs` for all ten languages (a test checks this).
- The school login (`src/moodle_login.rs`) must stay easy to read for non-programmers:
  the password goes only to moodle.ohg-goe.net and is never saved. If it changes, update
  `docs/MOODLE-LOGIN.md` (English and German) too.
- Talk to the owner in English.
