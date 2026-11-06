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
use crate::markdown;
use crate::theme;
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
        theme::install(&cc.egui_ctx);
        let settings = Settings::load();
        theme::set_appearance(&cc.egui_ctx, &settings.appearance);
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
        let p = theme::palette(ctx.style().visuals.dark_mode);
        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::none().fill(p.panel).inner_margin(egui::Margin::symmetric(18.0, 10.0)).stroke(Stroke::new(1.0_f32, p.border)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &self.settings.display_name(), 38.0);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).size(16.0).color(p.text));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (color, text) = match &self.conn {
                                Conn::Ready if self.busy => (p.success, "typing…".to_string()),
                                Conn::Ready => (p.success, "Online".to_string()),
                                Conn::Connecting => (p.warning, "Connecting…".to_string()),
                                Conn::Problem(_) => (p.danger, "Offline".to_string()),
                            };
                            theme::dot(ui, color);
                            let r = ui.label(RichText::new(text).small().color(p.weak));
                            if let Conn::Problem(e) = &self.conn {
                                r.on_hover_text(e);
                            }
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let gear = ui.add(egui::Button::new(RichText::new("⚙").size(18.0)).frame(false).min_size(egui::vec2(34.0, 34.0)));
                        if gear.on_hover_text("Settings").clicked() {
                            self.open_settings();
                        }
                        let new_chat = ui.add_enabled(
                            !self.chat_is_empty() || self.busy,
                            egui::Button::new("➕  New chat").rounding(999.0).min_size(egui::vec2(0.0, 32.0)),
                        );
                        if new_chat.on_hover_text("Start over (the bot forgets this conversation)").clicked() {
                            self.reset_chat();
                        }
                    });
                });
            });
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
        if let Conn::Problem(e) = &self.conn {
            let e = e.clone();
            notice(ui, p.danger, |ui| {
                ui.label(RichText::new(format!("Not connected. {e}")).color(p.text));
                if ui.button("Try again").clicked() {
                    self.start_refresh(false);
                }
                if ui.button("Settings").clicked() {
                    self.open_settings();
                }
            });
        }
        if let Some(e) = self.error.clone() {
            notice(ui, p.danger, |ui| {
                ui.label(RichText::new(e).color(p.text));
                if ui.button("↻ Retry").clicked() {
                    self.retry();
                }
                if theme::ghost_button(ui, "✖").on_hover_text("Dismiss").clicked() {
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

        let border = if focused { p.accent } else { p.border };
        egui::Frame::none()
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
                // attachments waiting for the next message
                if !self.pending.is_empty() {
                    let mut remove = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(4.0);
                        for f in &self.pending {
                            let (icon, note, color) = match &f.state {
                                FileState::Uploading => ("", "uploading…".to_string(), p.weak),
                                FileState::Ready(AttachmentData::Inline(_)) => ("📄", "added as text".to_string(), p.text),
                                FileState::Ready(AttachmentData::Uploaded(_)) => ("📎", "uploaded".to_string(), p.text),
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
                                    ui.label(RichText::new(&f.name).color(color)).on_hover_text(format!("{} – {note}", human_size(f.size)));
                                    if ui.add(egui::Button::new(RichText::new("✖").small().color(p.weak)).frame(false)).on_hover_text("Remove").clicked() {
                                        remove = Some(f.id);
                                    }
                                });
                            });
                        }
                    });
                    if let Some(id) = remove {
                        self.pending.retain(|f| f.id != id);
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(Layout::left_to_right(Align::Max), |ui| {
                    if theme::round_button(ui, theme::RoundIcon::Attach, true, 36.0)
                        .on_hover_text("Attach files (or drag them onto the window)")
                        .clicked()
                    {
                        self.pick_files();
                    }
                    let edit = egui::TextEdit::multiline(&mut self.input)
                        .id(input_id)
                        .frame(false)
                        .hint_text(RichText::new("Message…").color(p.weak))
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
                        if theme::round_button(ui, theme::RoundIcon::Stop, true, 36.0).on_hover_text("Stop the answer").clicked() {
                            self.stop();
                        }
                    } else {
                        let uploading = self.pending.iter().any(|f| matches!(f.state, FileState::Uploading));
                        let b = theme::round_button(ui, theme::RoundIcon::Send, self.can_send(), 36.0);
                        let b = if uploading { b.on_hover_text("Waiting for the upload to finish…") } else { b.on_hover_text("Send (Enter)") };
                        if b.clicked() {
                            send_now = true;
                        }
                    }
                });
            });
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Enter to send · Shift+Enter for a new line · AIS.chat can make mistakes").small().color(p.weak));
        });
        if send_now {
            self.send();
        }
    }

    fn chat(&mut self, ctx: &egui::Context) {
        let p = theme::palette(ctx.style().visuals.dark_mode);
        egui::CentralPanel::default().frame(egui::Frame::none().fill(p.bg)).show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                centered_column(ui, |ui| {
                    ui.add_space(18.0);
                    if self.chat_is_empty() {
                        self.welcome(ui, p);
                    }
                    let name = self.settings.display_name();
                    let last = self.messages.len().saturating_sub(1);
                    for (i, m) in self.messages.iter().enumerate() {
                        let waiting = self.busy && i == last && m.role == Role::Assistant && m.content.is_empty();
                        let activity = if self.busy && i == last { self.activity.as_str() } else { "" };
                        message_row(ui, m, &name, waiting, activity, p);
                        ui.add_space(14.0);
                    }
                    ui.add_space(8.0);
                });
            });
        });
    }

    /// Shown while the chat is still empty: who the bot is and a few ideas to start with.
    fn welcome(&mut self, ui: &mut egui::Ui, p: theme::Palette) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            theme::avatar(ui, &self.settings.display_name(), 64.0);
            ui.add_space(6.0);
            ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).size(22.0).color(p.text));
            ui.label(RichText::new("Ask a question, paste code, or attach files with + or by dragging them in.").color(p.weak));
            ui.add_space(10.0);
        });
        const IDEAS: [&str; 4] = [
            "Erkläre mir diesen Code Schritt für Schritt",
            "Hilf mir, meine App-Idee zu planen",
            "Finde den Fehler in meinem Programm",
            "Schreib mir ein kleines Python-Spiel",
        ];
        let mut picked = None;
        let cols = if ui.available_width() > 520.0 { 2 } else { 1 };
        for row in IDEAS.chunks(cols) {
            ui.columns(cols, |columns| {
                for (col, idea) in columns.iter_mut().zip(row) {
                    let w = col.available_width();
                    let b = egui::Button::new(RichText::new(*idea).color(p.text))
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .rounding(14.0)
                        .min_size(egui::vec2(w, 44.0));
                    if col.add(b).on_hover_text("Put this in the message box").clicked() {
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
            .fixed_size([520.0, 0.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").family(theme::bold()).size(20.0).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::ghost_button(ui, "✖").on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(6.0);

                section(ui, p, |ui| {
                    ui.label(RichText::new("Invite link").family(theme::bold()).color(p.text));
                    ui.label(RichText::new("Paste the link of the shared chat. Everything else is set up automatically.").small().color(p.weak));
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
                        let connect = theme::primary_button(ui, if self.connecting_from_settings { "Connecting…" } else { "Connect" });
                        if (connect.clicked() || pressed_enter) && !self.connecting_from_settings {
                            self.connect_from_settings();
                        }
                        if ui.add(egui::Button::new("Use class link").min_size(egui::vec2(0.0, 34.0))).on_hover_text(DEFAULT_INVITE_LINK).clicked() {
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

                if self.settings.is_ready() {
                    ui.add_space(8.0);
                    section(ui, p, |ui| {
                        ui.horizontal(|ui| {
                            theme::avatar(ui, &self.settings.display_name(), 32.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(self.settings.display_name()).family(theme::bold()).color(p.text));
                                let upload = if self.settings.upload_url.trim().is_empty() { "automatic" } else { "custom address" };
                                ui.label(RichText::new(format!("Connected · file upload {upload}")).small().color(p.weak))
                                    .on_hover_text(self.settings.upload_endpoint());
                            });
                        });
                    });
                }

                ui.add_space(8.0);
                section(ui, p, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Appearance").family(theme::bold()).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let current = self.settings.appearance.clone();
                            let mut choice = if current.is_empty() { "system".to_string() } else { current.clone() };
                            for (value, label) in [("dark", "Dark"), ("light", "Light"), ("system", "System")] {
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
                egui::CollapsingHeader::new(RichText::new("Advanced (only needed if something does not work)").color(p.weak)).show(ui, |ui| {
                    egui::Grid::new("adv").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        let auto_upload = {
                            let mut d = self.draft.clone();
                            d.upload_url.clear();
                            d.upload_endpoint()
                        };
                        ui.label("Upload address");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.upload_url).hint_text(format!("automatic: {auto_upload}")).desired_width(330.0))
                            .on_hover_text("Leave empty: files go to the address that belongs to the invite link.");
                        ui.end_row();
                        ui.label("next-action");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.next_action).desired_width(330.0));
                        ui.end_row();
                        ui.label("Deployment id");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.deployment_id).desired_width(330.0));
                        ui.end_row();
                        ui.label("User agent");
                        ui.add(egui::TextEdit::singleline(&mut self.draft.user_agent).hint_text("built in").desired_width(330.0));
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
                    ui.label(RichText::new("Import from a cURL").family(theme::bold()).color(p.text));
                    ui.label(
                        RichText::new(
                            "Only if Connect fails: in the browser press F12 → Network, send a message, right-click the \
                             dialog?inviteCode=… request → Copy → Copy as cURL, and paste it here.",
                        )
                        .small()
                        .color(p.weak),
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
                    ui.label(RichText::new(format!("Settings file: {}", Settings::path_display())).small().color(p.weak));
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::primary_button(ui, "Done").clicked() {
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
            painter.text(card.center() - egui::vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, "Drop files to attach", egui::FontId::new(22.0, theme::bold()), p.text);
            painter.text(card.center() + egui::vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, "PDFs, images, documents, code …", egui::FontId::proportional(14.0), p.weak);
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

fn message_row(ui: &mut egui::Ui, m: &ChatMessage, name: &str, waiting: bool, activity: &str, p: theme::Palette) {
    // gentle fade-in for new messages
    let t = (m.born.elapsed().as_secs_f32() / 0.25).min(1.0);
    if t < 1.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let max_w = (ui.available_width() * 0.8).max(220.0);

    match m.role {
        Role::User => {
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                ui.multiply_opacity(t);
                egui::Frame::none()
                    .fill(p.accent)
                    .rounding(Rounding { nw: 18.0, ne: 18.0, sw: 18.0, se: 6.0 })
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            let shown = m.display.as_deref().unwrap_or(&m.content);
                            if !shown.trim().is_empty() {
                                ui.label(RichText::new(shown).color(p.on_accent));
                            }
                            if !m.attachments.is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    for a in &m.attachments {
                                        egui::Frame::none()
                                            .fill(Color32::from_white_alpha(36))
                                            .rounding(999.0)
                                            .inner_margin(egui::Margin::symmetric(9.0, 3.0))
                                            .show(ui, |ui| {
                                                let icon = if matches!(a.data, AttachmentData::Inline(_)) { "📄" } else { "📎" };
                                                ui.label(RichText::new(format!("{icon} {}", a.name)).small().color(p.on_accent))
                                                    .on_hover_text(human_size(a.size));
                                            });
                                    }
                                });
                            }
                        });
                    });
            });
        }
        Role::Assistant => {
            ui.horizontal_top(|ui| {
                ui.multiply_opacity(t);
                theme::avatar(ui, name, 30.0);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    egui::Frame::none()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0_f32, p.border))
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
                            markdown::show(ui, m.display.as_deref().unwrap_or(&m.content), &m.id);
                            if !activity.is_empty() {
                                ui.horizontal(|ui| {
                                    theme::typing_dots(ui);
                                    ui.label(RichText::new(activity).small().color(p.weak));
                                });
                            }
                        });
                    if !m.initial && !waiting && !m.content.is_empty() {
                        let id = egui::Id::new(("copy-msg", &m.id));
                        let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
                        let now = ui.input(|i| i.time);
                        let recently = copied_at.map(|c| now - c < 1.5).unwrap_or(false);
                        let label = if recently { "✔ Copied" } else { "📋 Copy" };
                        let r = ui.add(egui::Button::new(RichText::new(label).small().color(p.weak)).frame(false));
                        if r.on_hover_text("Copy the whole answer").clicked() {
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
