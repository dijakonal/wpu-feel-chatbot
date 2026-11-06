//! AIS.chat Client – a small native chat window for one shared AIS.chat character.

// No console window next to the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ais;
mod app;
mod conversations;
mod markdown;
mod settings;
mod theme;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("AIS.chat")
            .with_inner_size([1060.0, 740.0])
            .with_min_inner_size([420.0, 400.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("ais-chat-client", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
