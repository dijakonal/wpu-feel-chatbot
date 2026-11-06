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
5. **⚙ Settings → Answer language** makes the bot always answer in one language
   (Deutsch, English, Français, Español, …), whatever language you write in.
   The app puts a short note in front of your newest message in the request –
   it is not shown in the window and not kept in the chat history, so a change
   applies from the next message on. *Automatic* sends no note.
   The start screen follows the language too: the bot's greeting and the
   suggestion cards switch to it in chats that have not started yet. The class
   bot's greeting is translated by the app itself; for other bots the app asks
   the bot once to translate its greeting and remembers the result.
6. **Several chats:** the list on the left (☰ hides it) holds all your chats,
   newest first. Each one has its own context – its own AIS.chat session,
   history, uploads and unsent draft – so the bot never mixes them up. An answer
   keeps arriving in its chat while you look at another one ("typing…" in the
   list). Right-click a chat (or double-click it) to rename it; ✖ deletes it
   after asking. Chats are saved and come back after a restart.
7. **➕ New chat** starts a new chat (the current one stays in the list); the round **■** button stops a long answer;
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
   **token**. *Branch* (default: the repository's default branch) and *Folder*
   (default: `ais-chat`, `/` = top level) are optional.
3. Choose **when**: only when you click **⬆ GitHub** on a code block, or
   **automatically** after every answer that contains code.
4. **Save & check access** tells you whether the token can write to the repository.

**Asking about your repository.** The bot cannot open links or GitHub by itself,
so the app fetches the files for it: click **+** in the message box →
**From GitHub**, tick the files (or **Select all code**) and **Attach**. The files
are added to your message like code files from your computer (text and code
files only, up to 200 KB each and 300 KB together; generated folders such as
`node_modules` are left out). When your message mentions "GitHub" or "repo"
and nothing is attached yet, the app offers this right above the message box.

**Saving code.** Each chat gets its own folder (`ais-chat/<chat name>/`), and every push is one
commit. File names come from the answer when the bot names the file (in a
first-line comment like `# main.py`, or in the sentence before the code block);
otherwise the blocks are saved as `code-1.py`, `code-2.js`, … . Saving again
overwrites the file with the newer version (the old one stays in the history).
The button turns into **✔ On GitHub** (click it to open the commit) or shows
what went wrong. Empty repositories and branches that do not exist yet work too.

The token is stored in `settings.json` on this computer – use a token that only
has access to this one repository, and don't use the app with your token on a
shared computer.

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
Windows and Linux and uploads the binaries as build artifacts.

## Code layout

| File | What it does |
|---|---|
| `src/main.rs` | opens the window |
| `src/app.rs` | chat window, input box, attachments, Settings window; background threads |
| `src/ais.rs` | the AIS.chat protocol: request body, streamed-answer decoder, file upload, auto-detection |
| `src/conversations.rs` | one chat's state, titles, saving/loading the chat list |
| `src/github.rs` | optional GitHub saving: file names, one commit per push (GitHub REST API) |
| `src/i18n.rs` | greeting and suggestion cards in the answer language |
| `src/settings.rs` | invite-link parsing, cURL import, loading/saving settings |
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
