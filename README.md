# AIS.chat Client

A small native desktop chat window (Windows + Linux) for one shared
[AIS.chat](https://ais-chat.schule) character, written in Rust with
[egui/eframe](https://github.com/emilk/egui). Messages are streamed in as the
bot writes them and shown as **Markdown** (headings, lists, tables, quotes, links,
code blocks with a **Copy** button). The look follows your system's light/dark mode.
The conversation history is kept locally and sent with every message (that is how
AIS.chat works).

This is an **unofficial** client: it sends the same request the website's chat
box sends. Be aware that AIS.chat "can make mistakes" – check what it tells you.

## First start

1. Run the app. The Settings window opens.
2. Paste your **invite link** (the `…/ua/characters/<id>/dialog?inviteCode=…`
   address) *and* a **cURL** of one chat request, then click **Import**:
   - open the invite link in Firefox/Chrome, press **F12 → Network**
   - send any message
   - right-click the `dialog?inviteCode=…` POST request → **Copy → Copy as cURL**
   - paste it into the app (the app only reads the link, `next-action` and
     `x-deployment-id` from it; it does not store the rest)
3. Press **Save**, close Settings and start chatting.

Settings (including the invite code) are stored in your user config folder,
**not** in this repository:

- Windows: `%APPDATA%\ais-chat-client\settings.json`
- Linux: `~/.config/ais-chat-client/settings.json`

Never commit your invite link or cURL captures to git.

## Attaching files

Click **Attach** (or drag files onto the window) to add files to your next message. They show up
above the input box; **x** removes one. You can send a message that is only files.

- **Text and code files (up to 200 KB)** work out of the box: the contents are pasted into the
  message under a `[Attached file: name]` heading. The window only shows the file name.
- **Any other file (PDF, images, Office files, up to 20 MB)** needs an *upload address*: open
  **Settings → File upload** and fill in the address of the site's upload request (and the form
  field name / extra headers if it uses them). The app sends the file there as a multipart POST,
  takes the file id out of the answer and sends it in the chat request's `fileIds`.
  To find the address: in the browser's **F12 → Network** tab attach a file in the real chat and
  look at the request that carries the file.

Uploaded files are listed again with every message of the chat, because the history is re-sent
every time as well. **New chat** forgets them.

## When it stops working

AIS.chat is a Next.js site; its `next-action` and deployment IDs change every
time the site is updated. If sending suddenly fails ("does not recognise the
saved request values", HTTP 404, empty replies):

1. Open **Settings → Auto-detect from site** (best effort), or
2. capture a fresh cURL as above and **Import** it again.

## Build & run

Needs a recent stable Rust toolchain (<https://rustup.rs>).

```bash
cargo run --release
```

Linux libraries (Debian/Ubuntu names):

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

Commit `Cargo.lock` after the first build. The GitHub Actions workflow builds
and tests on Windows and Linux and uploads the binaries.

Use https://app.ais-chat.schule/ua/characters/6d16c5dd-1555-435f-ac22-3ab521a692b1/dialog?inviteCode=RVHN1H1B as the import url

## Tests

```bash
cargo test
```

The tests include the decoder for AIS.chat's streamed answer format, checked
against a real captured reply.
# wpu-feel-chatbot
