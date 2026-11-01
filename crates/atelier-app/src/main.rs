//! Atelier, a PDF viewer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
mod default_app;
mod recent;
mod settings;
mod update;
mod worker;

use ::image;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> eframe::Result {
    let icon = load_icon();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: chrome::viewport(
            eframe::egui::ViewportBuilder::default()
                .with_title("Atelier")
                .with_inner_size([1100.0, 800.0])
                .with_min_inner_size([360.0, 240.0])
                .with_drag_and_drop(true)
                .with_icon(Arc::new(icon)),
        ),
        ..Default::default()
    };
    eframe::run_native(
        "Atelier",
        options,
        Box::new(move |cc| Ok(Box::new(app::Viewer::new(cc, path)))),
    )
}

fn load_icon() -> eframe::egui::IconData {
    let bytes = include_bytes!("../assets/atelier.png");
    let l = image::load_from_memory(bytes)
        .expect("could not decode icon")
        .into_rgba8();

    let (width, height) = l.dimensions();

    eframe::egui::IconData {
        rgba: l.into_raw(),
        width,
        height,
    }
}
