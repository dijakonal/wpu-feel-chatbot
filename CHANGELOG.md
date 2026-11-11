# Changelog

## Unreleased

- **School login:** the chat can only be used after logging in with the school's
  Moodle account. The app checks the user name and password at
  https://moodle.ohg-goe.net – the password is sent only there, is never saved,
  and the app logs out of Moodle again right after the check. The check is one
  commented file, `src/moodle_login.rs`, explained step by step in English and
  German in `docs/MOODLE-LOGIN.md`.
- **Accounts and chat sync (prototype):** *Settings → Account* logs in to a sync
  server (new repository
  [wpu-feel-chatbot-backend](https://github.com/dijakonal/wpu-feel-chatbot-backend)).
  Chats are then kept on the server and appear on every computer you log in on.
  New chats, answers, renames and deletions follow within seconds; if a chat was
  changed on two computers, the newer change wins. The bottom of the chat list
  shows who is logged in and when the last sync was. API keys and the GitHub
  token are never sent; the login token is kept in the system password store.

## 1.1.0 – 2026-11-09

- **Pick up where you left off:** the app opens on your last chat. A new chat
  shows a **Continue where you left off** button and your **recent chats** (with
  a preview of the last message) above the suggestions.
- **Menus in your language:** the whole window – buttons, settings, hints – is
  translated into German, English, French, Spanish, Italian, Dutch, Polish,
  Turkish, Ukrainian and Russian. *Settings → Language* sets the menus, the chat
  names and the bot's answers together; *Automatic* uses the system's language for
  the menus.
- **Answers in the right language:** the class bot's instructions are German, so
  it sometimes answered English questions in German. The app now tells it to answer
  in the language you write in (or the one you picked), and repeats a chosen
  language after your message.
- **Chat names sum up the chat:** after the first message the bot writes a short
  title for it (e.g. "Fibonacci numbers in Python") instead of showing the start
  of the message, in the language of the menus. Changing the language renames the
  chats to match. Chats from the
  last week get a title too. Renaming a chat yourself still wins.
- **Tidier chat list:** chats are grouped under *Today*, *Yesterday*, *Last 7 days*
  and *Older*, and each shows the start of its last message. Hovering a chat shows
  rename ✏ and delete 🗑 buttons.
- **Deleting a chat** happens at once, without a confirmation box, and a notice at
  the bottom offers **Undo** for a few seconds.
- The **Reply** button only appears after you select text by dragging, not after
  a plain click into an answer.
- **Settings** are split into tabs: *General*, *Free AI models*, *GitHub* and
  *Advanced*.

- **Model list** at the top of the window: the class bot (AIS.chat, default) or a
  free model from **Groq**, **OpenRouter**, **Mistral**, **Google Gemini** or any
  other OpenAI-compatible service – for when the class chat is not available. Each
  provider has its own free API key, separate from the GitHub token. The free
  models get the class bot's instructions, and a chat can switch models at any time.
- The window no longer waits for the system password store (it could freeze for
  up to 25 seconds on systems without a running keyring).

## 1.0.1 – 2026-11-06

**New downloads for Linux**
- **AppImage** – one file that runs on most distributions without installing:
  download, make it executable (`chmod +x`, or *Properties → Allow executing*),
  double-click.
- **.deb** for Debian, Ubuntu and Linux Mint – installs the app into the menu:
  `sudo apt install ./ais-chat-client_1.0.1_amd64.deb`
- the plain program file `ais-chat-client-linux-x86_64`
- The Linux files now also run on older systems (Ubuntu 22.04, Debian 12,
  Linux Mint 21 and newer).

**Other**
- New app icon (the ais.chat logo) for the window, the menu and the AppImage.

Windows: `ais-chat-client-windows-x86_64.exe` (Windows may warn that the app is
from an unknown publisher – click *More info → Run anyway*).

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
