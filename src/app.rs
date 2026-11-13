//! The window: chat, input box with attachments, and the Settings window.
//!
//! Network work runs on background threads that report back through a channel, so the window
//! never freezes. Setup is automatic: on start the app reads the current request values from
//! the site, and whenever AIS.chat is updated (sending fails with "not recognised") it reads
//! them again and re-sends the message without asking the user to do anything.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Key, Layout, Modifiers, RichText, Rounding, Stroke};

use crate::ais::{self, Attachment, AttachmentData, ChatMessage, Event, Role};
use crate::conversations::{self, Conversation, FileState, PendingFile};
use crate::i18n::{t, tf};
use crate::github;
use crate::markdown::{self, PushState};
use crate::theme;
use crate::settings::{Settings, DEFAULT_INVITE_LINK};

/// Text files up to this size are pasted into the message instead of being uploaded.
const INLINE_LIMIT: usize = 200 * 1024;

/// At most this much text from a GitHub repository goes into one message.
const REPO_BUDGET: usize = 300 * 1024;

const FALLBACK_GREETING: &str = "Hello! How can I help you?";

/// The plain-language explanation of the school login, shown under the login box.
const SCHOOL_LOGIN_EXPLAINED: &str = "https://github.com/dijakonal/wpu-feel-chatbot/blob/main/docs/MOODLE-LOGIN.md";

enum Msg {
    /// Background setup finished. `from_settings`: started by the Connect button.
    Refreshed { result: Result<(Settings, String), String>, from_settings: bool },
    /// A worker had to read fresh values from the site while sending.
    SettingsChanged(Settings),
    /// A GitHub push finished; `keys` are the code blocks it carried (`<message id>#<n>`).
    Pushed { keys: Vec<String>, result: Result<String, String> },
    /// "Check access" in Settings finished.
    GithubChecked(Result<String, String>),
    /// The file list of the GitHub repository: `(branch, files, list cut short)`.
    RepoListed(Result<(String, Vec<github::RepoFile>, bool), String>),
    /// One file from the repository was read for an attachment.
    RepoFileLoaded { conv: String, id: u64, result: Result<String, String> },
    /// The GitHub token and the AI keys were read from the password store (in the
    /// background, at start), as `(name, value)`.
    SecretsLoaded(Vec<(String, String)>),
    /// The model list of a provider was read: `(provider id, models)`.
    ModelsLoaded(String, Result<Vec<crate::models::Model>, String>),
    /// The bot translated its greeting (`None`: it did not work).
    GreetingTranslated { lang: String, source: String, text: Option<String> },
    /// The school login was checked at Moodle (see `moodle_login.rs`).
    /// `method` is "moodle" or "moin"; `sync_user` and `sync_code` are what the sync server
    /// gets instead of the school login (only after a successful check).
    SchoolLoginChecked { method: &'static str, username: String, result: crate::moodle_login::LoginResult, sync_user: String, sync_code: Option<String> },
    /// Logging in to the sync server finished: `(server, token, user name)`.
    LoggedIn(Result<(String, String, String, bool), String>),
    /// A sync finished: the new revision and the changes from elsewhere, or
    /// `(login no longer valid, message)`.
    /// `(login no longer valid, account waits for approval, message)` on failure.
    Synced(Result<(i64, Vec<crate::sync::Item>, Option<crate::sync::Credits>, bool), (bool, bool, String)>),
    /// The dashboard login for "Open the dashboard": the address to open.
    DashboardLink(Result<String, String>),
    /// The server said where the account's credits stand.
    Credits(crate::sync::Credits),
    /// The bot summarised the first message of a chat (`None`: it did not work).
    Summarised { conv: String, lang: String, title: Option<String> },
    // `conv` = id of the chat the work belongs to (it may no longer be the open one)
    Uploaded { conv: String, id: u64, result: Result<String, String> },
    Text { conv: String, gen: u64, text: String },
    Activity { conv: String, gen: u64, status: String },
    ReplyDone { conv: String, gen: u64, result: Result<(), String> },
}

#[derive(PartialEq)]
enum Conn {
    Connecting,
    Ready,
    Problem(String),
}

pub struct App {
    settings: Settings,
    conn: Conn,

    // chats; `App` derefs to the open one, so `self.messages`, `self.input`, … mean "of the
    // open chat"
    convs: Vec<Conversation>,
    active: usize,
    show_sidebar: bool,
    renaming: Option<(String, String)>,
    /// The chat deleted last and when (`ctx` time) – it can be brought back for a few seconds.
    undo_delete: Option<(Conversation, f64)>,
    next_file_id: u64,
    focus_input: bool,
    /// Where the floating "Reply" button sits while text is selected (last frame).
    reply_button: Option<egui::Rect>,
    /// Where the pointer was released after selecting text, and whether the drag went
    /// downwards (then the button goes below that point, otherwise above it).
    selection_anchor: Option<(egui::Pos2, bool)>,
    /// Where the current/last drag started.
    drag_origin: Option<egui::Pos2>,
    /// Open page of the settings dialog (0 General, 1 Free AI models, 2 GitHub, 3 Advanced).
    settings_tab: usize,

    /// When the last frame was drawn (drawing is paced here, not by vsync).
    last_frame: std::time::Instant,

    // ---- school login (Moodle): the app is locked until it worked
    school_login_ok: bool,
    school_login_checking: bool,
    school_login_user: String,
    /// Only held while typing; handed to the check and then gone (never saved).
    school_login_password: String,
    school_login_error: Option<&'static str>,
    /// The code for the sync server made from the school password (`moodle_login::sync_code`);
    /// only in memory, for this run.
    school_sync_code: Option<String>,
    /// The name the sync server knows the school account by.
    school_sync_user: String,
    /// How this run was unlocked: "moodle", "moin" (a school login) or "none" (continued
    /// without an account – see `guest`).
    login_method: &'static str,
    /// The choice on the start screen.
    login_choice: &'static str,
    /// The sync account exists but waits for approval by the person running the server.
    sync_pending: bool,
    /// The sync account may open the server's dashboard.
    sync_dashboard: bool,
    /// Logging in to the sync server with the school account was tried in this run.
    school_sync_tried: bool,

    // ---- account on the sync server
    sync_state: crate::sync::SyncState,
    /// The sync that is running (its changes are noted once the server took them).
    sync_in_flight: Option<crate::sync::Outgoing>,
    /// `ctx` time of the next sync (sooner after a change, every 30 s otherwise).
    next_sync: std::cell::Cell<f64>,
    /// Result of the last login or sync, when worth showing.
    sync_note: Option<(bool, String)>,
    logging_in: bool,
    /// The account's credits as the server last reported them.
    credits: Option<crate::sync::Credits>,
    /// Typed into the login form (never saved).
    account_password: String,

    // settings window
    show_settings: bool,
    link_input: String,
    curl_input: String,
    draft: Settings,
    settings_note: Option<(bool, String)>,
    connecting_from_settings: bool,
    github_note: Option<(bool, String)>,
    github_checking: bool,
    /// Language whose greeting translation is being fetched.
    translating: Option<String>,
    /// Chat whose title is being written by the bot right now (one at a time).
    summarising: Option<String>,
    /// `<chat id>:<language>` of summaries that failed in this run (not asked again until restart).
    summary_failed: std::collections::HashSet<String>,
    /// The "Attach from GitHub" dialog, while open.
    repo_picker: Option<RepoPicker>,
    /// Free models for the model list (built-in list until the catalog is read).
    models: Vec<crate::models::Model>,
    models_loading: bool,
    /// Search text in the model menu.
    model_filter: String,
    /// The model menu was just opened: scroll the current model into view once.
    scroll_to_model: bool,
    models_note: Option<(bool, String)>,
    /// GitHub state of code blocks, by `<message id>#<n>`.
    push_states: std::collections::HashMap<String, PushState>,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = Settings::load();
        let moodle_user = if settings.login_method == "moin" { settings.moin_user.clone() } else { settings.moodle_user.clone() };
        let login_choice = if settings.login_method == "moin" { "moin" } else { "moodle" };
        theme::set_appearance(&cc.egui_ctx, &settings.appearance);
        let (tx, rx) = channel();
        let mut app = Self {
            link_input: settings.invite_link.clone(),
            draft: settings.clone(),
            settings,
            conn: Conn::Connecting,
            convs: conversations::load(),
            active: 0,
            show_sidebar: true,
            renaming: None,
            undo_delete: None,
            next_file_id: 0,
            focus_input: true,
            reply_button: None,
            selection_anchor: None,
            drag_origin: None,
            settings_tab: 0,
            last_frame: std::time::Instant::now(),
            school_login_ok: false,
            school_login_checking: false,
            school_login_user: moodle_user,
            school_login_password: String::new(),
            school_login_error: None,
            school_sync_code: None,
            school_sync_user: String::new(),
            login_method: "moodle",
            login_choice,
            sync_pending: false,
            sync_dashboard: false,
            school_sync_tried: false,
            sync_state: crate::sync::SyncState::load(),
            sync_in_flight: None,
            next_sync: std::cell::Cell::new(0.0),
            sync_note: None,
            logging_in: false,
            credits: None,
            account_password: String::new(),
            show_settings: false,
            curl_input: String::new(),
            settings_note: None,
            connecting_from_settings: false,
            github_note: None,
            github_checking: false,
            translating: None,
            summarising: None,
            summary_failed: Default::default(),
            repo_picker: None,
            models: Vec::new(),
            models_loading: false,
            model_filter: String::new(),
            scroll_to_model: false,
            models_note: None,
            push_states: Default::default(),
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            title: String::new(),
        };
        // carry on where you left off: open the most recent chat (a new one if there is none)
        match app.convs.iter().enumerate().filter(|(_, c)| !c.is_empty()).max_by_key(|(_, c)| c.updated) {
            Some((i, _)) => app.active = i,
            None => app.new_chat(),
        }
        app.start_refresh(false);
        app.models = crate::models::suggested_models(app.settings.ai_provider());
        app.load_models();
        // the password store can take long to answer (or not exist): never wait for it here
        let tx = app.tx.clone();
        let ctx = app.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::SecretsLoaded(Settings::load_secrets()));
            ctx.request_repaint();
        });
        install_quote_capture(&cc.egui_ctx);
        app
    }

    // ------------------------------------------------------------------ background work

    fn start_refresh(&mut self, from_settings: bool) {
        let mut s = if from_settings { self.draft.clone() } else { self.settings.clone() };
        if !from_settings {
            self.conn = Conn::Connecting;
        }
        self.connecting_from_settings = from_settings;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = ais::refresh(&mut s).map(|summary| (s, summary)).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Refreshed { result, from_settings });
            ctx.request_repaint();
        });
    }

    fn add_file(&mut self, name: String, bytes: Vec<u8>) {
        let id = self.next_file_id;
        self.next_file_id += 1;
        let size = bytes.len();

        if size > ais::MAX_UPLOAD_BYTES {
            let msg = tf("too large (max {0} MB)", &[&(ais::MAX_UPLOAD_BYTES / 1024 / 1024).to_string()]);
            self.pending.push(PendingFile { id, name, size, state: FileState::Failed(msg) });
            return;
        }
        // Small text files are pasted into the message: that always works and the bot sees them right away.
        if ais::is_text_name(&name) && size <= INLINE_LIMIT {
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                self.pending.push(PendingFile { id, name, size, state: FileState::Ready(AttachmentData::Inline(text)) });
                return;
            }
        }
        // the free models only read text: PDFs, images and Office files need the class bot's upload
        if self.settings.uses_ai_models() {
            let msg = t("only text and code files work with the free models – switch to the class bot for this file").to_string();
            self.pending.push(PendingFile { id, name, size, state: FileState::Failed(msg) });
            return;
        }
        // Everything else goes to the upload address, which is worked out from the invite link.
        self.pending.push(PendingFile { id, name: name.clone(), size, state: FileState::Uploading });
        let s = self.settings.clone();
        let session = self.session_id.clone();
        let conv = self.id.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = ais::upload_file(&s, &name, bytes, &session).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Uploaded { conv, id, result });
            ctx.request_repaint();
        });
    }

    fn pick_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new().set_title(t("Attach files")).pick_files() {
            for p in paths {
                self.add_path(&p);
            }
        }
    }

    fn add_path(&mut self, p: &std::path::Path) {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        match std::fs::read(p) {
            Ok(bytes) => self.add_file(name, bytes),
            Err(e) => {
                let id = self.next_file_id;
                self.next_file_id += 1;
                self.pending.push(PendingFile { id, name, size: 0, state: FileState::Failed(tf("could not read: {0}", &[&e.to_string()])) });
            }
        }
    }

    /// Without a school login the chat needs an approved account on a sync server.
    fn chat_allowed(&self) -> bool {
        !self.guest() || (self.settings.sync_ready() && !self.sync_pending)
    }

    /// Continued without an account: only the class bot, no GitHub, no other bots, no
    /// advanced settings.
    fn guest(&self) -> bool {
        self.login_method == "none"
    }

    /// GitHub saving and reading – only with a school login.
    fn github_on(&self) -> bool {
        !self.guest() && self.settings.github_ready()
    }

    fn can_send(&self) -> bool {
        self.chat_allowed()
            && !self.busy
            && !self.pending.iter().any(|f| matches!(f.state, FileState::Uploading))
            && (!self.input.trim().is_empty() || self.pending.iter().any(|f| matches!(f.state, FileState::Ready(_))))
    }

    fn send(&mut self) {
        if !self.can_send() {
            return;
        }
        let attachments: Vec<Attachment> = std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|f| match f.state {
                FileState::Ready(data) => Some(Attachment { name: f.name, size: f.size, data }),
                _ => None,
            })
            .collect();
        let text = std::mem::take(&mut self.input);
        let mut msg = ChatMessage::user_with(text.trim(), attachments);
        if let Some(q) = self.quote.take() {
            msg = msg.with_quote(&q);
        }
        self.messages.push(msg);
        self.touch();
        self.start_reply();
        self.save_chats();
    }

    /// Ask for an answer to the conversation as it is now.
    fn start_reply(&mut self) {
        self.error = None;
        self.busy = true;
        self.activity.clear();
        self.focus_input = true;
        if let Some(last) = self.messages.iter_mut().rev().find(|m| m.role == Role::User) {
            last.touch();
        }
        let history = self.messages.clone();
        self.messages.push(ChatMessage::assistant_placeholder());

        let mut s = self.settings.clone();
        let session = self.session_id.clone();
        let conv = self.id.clone();
        let gen = self.gen;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        // with an account, the sync server is asked for a credit first
        let account = self.settings.sync_ready().then(|| (s.sync_server.trim().to_string(), s.sync_token.trim().to_string()));
        let guest = self.guest();
        std::thread::spawn(move || {
            if let Some((server, token)) = account {
                let refused = match crate::sync::spend_credit(&server, &token) {
                    Ok(crate::sync::Spend::Spent(c)) => {
                        let _ = tx.send(Msg::Credits(c));
                        None
                    }
                    Ok(crate::sync::Spend::Refused(c)) => {
                        let text = no_credits_text(&c);
                        let _ = tx.send(Msg::Credits(c));
                        Some(text)
                    }
                    Ok(crate::sync::Spend::NoCredits) => None,
                    // the sync account still waits: with a school login the chat works without
                    // credits (as without an account); without one it needs the approval
                    Ok(crate::sync::Spend::Pending) if guest => {
                        Some(t("Your account is waiting for approval by the person who runs the sync server.").to_string())
                    }
                    Ok(crate::sync::Spend::Pending) => None,
                    Err(e) => Some(tf("Could not check your credits: {0}", &[&e.to_string()])),
                };
                if let Some(text) = refused {
                    let _ = tx.send(Msg::ReplyDone { conv, gen, result: Err(text) });
                    ctx.request_repaint();
                    return;
                }
            }
            let result = reply_worker(&mut s, &history, &session, &conv, gen, &tx, &ctx);
            let _ = tx.send(Msg::ReplyDone { conv, gen, result: result.map_err(|e| e.to_string()) });
            ctx.request_repaint();
        });
    }

    fn retry(&mut self) {
        if self.busy {
            return;
        }
        if matches!(self.messages.last(), Some(m) if m.role == Role::Assistant && !m.initial) {
            self.messages.pop();
        }
        if matches!(self.messages.last(), Some(m) if m.role == Role::User) {
            self.start_reply();
        }
    }

    fn stop(&mut self) {
        self.gen += 1;
        self.busy = false;
        self.activity.clear();
        if let Some(m) = self.messages.last_mut() {
            if m.role == Role::Assistant && !m.initial {
                if m.content.trim().is_empty() {
                    self.messages.pop();
                } else {
                    m.content.push_str("\n\n*(stopped)*");
                }
            }
        }
        self.save_chats();
    }

    fn greeting(&self) -> String {
        let g = self.settings.displayed_greeting();
        if g.trim().is_empty() { FALLBACK_GREETING.to_string() } else { g }
    }

    /// For a bot whose greeting the app cannot translate itself: ask the bot once (in a
    /// separate session that no chat uses) and keep the answer in the settings.
    fn translate_greeting_if_needed(&mut self) {
        let lang = self.settings.language.trim().to_string();
        let can_ask = self.settings.is_ready() || self.settings.uses_ai_models();
        if !self.settings.greeting_needs_translation() || !can_ask || self.translating.as_deref() == Some(lang.as_str()) {
            return;
        }
        let Some((_, english)) = crate::settings::language(&lang) else { return };
        self.translating = Some(lang.clone());
        let s = self.settings.clone();
        let source = s.greeting.clone();
        let request = crate::i18n::translation_request(&source, english);
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let text = crate::backend::ask(&s, &request).ok().and_then(|reply| crate::i18n::clean_translation(&reply, &source));
            let _ = tx.send(Msg::GreetingTranslated { lang, source, text });
            ctx.request_repaint();
        });
    }

    /// Give chats a short title that sums up their first message, written by the bot in the
    /// background: the open chat first, then other chats of the last week (one at a time).
    fn summarise_titles(&mut self) {
        let can_ask = self.settings.is_ready() || self.settings.uses_ai_models();
        if self.summarising.is_some() || !can_ask {
            return;
        }
        // titles are part of the menus: they follow the menu language
        let lang = self.settings.ui_language();
        let english = crate::settings::language(&lang).map(|(_, e)| e.to_string());
        let week_ago = conversations::now() - 7 * 24 * 3600;
        let wanted = |c: &Conversation| !self.summary_failed.contains(&format!("{}:{lang}", c.id)) && c.needs_summary(&lang).is_some();
        let open = Some(self.active).filter(|&i| wanted(&self.convs[i]));
        // other chats: recent ones, and every one that has a title in another language
        let other = || self.convs.iter().position(|c| !c.busy && (c.updated > week_ago || c.summary.is_some()) && wanted(c));
        let Some(i) = open.or_else(other) else { return };
        let Some(first) = self.convs[i].needs_summary(&lang) else { return };
        let conv = self.convs[i].id.clone();
        self.summarising = Some(conv.clone());
        let s = self.settings.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let reply = crate::backend::ask(&s, &conversations::summary_request(&first, english.as_deref()));
            let title = reply.ok().and_then(|r| conversations::clean_summary(&r));
            let _ = tx.send(Msg::Summarised { conv, lang, title });
            ctx.request_repaint();
        });
    }

    /// Open an empty chat: reuse one that is still empty, otherwise start a new one.
    fn new_chat(&mut self) {
        self.renaming = None;
        self.focus_input = true;
        if let Some(i) = self.convs.iter().position(|c| c.is_empty() && !c.busy) {
            self.active = i;
            return;
        }
        let c = Conversation::new(&self.greeting());
        self.convs.insert(0, c);
        self.active = 0;
    }

    fn open_chat(&mut self, id: &str) {
        self.save_chats(); // keeps the draft of the chat we leave
        if let Some(i) = self.convs.iter().position(|c| c.id == id) {
            self.active = i;
            self.focus_input = true;
            self.renaming = None;
        }
    }

    fn delete_chat(&mut self, id: &str) {
        let Some(i) = self.convs.iter().position(|c| c.id == id) else { return };
        let open_id = self.id.clone();
        let removed = self.convs.remove(i);
        if self.settings.sync_ready() {
            // other computers delete it too (sent once Undo is no longer possible)
            self.sync_state.deleted.insert(removed.id.clone(), conversations::now());
            self.sync_state.save();
        }
        self.undo_delete = Some((removed, self.ctx.input(|i| i.time)));
        if self.convs.is_empty() {
            self.convs.push(Conversation::new(&self.greeting()));
        }
        // keep the open chat open; if it was the deleted one, open the newest
        self.active = self.convs.iter().position(|c| c.id == open_id).unwrap_or_else(|| self.newest_index());
        self.save_chats();
    }

    /// Bring the chat deleted last back.
    fn undo_delete_chat(&mut self) {
        let Some((c, _)) = self.undo_delete.take() else { return };
        if self.sync_state.deleted.remove(&c.id).is_some() {
            self.sync_state.save();
        }
        // added at the end, so the open chat keeps its index
        self.convs.push(c);
        self.save_chats();
    }

    /// The notice at the bottom after deleting a chat, with "Undo" (for 6 seconds).
    fn undo_toast(&mut self, ctx: &egui::Context) {
        const SHOW_FOR: f64 = 6.0;
        let Some((c, at)) = &self.undo_delete else { return };
        let left = SHOW_FOR - (ctx.input(|i| i.time) - at);
        if left <= 0.0 {
            self.undo_delete = None;
            self.sync_soon(); // the deletion can go to the other computers now
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let p = theme::palette(ctx.style().visuals.dark_mode);
        let title = c.title();
        let screen = ctx.screen_rect();
        let mut undo = false;
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("undo-toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -110.0])
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(p.surface_hi)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .rounding(12.0)
                    .shadow(egui::epaint::Shadow { offset: egui::vec2(0.0, 6.0), blur: 20.0, spread: 0.0, color: Color32::from_black_alpha(80) })
                    .inner_margin(egui::Margin { left: 14.0, right: 8.0, top: 6.0, bottom: 6.0 })
                    .show(ui, |ui| {
                        ui.set_max_width((screen.width() - 80.0).min(460.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🗑").color(p.weak));
                            let text = fit_end(ui, &tf("Deleted “{0}”", &[&title]), &egui::TextStyle::Body.resolve(ui.style()), 260.0);
                            ui.label(RichText::new(text).color(p.text));
                            if ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(t("Undo")).family(theme::bold())).min_size(egui::vec2(0.0, 30.0))).clicked() {
                                undo = true;
                            }
                            if theme::ghost_button(ui, "✖").on_hover_text(t("Dismiss")).clicked() {
                                dismiss = true;
                            }
                        });
                        // how long "Undo" is still possible
                        let r = ui.min_rect();
                        let bar = egui::Rect::from_min_size(egui::pos2(r.left(), r.bottom() + 3.0), egui::vec2(r.width() * (left / SHOW_FOR) as f32, 2.0));
                        ui.painter().rect_filled(bar, 1.0, p.accent);
                    });
            });
        if undo {
            self.undo_delete_chat();
        } else if dismiss {
            self.undo_delete = None;
            self.sync_soon();
        }
    }

    fn newest_index(&self) -> usize {
        (0..self.convs.len()).max_by_key(|&i| self.convs[i].updated).unwrap_or(0)
    }

    fn save_chats(&self) {
        if let Err(e) = conversations::save(&self.convs) {
            eprintln!("could not save chats: {e}");
        }
        // something changed: sync it in a moment
        let soon = self.ctx.input(|i| i.time) + 2.0;
        self.next_sync.set(self.next_sync.get().min(soon));
    }

    // ------------------------------------------------------------------ account & sync

    fn sync_soon(&self) {
        let soon = self.ctx.input(|i| i.time) + 1.0;
        self.next_sync.set(self.next_sync.get().min(soon));
    }

    fn account_key(&self) -> String {
        format!("{}|{}", self.settings.sync_server.trim(), self.settings.sync_user.trim())
    }

    /// Start a sync when one is due.
    fn sync_tick(&mut self, ctx: &egui::Context) {
        if !self.settings.sync_ready() || self.sync_in_flight.is_some() || self.logging_in {
            return;
        }
        let now = ctx.input(|i| i.time);
        let due = self.next_sync.get();
        if now < due {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - now).min(60.0)));
            return;
        }
        self.start_sync();
    }

    fn start_sync(&mut self) {
        let account = self.account_key();
        if self.sync_state.account != account {
            self.sync_state.reset(&account);
        }
        let mut out = crate::sync::outgoing(&self.convs, &self.sync_state);
        // a deletion that can still be undone waits
        if let Some((c, _)) = &self.undo_delete {
            out.items.retain(|i| i.id != c.id);
            out.sent_deleted.retain(|id| *id != c.id);
        }
        let server = self.settings.sync_server.trim().to_string();
        let token = self.settings.sync_token.trim().to_string();
        let send = out.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = crate::sync::sync(&server, &token, &send).map_err(|e| {
                // a login that no longer works, or a school account while the app runs without a
                // school login: forget the login (a school login logs in there again by itself)
                let unusable = e.downcast_ref::<crate::sync::Unauthorized>().is_some() || e.downcast_ref::<crate::sync::SchoolAccount>().is_some();
                (unusable, e.downcast_ref::<crate::sync::Pending>().is_some(), e.to_string())
            });
            let _ = tx.send(Msg::Synced(result));
            ctx.request_repaint();
        });
        self.sync_in_flight = Some(out);
        self.next_sync.set(f64::INFINITY);
    }

    /// After the school login: log in to the sync server with the school account, if a server
    /// is set up and nobody is logged in there yet. Once per run.
    fn school_sync_login(&mut self) {
        // the saved sync login must have been read first (it may already be logged in)
        if self.school_sync_tried || self.school_sync_code.is_none() || !crate::secret::loaded() {
            return;
        }
        if self.settings.sync_server.trim().is_empty() || self.settings.sync_ready() || self.logging_in {
            return;
        }
        self.school_sync_tried = true;
        let server = self.settings.sync_server.trim().to_string();
        self.start_school_sync_login(server);
    }

    fn start_school_sync_login(&mut self, server: String) {
        let Some(code) = self.school_sync_code.clone() else { return };
        let user = self.school_sync_user.clone();
        let method = self.login_method;
        self.logging_in = true;
        self.sync_note = None;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = crate::sync::login_with_school_account(&server, &user, &code, method)
                .map(|ok| (server, ok.token, ok.username, ok.pending))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::LoggedIn(result));
            ctx.request_repaint();
        });
    }

    fn start_login(&mut self, create: bool) {
        let server = match crate::sync::normalize_server(&self.draft.sync_server) {
            Ok(s) => s,
            Err(e) => {
                self.sync_note = Some((false, e.to_string()));
                return;
            }
        };
        let user = self.draft.sync_user.trim().to_string();
        let password = self.account_password.clone();
        if user.is_empty() || password.is_empty() {
            self.sync_note = Some((false, t("Enter the user name and password.").into()));
            return;
        }
        self.logging_in = true;
        self.sync_note = None;
        let method = self.login_method;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = crate::sync::login(&server, &user, &password, create, method)
                .map(|ok| (server, ok.token, ok.username, ok.pending))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::LoggedIn(result));
            ctx.request_repaint();
        });
    }

    /// Log out: the server forgets the token; the chats stay on this computer.
    fn logout(&mut self) {
        let server = self.settings.sync_server.trim().to_string();
        let token = self.settings.sync_token.trim().to_string();
        std::thread::spawn(move || crate::sync::logout(&server, &token));
        let mut s = self.settings.clone();
        s.sync_token.clear();
        self.draft.sync_token.clear();
        self.apply_settings(s);
        self.sync_state.reset("");
        self.sync_state.save();
        self.sync_in_flight = None;
        self.sync_note = None;
        self.credits = None;
        self.sync_pending = false;
        self.sync_dashboard = false;
    }

    /// "Open the dashboard": ask the server for a dashboard login and open it in the browser.
    fn open_dashboard(&mut self) {
        let server = self.settings.sync_server.trim().to_string();
        let token = self.settings.sync_token.trim().to_string();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::DashboardLink(crate::sync::dashboard_login(&server, &token).map_err(|e| e.to_string())));
            ctx.request_repaint();
        });
    }

    /// "12 of 50 credits used", if the server limits credits.
    fn credits_line(&self) -> Option<String> {
        let c = self.credits.as_ref()?;
        if c.blocked {
            return Some(t("This account was blocked.").to_string());
        }
        let limit = c.limit?;
        Some(tf("{0} of {1} credits used", &[&c.used.to_string(), &limit.to_string()]))
    }

    /// One line about the sync: what is going on, or when it last worked.
    fn sync_status(&self) -> (bool, String) {
        if self.sync_pending {
            return (true, t("Waiting for approval").to_string());
        }
        if self.sync_in_flight.is_some() {
            return (true, t("Syncing…").to_string());
        }
        if let Some((false, e)) = &self.sync_note {
            return (false, e.clone());
        }
        if self.sync_state.last_sync > 0 && self.sync_state.account == self.account_key() {
            (true, tf("Last synced {0}", &[&conversations::when(self.sync_state.last_sync)]))
        } else {
            (true, t("Not synced yet").to_string())
        }
    }

    fn chat_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn apply_settings(&mut self, s: Settings) {
        let before = self.greeting();
        self.settings = s;
        let _ = self.settings.save();
        self.refresh_greetings(&before);
        self.translate_greeting_if_needed();
    }

    /// Chats that have not started yet show the current greeting (bot or language changed).
    fn refresh_greetings(&mut self, before: &str) {
        let g = self.greeting();
        if g != before {
            for c in &mut self.convs {
                c.set_greeting(&g);
            }
        }
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Refreshed { result, from_settings } => {
                    if from_settings {
                        self.connecting_from_settings = false;
                    }
                    match result {
                        Ok((found, summary)) => {
                            self.link_input = found.invite_link.clone();
                            let next = if from_settings {
                                // Connect in Settings: the form's values, with the current secrets
                                let mut s = found.clone();
                                s.keep_secrets_of(&self.settings);
                                s
                            } else {
                                // at start: only what connecting found out
                                let mut s = self.settings.clone();
                                s.take_connection_from(&found);
                                s
                            };
                            self.draft.take_connection_from(&found);
                            self.apply_settings(next);
                            self.conn = Conn::Ready;
                            if from_settings {
                                self.settings_note = Some((true, summary));
                            }
                        }
                        Err(e) => {
                            if from_settings {
                                self.settings_note = Some((false, e));
                            } else if self.settings.is_ready() {
                                // Saved values may still work; sending re-checks them if needed.
                                self.conn = Conn::Ready;
                            } else {
                                self.conn = Conn::Problem(e);
                            }
                        }
                    }
                }
                Msg::Pushed { keys, result } => {
                    let state = match result {
                        Ok(url) => PushState::Done(url),
                        Err(e) => PushState::Failed(e),
                    };
                    for k in keys {
                        self.push_states.insert(k, state.clone());
                    }
                }
                Msg::RepoListed(result) => {
                    if let Some(picker) = &mut self.repo_picker {
                        picker.loading = false;
                        match result {
                            Ok((branch, files, truncated)) => {
                                picker.branch = branch;
                                picker.files = files;
                                picker.truncated = truncated;
                            }
                            Err(e) => picker.error = Some(e),
                        }
                    }
                }
                Msg::RepoFileLoaded { conv, id, result } => {
                    let Some(c) = self.convs.iter_mut().find(|c| c.id == conv) else { continue };
                    if let Some(f) = c.pending.iter_mut().find(|f| f.id == id) {
                        f.state = match result {
                            Ok(text) => FileState::Ready(AttachmentData::Inline(text)),
                            Err(e) => FileState::Failed(e),
                        };
                    }
                }
                Msg::SchoolLoginChecked { method, username, result, sync_user, sync_code } => {
                    self.school_sync_code = sync_code;
                    self.school_sync_user = sync_user;
                    use crate::moodle_login::LoginResult;
                    self.school_login_checking = false;
                    match result {
                        LoginResult::Accepted => {
                            self.school_login_ok = true;
                            self.login_method = method;
                            crate::sync::set_app_login(method);
                            self.school_login_error = None;
                            self.focus_input = true;
                            // remember the choice and the user name (not the password) for next time
                            let mut s = self.settings.clone();
                            s.login_method = method.to_string();
                            if method == "moin" {
                                s.moin_user = username;
                            } else {
                                s.moodle_user = username;
                            }
                            if s != self.settings {
                                self.apply_settings(s);
                            }
                        }
                        LoginResult::WrongNameOrPassword => self.school_login_error = Some("Wrong user name or password."),
                        LoginResult::CouldNotCheck(problem) => self.school_login_error = Some(problem),
                    }
                }
                Msg::LoggedIn(result) => {
                    self.logging_in = false;
                    match result {
                        Ok((server, token, user, pending)) => {
                            self.sync_pending = pending;
                            let mut s = self.settings.clone();
                            s.sync_server = server.clone();
                            s.sync_user = user.clone();
                            s.sync_token = token.clone();
                            self.draft.sync_server = server;
                            self.draft.sync_user = user;
                            self.draft.sync_token = token;
                            self.apply_settings(s);
                            self.account_password.clear();
                            // start from zero: send every chat, get every chat
                            let account = self.account_key();
                            self.sync_state.reset(&account);
                            self.sync_state.save();
                            self.sync_note = Some((true, if pending {
                                t("Logged in. Waiting for approval by the person who runs the sync server.").into()
                            } else {
                                t("Logged in. Your chats are being synced.").into()
                            }));
                            self.next_sync.set(0.0);
                        }
                        Err(e) => self.sync_note = Some((false, e)),
                    }
                }
                Msg::Credits(c) => self.credits = Some(c),
                Msg::DashboardLink(result) => match result {
                    Ok(address) => self.ctx.open_url(egui::OpenUrl::new_tab(address)),
                    Err(e) => self.sync_note = Some((false, e)),
                },
                Msg::Synced(result) => {
                    let Some(out) = self.sync_in_flight.take() else { continue }; // logged out meanwhile
                    let now = self.ctx.input(|i| i.time);
                    match result {
                        Ok((rev, changes, credits, dashboard)) => {
                            self.credits = credits;
                            self.sync_dashboard = dashboard;
                            self.sync_pending = false;
                            let active_id = self.id.clone();
                            let applied = crate::sync::apply(&mut self.convs, &mut self.sync_state, &out, rev, changes);
                            if self.convs.is_empty() {
                                self.convs.push(Conversation::new(&self.greeting()));
                            }
                            // keep the open chat open; if it was deleted elsewhere, open the newest
                            self.active = self.convs.iter().position(|c| c.id == active_id).unwrap_or_else(|| self.newest_index());
                            if applied.anything() {
                                // (not save_chats: that would start the next sync right away)
                                let _ = conversations::save(&self.convs);
                            }
                            self.sync_state.save();
                            if matches!(self.sync_note, Some((false, _))) {
                                self.sync_note = None;
                            }
                        }
                        Err((expired, pending, e)) => {
                            self.sync_pending = pending;
                            if expired {
                                let mut s = self.settings.clone();
                                s.sync_token.clear();
                                self.draft.sync_token.clear();
                                self.apply_settings(s);
                            }
                            self.sync_note = Some((false, e));
                        }
                    }
                    self.next_sync.set(self.next_sync.get().min(now + 30.0));
                }
                Msg::SecretsLoaded(secrets) => {
                    let had_key = !self.settings.ai_key().trim().is_empty();
                    for (name, value) in secrets.into_iter().filter(|(_, v)| !v.trim().is_empty()) {
                        self.settings.set_secret(&name, value.clone());
                        self.draft.set_secret(&name, value);
                    }
                    if !had_key && !self.settings.ai_key().trim().is_empty() {
                        self.load_models();
                    }
                    // the settings hold the stored secrets now, so saving cannot lose them
                    crate::secret::mark_loaded();
                }
                Msg::ModelsLoaded(provider, result) => {
                    // an answer for a provider that is no longer selected is not used
                    if provider != self.settings.ai_provider().id {
                        continue;
                    }
                    self.models_loading = false;
                    match result {
                        Ok(models) => {
                            self.models = models;
                            if self.models_note.as_ref().map(|(ok, _)| *ok).unwrap_or(false) {
                                self.models_note = Some((true, tf("Ready: {0} models – pick one in the list at the top.", &[&self.models.len().to_string()])));
                            }
                        }
                        // the suggested models stay; sending tells what is wrong if it matters
                        Err(e) => self.models_note = Some((false, tf("Could not read the model list: {0}", &[&e]))),
                    }
                }
                Msg::GreetingTranslated { lang, source, text } => {
                    if self.translating.as_deref() == Some(lang.as_str()) {
                        self.translating = None;
                    }
                    if let Some(text) = text {
                        let before = self.greeting();
                        self.settings.greeting_translations.insert(lang, [source, text]);
                        let _ = self.settings.save();
                        self.refresh_greetings(&before);
                    }
                }
                Msg::Summarised { conv, lang, title } => {
                    self.summarising = None;
                    match title {
                        Some(t) => {
                            if let Some(c) = self.convs.iter_mut().find(|c| c.id == conv) {
                                c.summary = Some(t);
                                c.summary_lang = lang;
                                self.save_chats();
                            }
                        }
                        None => {
                            self.summary_failed.insert(format!("{conv}:{lang}"));
                        }
                    }
                }
                Msg::GithubChecked(result) => {
                    self.github_checking = false;
                    self.github_note = Some(match result {
                        Ok(t) => (true, t),
                        Err(e) => (false, e),
                    });
                }
                Msg::SettingsChanged(found) => {
                    // AIS.chat changed while answering: take its new request values only
                    let mut s = self.settings.clone();
                    s.take_connection_from(&found);
                    self.draft.take_connection_from(&found);
                    self.apply_settings(s);
                    self.conn = Conn::Ready;
                }
                Msg::Uploaded { conv, id, result } => {
                    let Some(c) = self.convs.iter_mut().find(|c| c.id == conv) else { continue };
                    if let Some(f) = c.pending.iter_mut().find(|f| f.id == id) {
                        f.state = match result {
                            Ok(file_id) => FileState::Ready(AttachmentData::Uploaded(file_id)),
                            Err(e) => FileState::Failed(e),
                        };
                    }
                }
                Msg::Text { conv, gen, text } => {
                    if let Some(c) = self.convs.iter_mut().find(|c| c.id == conv && c.gen == gen) {
                        if let Some(m) = c.messages.last_mut() {
                            m.content.push_str(&text);
                        }
                    }
                }
                Msg::Activity { conv, gen, status } => {
                    if let Some(c) = self.convs.iter_mut().find(|c| c.id == conv && c.gen == gen) {
                        c.activity = status;
                    }
                }
                Msg::ReplyDone { conv, gen, result } => {
                    let Some(c) = self.convs.iter_mut().find(|c| c.id == conv && c.gen == gen) else { continue };
                    c.busy = false;
                    c.activity.clear();
                    c.touch();
                    if let Err(e) = result {
                        if matches!(c.messages.last(), Some(m) if m.role == Role::Assistant && !m.initial && m.content.trim().is_empty())
                        {
                            c.messages.pop();
                        }
                        c.error = Some(e);
                    } else {
                        self.conn = Conn::Ready;
                        // automatic mode: save the answer's code right away
                        let answer = c.messages.last().filter(|m| m.role == Role::Assistant).map(|m| m.id.clone());
                        if let Some(answer) = answer.filter(|_| self.github_on() && self.settings.github_auto) {
                            self.push_code(&conv, &answer, None);
                        }
                    }
                    self.save_chats();
                }
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    fn top_bar(&mut self, ctx: &egui::Context) {
        let p = theme::palette(ctx.style().visuals.dark_mode);
        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::none().fill(p.panel).inner_margin(egui::Margin::symmetric(18.0, 10.0)).stroke(Stroke::new(1.0_f32, p.border)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let menu = ui.add(theme::Btn::new(theme::Kind::Ghost, RichText::new("☰").size(18.0)).min_size(egui::vec2(34.0, 34.0)));
                    let tip = if self.show_sidebar { t("Hide the chat list") } else { t("Show the chat list") };
                    if menu.on_hover_text(tip).clicked() {
                        self.show_sidebar = !self.show_sidebar;
                    }
                    theme::avatar(ui, &self.settings.display_name(), 38.0);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).size(16.0).color(p.text));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let github = self.settings.uses_ai_models();
                            let (color, text) = match &self.conn {
                                _ if self.busy => (p.success, t("typing…").to_string()),
                                _ if github => (p.success, tf("via {0}", &[self.settings.ai_provider().name])),
                                Conn::Ready => (p.success, t("Online").to_string()),
                                Conn::Connecting => (p.warning, t("Connecting…").to_string()),
                                Conn::Problem(_) => (p.danger, t("Offline").to_string()),
                            };
                            theme::dot(ui, color);
                            let r = ui.label(RichText::new(text).small().color(p.weak));
                            if let Conn::Problem(e) = &self.conn {
                                r.on_hover_text(e);
                            }
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let gear = ui.add(theme::Btn::new(theme::Kind::Ghost, RichText::new("⚙").size(18.0)).min_size(egui::vec2(34.0, 34.0)));
                        if gear.on_hover_text(t("Settings")).clicked() {
                            self.open_settings();
                        }
                        let new_chat = ui.add(
                            theme::Btn::new(theme::Kind::Secondary, format!("➕  {}", t("New chat")))
                                .rounding(999.0)
                                .min_size(egui::vec2(0.0, 32.0))
                                .enabled(!self.chat_is_empty()),
                        );
                        if new_chat.on_hover_text(t("Start a new chat (this one stays in the list)")).clicked() {
                            self.new_chat();
                        }
                        ui.add_space(4.0);
                        self.model_picker(ui, p);
                    });
                });
            });
    }

    /// The list of chats on the left, grouped by day.
    fn sidebar(&mut self, ctx: &egui::Context) {
        if !self.show_sidebar {
            return;
        }
        let p = theme::palette(ctx.style().visuals.dark_mode);
        egui::SidePanel::left("chats")
            .exact_width(260.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::none().fill(p.panel).stroke(Stroke::new(1.0_f32, p.border)).inner_margin(egui::Margin::symmetric(10.0, 12.0)))
            .show(ctx, |ui| {
                let w = ui.available_width();
                let new = ui.add(theme::Btn::new(theme::Kind::Secondary, format!("➕  {}", t("New chat"))).min_size(egui::vec2(w, 36.0)).enabled(!self.chat_is_empty()));
                if new.on_hover_text(t("Start a new chat (this one stays in the list)")).clicked() {
                    self.new_chat();
                }
                ui.add_space(6.0);

                let mut order: Vec<usize> = (0..self.convs.len()).collect();
                order.sort_by(|&a, &b| self.convs[b].updated.cmp(&self.convs[a].updated));
                let active_id = self.id.clone();
                let mut open = None;
                let mut delete = None;
                const FOOTER: f32 = 68.0;
                let list_h = (ui.available_height() - FOOTER).max(60.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut group = "";
                    for i in order {
                        let c = &self.convs[i];
                        // the open chat is listed even while empty; other empty chats are not
                        if c.is_empty() && c.id != active_id {
                            continue;
                        }
                        let g = conversations::day_group(c.updated);
                        if g != group {
                            group = g;
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                ui.label(RichText::new(t(g).to_uppercase()).size(10.5).family(theme::bold()).color(p.weak));
                            });
                            ui.add_space(2.0);
                        }
                        match self.chat_row(ui, i, c.id == active_id, p) {
                            RowAction::None => {}
                            RowAction::Open(id) => open = Some(id),
                            RowAction::Delete(id) => delete = Some(id),
                        }
                    }
                });
                if let Some(id) = delete {
                    self.delete_chat(&id);
                } else if let Some(id) = open {
                    self.open_chat(&id);
                }

                // the account card: who is logged in, the sync state and the credits;
                // a click opens Settings → Account
                ui.add_space(6.0);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, FOOTER - 6.0), egui::Sense::click());
                let hov = theme::hover_anim(ui, resp.id, resp.hovered());
                ui.painter().rect(rect, 12.0, theme::mix(p.surface, p.surface_hi, hov), Stroke::new(1.0_f32, p.border));
                let name_font = egui::FontId::new(13.0, theme::bold());
                let line_font = egui::FontId::proportional(11.5);
                if self.settings.sync_ready() {
                    let avatar = egui::Rect::from_center_size(egui::pos2(rect.left() + 26.0, rect.center().y), egui::vec2(30.0, 30.0));
                    theme::avatar(&mut ui.new_child(egui::UiBuilder::new().max_rect(avatar)), &self.settings.sync_user, 30.0);
                    let x = rect.left() + 50.0;
                    let room = rect.right() - 12.0 - x;
                    let (ok, status) = self.sync_status();
                    let credits = self.credits.clone().filter(|_| ok);
                    let blocked = credits.as_ref().is_some_and(|c| c.blocked);
                    // the credits matter more than the time of the last sync
                    let (line, color) = match self.credits_line().filter(|_| ok) {
                        Some(c) => (c, if blocked { p.danger } else { p.weak }),
                        None => (if ok { status } else { format!("⚠ {status}") }, if ok { p.weak } else { p.danger }),
                    };
                    let meter = credits.as_ref().filter(|c| !c.blocked).and_then(|c| c.limit.map(|l| (c.used, l)));
                    let top = if meter.is_some() { rect.top() + 9.0 } else { rect.center().y - 16.0 };
                    let name = fit_end(ui, &self.settings.sync_user, &name_font, room);
                    ui.painter().text(egui::pos2(x, top), egui::Align2::LEFT_TOP, name, name_font, p.text);
                    let line = fit_end(ui, &line, &line_font, room);
                    ui.painter().text(egui::pos2(x, top + 17.0), egui::Align2::LEFT_TOP, line, line_font, color);
                    if let Some((used, limit)) = meter {
                        let bar = egui::Rect::from_min_size(egui::pos2(x, top + 36.0), egui::vec2(room, 4.0));
                        credit_meter(ui.painter(), bar, used, limit, p);
                    }
                } else {
                    let text = fit_end(ui, t("Log in to sync your chats"), &line_font, w - 24.0);
                    let color = theme::mix(p.weak, p.text, hov);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.5), color);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t("Account")).clicked() {
                    self.open_settings();
                    self.settings_tab = 1;
                }
            });
    }

    /// One chat in the side list: title, a line from the last message, and – on hover –
    /// rename and delete.
    fn chat_row(&mut self, ui: &mut egui::Ui, i: usize, active: bool, p: theme::Palette) -> RowAction {
        let id = self.convs[i].id.clone();
        let w = ui.available_width();
        let row_id = egui::Id::new(("chat-row", &id));

        // renaming: a text field instead of the title
        if let Some((rid, text)) = &mut self.renaming {
            if *rid == id {
                let r = ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(w)
                        .margin(egui::Margin::symmetric(10.0, 12.0))
                        .hint_text(t("Name of the chat")),
                );
                r.request_focus();
                let done = r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter));
                let cancel = ui.input(|i| i.key_pressed(Key::Escape));
                if cancel {
                    self.renaming = None;
                } else if done {
                    let t = text.trim().to_string();
                    self.convs[i].custom_title = if t.is_empty() { None } else { Some(t) };
                    self.renaming = None;
                    self.save_chats();
                }
                ui.label(RichText::new(t("Enter to save · Esc to cancel")).size(10.5).color(p.weak));
                return RowAction::None;
            }
        }

        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 52.0), egui::Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let hov = theme::hover_anim(ui, row_id, hovered);
        let fill = if active { p.accent_soft } else { theme::mix(Color32::TRANSPARENT, p.surface_hi, hov) };
        ui.painter().rect_filled(rect, 10.0, fill);

        let c = &self.convs[i];
        let busy = c.busy;
        // room for the two icons while hovered
        let right_space = if hovered && !busy { 64.0 } else { 10.0 };
        let max_w = rect.width() - 24.0 - right_space;
        let title_font = egui::FontId::new(13.5, theme::bold());
        let title_color = if active { p.text } else { theme::mix(theme::mix(p.weak, p.text, 0.7), p.text, hov) };
        let title = fit_end(ui, &c.title(), &title_font, max_w);
        ui.painter().text(rect.left_top() + egui::vec2(12.0, 9.0), egui::Align2::LEFT_TOP, title, title_font, title_color);

        let (sub, sub_color) = if busy {
            (t("typing…").to_string(), p.success)
        } else if c.error.is_some() {
            (t("⚠ not sent – open to retry").to_string(), p.danger)
        } else {
            let preview = c.preview();
            (if preview.is_empty() { conversations::when(c.updated) } else { preview }, p.weak)
        };
        let sub_font = egui::FontId::proportional(11.5);
        let sub = fit_end(ui, &sub, &sub_font, max_w);
        ui.painter().text(rect.left_top() + egui::vec2(12.0, 29.0), egui::Align2::LEFT_TOP, sub, sub_font, sub_color);

        let mut action = RowAction::None;
        if hovered && !busy {
            // rename and delete, right-aligned on the row
            let btn = |ui: &mut egui::Ui, center: egui::Pos2, icon: &str, tip: &str, color: Color32| {
                let r = egui::Rect::from_center_size(center, egui::vec2(26.0, 26.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                child
                    .add(theme::Btn::new(theme::Kind::Ghost, RichText::new(icon).size(13.0).color(color)).min_size(egui::vec2(26.0, 26.0)).rounding(8.0))
                    .on_hover_text(tip)
                    .clicked()
            };
            let y = rect.center().y;
            if btn(ui, egui::pos2(rect.right() - 20.0, y), "🗑", t("Delete this chat (you can undo it)"), p.danger) {
                return RowAction::Delete(id);
            }
            if btn(ui, egui::pos2(rect.right() - 48.0, y), "✏", t("Rename"), p.weak) {
                self.renaming = Some((id.clone(), self.convs[i].title()));
                return RowAction::None;
            }
        }

        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            action = RowAction::Open(id.clone());
        }
        resp.context_menu(|ui| {
            if ui.button(format!("✏  {}", t("Rename"))).clicked() {
                self.renaming = Some((id.clone(), self.convs[i].title()));
                ui.close_menu();
            }
            if ui.add_enabled(!self.convs[i].busy, egui::Button::new(format!("🗑  {}", t("Delete")))).clicked() {
                action = RowAction::Delete(id.clone());
                ui.close_menu();
            }
        });
        if resp.double_clicked() {
            self.renaming = Some((id.clone(), self.convs[i].title()));
        }
        action
    }

    /// Save code from an answer to GitHub: block `only` of message `msg` in chat `conv`, or
    /// all its code blocks.
    fn push_code(&mut self, conv: &str, msg: &str, only: Option<usize>) {
        if self.guest() {
            if let Some(n) = only {
                self.push_states.insert(format!("{msg}#{n}"), PushState::Failed("GitHub is not available without a school login.".into()));
            }
            return;
        }
        if !self.settings.github_ready() {
            // (the check above already handled guests) a click on a block says why nothing happens; automatic saving just waits
            if let Some(n) = only {
                let why = if self.settings.github_token.trim().is_empty() {
                    "The GitHub token is missing. Enter it in Settings → GitHub."
                } else {
                    "GitHub is not set up. Check Settings → GitHub."
                };
                self.push_states.insert(format!("{msg}#{n}"), PushState::Failed(why.to_string()));
            }
            return;
        }
        let Some(ci) = self.convs.iter().position(|c| c.id == conv) else { return };
        let Some(m) = self.convs[ci].messages.iter().find(|m| m.id == msg) else { return };
        let blocks = markdown::code_blocks(&m.content);
        // automatically: only blocks the answer names a file for (not command snippets);
        // a click on one block saves that block either way
        let chosen: Vec<(usize, &markdown::CodeBlockInfo)> = blocks
            .iter()
            .enumerate()
            .filter(|(n, b)| match only {
                Some(o) => o == *n,
                None => github::named_file(b).is_some(),
            })
            .collect();
        if chosen.is_empty() {
            return;
        }
        let files = github::files_for(&chosen, &self.settings.github_folder());
        let keys: Vec<String> = chosen.iter().map(|(n, _)| format!("{msg}#{n}")).collect();
        let title = self.convs[ci].title();
        for k in &keys {
            self.push_states.insert(k.clone(), PushState::Pushing);
        }
        let cfg = self.settings.github_config();
        let s = self.settings.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = save_to_github(&s, &cfg, files, &title).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Pushed { keys, result });
            ctx.request_repaint();
        });
        self.save_chats();
    }

    fn open_repo_picker(&mut self) {
        self.repo_picker = Some(RepoPicker {
            loading: true,
            error: None,
            branch: String::new(),
            files: Vec::new(),
            selected: Default::default(),
            filter: String::new(),
            truncated: false,
        });
        let cfg = self.settings.github_config();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::RepoListed(github::list_files(&cfg).map_err(|e| e.to_string())));
            ctx.request_repaint();
        });
    }

    /// Attach the files chosen in the dialog to the open chat's next message (read in the
    /// background; they show as "loading" until then).
    fn attach_repo_files(&mut self) {
        let Some(picker) = self.repo_picker.take() else { return };
        let mut jobs = Vec::new();
        for f in picker.files.iter().filter(|f| picker.selected.contains(&f.path)) {
            let id = self.next_file_id;
            self.next_file_id += 1;
            self.pending.push(PendingFile { id, name: f.path.clone(), size: f.size, state: FileState::Uploading });
            jobs.push((id, f.path.clone()));
        }
        let cfg = self.settings.github_config();
        let branch = picker.branch.clone();
        let conv = self.id.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            for (id, path) in jobs {
                let result = github::read_file(&cfg, &branch, &path).map_err(|e| e.to_string());
                let _ = tx.send(Msg::RepoFileLoaded { conv: conv.clone(), id, result });
                ctx.request_repaint();
            }
        });
        self.focus_input = true;
    }

    /// The "Attach from GitHub" dialog.
    fn repo_picker_window(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.repo_picker else { return };
        let p = theme::palette(ctx.style().visuals.dark_mode);
        let repo = github::normalize_repo(&self.settings.github_repo).unwrap_or_default();
        let (mut close, mut attach) = (false, false);

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("repo-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(110));
            if r.clicked() {
                close = true;
            }
        });

        egui::Window::new("Attach from GitHub")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .default_width(560.0)
            .min_width(560.0)
            .max_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(t("Attach from GitHub")).family(theme::bold()).size(20.0).color(p.text));
                        let branch = if picker.branch.is_empty() { String::new() } else { format!(" · {}", picker.branch) };
                        ui.label(RichText::new(format!("{repo}{branch}")).small().color(p.weak));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if theme::ghost_button(ui, "✖").on_hover_text(t("Close")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(6.0);

                if picker.loading {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(RichText::new(t("Reading the file list…")).color(p.weak));
                    });
                    return;
                }
                if let Some(e) = &picker.error {
                    ui.add(egui::Label::new(RichText::new(format!("⚠ {e}")).color(p.danger)).wrap());
                    return;
                }
                ui.label(
                    RichText::new(t("The app reads the chosen files and attaches them to your message, so the bot can see the code."))
                        .small()
                        .color(p.weak),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut picker.filter).hint_text(format!("🔍 {}", t("Filter, e.g. .py or src/"))).desired_width(300.0));
                    if theme::secondary_button(ui, t("Select all code")).on_hover_text(t("All text and code files that fit")).clicked() {
                        let filter = picker.filter.to_lowercase();
                        let mut total = picker.selected_size();
                        for f in &picker.files {
                            if picker.selected.contains(&f.path) || not_attachable(f).is_some() || !f.path.to_lowercase().contains(&filter) {
                                continue;
                            }
                            if total + f.size <= REPO_BUDGET {
                                total += f.size;
                                picker.selected.insert(f.path.clone());
                            }
                        }
                    }
                    if theme::ghost_button(ui, t("Clear")).clicked() {
                        picker.selected.clear();
                    }
                });
                ui.add_space(4.0);

                let filter = picker.filter.to_lowercase();
                egui::Frame::none().fill(p.surface).stroke(Stroke::new(1.0_f32, p.border)).rounding(10.0).inner_margin(6.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height((screen.height() - 330.0).clamp(140.0, 380.0)).auto_shrink([false, true]).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let mut shown = 0;
                        for f in picker.files.iter().filter(|f| f.path.to_lowercase().contains(&filter)) {
                            shown += 1;
                            let mut on = picker.selected.contains(&f.path);
                            let reason = not_attachable(f);
                            ui.horizontal(|ui| {
                                // long paths are shortened so the size column stays in place
                                let font = egui::TextStyle::Body.resolve(ui.style());
                                let label = fit_text(ui, &f.path, &font, ui.available_width() - 110.0);
                                let r = ui.add_enabled(reason.is_none(), egui::Checkbox::new(&mut on, label));
                                match reason {
                                    Some(why) => {
                                        r.on_disabled_hover_text(format!("{}\n{why}", f.path));
                                    }
                                    None => {
                                        r.on_hover_text(&f.path);
                                    }
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(human_size(f.size)).small().color(p.weak));
                                });
                            });
                            if on {
                                picker.selected.insert(f.path.clone());
                            } else {
                                picker.selected.remove(&f.path);
                            }
                        }
                        if shown == 0 {
                            let text = if picker.files.is_empty() { t("The repository has no files yet.") } else { t("No file matches the filter.") };
                            ui.label(RichText::new(text).color(p.weak));
                        }
                    });
                });
                if picker.truncated {
                    ui.label(RichText::new(t("The repository is very large – GitHub listed only part of it.")).small().color(p.warning));
                }

                ui.add_space(6.0);
                let total = picker.selected_size();
                let too_big = total > REPO_BUDGET;
                ui.horizontal(|ui| {
                    let n = picker.selected.len();
                    let summary = tf("Files: {0} · {1} of {2}", &[&n.to_string(), &human_size(total), &human_size(REPO_BUDGET)]);
                    ui.label(RichText::new(summary).small().color(if too_big { p.danger } else { p.weak }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if n == 0 { t("Attach").to_string() } else { tf("Attach {0}", &[&n.to_string()]) };
                        let b = theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).enabled(n > 0 && !too_big);
                        if ui.add(b).on_disabled_hover_text(t("Choose files (at most 300 KB in total)")).clicked() {
                            attach = true;
                        }
                        if theme::secondary_button(ui, t("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            });

        if attach {
            self.attach_repo_files();
        } else if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.repo_picker = None;
            self.focus_input = true;
        }
    }

    fn check_github(&mut self) {
        self.github_checking = true;
        self.github_note = None;
        let cfg = self.settings.github_config();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::GithubChecked(github::check(&cfg).map_err(|e| e.to_string())));
            ctx.request_repaint();
        });
    }

    /// The optional GitHub section of the Settings window.
    fn github_settings(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        let mut enabled = self.settings.github_enabled;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(t("GitHub (optional)")).family(theme::bold()).color(p.text));
                ui.label(RichText::new(t("Let the bot read your repository and save code to it.")).small().color(p.weak));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = if enabled { t("Switch GitHub off") } else { t("Switch GitHub on") };
                theme::toggle(ui, &mut enabled).on_hover_text(tip);
            });
        });
        if enabled != self.settings.github_enabled {
            let mut s = self.settings.clone();
            s.github_enabled = enabled;
            self.draft.github_enabled = enabled;
            self.apply_settings(s);
        }
        if !enabled {
            return;
        }
        ui.add_space(4.0);
        egui::Grid::new("github").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            let w = 360.0;
            ui.label(t("Repository"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.github_repo).hint_text("owner/name").desired_width(w));
            ui.end_row();
            ui.label(t("Token"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.github_token).password(true).hint_text("github_pat_…").desired_width(w))
                .on_hover_text(match self.settings.token_status() {
                    crate::secret::Status::Stored => t("Kept in your system's password store (not in settings.json)."),
                    crate::secret::Status::Unavailable => "This system has no password store, so the token is kept in settings.json.",
                    crate::secret::Status::Unknown => t("Kept in your system's password store if there is one, otherwise in settings.json."),
                });
            ui.end_row();
            ui.label(t("Branch"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.github_branch).hint_text(t("default branch")).desired_width(w));
            ui.end_row();
            ui.label(t("Folder"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.github_folder).hint_text(t("top level")).desired_width(w))
                .on_hover_text(t("Empty: files go to their own paths in the repository (src/main.rs updates src/main.rs). Enter a folder to put everything inside it."));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Create a token:")).small().color(p.weak));
            ui.hyperlink_to(RichText::new("github.com → Fine-grained token").small(), "https://github.com/settings/personal-access-tokens/new")
                .on_hover_text(t("Repository access: only this repository · Permissions: Contents → Read and write"));
        });
        ui.add_space(2.0);
        let mut auto = self.settings.github_auto;
        ui.radio_value(&mut auto, false, t("Only when I click ⬆ GitHub on a code block"));
        ui.radio_value(&mut auto, true, t("Automatically: every file an answer contains (named code blocks)")).on_hover_text(
            t("Files that already exist are merged with the new code first, so a partial answer does not delete the rest of the file."),
        );
        if auto != self.settings.github_auto {
            let mut s = self.settings.clone();
            s.github_auto = auto;
            self.draft.github_auto = auto;
            self.apply_settings(s);
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let label = if self.github_checking { t("Checking…") } else { t("Save & check access") };
            let b = ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).enabled(!self.github_checking));
            if b.clicked() {
                let mut s = self.settings.clone();
                s.github_repo = github::normalize_repo(&self.draft.github_repo).unwrap_or_else(|| self.draft.github_repo.trim().to_string());
                s.github_token = self.draft.github_token.trim().to_string();
                s.github_branch = self.draft.github_branch.trim().to_string();
                s.github_folder = self.draft.github_folder.trim().to_string();
                self.draft.github_repo = s.github_repo.clone();
                self.apply_settings(s);
                self.check_github();
            }
            if self.github_checking {
                ui.add(egui::Spinner::new().size(16.0));
            }
        });
        if let Some((ok, note)) = &self.github_note {
            let c = if *ok { p.success } else { p.danger };
            ui.add(egui::Label::new(RichText::new(if *ok { format!("✔ {note}") } else { format!("⚠ {note}") }).color(c)).wrap());
        }
    }

    /// The box that asks for the school login before the chat can be used.
    /// The check itself is in `moodle_login.rs`.
    /// The start screen: log in with the OHG's Moodle or with moin.schule – or continue without
    /// an account (class bot only, see `guest`). The checks are in `moodle_login.rs` and
    /// `moin_login.rs`.
    fn school_login_screen(&mut self, ctx: &egui::Context) {
        let p = theme::palette(ctx.style().visuals.dark_mode);
        let mut submit = false;
        let mut guest = false;
        egui::CentralPanel::default().frame(egui::Frame::none().fill(p.bg)).show(ctx, |ui| {
            const PAD: f32 = 28.0;
            let width = (ui.available_width() - 2.0 * PAD - 24.0).min(400.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space((ui.available_height() - 720.0).max(16.0) / 2.0);
            ui.vertical_centered(|ui| {
              let card = egui::Frame::none()
                  .fill(p.panel)
                  .stroke(Stroke::new(1.0_f32, p.border))
                  .rounding(18.0)
                  .inner_margin(egui::Margin::same(PAD))
                  .shadow(egui::epaint::Shadow { offset: egui::vec2(0.0, 10.0), blur: 32.0, spread: 0.0, color: Color32::from_black_alpha(40) });
              card.show(ui, |ui| {
              ui.set_width(width);
              ui.vertical_centered(|ui| {
                theme::avatar(ui, &self.settings.display_name(), 56.0);
                ui.add_space(10.0);
                ui.label(RichText::new(t("Log in")).family(theme::bold()).size(20.0).color(p.text));
                ui.label(RichText::new(t("How do you want to log in?")).color(p.weak));
                ui.add_space(12.0);

                // the two school logins
                let enabled = !self.school_login_checking;
                for (method, title, sub) in [
                    ("moodle", t("OHG network"), t("Moodle of the OHG (moodle.ohg-goe.net)")),
                    ("moin", "moin.schule", t("The school login of Lower Saxony")),
                ] {
                    let chosen = self.login_choice == method;
                    let r = ui.add_enabled_ui(enabled, |ui| menu_row(ui, p, chosen, if method == "moodle" { "🎓" } else { "🔑" }, title, sub, &[])).inner;
                    if r.clicked() && !chosen {
                        self.login_choice = method;
                        self.school_login_error = None;
                        self.school_login_user = if method == "moin" { self.settings.moin_user.clone() } else { self.settings.moodle_user.clone() };
                    }
                }
                ui.add_space(10.0);

                ui.add_enabled(enabled, login_field(&mut self.school_login_user, t("User name"), false, width));
                ui.add_space(6.0);
                let pw = ui.add_enabled(enabled, login_field(&mut self.school_login_password, t("Password"), true, width));
                if pw.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                ui.add_space(10.0);
                let label = if self.school_login_checking { t("Checking…") } else { t("Log in") };
                let button = theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold()))
                    .min_size(egui::vec2(width, 40.0))
                    .enabled(enabled);
                if ui.add(button).clicked() {
                    submit = true;
                }
                if self.school_login_checking {
                    ui.add_space(6.0);
                    ui.add(egui::Spinner::new().size(18.0));
                }
                if let Some(error) = self.school_login_error {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new(format!("⚠ {}", t(error))).color(p.danger)).wrap());
                }

                // What happens with the password – said right where it is typed in.
                ui.add_space(14.0);
                let host = if self.login_choice == "moin" { "auth.moin.schule" } else { "moodle.ohg-goe.net" };
                egui::Frame::none().fill(p.surface_hi).rounding(10.0).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(tf("Your password is only sent to {0} to check it. It is not saved and not sent anywhere else.", &[host]))
                                .small()
                                .color(p.weak),
                        )
                        .wrap(),
                    );
                    ui.hyperlink_to(RichText::new(t("How this works")).small(), SCHOOL_LOGIN_EXPLAINED);
                });

                // the third way: no account
                ui.add_space(14.0);
                ui.label(RichText::new(t("or")).small().color(p.weak));
                ui.add_space(6.0);
                let without = theme::Btn::new(theme::Kind::Secondary, t("Continue without an account")).min_size(egui::vec2(width, 36.0)).enabled(enabled);
                if ui.add(without).clicked() {
                    guest = true;
                }
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(t("Only the class bot, and only with an account on a sync server that its owner has approved. No GitHub, no other bots or models, no advanced settings."))
                            .small()
                            .color(p.weak),
                    )
                    .wrap(),
                );
              });
              });
            });
            });
        });
        if guest {
            self.continue_without_account();
        } else if submit && !self.school_login_checking {
            self.start_school_login();
        }
    }

    /// The third way in: no school login. Only the class bot; chatting needs an approved
    /// account on a sync server (`chat_allowed`).
    fn continue_without_account(&mut self) {
        self.school_login_ok = true;
        self.login_method = "none";
        crate::sync::set_app_login("none");
        self.school_login_password.clear();
        self.school_login_error = None;
        let mut s = self.settings.clone();
        s.login_method = "guest".into();
        s.model.clear(); // the free models are not available
        if s != self.settings {
            self.draft.model.clear();
            self.apply_settings(s);
        }
    }

    /// Hand the user name and password to the check (in the background) and forget the
    /// password here right away.
    fn start_school_login(&mut self) {
        let username = self.school_login_user.trim().to_string();
        let password = std::mem::take(&mut self.school_login_password); // the box is empty again
        if username.is_empty() || password.is_empty() {
            self.school_login_error = Some("Wrong user name or password.");
            return;
        }
        let method = self.login_choice;
        self.school_login_checking = true;
        self.school_login_error = None;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = if method == "moin" {
                crate::moin_login::check_login(&username, &password)
            } else {
                crate::moodle_login::check_login(&username, &password)
            };
            // For the sync server: the school user name (moin.schule accounts get "moin-" in
            // front, so they cannot clash with Moodle names) and a code made from the password,
            // not the password itself.
            let sync_user = if method == "moin" { format!("moin-{}", username.to_lowercase()) } else { username.to_lowercase() };
            let accepted = result == crate::moodle_login::LoginResult::Accepted;
            let sync_code = accepted.then(|| crate::moodle_login::sync_code(&sync_user, &password));
            drop(password); // not needed any more
            let _ = tx.send(Msg::SchoolLoginChecked { method, username, result, sync_user, sync_code });
            ctx.request_repaint();
        });
    }

    /// Settings → Account: log in to the sync server, or see the sync state.
    fn account_settings(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        ui.label(RichText::new(t("Account")).family(theme::bold()).color(p.text));
        if self.settings.sync_ready() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                theme::avatar(ui, &self.settings.sync_user, 32.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(tf("Logged in as {0}", &[&self.settings.sync_user])).family(theme::bold()).color(p.text));
                    ui.label(RichText::new(&self.settings.sync_server).small().color(p.weak));
                });
            });
            ui.add_space(4.0);
            let (ok, status) = self.sync_status();
            let shown = if !ok { format!("⚠ {status}") } else if self.sync_in_flight.is_some() || self.sync_pending { status } else { format!("✔ {status}") };
            ui.add(egui::Label::new(RichText::new(shown).color(if ok { p.weak } else { p.danger })).wrap());
            if let Some(line) = self.credits_line() {
                let refill = self.credits.as_ref().and_then(|c| c.resets_at).map(|at| tf("New credits: {0}", &[&conversations::when_exact(at)]));
                let text = match refill {
                    Some(r) => format!("{line} · {r}"),
                    None => line,
                };
                ui.add(egui::Label::new(RichText::new(text).color(p.weak)).wrap()).on_hover_text(t("One credit = one message to the bot."));
                if let Some((used, limit)) = self.credits.as_ref().filter(|c| !c.blocked).and_then(|c| c.limit.map(|l| (c.used, l))) {
                    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(320.0), 6.0), egui::Sense::hover());
                    credit_meter(ui.painter(), bar, used, limit, p);
                    ui.add_space(4.0);
                }
            }
            ui.horizontal(|ui| {
                let syncing = self.sync_in_flight.is_some();
                if ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(t("Sync now")).family(theme::bold())).enabled(!syncing)).clicked() {
                    self.sync_note = None;
                    self.next_sync.set(0.0);
                }
                if theme::secondary_button(ui, t("Log out")).clicked() {
                    self.logout();
                }
                if self.sync_dashboard && theme::secondary_button(ui, t("Open the dashboard")).on_hover_text(t("You may manage this sync server.")).clicked() {
                    self.open_dashboard();
                }
                if syncing {
                    ui.add(egui::Spinner::new().size(16.0));
                }
            });
            ui.label(RichText::new(t("Your chats stay on this computer when you log out.")).small().color(p.weak));
            return;
        }

        ui.label(RichText::new(t("Log in to keep your chats on a server and continue them on any computer.")).small().color(p.weak));
        ui.add_space(4.0);
        // the school account (from the start screen) works without typing anything
        if self.school_sync_code.is_some() && !self.school_sync_user.is_empty() {
            let label = tf("Log in as {0} (school account)", &[&self.school_sync_user]);
            let button = theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).enabled(!self.logging_in);
            if ui.add(button).clicked() {
                match crate::sync::normalize_server(&self.draft.sync_server) {
                    Ok(server) => self.start_school_sync_login(server),
                    Err(e) => self.sync_note = Some((false, e.to_string())),
                }
            }
            ui.label(RichText::new(t("Uses your school login. Your password is not sent to the sync server.")).small().color(p.weak));
            ui.add_space(6.0);
            ui.label(RichText::new(t("Or with a separate account:")).small().color(p.weak));
        }
        let mut submit = false;
        egui::Grid::new("account").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(t("Server address"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.sync_server).hint_text("192.168.1.20:8787").desired_width(300.0))
                .on_hover_text(t("The address the sync server prints when it starts."));
            ui.end_row();
            ui.label(t("User name"));
            ui.add(egui::TextEdit::singleline(&mut self.draft.sync_user).desired_width(300.0));
            ui.end_row();
            ui.label(t("Password"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.account_password).password(true).desired_width(300.0));
            submit = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.end_row();
        });
        if let Ok(server) = crate::sync::normalize_server(&self.draft.sync_server) {
            if crate::sync::is_insecure(&server) {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("⚠ {}", t("Without https the password is sent unencrypted – only use this in a network you trust.")))
                            .small()
                            .color(p.warning),
                    )
                    .wrap(),
                );
            }
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let label = if self.logging_in { t("Logging in…") } else { t("Log in") };
            let login = ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).enabled(!self.logging_in));
            if (login.clicked() || submit) && !self.logging_in {
                self.start_login(false);
            }
            if ui.add(theme::Btn::new(theme::Kind::Secondary, t("Create account")).enabled(!self.logging_in)).clicked() {
                self.start_login(true);
            }
            if self.logging_in {
                ui.add(egui::Spinner::new().size(16.0));
            }
        });
        if let Some((ok, note)) = &self.sync_note {
            let c = if *ok { p.success } else { p.danger };
            ui.add(egui::Label::new(RichText::new(if *ok { format!("✔ {note}") } else { format!("⚠ {note}") }).color(c)).wrap());
        }
    }

    fn open_settings(&mut self) {
        self.draft = self.settings.clone();
        self.link_input = self.settings.invite_link.clone();
        self.settings_note = None;
        self.show_settings = true;
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        let p = theme::palette(ctx.style().visuals.dark_mode);
        egui::TopBottomPanel::bottom("input")
            .frame(egui::Frame::none().fill(p.bg).inner_margin(egui::Margin { left: 16.0, right: 16.0, top: 22.0, bottom: 10.0 }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                centered_column(ui, |ui| self.composer(ui, p));
            });
    }

    fn composer(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        if !self.chat_allowed() {
            let text = if self.sync_pending {
                t("Your account is waiting for approval by the person who runs the sync server.")
            } else {
                t("Without a school login, you need an approved account on a sync server to chat.")
            };
            notice(ui, p.accent, |ui| {
                ui.add(egui::Label::new(RichText::new(text).color(p.text)).wrap());
                if !self.sync_pending && theme::secondary_button(ui, t("Log in to a sync server")).clicked() {
                    self.open_settings();
                    self.settings_tab = 1;
                }
            });
        }
        if let (Conn::Problem(e), false) = (&self.conn, self.settings.uses_ai_models()) {
            let e = e.clone();
            notice(ui, p.danger, |ui| {
                ui.label(RichText::new(format!("{} {e}", t("Not connected."))).color(p.text));
                if theme::secondary_button(ui, t("Try again")).clicked() {
                    self.start_refresh(false);
                }
                if theme::secondary_button(ui, t("Settings")).clicked() {
                    self.open_settings();
                }
            });
        }
        if let Some(e) = self.error.clone() {
            notice(ui, p.danger, |ui| {
                ui.label(RichText::new(e).color(p.text));
                if theme::secondary_button(ui, &format!("↻ {}", t("Retry"))).clicked() {
                    self.retry();
                }
                if theme::ghost_button(ui, "✖").on_hover_text(t("Dismiss")).clicked() {
                    self.error = None;
                }
            });
        }

        let input_id = egui::Id::new("chat-input");
        let focused = ui.memory(|m| m.has_focus(input_id));
        let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
        let mut send_now = false;
        if focused && enter {
            ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            send_now = true;
        }

        let composer_id = egui::Id::new("composer");
        let hover = theme::remembered_hover(ui, composer_id);
        let border = if focused { p.accent } else { theme::mix(p.border, p.accent, hover * 0.6) };
        let composer = egui::Frame::none()
            .fill(p.surface)
            .rounding(20.0)
            .stroke(Stroke::new(if focused { 1.5_f32 } else { 1.0_f32 }, border))
            .shadow(egui::epaint::Shadow {
                offset: egui::vec2(0.0, 4.0),
                blur: 14.0,
                spread: 0.0,
                color: Color32::from_black_alpha(if ui.visuals().dark_mode { 70 } else { 18 }),
            })
            .inner_margin(egui::Margin { left: 8.0, right: 8.0, top: 8.0, bottom: 8.0 })
            .show(ui, |ui| {
                // the part of an answer this message replies to
                if let Some(q) = self.quote.clone() {
                    let mut cancel = false;
                    egui::Frame::none()
                        .fill(p.accent_soft)
                        .rounding(12.0)
                        .inner_margin(egui::Margin { left: 16.0, right: 6.0, top: 6.0, bottom: 6.0 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let r = ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(ui.available_width() - 34.0);
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(RichText::new(format!("↩ {}", t("Replying to"))).small().family(theme::bold()).color(p.accent_hover));
                                    ui.add(egui::Label::new(RichText::new(short_quote(&q, 220)).small().color(p.weak)).wrap());
                                });
                                let x = theme::Btn::new(theme::Kind::Ghost, RichText::new("✖").small()).min_size(egui::vec2(26.0, 26.0)).rounding(999.0);
                                if ui.add(x).on_hover_text(t("Don't reply to this (Esc)")).clicked() {
                                    cancel = true;
                                }
                            });
                            let bar = egui::Rect::from_min_max(
                                r.response.rect.left_top() - egui::vec2(9.0, 0.0),
                                r.response.rect.left_bottom() - egui::vec2(6.0, 0.0),
                            );
                            ui.painter().rect_filled(bar, 2.0, p.accent);
                        });
                    if cancel || (focused && ui.input(|i| i.key_pressed(Key::Escape))) {
                        self.quote = None;
                    }
                    ui.add_space(2.0);
                }

                // the bot cannot open GitHub itself: offer the repository's files instead
                let mentions_repo = {
                    let t = self.input.to_lowercase();
                    t.contains("github") || t.contains("repo")
                };
                if mentions_repo && self.pending.is_empty() && self.github_on() && self.repo_picker.is_none() {
                    let repo = github::normalize_repo(&self.settings.github_repo).unwrap_or_default();
                    let mut open = false;
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("💡 {}", t("The bot cannot open GitHub by itself –"))).small().color(p.weak));
                        let b = theme::Btn::new(theme::Kind::Secondary, RichText::new(tf("attach files from {0}", &[&repo])).small())
                            .min_size(egui::vec2(0.0, 26.0))
                            .rounding(999.0);
                        open = ui.add(b).clicked();
                    });
                    if open {
                        self.open_repo_picker();
                    }
                }

                // attachments waiting for the next message
                if !self.pending.is_empty() {
                    let mut remove = None;
                    // icon/spinner + name + ✖ + margins and gaps around the name
                    const EXTRA: f32 = 80.0;
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let row = ui.available_width() - 8.0;
                    let names: Vec<String> = self.pending.iter().map(|f| fit_text(ui, &f.name, &font, row - EXTRA)).collect();
                    let widths: Vec<f32> = names.iter().map(|n| text_width(ui, n, &font) + EXTRA).collect();
                    for line in chip_lines(&widths, row, 8.0) {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            for i in line {
                                let f = &self.pending[i];
                                let (icon, note, color) = match &f.state {
                                    FileState::Uploading => ("", t("loading…").to_string(), p.weak),
                                    FileState::Ready(AttachmentData::Inline(_)) => ("📄", t("added as text").to_string(), p.text),
                                    FileState::Ready(AttachmentData::Uploaded(_)) => ("📎", t("uploaded").to_string(), p.text),
                                    FileState::Failed(e) => ("⚠", e.clone(), p.danger),
                                };
                                theme::chip_frame(ui).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        if matches!(f.state, FileState::Uploading) {
                                            ui.add(egui::Spinner::new().size(12.0));
                                        } else {
                                            ui.label(RichText::new(icon).color(color));
                                        }
                                        ui.add(egui::Label::new(RichText::new(&names[i]).color(color)).extend())
                                            .on_hover_text(format!("{} · {} – {note}", f.name, human_size(f.size)));
                                        let x = theme::Btn::new(theme::Kind::Ghost, RichText::new("✖").small()).min_size(egui::vec2(20.0, 20.0)).rounding(999.0);
                                        if ui.add(x).on_hover_text(t("Remove")).clicked() {
                                            remove = Some(f.id);
                                        }
                                    });
                                });
                            }
                        });
                    }
                    if let Some(id) = remove {
                        self.pending.retain(|f| f.id != id);
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(Layout::left_to_right(Align::Max), |ui| {
                    let attach = theme::round_button(ui, theme::RoundIcon::Attach, true, 36.0);
                    if self.github_on() {
                        // two sources: this computer or the GitHub repository
                        let menu = ui.make_persistent_id("attach-menu");
                        let attach = attach.on_hover_text(t("Attach files from this computer or from GitHub"));
                        if attach.clicked() {
                            ui.memory_mut(|m| m.toggle_popup(menu));
                        }
                        let repo = github::normalize_repo(&self.settings.github_repo).unwrap_or_default();
                        let (mut local, mut remote) = (false, false);
                        egui::popup::popup_above_or_below_widget(
                            ui,
                            menu,
                            &attach,
                            egui::AboveOrBelow::Above,
                            egui::PopupCloseBehavior::CloseOnClick,
                            |ui| {
                                ui.set_min_width(300.0);
                                let item = |text: String| {
                                    egui::Button::new(text).wrap_mode(egui::TextWrapMode::Extend).min_size(egui::vec2(300.0, 34.0))
                                };
                                local = ui.add(item(format!("📄  {}", t("Files from this computer")))).clicked();
                                remote = ui.add(item(format!("⬇  {}", tf("From GitHub · {0}", &[&repo])))).clicked();
                            },
                        );
                        if local {
                            self.pick_files();
                        }
                        if remote {
                            self.open_repo_picker();
                        }
                    } else if attach.on_hover_text(t("Attach files (or drag them onto the window)")).clicked() {
                        self.pick_files();
                    }
                    let edit = egui::TextEdit::multiline(&mut self.input)
                        .id(input_id)
                        .frame(false)
                        .hint_text(RichText::new(t("Message…")).color(p.weak))
                        .desired_rows(1)
                        .margin(egui::Margin::symmetric(4.0, 9.0))
                        .desired_width(ui.available_width() - 46.0)
                        .lock_focus(true);
                    let resp = ui.add(edit);
                    if self.focus_input {
                        resp.request_focus();
                        self.focus_input = false;
                    }
                    if self.busy {
                        if theme::round_button(ui, theme::RoundIcon::Stop, true, 36.0).on_hover_text(t("Stop the answer")).clicked() {
                            self.stop();
                        }
                    } else {
                        let uploading = self.pending.iter().any(|f| matches!(f.state, FileState::Uploading));
                        let b = theme::round_button(ui, theme::RoundIcon::Send, self.can_send(), 36.0);
                        let b = if uploading { b.on_hover_text(t("Waiting for the upload to finish…")) } else { b.on_hover_text(t("Send (Enter)")) };
                        if b.clicked() {
                            send_now = true;
                        }
                    }
                });
            });
        theme::remember_hover(ui, composer_id, composer.response.hovered() || ui.rect_contains_pointer(composer.response.rect));
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(t("Enter to send · Shift+Enter for a new line · select text in an answer to reply to it")).small().color(p.weak));
        });
        if send_now {
            self.send();
        }
    }

    fn chat(&mut self, ctx: &egui::Context) {
        let p = theme::palette(ctx.style().visuals.dark_mode);
        // each chat keeps its own scroll position; a chat opens at its end, the start screen of
        // an empty chat at the top
        let (scroll_id, stick) = (self.id.clone(), !self.chat_is_empty());
        egui::CentralPanel::default().frame(egui::Frame::none().fill(p.bg)).show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt(("chat", scroll_id)).stick_to_bottom(stick).auto_shrink([false, false]).show(ui, |ui| {
                centered_column(ui, |ui| {
                    ui.add_space(18.0);
                    if self.chat_is_empty() {
                        self.welcome(ui, p);
                    }
                    let name = self.settings.display_name();
                    let last = self.messages.len().saturating_sub(1);
                    let github = self.github_on();
                    let mut push = None;
                    for (i, m) in self.messages.iter().enumerate() {
                        let waiting = self.busy && i == last && m.role == Role::Assistant && m.content.is_empty();
                        let streaming = self.busy && i == last;
                        let activity = if streaming { self.activity.as_str() } else { "" };
                        // GitHub buttons once the answer is complete
                        let states = (github && !streaming && !m.initial).then_some(&self.push_states);
                        if let Some(n) = message_row(ui, m, &name, waiting, activity, p, states) {
                            push = Some((m.id.clone(), n));
                        }
                        ui.add_space(14.0);
                    }
                    if let Some((msg, n)) = push {
                        let conv = self.id.clone();
                        self.push_code(&conv, &msg, Some(n));
                    }
                    ui.add_space(8.0);
                });
            });
        });
    }

    /// Shown while the chat is still empty: who the bot is and a few ideas to start with.
    fn welcome(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        // with earlier chats to show, the header is smaller so everything fits
        let has_history = self.convs.iter().any(|c| !c.is_empty() && c.id != self.id);
        ui.add_space(if has_history { 8.0 } else { 24.0 });
        ui.vertical_centered(|ui| {
            theme::avatar(ui, &self.settings.display_name(), if has_history { 44.0 } else { 64.0 });
            ui.add_space(6.0);
            ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).size(if has_history { 19.0 } else { 22.0 }).color(p.text));
            ui.label(RichText::new(t("Ask a question, paste code, or attach files with + or by dragging them in.")).color(p.weak));
            ui.add_space(10.0);
        });

        // earlier chats: continue the last one, or pick one of the recent ones
        let mut recent: Vec<&Conversation> = self.convs.iter().filter(|c| !c.is_empty() && c.id != self.id).collect();
        recent.sort_by(|a, b| b.updated.cmp(&a.updated));
        let mut open = None;
        if let Some(last) = recent.first() {
            let w = ui.available_width();
            let label = format!("↩  {}  {}", t("Continue where you left off:"), last.title());
            let label = fit_text(ui, &label, &egui::FontId::new(14.5, theme::bold()), w - 40.0);
            let b = theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).min_size(egui::vec2(w, 44.0)).rounding(14.0);
            if ui.add(b).on_hover_text(tf("Open your last chat ({0})", &[&conversations::when(last.updated)])).clicked() {
                open = Some(last.id.clone());
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("RECENT CHATS")).size(11.0).family(theme::bold()).color(p.weak));
                if !self.show_sidebar && recent.len() > 3 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(theme::Btn::new(theme::Kind::Ghost, RichText::new(tf("Show all {0} chats", &[&recent.len().to_string()])).small())).clicked() {
                            self.show_sidebar = true;
                        }
                    });
                }
            });
            for c in recent.iter().take(3) {
                let when = conversations::when(c.updated);
                let preview = c.preview();
                let sub = if preview.is_empty() { "…".to_string() } else { preview };
                if menu_row(ui, p, false, "💬", &c.title(), &sub, &[when.as_str()]).on_hover_text(t("Open this chat")).clicked() {
                    open = Some(c.id.clone());
                }
            }
            ui.add_space(14.0);
            ui.label(RichText::new(t("OR START WITH AN IDEA")).size(11.0).family(theme::bold()).color(p.weak));
            ui.add_space(2.0);
        }
        if let Some(id) = open {
            self.open_chat(&id);
            return;
        }
        let ideas = crate::i18n::suggestions(&self.settings.ui_language());
        let mut picked = None;
        let cols = if ui.available_width() > 520.0 { 2 } else { 1 };
        for row in ideas.chunks(cols) {
            ui.columns(cols, |columns| {
                for (col, idea) in columns.iter_mut().zip(row) {
                    let w = col.available_width();
                    let b = theme::Btn::new(theme::Kind::Card, *idea).min_size(egui::vec2(w, 46.0));
                    if col.add(b).on_hover_text(t("Put this in the message box")).clicked() {
                        picked = Some(*idea);
                    }
                }
            });
        }
        if let Some(idea) = picked {
            self.input = format!("{idea}: ");
            self.focus_input = true;
        }
        ui.add_space(28.0);
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let p = theme::palette(ctx.style().visuals.dark_mode);
        let mut close = false;

        // dim the chat behind the dialog
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("settings-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(110));
            if r.clicked() {
                close = true;
            }
        });

        egui::Window::new("Settings")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .default_width(520.0)
            .min_width(520.0)
            .max_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Settings")).family(theme::bold()).size(20.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::ghost_button(ui, "✖").on_hover_text(t("Close")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // without a school login: only General and Account
                    let tabs = [t("General"), t("Account"), t("Free AI models"), t("GitHub"), t("Advanced")];
                    let shown = if self.guest() { 2 } else { tabs.len() };
                    if self.settings_tab >= shown {
                        self.settings_tab = 0;
                    }
                    for (i, label) in tabs.into_iter().take(shown).enumerate() {
                        let on = self.settings_tab == i;
                        let kind = if on { theme::Kind::Secondary } else { theme::Kind::Ghost };
                        let text = RichText::new(label).color(if on { p.text } else { p.weak });
                        let text = if on { text.family(theme::bold()) } else { text };
                        if ui.add(theme::Btn::new(kind, text).min_size(egui::vec2(0.0, 32.0)).rounding(999.0)).clicked() {
                            self.settings_tab = i;
                        }
                    }
                });
                ui.add_space(6.0);
                let tab = self.settings_tab;
                // the contents scroll when the window is too small for them
                let max_h = (ctx.screen_rect().height() - 230.0).clamp(200.0, 460.0);
                egui::ScrollArea::vertical().id_salt(("settings", tab)).max_height(max_h).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    // same height on every tab, so the tabs do not jump around
                    ui.set_min_height(max_h);
                    if tab == 0 && self.guest() {
                        notice(ui, p.accent, |ui| {
                            ui.add(egui::Label::new(RichText::new(t("Without a school login, other bots, free AI models, GitHub and the advanced settings are not available.")).color(p.text)).wrap());
                        });
                        ui.add_space(8.0);
                    }
                    if tab == 0 {
                    // the invite link (which bot) can only be changed with a school login
                    let guest = self.guest();
                    if !guest {
                    section(ui, p, |ui| {
                        ui.label(RichText::new(t("Invite link")).family(theme::bold()).color(p.text));
                        ui.label(RichText::new(t("Paste the link of the shared chat. Everything else is set up automatically.")).small().color(p.weak));
                        ui.add_space(2.0);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.link_input)
                                .hint_text("https://app.ais-chat.schule/ua/characters/…/dialog?inviteCode=…")
                                .margin(egui::Margin::symmetric(10.0, 8.0))
                                .desired_width(f32::INFINITY),
                        );
                        let pressed_enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let label = if self.connecting_from_settings { t("Connecting…") } else { t("Connect") };
                            let connect = ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(label).family(theme::bold())).enabled(!self.connecting_from_settings));
                            if (connect.clicked() || pressed_enter) && !self.connecting_from_settings {
                                self.connect_from_settings();
                            }
                            if theme::secondary_button(ui, t("Use class link")).on_hover_text(DEFAULT_INVITE_LINK).clicked() {
                                self.link_input = DEFAULT_INVITE_LINK.to_string();
                                self.connect_from_settings();
                            }
                            if self.connecting_from_settings {
                                ui.add(egui::Spinner::new().size(16.0));
                            }
                        });
                        if let Some((ok, note)) = &self.settings_note {
                            let c = if *ok { p.success } else { p.danger };
                            ui.label(RichText::new(if *ok { format!("✔ {note}") } else { format!("⚠ {note}") }).color(c));
                        }
                    });
                    }

                    if self.settings.is_ready() {
                        ui.add_space(8.0);
                        section(ui, p, |ui| {
                            ui.horizontal(|ui| {
                                theme::avatar(ui, &self.settings.display_name(), 32.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).color(p.text));
                                    let upload = if self.settings.upload_url.trim().is_empty() {
                                        t("Connected · file upload automatic")
                                    } else {
                                        t("Connected · file upload custom address")
                                    };
                                    ui.label(RichText::new(upload).small().color(p.weak))
                                        .on_hover_text(self.settings.upload_endpoint());
                                });
                            });
                        });
                    }

                    ui.add_space(8.0);
                    section(ui, p, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t("Appearance")).family(theme::bold()).color(p.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let current = self.settings.appearance.clone();
                                let mut choice = if current.is_empty() { "system".to_string() } else { current.clone() };
                                for (value, label) in [("dark", t("Dark")), ("light", t("Light")), ("system", t("System"))] {
                                    ui.selectable_value(&mut choice, value.to_string(), label);
                                }
                                if choice != current && !(current.is_empty() && choice == "system") {
                                    theme::set_appearance(ui.ctx(), &choice);
                                    let mut s = self.settings.clone();
                                    s.appearance = choice.clone();
                                    self.draft.appearance = choice;
                                    self.apply_settings(s);
                                }
                            });
                        });
                    });

                    ui.add_space(8.0);
                    section(ui, p, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(t("Language")).family(theme::bold()).color(p.text));
                                ui.label(RichText::new(t("Menus, chat names and the bot's answers.")).small().color(p.weak));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let current = self.settings.language.clone();
                                let shown = crate::settings::language(&current).map(|(n, _)| n).unwrap_or(t("Automatic"));
                                let mut choice = current.clone();
                                egui::ComboBox::from_id_salt("answer-language").selected_text(shown).width(170.0).height(480.0).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut choice, String::new(), t("Automatic"))
                                        .on_hover_text(t("Menus in your system's language; the bot answers in the language you write in"));
                                    for (code, native, english) in crate::settings::LANGUAGES {
                                        ui.selectable_value(&mut choice, code.to_string(), *native).on_hover_text(*english);
                                    }
                                });
                                if choice != current {
                                    let mut s = self.settings.clone();
                                    s.language = choice.clone();
                                    self.draft.language = choice;
                                    self.apply_settings(s);
                                }
                            });
                        });
                    });

                    }
                    if tab == 1 {
                        section(ui, p, |ui| self.account_settings(ui, p));
                    }
                    if tab == 2 {
                        section(ui, p, |ui| self.models_settings(ui, p));
                    }
                    if tab == 3 {
                        section(ui, p, |ui| self.github_settings(ui, p));
                    }
                    if tab == 4 {
                    ui.label(RichText::new(t("Only needed if something does not work – Connect normally fills these in.")).small().color(p.weak));
                    ui.add_space(8.0);
                    section(ui, p, |ui| {
                        egui::Grid::new("adv").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            let auto_upload = {
                                let mut d = self.draft.clone();
                                d.upload_url.clear();
                                d.upload_endpoint()
                            };
                            ui.label(t("Upload address"));
                            ui.add(egui::TextEdit::singleline(&mut self.draft.upload_url).hint_text(tf("automatic: {0}", &[&auto_upload])).desired_width(330.0))
                                .on_hover_text(t("Leave empty: files go to the address that belongs to the invite link."));
                            ui.end_row();
                            ui.label("next-action");
                            ui.add(egui::TextEdit::singleline(&mut self.draft.next_action).desired_width(330.0));
                            ui.end_row();
                            ui.label(t("Deployment id"));
                            ui.add(egui::TextEdit::singleline(&mut self.draft.deployment_id).desired_width(330.0));
                            ui.end_row();
                            ui.label("GitHub API");
                            ui.add(egui::TextEdit::singleline(&mut self.draft.github_api).hint_text("https://api.github.com").desired_width(330.0))
                                .on_hover_text(t("Only for GitHub Enterprise: https://your-host/api/v3"));
                            ui.end_row();
                            ui.label(t("User agent"));
                            ui.add(egui::TextEdit::singleline(&mut self.draft.user_agent).hint_text(t("built in")).desired_width(330.0));
                            ui.end_row();
                        });
                        ui.horizontal(|ui| {
                            if theme::secondary_button(ui, t("Save these values")).clicked() {
                                let s = self.draft.clone();
                                self.apply_settings(s);
                                self.settings_note = Some((true, t("Saved.").into()));
                            }
                            if theme::secondary_button(ui, t("Detect again")).on_hover_text(t("Read the current values from the AIS.chat page")).clicked() {
                                self.draft.next_action.clear();
                                self.start_refresh(true);
                            }
                        });

                        ui.add_space(6.0);
                        ui.label(RichText::new(t("Import from a cURL")).family(theme::bold()).color(p.text));
                        ui.label(
                            RichText::new(t(
                                "Only if Connect fails: in the browser press F12 → Network, send a message, right-click the dialog?inviteCode=… request → Copy → Copy as cURL, and paste it here.",
                            ))
                            .small()
                            .color(p.weak),
                        );
                        ui.add(egui::TextEdit::multiline(&mut self.curl_input).desired_rows(3).desired_width(f32::INFINITY));
                        if theme::secondary_button(ui, t("Import")).clicked() {
                            let mut s = self.draft.clone();
                            match s.import_curl(&self.curl_input) {
                                Ok(summary) => {
                                    self.curl_input.clear();
                                    self.link_input = s.invite_link.clone();
                                    self.draft = s.clone();
                                    self.apply_settings(s);
                                    self.conn = Conn::Ready;
                                    self.settings_note = Some((true, summary));
                                }
                                Err(e) => self.settings_note = Some((false, e.to_string())),
                            }
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(tf("Settings file: {0}", &[&Settings::path_display()])).small().color(p.weak));
                    });
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::primary_button(ui, t("Done")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.show_settings = false;
            self.focus_input = true;
        }
    }

    fn connect_from_settings(&mut self) {
        let text = self.link_input.trim().to_string();
        let mut s = self.draft.clone();
        // People sometimes paste a whole cURL into the link box: accept that too.
        let applied = if text.starts_with("curl") { s.import_curl(&text).map(|_| ()) } else { s.apply_invite_link(&text) };
        match applied {
            Ok(()) => {
                self.draft = s;
                self.settings_note = None;
                self.start_refresh(true);
            }
            Err(e) => self.settings_note = Some((false, e.to_string())),
        }
    }

    /// Which model answers: the AIS.chat class bot, or a free model of the AI provider.
    fn model_picker(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        let current = self.settings.model.clone();
        let label = if current.is_empty() { format!("🎓 {}", t("Class bot")) } else { format!("⚡ {}", crate::models::pretty_name(&current)) };
        let provider = self.settings.ai_provider();
        let has_key = !self.settings.ai_key().trim().is_empty();
        let mut choice = current.clone();
        let mut open_settings = false;
        // a button with its own popup (egui's ComboBox kept an older, smaller size and cut
        // models off)
        let text = fit_text(ui, &label, &egui::TextStyle::Button.resolve(ui.style()), 180.0);
        let button = ui.add(theme::Btn::new(theme::Kind::Secondary, format!("{text}    ")).min_size(egui::vec2(220.0, 32.0)).rounding(999.0));
        // a small arrow at the right end (drawn: the font has no arrow glyph)
        let c = egui::pos2(button.rect.right() - 18.0, button.rect.center().y);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![c + egui::vec2(-4.5, -2.0), c + egui::vec2(4.5, -2.0), c + egui::vec2(0.0, 3.0)],
            p.weak,
            Stroke::NONE,
        ));
        let menu = ui.make_persistent_id("model-menu");
        if button.clicked() {
            ui.memory_mut(|m| m.toggle_popup(menu));
            self.model_filter.clear();
            self.scroll_to_model = true;
        }
        let mut close = false;
        // closes on a click outside (or after choosing) – not when typing in the search field
        egui::popup::popup_below_widget(ui, menu, &button, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_width(360.0);
            ui.spacing_mut().item_spacing.y = 2.0;

            menu_heading(ui, p, "AIS.chat", None);
            let class = tf("Class bot · {0}", &[&self.settings.display_name()]);
            if menu_row(ui, p, choice.is_empty(), "🎓", &class, t("Also reads PDFs, images and Office files"), &[]).clicked() {
                choice.clear();
                close = true;
            }

            if self.guest() {
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(t("Without a school login, other bots, free AI models, GitHub and the advanced settings are not available.")).small().color(p.weak)).wrap());
                return;
            }
            ui.add_space(6.0);
            menu_heading(ui, p, &tf("Free via {0}", &[provider.name]), self.models_loading.then_some(()));
            if !has_key {
                ui.add(egui::Label::new(RichText::new(tf("{0} needs a free API key.", &[provider.name])).small().color(p.warning)).wrap());
            } else if self.models.is_empty() {
                ui.label(RichText::new(t("No models yet – check the key in Settings.")).small().color(p.weak));
            } else {
                if self.models.len() > 6 {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.model_filter)
                            .hint_text(format!("🔍 {}", t("Search models")))
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8.0, 6.0)),
                    );
                    ui.add_space(2.0);
                }
                let filter = self.model_filter.trim().to_lowercase();
                let shown: Vec<&crate::models::Model> = self
                    .models
                    .iter()
                    .filter(|m| filter.is_empty() || m.id.to_lowercase().contains(&filter) || crate::models::pretty_name(&m.id).to_lowercase().contains(&filter))
                    .collect();
                if shown.is_empty() {
                    ui.label(RichText::new(t("No model matches.")).small().color(p.weak));
                }
                egui::ScrollArea::vertical().id_salt("model-list").max_height(320.0).show(ui, |ui| {
                    for m in shown {
                        let name = crate::models::pretty_name(&m.id);
                        let tags = crate::models::model_tags(&m.id);
                        let row = menu_row(ui, p, choice == m.id, "⚡", &name, &m.id, &tags);
                        if choice == m.id && self.scroll_to_model {
                            row.scroll_to_me(Some(Align::Center));
                            self.scroll_to_model = false;
                        }
                        if row.clicked() {
                            choice = m.id.clone();
                            close = true;
                        }
                    }
                });
            }

            ui.add_space(4.0);
            ui.separator();
            let label = if has_key { format!("⚙  {}", t("Provider & key…")) } else { format!("⚙  {}", t("Set up a free model…")) };
            if ui.add(theme::Btn::new(theme::Kind::Ghost, label).min_size(egui::vec2(0.0, 30.0))).clicked() {
                open_settings = true;
                close = true;
            }
        });
        if close {
            ui.memory_mut(|m| m.close_popup());
        }
        button.on_hover_text(t("Which model answers"));
        if open_settings {
            self.open_settings();
        }
        if choice != current {
            let mut s = self.settings.clone();
            s.model = choice.clone();
            if !choice.is_empty() && s.ai_provider.trim().is_empty() {
                s.ai_provider = provider.id.to_string(); // remember which provider the model is from
            }
            self.draft.model = choice.clone();
            self.draft.ai_provider = s.ai_provider.clone();
            self.apply_settings(s);
        }
    }

    /// Read the provider's model list in the background (its suggested models stay until then).
    fn load_models(&mut self) {
        let provider = self.settings.ai_provider();
        let (base, key) = (self.settings.ai_base(), self.settings.ai_key());
        if key.trim().is_empty() || base.trim().is_empty() {
            return;
        }
        self.models_loading = true;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = crate::models::fetch_models(&base, &key, provider.id).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModelsLoaded(provider.id.to_string(), result));
            ctx.request_repaint();
        });
    }

    /// Settings: the provider of the free models and its own API key (not the GitHub token).
    fn models_settings(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        ui.label(RichText::new(t("Free AI models")).family(theme::bold()).color(p.text));
        ui.label(
            RichText::new(t("Pick a free model in the list at the top instead of the class bot. Each provider needs its own free API key."))
                .small()
                .color(p.weak),
        );
        ui.add_space(4.0);
        let current = self.settings.ai_provider().id.to_string();
        let mut chosen = current.clone();
        egui::Grid::new("ai").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(t("Provider"));
            egui::ComboBox::from_id_salt("ai-provider").selected_text(self.settings.ai_provider().name).width(300.0).show_ui(ui, |ui| {
                for pr in crate::models::PROVIDERS {
                    ui.selectable_value(&mut chosen, pr.id.to_string(), pr.name);
                }
            });
            ui.end_row();
            if chosen == "custom" {
                ui.label(t("Address"));
                ui.add(egui::TextEdit::singleline(&mut self.draft.ai_base_url).hint_text("https://…/v1").desired_width(300.0))
                    .on_hover_text(t("The OpenAI-compatible API base, e.g. http://localhost:11434/v1 for Ollama"));
                ui.end_row();
            }
            ui.label(t("API key"));
            let key = self.draft.ai_keys.entry(chosen.clone()).or_default();
            ui.add(egui::TextEdit::singleline(key).password(true).hint_text(t("key from the provider")).desired_width(300.0)).on_hover_text(
                match self.settings.ai_key_status() {
                    crate::secret::Status::Stored => t("Kept in your system's password store (not in settings.json)."),
                    crate::secret::Status::Unavailable => t("This system has no password store, so the key is kept in settings.json."),
                    crate::secret::Status::Unknown => t("Kept in your system's password store if there is one, otherwise in settings.json."),
                },
            );
            ui.end_row();
        });
        if chosen != current {
            // another provider: its models differ, so the class bot answers until one is picked
            let mut s = self.settings.clone();
            s.ai_provider = chosen.clone();
            if s.uses_ai_models() {
                s.model.clear();
            }
            self.draft.ai_provider = chosen.clone();
            self.draft.model = s.model.clone();
            self.apply_settings(s);
            self.models = crate::models::suggested_models(self.settings.ai_provider());
            self.models_note = None;
            self.load_models();
        }
        ui.horizontal(|ui| {
            if theme::primary_button(ui, t("Save key")).clicked() {
                let mut s = self.settings.clone();
                let key = self.draft.ai_keys.get(&chosen).map(|k| k.trim().to_string()).unwrap_or_default();
                if key.is_empty() {
                    s.ai_keys.remove(&chosen);
                } else {
                    s.ai_keys.insert(chosen.clone(), key);
                }
                s.ai_base_url = self.draft.ai_base_url.trim().to_string();
                self.apply_settings(s);
                self.models_note = Some((true, t("Saved – the model list is being read.").into()));
                self.load_models();
            }
            let pr = self.settings.ai_provider();
            if !pr.key_url.is_empty() {
                ui.hyperlink_to(RichText::new(tf("Get a free key at {0}", &[pr.name])).small(), pr.key_url);
            }
        });
        if let Some((ok, note)) = &self.models_note {
            let c = if *ok { p.success } else { p.warning };
            ui.add(egui::Label::new(RichText::new(note).small().color(c)).wrap());
        }
        ui.label(
            RichText::new(t("This key is only for the AI models. The GitHub token (GitHub tab) is only for your repository."))
                .small()
                .color(p.weak),
        );
    }

    /// Floating "Reply" button next to selected text.
    fn reply_to_selection(&mut self, ctx: &egui::Context) {
        let has_selection = egui::text_selection::LabelSelectionState::load(ctx).has_selection();
        let (down, released, pos, origin) =
            ctx.input(|i| (i.pointer.primary_down(), i.pointer.any_released(), i.pointer.interact_pos(), i.pointer.press_origin()));
        if origin.is_some() {
            self.drag_origin = origin;
        }
        let origin = self.drag_origin;
        if !has_selection || self.show_settings {
            self.selection_anchor = None;
        } else if released || (self.selection_anchor.is_none() && !down) {
            // a plain click also leaves an (empty) selection behind: only a real drag counts
            let dragged = matches!((origin, pos), (Some(o), Some(p)) if o.distance(p) > 4.0);
            if let (Some(p), true) = (pos, dragged) {
                let downwards = origin.map(|o| p.y >= o.y - 4.0).unwrap_or(true);
                self.selection_anchor = Some((p, downwards));
            }
        }
        self.reply_button = None;
        let Some((anchor, downwards)) = self.selection_anchor else { return };
        if down {
            return;
        }
        // keep the button off the selected lines: below the last one, or above the first one
        let screen = ctx.screen_rect();
        let size = egui::vec2(96.0, 34.0);
        let below = anchor.y + 16.0;
        let above = anchor.y - size.y - 16.0;
        let mut at = egui::pos2(anchor.x - size.x / 2.0, if downwards { below } else { above });
        if at.y + size.y > screen.bottom() - 8.0 {
            at.y = above;
        } else if at.y < screen.top() + 8.0 {
            at.y = below;
        }
        at.x = at.x.clamp(screen.left() + 8.0, screen.right() - size.x - 8.0);
        let resp = egui::Area::new(egui::Id::new("reply-to-selection"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                let p = theme::of(ui);
                egui::Frame::none()
                    .shadow(egui::epaint::Shadow { offset: egui::vec2(0.0, 4.0), blur: 14.0, spread: 0.0, color: Color32::from_black_alpha(60) })
                    .rounding(999.0)
                    .show(ui, |ui| {
                        ui.add(theme::Btn::new(theme::Kind::Primary, RichText::new(format!("↩  {}", t("Reply"))).family(theme::bold()).color(p.on_accent)).rounding(999.0))
                            .on_hover_text(t("Reply to the selected text"))
                    })
                    .inner
            });
        self.reply_button = Some(resp.inner.rect);
    }

    fn drop_zone(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(p) = &f.path {
                self.add_path(p);
            } else if let Some(bytes) = &f.bytes {
                let name = if f.name.is_empty() { "file".to_string() } else { f.name.clone() };
                self.add_file(name, bytes.to_vec());
            }
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let p = theme::palette(ctx.style().visuals.dark_mode);
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
            let card = egui::Rect::from_center_size(screen.center(), egui::vec2(screen.width().min(460.0) - 40.0, 160.0));
            painter.rect(card, 20.0, p.panel, Stroke::new(2.0_f32, p.accent));
            painter.text(card.center() - egui::vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, t("Drop files to attach"), egui::FontId::new(22.0, theme::bold()), p.text);
            painter.text(card.center() + egui::vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, t("PDFs, images, documents, code …"), egui::FontId::proportional(14.0), p.weak);
        }
    }
}

/// State of the "Attach from GitHub" dialog.
struct RepoPicker {
    loading: bool,
    error: Option<String>,
    branch: String,
    files: Vec<github::RepoFile>,
    selected: std::collections::BTreeSet<String>,
    filter: String,
    truncated: bool,
}

impl RepoPicker {
    fn selected_size(&self) -> usize {
        self.files.iter().filter(|f| self.selected.contains(&f.path)).map(|f| f.size).sum()
    }
}

/// Why a repository file cannot be attached (`None` = it can).
fn not_attachable(f: &github::RepoFile) -> Option<&'static str> {
    if !ais::is_text_name(&f.path) {
        Some(t("Only text and code files can be read by the bot this way."))
    } else if f.size > INLINE_LIMIT {
        Some(t("Larger than 200 KB."))
    } else {
        None
    }
}

/// Save files to GitHub without losing what is already there:
/// - code the answer did not name a file for: the bot picks the file it belongs to (from the
///   repository's file list) or a fitting new path; if it cannot, a name guessed from the code
/// - a file that exists is merged with the new code first (the bot writes the complete file)
/// - new files are saved as they are, unchanged files are left out
///
/// All in one commit. Returns the commit's address.
fn save_to_github(s: &Settings, cfg: &github::Config, files: Vec<github::FileOut>, chat_title: &str) -> anyhow::Result<String> {
    // One save at a time: two at once (automatic saving and a click, or two answers) read the
    // same files and the same branch, and the second then failed or merged into old code.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let branch = github::target_branch(cfg)?;
    let folder = s.github_folder();
    let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };

    // the repository's files (inside the folder, if one is set) – only needed for unnamed code
    let mut repo_files: Option<Vec<String>> = None;
    let mut placed = Vec::new();
    let mut out: Vec<github::FileOut> = Vec::new();
    let mut merged = Vec::new();
    for mut f in files {
        if let Some(info) = f.unnamed.take() {
            if repo_files.is_none() {
                let list = github::list_files(cfg).map(|(_, files, _)| files).unwrap_or_default();
                repo_files = Some(list.into_iter().filter_map(|f| f.path.strip_prefix(&prefix).map(str::to_string)).collect());
            }
            let list = repo_files.as_deref().unwrap_or_default();
            if let Ok(path) = crate::merge::place(s, list, &info.lang, &info.before, &f.content) {
                f.path = format!("{prefix}{path}");
                placed.push(f.path.clone());
            }
        }
        if let Some(current) = github::read_file_if_exists(cfg, &branch, &f.path)? {
            let content = crate::merge::merge(s, &f.path, &current, &f.content)?;
            if content.trim_end() == current.trim_end() {
                continue; // already on GitHub like this
            }
            if content.trim_end() != f.content.trim_end() {
                merged.push(f.path.clone());
            }
            f.content = content;
        }
        // two blocks for the same file: the second one is merged into the first
        if let Some(prev) = out.iter_mut().find(|o| o.path == f.path) {
            prev.content = crate::merge::merge(s, &f.path, &prev.content, &f.content)?;
            continue;
        }
        out.push(f);
    }
    if out.is_empty() {
        let repo = github::normalize_repo(&cfg.repo).unwrap_or_default();
        return Ok(format!("https://github.com/{repo}/tree/{branch}"));
    }
    let paths: Vec<&str> = out.iter().map(|f| f.path.as_str()).collect();
    let mut message = format!("Update {}\n\nSaved from the AIS.chat chat \"{chat_title}\".", paths.join(", "));
    if !placed.is_empty() {
        message.push_str(&format!("\nFile chosen by the bot: {}", placed.join(", ")));
    }
    if !merged.is_empty() {
        message.push_str(&format!("\nMerged into the existing file: {}", merged.join(", ")));
    }
    github::push(cfg, &out, &message)
}

enum RowAction {
    None,
    Open(String),
    Delete(String),
}

/// Send `history`, streaming the answer to the window. If the site was updated, read fresh
/// values once and try again.
fn reply_worker(
    s: &mut Settings,
    history: &[ChatMessage],
    session: &str,
    conv: &str,
    gen: u64,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    // AIS.chat needs its request values; the free models do not
    if !s.is_ready() && !s.uses_ai_models() {
        ais::refresh(s)?;
        let _ = tx.send(Msg::SettingsChanged(s.clone()));
    }
    let mut attempt = 0;
    loop {
        let mut got_text = false;
        let result = crate::backend::stream(s, history, session, |ev| {
            let msg = match ev {
                Event::Text(t) => {
                    got_text = true;
                    Msg::Text { conv: conv.to_string(), gen, text: t.clone() }
                }
                Event::Activity(a) => Msg::Activity { conv: conv.to_string(), gen, status: a.clone() },
                _ => return,
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        match result {
            Ok(_) => return Ok(()),
            Err(e) if attempt == 0 && !got_text && !s.uses_ai_models() && ais::is_site_changed(&e) => {
                attempt += 1;
                let _ = tx.send(Msg::Activity { conv: conv.to_string(), gen, status: "AIS.chat was updated – reconnecting…".into() });
                ctx.request_repaint();
                ais::refresh(s).map_err(|re| anyhow::anyhow!("{e} Reconnecting did not work either: {re}"))?;
                let _ = tx.send(Msg::SettingsChanged(s.clone()));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Lay out `add` in a centred column of comfortable reading width.
fn centered_column<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let avail = ui.available_width();
    let width = (avail - 32.0).clamp(240.0, 780.0).min(avail);
    let pad = ((avail - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

/// Draws one message. With `push_states`, code blocks get a GitHub button; returns the number of
/// the code block whose button was clicked.
fn message_row(
    ui: &mut egui::Ui,
    m: &ChatMessage,
    name: &str,
    waiting: bool,
    activity: &str,
    p: theme::Palette,
    push_states: Option<&std::collections::HashMap<String, PushState>>,
) -> Option<usize> {
    let mut clicked = None;
    // gentle fade-in for new messages
    let fade_in = (m.born.elapsed().as_secs_f32() / 0.25).min(1.0);
    if fade_in < 1.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let max_w = (ui.available_width() * 0.8).max(220.0);

    match m.role {
        Role::User => {
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                ui.multiply_opacity(fade_in);
                egui::Frame::none()
                    .fill(p.accent)
                    .rounding(Rounding { nw: 18.0, ne: 18.0, sw: 18.0, se: 6.0 })
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        // size the bubble to its text instead of the full row
                        let shown = m.display.as_deref().unwrap_or(&m.content);
                        let text_w = ui
                            .painter()
                            .layout(shown.to_string(), egui::TextStyle::Body.resolve(ui.style()), p.on_accent, max_w)
                            .size()
                            .x;
                        let files_w: f32 = m.attachments.iter().map(|a| a.name.chars().count() as f32 * 7.0 + 44.0).sum();
                        let quote_w = m
                            .quote
                            .as_deref()
                            .map(|q| {
                                let font = egui::TextStyle::Small.resolve(ui.style());
                                ui.painter().layout(short_quote(q, 240), font, p.on_accent, max_w - 24.0).size().x + 24.0
                            })
                            .unwrap_or(0.0);
                        let w = text_w.max(files_w.min(max_w)).max(quote_w).clamp(10.0, max_w) + 1.0;
                        ui.set_max_width(w);
                        ui.set_min_width(w);
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            if let Some(q) = &m.quote {
                                let r = egui::Frame::none()
                                    .fill(Color32::from_black_alpha(40))
                                    .rounding(8.0)
                                    .inner_margin(egui::Margin { left: 12.0, right: 8.0, top: 5.0, bottom: 5.0 })
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.add(
                                            egui::Label::new(RichText::new(short_quote(q, 240)).small().color(p.on_accent.gamma_multiply(0.85))).wrap(),
                                        );
                                    });
                                let rect = r.response.rect;
                                let bar = egui::Rect::from_min_max(rect.left_top() + egui::vec2(4.0, 5.0), rect.left_bottom() + egui::vec2(7.0, -5.0));
                                ui.painter().rect_filled(bar, 1.5, Color32::from_white_alpha(170));
                                ui.add_space(2.0);
                            }
                            if !shown.trim().is_empty() {
                                ui.label(RichText::new(shown).color(p.on_accent));
                            }
                            if !m.attachments.is_empty() {
                                let font = egui::TextStyle::Small.resolve(ui.style());
                                let row = ui.available_width();
                                let texts: Vec<String> = m
                                    .attachments
                                    .iter()
                                    .map(|a| {
                                        let icon = if matches!(a.data, AttachmentData::Inline(_)) { "📄" } else { "📎" };
                                        fit_text(ui, &format!("{icon} {}", a.name), &font, row - 18.0)
                                    })
                                    .collect();
                                let widths: Vec<f32> = texts.iter().map(|t| text_width(ui, t, &font) + 18.0).collect();
                                for line in chip_lines(&widths, row, 6.0) {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        for i in line {
                                            let a = &m.attachments[i];
                                            egui::Frame::none()
                                                .fill(Color32::from_white_alpha(36))
                                                .rounding(999.0)
                                                .inner_margin(egui::Margin::symmetric(9.0, 3.0))
                                                .show(ui, |ui| {
                                                    let label = egui::Label::new(RichText::new(&texts[i]).small().color(p.on_accent)).extend();
                                                    ui.add(label).on_hover_text(format!("{} · {}", a.name, human_size(a.size)));
                                                });
                                        }
                                    });
                                }
                            }
                        });
                    });
            });
        }
        Role::Assistant => {
            ui.horizontal_top(|ui| {
                ui.multiply_opacity(fade_in);
                theme::avatar(ui, name, 30.0);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    let card_id = egui::Id::new(("card", &m.id));
                    let hover = theme::remembered_hover(ui, card_id);
                    let card = egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0_f32, theme::mix(p.border, p.accent, hover * 0.45)))
                        .rounding(Rounding { nw: 6.0, ne: 18.0, sw: 18.0, se: 18.0 })
                        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_max_width(max_w);
                            if waiting {
                                ui.horizontal(|ui| {
                                    theme::typing_dots(ui);
                                    if !activity.is_empty() {
                                        ui.label(RichText::new(activity).small().color(p.weak));
                                    }
                                });
                                return;
                            }
                            let text = m.display.as_deref().unwrap_or(&m.content);
                            match push_states {
                                Some(states) => {
                                    let lookup = |n: usize| states.get(&format!("{}#{n}", m.id)).cloned().unwrap_or(PushState::Idle);
                                    clicked = markdown::show_with(ui, text, &m.id, Some(&lookup));
                                }
                                None => markdown::show(ui, text, &m.id),
                            }
                            if !activity.is_empty() {
                                ui.horizontal(|ui| {
                                    theme::typing_dots(ui);
                                    ui.label(RichText::new(activity).small().color(p.weak));
                                });
                            }
                        });
                    theme::remember_hover(ui, card_id, ui.rect_contains_pointer(card.response.rect));
                    if !m.initial && !waiting && !m.content.is_empty() {
                        let id = egui::Id::new(("copy-msg", &m.id));
                        let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
                        let now = ui.input(|i| i.time);
                        let recently = copied_at.map(|c| now - c < 1.5).unwrap_or(false);
                        let label = if recently { format!("✔ {}", t("Copied")) } else { format!("📋 {}", t("Copy")) };
                        let r = ui.add(theme::Btn::new(theme::Kind::Ghost, RichText::new(label).small()));
                        if r.on_hover_text(t("Copy the whole answer")).clicked() {
                            ui.ctx().copy_text(m.content.clone());
                            ui.data_mut(|d| d.insert_temp(id, now));
                        }
                        if recently {
                            ui.ctx().request_repaint_after(Duration::from_millis(300));
                        }
                    }
                });
            });
        }
    }
    clicked
}

/// A small grey heading in the model menu, with a spinner while something loads.
fn menu_heading(ui: &mut egui::Ui, p: theme::Palette, text: &str, loading: Option<()>) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(text.to_uppercase()).size(11.0).family(theme::bold()).color(p.weak));
        if loading.is_some() {
            ui.add(egui::Spinner::new().size(11.0));
        }
    });
}

/// One choice in the model menu: icon, name, a line below, tags, and a ✓ when selected.
fn menu_row(ui: &mut egui::Ui, p: theme::Palette, selected: bool, icon: &str, title: &str, sub: &str, tags: &[&str]) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
    let t = theme::hover_anim(ui, resp.id, resp.hovered());
    let fill = if selected { p.accent_soft } else { theme::mix(Color32::TRANSPARENT, p.surface_hi, t) };
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, fill);

    // icon in a small circle
    let ic = egui::pos2(rect.left() + 22.0, rect.center().y);
    painter.circle_filled(ic, 14.0, if selected { p.accent } else { p.surface_hi });
    painter.text(ic, egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(14.0), if selected { p.on_accent } else { p.text });

    // tags and ✓ on the right
    let mut right = rect.right() - 10.0;
    if selected {
        painter.text(egui::pos2(right, rect.center().y), egui::Align2::RIGHT_CENTER, "✔", egui::FontId::proportional(14.0), p.accent_hover);
        right -= 22.0;
    }
    for tag in tags.iter().rev() {
        let galley = painter.layout_no_wrap(tag.to_string(), egui::FontId::proportional(10.5), p.accent_hover);
        let pill = egui::Rect::from_min_size(
            egui::pos2(right - galley.size().x - 12.0, rect.center().y - 9.0),
            egui::vec2(galley.size().x + 12.0, 18.0),
        );
        painter.rect_filled(pill, 999.0, p.accent_soft);
        painter.galley(pill.center() - galley.size() / 2.0, galley, p.accent_hover);
        right = pill.left() - 4.0;
    }

    // name and the line below, shortened to the room left
    let left = rect.left() + 44.0;
    let room = right - left - 4.0;
    let title_font = egui::FontId::new(14.0, theme::bold());
    let sub_font = egui::FontId::proportional(11.5);
    let title = fit_end(ui, title, &title_font, room);
    let sub = fit_end(ui, sub, &sub_font, room);
    let painter = ui.painter();
    painter.text(egui::pos2(left, rect.top() + 7.0), egui::Align2::LEFT_TOP, title, title_font, p.text);
    painter.text(egui::pos2(left, rect.top() + 26.0), egui::Align2::LEFT_TOP, sub, sub_font, p.weak);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A card inside the Settings dialog.
fn section(ui: &mut egui::Ui, p: theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(p.surface_hi.gamma_multiply(0.6))
        .stroke(Stroke::new(1.0_f32, p.border))
        .rounding(14.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

fn notice(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .rounding(12.0)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("⚠").color(color));
                add(ui)
            });
        });
    ui.add_space(6.0);
}

fn capture_armed_id() -> egui::Id {
    egui::Id::new("quote-capture-armed")
}

fn capture_text_id() -> egui::Id {
    egui::Id::new("quote-capture-text")
}

/// Runs after egui's own text-selection code at the end of every frame: when the Reply button
/// was pressed, take the text egui collected for the clipboard and keep it for the quote
/// instead (the clipboard is left alone).
fn install_quote_capture(ctx: &egui::Context) {
    ctx.on_end_pass(
        "quote-capture",
        std::sync::Arc::new(|ctx: &egui::Context| {
            let armed = ctx.data_mut(|d| d.remove_temp::<bool>(capture_armed_id())).unwrap_or(false);
            if armed {
                let text = ctx.output_mut(|o| std::mem::take(&mut o.copied_text));
                ctx.data_mut(|d| d.insert_temp(capture_text_id(), text));
                ctx.request_repaint();
            }
        }),
    );
}

/// Tidy selected text for quoting: trim, no runs of blank lines, at most 1500 characters.
fn clean_quote(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in text.trim().lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    let out = out.trim_end().to_string();
    if out.chars().count() > 1500 {
        format!("{}…", out.chars().take(1500).collect::<String>())
    } else {
        out
    }
}

/// First `max` characters on at most four lines, for previews.
fn short_quote(q: &str, max: usize) -> String {
    let lines: Vec<&str> = q.lines().collect();
    let mut s = lines.iter().take(4).copied().collect::<Vec<_>>().join("\n");
    let cut = s.chars().count() > max;
    if cut {
        s = s.chars().take(max).collect();
    }
    if cut || lines.len() > 4 {
        s.push('…');
    }
    s
}

fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x
}

/// `text` shortened with "…" (in the middle, so the file extension stays visible) until it is
/// at most `max` pixels wide.
/// A thin bar that fills up as credits are spent: orange from 80 %, red when all are used.
fn credit_meter(painter: &egui::Painter, rect: egui::Rect, used: i64, limit: i64, p: theme::Palette) {
    let share = if limit <= 0 { 1.0 } else { (used as f32 / limit as f32).clamp(0.0, 1.0) };
    let color = if share >= 1.0 {
        p.danger
    } else if share >= 0.8 {
        p.warning
    } else {
        p.accent
    };
    let rounding = rect.height() / 2.0;
    painter.rect_filled(rect, rounding, p.surface_hi);
    if share > 0.0 {
        let filled = egui::Rect::from_min_size(rect.min, egui::vec2((rect.width() * share).max(rect.height()), rect.height()));
        painter.rect_filled(filled, rounding, color);
    }
}

/// What the chat says when the server refuses a credit.
fn no_credits_text(c: &crate::sync::Credits) -> String {
    if c.blocked {
        return t("This account was blocked.").to_string();
    }
    let (used, limit) = (c.used.to_string(), c.limit.unwrap_or(0).to_string());
    match c.resets_at {
        Some(at) => tf("No credits left ({0} of {1} used). New credits: {2}", &[&used, &limit, &conversations::when_exact(at)]),
        None => tf("No credits left ({0} of {1} used).", &[&used, &limit]),
    }
}

/// A text box of the school login screen.
fn login_field<'a>(text: &'a mut String, hint: &str, password: bool, width: f32) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint.to_string())
        .password(password)
        .margin(egui::Margin::symmetric(12.0, 10.0))
        .desired_width(width)
}

/// Like `fit_text`, but cuts at the end – for sentences (titles, previews).
fn fit_end(ui: &egui::Ui, text: &str, font: &egui::FontId, max: f32) -> String {
    if text_width(ui, text, font) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    // the longest start that still fits with "…"
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s: String = chars[..mid].iter().collect::<String>().trim_end().to_string() + "…";
        if text_width(ui, &s, font) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
}

fn fit_text(ui: &egui::Ui, text: &str, font: &egui::FontId, max: f32) -> String {
    if text_width(ui, text, font) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 4 {
        keep -= 1;
        let tail = (keep / 3).max(1);
        let head = keep - tail;
        let s: String = chars[..head].iter().chain(['…'].iter()).chain(chars[chars.len() - tail..].iter()).collect();
        if text_width(ui, &s, font) <= max {
            return s;
        }
    }
    "…".to_string()
}

/// Split chips of the given widths into lines of at most `row` pixels (`gap` between chips).
/// Every chip lands on some line, even one wider than a whole line.
fn chip_lines(widths: &[f32], row: f32, gap: f32) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if used + gap + w <= row => {
                line.push(i);
                used += gap + w;
            }
            _ => {
                lines.push(vec![i]);
                used = *w;
            }
        }
    }
    lines
}

fn human_size(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else if n >= 1024 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{n} bytes")
    }
}

impl eframe::App for App {
    /// Paint the window background in the theme colour, so no dark seams show between panels.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    /// While the "Reply" button is shown, a click on it makes egui collect the selected text
    /// (as if Ctrl+C was pressed) in the same frame, before the click clears the selection.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(rect) = self.reply_button else { return };
        let pressed_on_button = raw_input.events.iter().any(|e| {
            matches!(e, egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } if rect.contains(*pos))
        });
        if pressed_on_button {
            raw_input.events.push(egui::Event::Copy);
            ctx.data_mut(|d| d.insert_temp(capture_armed_id(), true));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_chats();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // At most about 60 frames a second (vsync is off, see `main`), so animations do not
        // use a whole processor core.
        const FRAME: Duration = Duration::from_millis(16);
        let since_last = self.last_frame.elapsed();
        if since_last < FRAME {
            std::thread::sleep(FRAME - since_last);
        }
        self.last_frame = std::time::Instant::now();

        crate::i18n::set_ui_language(&self.settings.ui_language());
        self.handle_messages();

        // Nothing else happens until the school login (Moodle) has worked.
        if !self.school_login_ok {
            self.school_login_screen(ctx);
            return;
        }

        self.summarise_titles();
        self.school_sync_login();
        self.sync_tick(ctx);

        // text captured by the Reply button last frame
        if let Some(text) = ctx.data_mut(|d| d.remove_temp::<String>(capture_text_id())) {
            let text = clean_quote(&text);
            if !text.is_empty() {
                self.quote = Some(text);
                self.focus_input = true;
            }
            self.selection_anchor = None;
            let mut sel = egui::text_selection::LabelSelectionState::load(ctx);
            sel.clear_selection();
            sel.store(ctx);
        }

        let title = format!("{} – AIS.chat", self.settings.display_name());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        self.drop_zone(ctx);
        self.top_bar(ctx);
        self.sidebar(ctx);
        self.bottom_bar(ctx);
        self.chat(ctx);
        self.settings_window(ctx);
        self.repo_picker_window(ctx);
        self.undo_toast(ctx);
        self.reply_to_selection(ctx);

        if self.busy || self.pending.iter().any(|f| matches!(f.state, FileState::Uploading)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_quote, short_quote};

    #[test]
    fn cleans_quotes() {
        assert_eq!(clean_quote("  a  \n\n\n\nb\n "), "a\n\nb");
        assert_eq!(clean_quote(&"x".repeat(2000)).chars().count(), 1501);
    }

    /// Needs the stand-in GitHub on 127.0.0.1:8765 and the real AIS.chat:
    /// `cargo test -- --ignored live_save`
    #[test]
    #[ignore]
    fn live_save_places_and_merges_unnamed_code() {
        let mut s = crate::settings::Settings::default();
        s.apply_invite_link(crate::settings::DEFAULT_INVITE_LINK).unwrap();
        crate::ais::refresh(&mut s).expect("setup");
        s.github_enabled = true;
        s.github_repo = "lena/schulprojekt".into();
        s.github_token = "t".into();
        s.github_api = "http://127.0.0.1:8765".into();
        let block = crate::markdown::CodeBlockInfo {
            lang: "rust".into(),
            code: "    /// Personal access token. Kept in the OS keystore; only read from the file to migrate old versions.\n    #[serde(skip_serializing)]\n    pub github_token: String,".into(),
            before: "Change the token field in the settings struct:".into(),
        };
        let files = crate::github::files_for(&[(3, &block)], "");
        assert_eq!(files[0].path, "snippet-4.rs");
        let url = super::save_to_github(&s, &s.github_config(), files, "keystore").expect("save");
        println!("{url}");
    }

    #[test]
    fn splits_chips_into_lines() {
        use super::chip_lines;
        assert_eq!(chip_lines(&[100.0, 100.0, 100.0], 250.0, 10.0), vec![vec![0, 1], vec![2]]);
        assert_eq!(chip_lines(&[300.0, 50.0], 250.0, 10.0), vec![vec![0], vec![1]], "too wide still gets its own line");
        assert_eq!(chip_lines(&[], 250.0, 10.0), Vec::<Vec<usize>>::new());
    }

    #[test]
    fn shortens_quotes() {
        assert_eq!(short_quote("1\n2\n3\n4\n5", 100), "1\n2\n3\n4…");
        assert_eq!(short_quote("abcdef", 3), "abc…");
        assert_eq!(short_quote("ok", 10), "ok");
    }
}

/// `App` stands for the open chat when a chat field is used (`self.messages`, `self.input`, …).
impl std::ops::Deref for App {
    type Target = Conversation;
    fn deref(&self) -> &Conversation {
        &self.convs[self.active]
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Conversation {
        &mut self.convs[self.active]
    }
}
