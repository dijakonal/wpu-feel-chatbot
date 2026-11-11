//! AIS.chat Client – a small native chat window for one shared AIS.chat character.

// No console window next to the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ais;
mod app;
mod backend;
mod conversations;
mod github;
mod i18n;
mod markdown;
mod merge;
mod models;
mod moodle_login;
mod secret;
mod settings;
mod sync;
mod theme;
mod ui_text;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("AIS.chat")
            .with_inner_size([1060.0, 740.0])
            .with_min_inner_size([420.0, 400.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        // With vsync, drawing waits for the display. On Wayland that wait never ends while the
        // window is hidden (another workspace, minimised), so a redraw in the background – a
        // sync, an arriving answer – froze the app and the desktop reported "not responding".
        // The app paces its drawing itself instead (see `App::update`).
        vsync: false,
        ..Default::default()
    };
    eframe::run_native("ais-chat-client", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
