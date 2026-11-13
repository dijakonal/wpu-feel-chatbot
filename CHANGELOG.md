# Changelog

Every commit raises the last number of the version (in `Cargo.toml`) and is listed here.

## 1.2.7 – 2026-11-13

- **Open the dashboard** opens the dashboard's https address, now that the sync server
  serves its dashboard over https (sync server 0.3.7). The browser may warn once about
  the server's own certificate; the server shows its fingerprint to check it.

## 1.2.6 – 2026-11-13

- **Open the dashboard:** if the server's owner gave your sync account dashboard
  permissions, *Settings → Account* shows **Open the dashboard**. It opens the
  server's dashboard in the browser, already logged in – also for accounts made with
  a school login, which have no password you could type there.
- **Without a school login, only accounts made without one:** the app now tells the
  sync server how it was opened. Opened without a school login, it cannot log in to
  an account made with Moodle or moin.schule; a saved login of such an account is
  logged out with an explanation (a school login logs in there again by itself).

## 1.2.5 – 2026-11-13

- **Three ways in:** the start screen now offers **OHG network** (the Moodle login as
  before), **moin.schule** (the school login of Lower Saxony, checked at
  auth.moin.schule in the same open way) and **Continue without an account**. The
  app remembers the last choice and user name.
- **Without an account** only the class bot answers, and chatting needs an account
  on a sync server that the person running it has approved. GitHub, the free AI
  models, changing the invite link and the advanced settings (upload address and
  others) are not available.
- **Approval:** new sync accounts wait until the server's owner approves them. The
  app shows "Waiting for approval" in *Settings → Account*, on the account card and
  above the message box. With a school login the chat keeps working meanwhile, just
  without credits.
- The sync server is told how you logged in (Moodle, moin.schule or no school
  login), so it can warn before approving an account without one.
- `docs/MOODLE-LOGIN.md` explains the moin.schule check and the account-free mode in
  English and German.

## 1.2.4 – 2026-11-12

- **Sync login with the school account:** after the school login on the start
  screen, the app logs in to the sync server by itself, with your school user name.
  The account is created on first use, so the same school login gives you the same
  chats on every computer. *Settings → Account* also has a **Log in as … (school
  account)** button for when the server address is entered later.
- **Your school password is still not sent to the sync server:** the app turns it
  into a separate code (Argon2id) that cannot be turned back into the password, and
  only that code goes there. `docs/MOODLE-LOGIN.md` explains this in English and
  German.
- If the school password was changed, the old sync account no longer accepts the
  new code; the app says so, and the account can be deleted in the server's
  dashboard.

## 1.2.3 – 2026-11-12

- **Fixed: GitHub token, API keys and the sync login disappeared after a restart.**
  When the app started, it reconnected to the class chat with a copy of the settings
  made before the saved tokens were read, and saving that copy deleted them from the
  system password store. The app now only takes the chat's connection details from
  that reconnect, and never deletes a saved token before it has read it. (Tokens that
  were already lost have to be entered once more.)
- **Fixed: changing the language or the model while an answer was coming in could
  be undone** when the class chat reconnected in the middle of it.
- **Saving to GitHub is more reliable:**
  - Saves run one after another. Two at once – automatic saving and a click, or two
    answers in a row – often failed with "the branch may have changed" or merged
    into an old version of the file.
  - If the branch changed on GitHub in the meantime (for example a push from
    another computer), the app builds the commit again on top of it instead of
    failing (up to three tries).
  - Clicking **⬆ GitHub** without a token now says that the token is missing,
    instead of doing nothing.

## 1.2.2 – 2026-11-12

- Removed `CLAUDE.md` (notes for the AI assistant) again.

## 1.2.1 – 2026-11-12

- **Changelog:** from now on every change gets its own version number and an entry
  here.
- **Fixed: could not connect to a sync server on another computer.** An address
  like `192.168.1.20:8787` was opened with https, but a server started on a laptop
  speaks plain http. Addresses in the same network (`192.168.…`, `10.…`,
  `laptop.local`, …) now use http. When the server cannot be reached, the app says
  why: wrong address, server not running, firewall, or https/http mixed up.

## 1.2.0 – 2026-11-12

- **Nicer account display:** the bottom of the chat list is now a small account
  card with your name, the sync state and a credit bar that turns orange when
  credits run low and red when they are used up. *Settings → Account* shows the
  same bar, and the school login screen sits in a framed card.
- **Fixed "Application Not Responding" on Linux (Wayland):** the desktop reported
  the app as frozen when its window was hidden, for example after switching to
  another workspace. The app no longer waits for the display before drawing.
- **Credits:** with an account on the sync server, every message to the bot costs
  one credit. The person who runs the server sets the limit in its new dashboard
  (per day, per week, per account). The bottom of the chat list shows the balance
  ("12 of 50 credits used"); when the limit is reached, the chat says so and when
  new credits arrive.
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
