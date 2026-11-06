//! The window: chat, input box with attachments, and the Settings window.
//!
//! Network work runs on background threads that report back through a channel, so the window
//! never freezes. Setup is automatic: on start the app reads the current request values from
//! the site, and whenever AIS.chat is updated (sending fails with "not recognised") it reads
//! them again and re-sends the message without asking the user to do anything.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Key, Layout, Modifiers, RichText};

use crate::ais::{self, Attachment, AttachmentData, ChatMessage, Event, Role};
use crate::markdown;
use crate::settings::{Settings, DEFAULT_INVITE_LINK};

/// Text files up to this size are pasted into the message instead of being uploaded.
const INLINE_LIMIT: usize = 200 * 1024;

const FALLBACK_GREETING: &str = "Hello! How can I help you?";

enum Msg {
    /// Background setup finished. `from_settings`: started by the Connect button.
    Refreshed { result: Result<(Settings, String), String>, from_settings: bool },
    /// A worker had to read fresh values from the site while sending.
    SettingsChanged(Settings),
    Uploaded { id: u64, gen: u64, result: Result<String, String> },
    Text { gen: u64, text: String },
    Activity { gen: u64, status: String },
    ReplyDone { gen: u64, result: Result<(), String> },
}

enum FileState {
    Uploading,
    Ready(AttachmentData),
    Failed(String),
}

struct PendingFile {
    id: u64,
    name: String,
    size: usize,
    state: FileState,
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

    // chat
    messages: Vec<ChatMessage>,
    session_id: String,
    input: String,
    pending: Vec<PendingFile>,
    next_file_id: u64,
    /// Bumped on New chat / Stop so late answers from old requests are ignored.
    gen: u64,
    busy: bool,
    activity: String,
    error: Option<String>,
    focus_input: bool,

    // settings window
    show_settings: bool,
    link_input: String,
    curl_input: String,
    draft: Settings,
    settings_note: Option<(bool, String)>,
    connecting_from_settings: bool,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let (tx, rx) = channel();
        let mut app = Self {
            link_input: settings.invite_link.clone(),
            draft: settings.clone(),
            settings,
            conn: Conn::Connecting,
            messages: Vec::new(),
            session_id: ais::new_session_id(),
            input: String::new(),
            pending: Vec::new(),
            next_file_id: 0,
            gen: 0,
            busy: false,
            activity: String::new(),
            error: None,
            focus_input: true,
            show_settings: false,
            curl_input: String::new(),
            settings_note: None,
            connecting_from_settings: false,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            title: String::new(),
        };
        app.reset_chat();
        app.start_refresh(false);
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
            let msg = format!("too large (max {} MB)", ais::MAX_UPLOAD_BYTES / 1024 / 1024);
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
        // Everything else goes to the upload address, which is worked out from the invite link.
        self.pending.push(PendingFile { id, name: name.clone(), size, state: FileState::Uploading });
        let s = self.settings.clone();
        let session = self.session_id.clone();
        let gen = self.gen;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = ais::upload_file(&s, &name, bytes, &session).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Uploaded { id, gen, result });
            ctx.request_repaint();
        });
    }

    fn pick_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files() {
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
                self.pending.push(PendingFile { id, name, size: 0, state: FileState::Failed(format!("could not read: {e}")) });
            }
        }
    }

    fn can_send(&self) -> bool {
        !self.busy
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
        self.messages.push(ChatMessage::user_with(text.trim(), attachments));
        self.start_reply();
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
        let gen = self.gen;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = reply_worker(&mut s, &history, &session, gen, &tx, &ctx);
            let _ = tx.send(Msg::ReplyDone { gen, result: result.map_err(|e| e.to_string()) });
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
    }

    fn reset_chat(&mut self) {
        self.gen += 1;
        self.busy = false;
        self.activity.clear();
        self.error = None;
        self.pending.clear();
        self.session_id = ais::new_session_id();
        let greeting = if self.settings.greeting.trim().is_empty() { FALLBACK_GREETING } else { self.settings.greeting.as_str() };
        self.messages = vec![ChatMessage::greeting(greeting)];
        self.focus_input = true;
    }

    fn chat_is_empty(&self) -> bool {
        self.messages.iter().all(|m| m.initial)
    }

    fn apply_settings(&mut self, s: Settings) {
        let greeting_changed = s.greeting != self.settings.greeting;
        self.settings = s;
        let _ = self.settings.save();
        if greeting_changed && self.chat_is_empty() && !self.busy {
            self.reset_chat();
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
                        Ok((s, summary)) => {
                            self.draft = s.clone();
                            self.link_input = s.invite_link.clone();
                            self.apply_settings(s);
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
                Msg::SettingsChanged(s) => {
                    self.apply_settings(s);
                    self.conn = Conn::Ready;
                }
                Msg::Uploaded { id, gen, result } => {
                    if gen != self.gen {
                        continue;
                    }
                    if let Some(f) = self.pending.iter_mut().find(|f| f.id == id) {
                        f.state = match result {
                            Ok(file_id) => FileState::Ready(AttachmentData::Uploaded(file_id)),
                            Err(e) => FileState::Failed(e),
                        };
                    }
                }
                Msg::Text { gen, text } if gen == self.gen => {
                    if let Some(m) = self.messages.last_mut() {
                        m.content.push_str(&text);
                    }
                }
                Msg::Activity { gen, status } if gen == self.gen => self.activity = status,
                Msg::ReplyDone { gen, result } if gen == self.gen => {
                    self.busy = false;
                    self.activity.clear();
                    if let Err(e) = result {
                        if matches!(self.messages.last(), Some(m) if m.role == Role::Assistant && !m.initial && m.content.trim().is_empty())
                        {
                            self.messages.pop();
                        }
                        self.error = Some(e);
                    } else {
                        self.conn = Conn::Ready;
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(self.settings.display_name());
                let (color, tip) = match &self.conn {
                    Conn::Ready => (Color32::from_rgb(60, 170, 90), "Connected".to_string()),
                    Conn::Connecting => (Color32::from_rgb(220, 170, 40), "Connecting…".to_string()),
                    Conn::Problem(e) => (Color32::from_rgb(210, 70, 60), format!("Not connected: {e}")),
                };
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, color);
                resp.on_hover_text(&tip);
                if self.conn == Conn::Connecting {
                    ui.label(RichText::new("connecting…").weak());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("⚙ Settings").on_hover_text("Invite link and connection").clicked() {
                        self.open_settings();
                    }
                    if ui.button("➕ New chat").on_hover_text("Start over (the bot forgets this conversation)").clicked() {
                        self.reset_chat();
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn open_settings(&mut self) {
        self.draft = self.settings.clone();
        self.link_input = self.settings.invite_link.clone();
        self.settings_note = None;
        self.show_settings = true;
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("input").show(ctx, |ui| {
            ui.add_space(6.0);

            if let Conn::Problem(e) = &self.conn {
                let e = e.clone();
                notice(ui, Color32::from_rgb(210, 70, 60), |ui| {
                    ui.label(format!("Not connected: {e}"));
                    if ui.button("Try again").clicked() {
                        self.start_refresh(false);
                    }
                    if ui.button("Open Settings").clicked() {
                        self.open_settings();
                    }
                });
            }
            if let Some(e) = self.error.clone() {
                notice(ui, Color32::from_rgb(210, 70, 60), |ui| {
                    ui.label(e);
                    if ui.button("↻ Retry").clicked() {
                        self.retry();
                    }
                    if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            }

            // attachments waiting for the next message
            if !self.pending.is_empty() {
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for f in &self.pending {
                        let (icon, note, color) = match &f.state {
                            FileState::Uploading => ("⏳", "uploading…".to_string(), None),
                            FileState::Ready(AttachmentData::Inline(_)) => ("📄", "added as text".to_string(), None),
                            FileState::Ready(AttachmentData::Uploaded(_)) => ("📎", "uploaded".to_string(), None),
                            FileState::Failed(e) => ("⚠", e.clone(), Some(Color32::from_rgb(210, 70, 60))),
                        };
                        egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(6.0, 2.0)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut t = RichText::new(format!("{icon} {}", f.name));
                                if let Some(c) = color {
                                    t = t.color(c);
                                }
                                ui.label(t).on_hover_text(format!("{} – {note}", human_size(f.size)));
                                if matches!(f.state, FileState::Uploading) {
                                    ui.spinner();
                                }
                                if ui.small_button("✖").on_hover_text("Remove").clicked() {
                                    remove = Some(f.id);
                                }
                            });
                        });
                    }
                });
                if let Some(id) = remove {
                    self.pending.retain(|f| f.id != id);
                }
            }

            let input_id = egui::Id::new("chat-input");
            let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            let focused = ui.memory(|m| m.has_focus(input_id));
            let mut send_now = false;
            if focused && enter {
                ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                send_now = true;
            }

            ui.horizontal(|ui| {
                if ui.add_sized([36.0, 36.0], egui::Button::new("📎")).on_hover_text("Attach files (or drag them onto the window)").clicked() {
                    self.pick_files();
                }
                let send_w = 76.0;
                let edit = egui::TextEdit::multiline(&mut self.input)
                    .id(input_id)
                    .hint_text("Type your message…   Enter = send, Shift+Enter = new line")
                    .desired_rows(2)
                    .desired_width(ui.available_width() - send_w - 8.0)
                    .lock_focus(true);
                let resp = ui.add(edit);
                if self.focus_input {
                    resp.request_focus();
                    self.focus_input = false;
                }
                if self.busy {
                    if ui.add_sized([send_w, 36.0], egui::Button::new("■ Stop")).on_hover_text("Stop the answer").clicked() {
                        self.stop();
                    }
                } else {
                    let uploading = self.pending.iter().any(|f| matches!(f.state, FileState::Uploading));
                    let b = ui.add_enabled(self.can_send(), egui::Button::new(RichText::new("Send").strong()).min_size([send_w, 36.0].into()));
                    let b = if uploading { b.on_disabled_hover_text("Waiting for the upload to finish…") } else { b };
                    if b.clicked() {
                        send_now = true;
                    }
                }
            });
            if send_now {
                self.send();
            }
            ui.add_space(6.0);
        });
    }

    fn chat(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(8.0);
                let last = self.messages.len().saturating_sub(1);
                for (i, m) in self.messages.iter().enumerate() {
                    let waiting = self.busy && i == last && m.role == Role::Assistant && m.content.is_empty();
                    message_bubble(ui, m, waiting, &self.activity);
                    ui.add_space(10.0);
                }
                if self.busy && !self.activity.is_empty() && !self.messages.last().map(|m| m.content.is_empty()).unwrap_or(true) {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(&self.activity).italics().weak());
                    });
                }
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("Invite link").strong());
                ui.label(RichText::new("Paste the link of the shared chat. Everything else is set up automatically.").weak());
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.link_input)
                        .hint_text("https://app.ais-chat.schule/ua/characters/…/dialog?inviteCode=…")
                        .desired_width(f32::INFINITY),
                );
                let pressed_enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.horizontal(|ui| {
                    let connect = ui.add_enabled(!self.connecting_from_settings, egui::Button::new(RichText::new("Connect").strong()));
                    if connect.clicked() || pressed_enter {
                        self.connect_from_settings();
                    }
                    if ui.button("Use class link").on_hover_text(DEFAULT_INVITE_LINK).clicked() {
                        self.link_input = DEFAULT_INVITE_LINK.to_string();
                        self.connect_from_settings();
                    }
                    if self.connecting_from_settings {
                        ui.spinner();
                        ui.label("Connecting…");
                    }
                });
                if let Some((ok, note)) = &self.settings_note {
                    let c = if *ok { Color32::from_rgb(60, 170, 90) } else { Color32::from_rgb(210, 70, 60) };
                    ui.label(RichText::new(if *ok { format!("✔ {note}") } else { format!("⚠ {note}") }).color(c));
                }
                if self.settings.is_ready() {
                    ui.label(RichText::new(format!("Chat: {}", self.settings.display_name())).weak());
                    let upload = if self.settings.upload_url.trim().is_empty() { "automatic" } else { "custom address" };
                    ui.label(RichText::new(format!("File upload: {upload}")).weak())
                        .on_hover_text(self.settings.upload_endpoint());
                }

                ui.add_space(8.0);
                egui::CollapsingHeader::new("Advanced (only needed if something does not work)").show(ui, |ui| {
                    egui::Grid::new("adv").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Upload address");
                        let auto_upload = {
                            let mut d = self.draft.clone();
                            d.upload_url.clear();
                            d.upload_endpoint()
                        };
                        ui.add(
                            egui::TextEdit::singleline(&mut self.draft.upload_url)
                                .hint_text(format!("automatic: {auto_upload}"))
                                .desired_width(340.0),
                        )
                        .on_hover_text("Leave empty: files go to the address that belongs to the invite link.");
                        ui.end_row();
                        ui.label("next-action");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.next_action).desired_width(340.0));
                        ui.end_row();
                        ui.label("Deployment id");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.deployment_id).desired_width(340.0));
                        ui.end_row();
                        ui.label("User agent");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.user_agent).hint_text("built in").desired_width(340.0));
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Save these values").clicked() {
                            let s = self.draft.clone();
                            self.apply_settings(s);
                            self.settings_note = Some((true, "Saved.".into()));
                        }
                        if ui.button("Detect again").on_hover_text("Read the current values from the AIS.chat page").clicked() {
                            self.draft.next_action.clear();
                            self.start_refresh(true);
                        }
                    });

                    ui.add_space(6.0);
                    ui.label(RichText::new("Import from a cURL").strong());
                    ui.label(
                        RichText::new(
                            "Only if Connect fails: in the browser press F12 → Network, send a message, right-click the \
                             dialog?inviteCode=… request → Copy → Copy as cURL, and paste it here.",
                        )
                        .weak(),
                    );
                    ui.add(egui::TextEdit::multiline(&mut self.curl_input).desired_rows(3).desired_width(f32::INFINITY));
                    if ui.button("Import").clicked() {
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
                    ui.label(RichText::new(format!("Settings file: {}", Settings::path_display())).small().weak());
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if !open || close {
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
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            painter.text(screen.center(), egui::Align2::CENTER_CENTER, "Drop files to attach them", egui::FontId::proportional(24.0), Color32::WHITE);
        }
    }
}

/// Send `history`, streaming the answer to the window. If the site was updated, read fresh
/// values once and try again.
fn reply_worker(s: &mut Settings, history: &[ChatMessage], session: &str, gen: u64, tx: &Sender<Msg>, ctx: &egui::Context) -> anyhow::Result<()> {
    if !s.is_ready() {
        ais::refresh(s)?;
        let _ = tx.send(Msg::SettingsChanged(s.clone()));
    }
    let mut attempt = 0;
    loop {
        let mut got_text = false;
        let body = ais::build_body(s, history, session);
        let result = ais::stream_reply(s, body, |ev| {
            let msg = match ev {
                Event::Text(t) => {
                    got_text = true;
                    Msg::Text { gen, text: t.clone() }
                }
                Event::Activity(a) => Msg::Activity { gen, status: a.clone() },
                _ => return,
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        match result {
            Ok(_) => return Ok(()),
            Err(e) if attempt == 0 && !got_text && ais::is_site_changed(&e) => {
                attempt += 1;
                let _ = tx.send(Msg::Activity { gen, status: "AIS.chat was updated – reconnecting…".into() });
                ctx.request_repaint();
                ais::refresh(s).map_err(|re| anyhow::anyhow!("{e} Reconnecting did not work either: {re}"))?;
                let _ = tx.send(Msg::SettingsChanged(s.clone()));
            }
            Err(e) => return Err(e),
        }
    }
}

fn message_bubble(ui: &mut egui::Ui, m: &ChatMessage, waiting: bool, activity: &str) {
    let dark = ui.visuals().dark_mode;
    let (fill, align) = match m.role {
        Role::User => (if dark { Color32::from_rgb(34, 62, 104) } else { Color32::from_rgb(216, 231, 252) }, Align::Max),
        Role::Assistant => (if dark { Color32::from_gray(38) } else { Color32::from_gray(238) }, Align::Min),
    };
    let max_w = (ui.available_width() * 0.85).max(200.0);

    // gentle fade-in for new messages
    let t = (m.born.elapsed().as_secs_f32() / 0.25).min(1.0);
    if t < 1.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    ui.with_layout(Layout::top_down(align), |ui| {
        ui.multiply_opacity(t);
        egui::Frame::none().fill(fill).rounding(10.0).inner_margin(egui::Margin::symmetric(12.0, 8.0)).show(ui, |ui| {
            ui.set_max_width(max_w);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if waiting {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(if activity.is_empty() { "Thinking…" } else { activity }).italics().weak());
                    });
                    return;
                }
                let shown = m.display.as_deref().unwrap_or(&m.content);
                match m.role {
                    Role::Assistant => markdown::show(ui, shown, &m.id),
                    Role::User => {
                        if !shown.trim().is_empty() {
                            ui.label(shown);
                        }
                    }
                }
                if !m.attachments.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for a in &m.attachments {
                            let icon = match a.data {
                                AttachmentData::Inline(_) => "📄",
                                AttachmentData::Uploaded(_) => "📎",
                            };
                            ui.label(RichText::new(format!("{icon} {}", a.name)).small()).on_hover_text(human_size(a.size));
                        }
                    });
                }
            });
        });
        if m.role == Role::Assistant && !m.initial && !waiting && !m.content.is_empty() {
            let id = egui::Id::new(("copy-msg", &m.id));
            let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
            let now = ui.input(|i| i.time);
            let recently = copied_at.map(|c| now - c < 1.5).unwrap_or(false);
            let r = ui.add(egui::Button::new(RichText::new(if recently { "✔ Copied" } else { "📋 Copy answer" }).small()).frame(false));
            if r.clicked() {
                ui.ctx().copy_text(m.content.clone());
                ui.data_mut(|d| d.insert_temp(id, now));
            }
            if recently {
                ui.ctx().request_repaint_after(Duration::from_millis(300));
            }
        }
    });
}

fn notice(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .stroke(egui::Stroke::new(1.0_f32, color))
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(add);
        });
    ui.add_space(4.0);
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();

        let title = format!("{} – AIS.chat", self.settings.display_name());
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        self.drop_zone(ctx);
        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.chat(ctx);
        self.settings_window(ctx);

        if self.busy || self.pending.iter().any(|f| matches!(f.state, FileState::Uploading)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
