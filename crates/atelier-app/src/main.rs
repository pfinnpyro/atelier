//! Atelier, a PDF viewer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod worker;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Atelier")
            .with_inner_size([1100.0, 800.0])
            .with_min_inner_size([360.0, 240.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Atelier",
        options,
        Box::new(move |cc| Ok(Box::new(app::Viewer::new(cc, path)))),
    )
}
