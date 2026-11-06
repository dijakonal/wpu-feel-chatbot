# Changelog

## 1.0.0 – 2026-11-06

First release of the AIS.chat desktop client.

**Chatting**
- Connects to the class chat by itself – no setup. Another shared bot: paste its
  invite link in Settings.
- Answers stream in and are shown as Markdown (headings, lists, tables, code
  blocks with Copy).
- Several chats, each with its own context, in a list on the left; they are saved
  and come back after a restart. Rename or delete them with a right-click.
- Select part of an answer and click **Reply** to ask about exactly that part.
- **Answer language**: the bot answers in the chosen language (Deutsch, English,
  Français, Español, Italiano, Nederlands, Polski, Türkçe, Українська, Русский),
  and the start screen follows it.

**Files**
- Attach files with **+** or by dragging them onto the window: code and text files
  are pasted into the message, PDFs, images and Office files are uploaded.

**GitHub (optional, off by default)**
- Let the bot read files from your repository: **+ → From GitHub**.
- Save code from answers straight into your repository's files with **⬆ GitHub**,
  or automatically: a block the answer names (e.g. `src/main.rs`) updates that
  file on your branch, one commit per answer. The bot is asked to always show
  whole files, and if it shows only a part, the app merges it into the existing
  file instead of overwriting it. Code without a file name is placed by the bot
  into the file it belongs to (or gets a fitting new name).
- The GitHub token is kept in the system's password store (Windows Credential
  Manager / GNOME Keyring / KWallet), not in `settings.json`.

**Look**
- Modern light and dark design (follows the system, or choose in Settings).

**Under the hood**
- Keeps working when AIS.chat is updated: the app reads the new request values from
  the site and sends the message again.

Download the file for your system below. Windows: `ais-chat-client-windows-x86_64.exe`
(Windows may warn that the app is from an unknown publisher – click *More info →
Run anyway*). Linux: unpack `ais-chat-client-linux-x86_64.tar.gz` and run
`./ais-chat-client`.
