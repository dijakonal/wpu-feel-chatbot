# AIS.chat Client

A small native desktop chat window (Windows + Linux) for one shared
[AIS.chat](https://ais-chat.schule) character, written in Rust with
[egui/eframe](https://github.com/emilk/egui). Answers are streamed in as the
bot writes them and shown as **Markdown** (headings, lists, tables, quotes, links,
code blocks with a **Copy** button). It has a clean, modern look (Inter font,
rounded chat bubbles, floating message box) in light and dark mode – it follows the
system by default, or pick one under **⚙ Settings → Appearance**.

This is an **unofficial** client: it sends the same requests the website's chat
box sends. AIS.chat "can make mistakes" – check what it tells you.

## Download

Get the newest version from the
[**Releases page**](https://github.com/dijakonal/wpu-feel-chatbot/releases/latest):

- **Windows:** `ais-chat-client-windows-x86_64.exe` – just start it. Windows may
  say the app is from an unknown publisher: click *More info → Run anyway*.
- **Linux** (Ubuntu 22.04, Debian 12, Linux Mint 21 or newer – or any distribution
  via the AppImage):
  - `ais-chat-client-linux-x86_64.AppImage` – no installation: make it executable
    (`chmod +x`, or *Properties → Allow executing*) and double-click it.
  - `ais-chat-client_<version>_amd64.deb` – Debian/Ubuntu/Mint, adds the app to
    the menu: `sudo apt install ./ais-chat-client_<version>_amd64.deb`
  - `ais-chat-client-linux-x86_64` (the plain program) or the same in a `.tar.gz` –
    run it directly (on X11 it needs `libxkbcommon-x11-0`, see below).

To build it yourself, see [Build & run](#build--run).

## Using it

1. Start the app. It connects to the class chat by itself ("Online" with a green
   dot under the name means it is ready).
2. Type a message, or click one of the suggestions on the start screen.
   **Enter** sends, **Shift+Enter** starts a new line.
3. **+** in the message box (or dragging files onto the window) attaches files.
4. To ask about one part of an answer, **select that text** and click the
   **↩ Reply** button that appears next to it. The quote shows above the message
   box (✖ or Esc removes it) and above your message in the chat; the bot gets it
   as a Markdown quote (`> …`) in front of your question.
5. **⚙ Settings → Language** sets the language of the menus, the chat names and
   the bot's answers (Deutsch, English, Français, Español, Italiano, Nederlands,
   Polski, Türkçe, Українська, Русский). The bot then always answers in that
   language, whatever language you write in. The app puts a short note in front of
   your newest message in the request (and repeats the language after it) – it is
   not shown in the window and not kept in the chat history, so a change applies
   from the next message on. With *Automatic* the menus use your system's language
   and the bot answers in the language you write in.
   The start screen follows the language too: the bot's greeting and the
   suggestion cards switch to it in chats that have not started yet. The class
   bot's greeting is translated by the app itself; for other bots the app asks
   the bot once to translate its greeting and remembers the result.
6. **Several chats:** the list on the left (☰ hides it) holds all your chats,
   grouped by day (Today, Yesterday, …). Each chat is named with a short summary
   of its first message, written by the bot. Each one has its own context – its own AIS.chat session,
   history, uploads and unsent draft – so the bot never mixes them up. An answer
   keeps arriving in its chat while you look at another one ("typing…" in the
   list). Hover a chat for ✏ rename and 🗑 delete (a deleted chat can
   be brought back with **Undo** for a few seconds). Chats are saved and come back after a restart: the app opens on your
   last chat, and every new chat starts with **Continue where you left off** and
   your **recent chats** (saved on this computer; with an account also on the
   sync server, see below).
7. **Account (optional, prototype):** **⚙ Settings → Account** logs in to a sync
   server ([wpu-feel-chatbot-backend](https://github.com/dijakonal/wpu-feel-chatbot-backend)).
   Your chats are then kept on the server and appear on every computer you log in
   on: new chats, answers, renames and deletions are synced a few seconds after
   they happen and every 30 seconds. The line at the bottom of the chat list shows
   who is logged in and when the last sync was. API keys and the GitHub token are
   never sent. Logging out keeps the chats on this computer. The login token is
   kept in the system password store like the other secrets.
8. **➕ New chat** starts a new chat (the current one stays in the list); the round **■** button stops a long answer;
   **Copy** under an answer and on code blocks puts text on the clipboard.

There is nothing to set up. To use a *different* shared character, open
**⚙ Settings**, paste its invite link
(`https://app.ais-chat.schule/ua/characters/<id>/dialog?inviteCode=<code>`)
and press **Connect**. **Use class link** switches back to the class chat:

<https://app.ais-chat.schule/ua/characters/6d16c5dd-1555-435f-ac22-3ab521a692b1/dialog?inviteCode=RVHN1H1B>

### What the app works out by itself

From the invite link alone the app reads, from the AIS.chat page:

| Value | Where it comes from |
|---|---|
| character name and greeting | the page's embedded data |
| `next-action` (the "send message" server action) | the page's JavaScript (`sendCharacterMessageAction`) |
| `x-deployment-id` | the page's `sentry-release` tag |
| **upload address** | always `<site>/api/v1/shared-chat/files` |

AIS.chat is a Next.js site and its `next-action` / deployment ids change with
every site update. When a message is rejected because of that
(`Invalid Server Action request`, HTTP 404, empty reply), the app reads fresh
values from the page and sends the message again automatically – you only see
"AIS.chat was updated – reconnecting…" for a moment.

Settings are stored in your user config folder, **not** in this repository:

- Windows: `%APPDATA%\ais-chat-client\settings.json`
- Linux: `~/.config/ais-chat-client/settings.json`

Your chats are saved next to it in `conversations.json` (the newest 200).

## Free AI models (optional)

If the class chat is not available (or its limits are reached), a free model can
answer instead: pick one in the **model list at the top** of the window
(🎓 *Class bot* is the default; ⚡ entries are free models). Switching works in
the middle of a chat – the next answer comes from the chosen model, and it sees
the whole conversation.

Set it up under **⚙ Settings → Free AI models**:

1. Choose a **provider**: **Groq** (default), **OpenRouter**, **Mistral**,
   **Google Gemini**, or **Other** – any OpenAI-compatible service, e.g. a local
   Ollama at `http://localhost:11434/v1`.
2. Get a free **API key** from the provider (the app links to the page) and press
   **Save key**. The app then reads the provider's model list.

- Each provider has **its own key**, and it is **not** the GitHub token: the
  GitHub token is only for your repository. Keys are kept in the system's
  password store (or in `settings.json` on systems that have none).
- The free models get the class bot's instructions (read from its AIS.chat page),
  so they behave like the class bot. The answer language and everything else work
  the same.
- Free use has **limits** (requests per minute/day); when one is reached, the app
  says so – pick another model or wait.
- The free models only read **text and code files**; PDFs, images and Office
  files need the class bot.
- Check each provider's terms (age limit, whether inputs are used for training)
  before using it in class.

(GitHub Models, which earlier versions offered here, was shut down by GitHub on
July 30, 2026.)

## Attaching files

Attached files show up above the input box (**✖** removes one). You can send a
message that is only files.

- **Small text and code files (up to 200 KB)** are pasted into the message under
  an `[Attached file: name]` heading (the window only shows the file name).
- **Everything else (PDF, images, Office files, … up to 20 MB)** is uploaded right
  away to the chat's upload address, exactly like the paperclip on the website
  (multipart POST with `file`, `inviteCode`, `entityType=character`, `entityId`,
  `sharedSessionId`). The returned file id travels in the chat request's
  `fileIds`. While the bot reads the file the window shows
  "Reading the attached file…".

Uploaded files are listed again with every message of the chat, because the
history is re-sent every time as well. They belong to their chat: a new chat starts without them.

## GitHub: reading your repository and saving code (optional)

Off by default. Switch it on under **⚙ Settings → GitHub (optional)**:

1. Create a **fine-grained personal access token** on GitHub
   (*Settings → Developer settings → Fine-grained tokens*, or the link in the
   app): *Repository access* → only your project repository, *Permissions* →
   **Contents: Read and write**.
2. Enter the **repository** (`owner/name` or its github.com address) and the
   **token**. *Branch* (default: the repository's default branch, usually
   `main`) and *Folder* (default: empty = the top level of the repository) are
   optional.
3. Choose **when**: only when you click **⬆ GitHub** on a code block, or
   **automatically** for every file an answer contains.
4. **Save & check access** tells you whether the token can write to the repository.

**Asking about your repository.** The bot cannot open links or GitHub by itself,
so the app fetches the files for it: click **+** in the message box →
**From GitHub**, tick the files (or **Select all code**) and **Attach**. The files
are added to your message like code files from your computer (text and code
files only, up to 200 KB each and 300 KB together; generated folders such as
`node_modules` are left out). When your message mentions "GitHub" or "repo"
and nothing is attached yet, the app offers this right above the message box.

**Saving code.** Code goes straight into your repository at the path the answer
gives it: when the bot writes "The file `src/main.rs` …" (or starts the code with a
comment like `// src/main.rs`), that block **replaces `src/main.rs`** on the
branch. All files of one answer become one commit ("Update Cargo.toml,
src/main.rs"). Automatic mode only saves blocks that name a file, so command
snippets like `cargo run` are not committed.

**Code without a file name.** When you click **⬆ GitHub** on a block the answer
did not name a file for, the app asks the bot where it belongs, showing it the
repository's file list: a change to an existing file (say a new field for the
settings struct) is merged into that file (`src/settings.rs`), new code gets a
fitting new path (`src/shuffle.rs`). If the bot cannot tell, the name comes from
the code itself (`fn load_settings` → `load_settings.rs`, `# Mein Spiel` →
`mein-spiel.md`, otherwise `snippet-1.py`, …).

**Nothing gets lost.** While GitHub saving is on, the bot is asked (in a hidden
note with every message) to always show the *complete* file with its path. And if
an answer still shows only part of a file, the app does not simply overwrite the
file: it fetches the current version from GitHub and lets the bot merge the new
code into it (in a separate request you do not see), then saves the complete
result – "Merged into the existing file" is noted in the commit. If merging does
not work, nothing is saved and the button says why. New files are saved as they
are, and files that are already up to date are left alone.
The button turns into **✔ On GitHub** (click it to open the commit) or shows
what went wrong. Empty repositories and branches that do not exist yet work too.

The token is stored in your system's credential store (Windows Credential Manager
or the Linux Secret Service, e.g. GNOME Keyring / KWallet), **not** in
`settings.json`. A token from an older version is moved there automatically. On a
system without a credential store the token stays in `settings.json` instead.
Still use a token that only has access to this one repository.

## If something does not work

The dot next to the name turns red and a message says why. Usually **Try again**
is enough. Otherwise open **⚙ Settings → Advanced**:

- **Detect again** re-reads all values from the site.
- **Upload address** – leave empty for the automatic address; only fill it in if
  AIS.chat moves its upload endpoint.
- **Import from a cURL** – last resort: open the invite link in Firefox/Chrome,
  press **F12 → Network**, send a message, right-click the `dialog?inviteCode=…`
  POST request → **Copy → Copy as cURL** and paste it (only the link,
  `next-action` and `x-deployment-id` are read from it). A cURL pasted into the
  invite-link box works too.

Don't commit cURL captures to git; they can contain cookies.

## Build & run

Needs a recent stable Rust toolchain (<https://rustup.rs>).

```bash
cargo run --release
```

Linux libraries (Debian/Ubuntu names) for building:

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

On X11 the app also needs `libxkbcommon-x11-0` at runtime. The **+** file
picker uses the desktop's file-chooser portal (`xdg-desktop-portal`, present on
GNOME/KDE desktops); dragging files onto the window always works.

The GitHub Actions workflow (`.github/workflows/build.yml`) tests and builds on
Windows and Linux for every push and uploads the binaries as build artifacts.

**Making a release:** add a section for the new version to `CHANGELOG.md`, set
the same `version` in `Cargo.toml`, commit, then push a tag:

```bash
git tag v1.0.1 && git push origin v1.0.1
```

The workflow then builds both versions and publishes a GitHub Release with the
downloads and the CHANGELOG section as release notes.

Without a tag push: on GitHub open **Actions → Build → Run workflow**, tick
**release** and start it – the workflow creates the tag `v<version>` from
`Cargo.toml` itself (and refuses if that release already exists).

## Code layout

| File | What it does |
|---|---|
| `src/main.rs` | opens the window |
| `src/app.rs` | chat window, input box, attachments, Settings window; background threads |
| `src/ais.rs` | the AIS.chat protocol: request body, streamed-answer decoder, file upload, auto-detection |
| `src/conversations.rs` | one chat's state, titles, saving/loading the chat list |
| `src/github.rs` | optional GitHub saving: file names, one commit per push (GitHub REST API) |
| `src/i18n.rs` | menu language, greeting and suggestion cards |
| `src/ui_text.rs` | the translations of every menu text |
| `src/backend.rs` | sends requests to AIS.chat or to a free AI model |
| `src/models.rs` | free AI providers (OpenAI-style API): model lists, streaming chat |
| `src/settings.rs` | invite-link parsing, cURL import, loading/saving settings |
| `src/sync.rs` | account login and chat sync with the sync server |
| `src/markdown.rs` | Markdown → egui renderer |
| `src/theme.rs` | colours (light/dark), fonts, widget style, avatar / send button / typing dots |
| `assets/fonts/` | Inter (SIL Open Font License, see `Inter-LICENSE.txt`), built into the binary |

## Tests

```bash
cargo test
```

The tests include the decoder for AIS.chat's streamed answer format, checked
against real captured replies (including the `ai_activity` status rows sent while
the bot reads an uploaded file). One test talks to the real site and is skipped by
default:

```bash
cargo test -- --ignored live_
```


## What Im working on right now
- Adding support for different models and chatbot api's
- Improving ui
- Adding a cli tool for both linux and windows
- Adding the ability to modify local files

If anyone wants to help me further develop this tool (especially if theyre in the same course that Im in) please reach out to me Id love to colab with other people on this
