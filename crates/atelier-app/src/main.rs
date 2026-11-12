//! Atelier, a PDF viewer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
mod default_app;
mod icons;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod menu_bar;
mod recent;
mod settings;
mod update;
mod worker;

use ::image;
use std::path::PathBuf;
use std::sync::Arc;

/// The argument that starts Atelier as the helper that runs form scripts.
pub const SCRIPT_HELPER: &str = "--form-script";

fn main() -> eframe::Result {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == SCRIPT_HELPER)
    {
        script_helper();
        return Ok(());
    }
    let icon = load_icon();
    // Old macOS versions pass a process serial number to apps that Finder
    // starts; it is not a file.
    let path = std::env::args_os()
        .nth(1)
        .filter(|a| !a.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from);
    #[cfg(target_os = "macos")]
    macos::listen_for_apple_events();
    let options = eframe::NativeOptions {
        viewport: chrome::viewport(
            eframe::egui::ViewportBuilder::default()
                .with_title("Atelier")
                .with_app_id("org.atelier.Viewer")
                .with_inner_size([1100.0, 800.0])
                // Keep the viewer chrome usable, including at high display scaling.
                .with_min_inner_size([800.0, 600.0])
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

/// Reads a commit from standard input, runs its scripts in the sandbox and
/// writes what they did to standard output. Runs in its own process so a
/// script that crashes or exhausts memory cannot take the viewer down.
fn script_helper() {
    use std::io::{Read, Write};
    let mut input = Vec::new();
    if std::io::stdin()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut input)
        .is_err()
    {
        return;
    }
    let Some(commit) = atelier_script::decode_commit(&input) else {
        return;
    };
    let outcome = atelier_script::run_commit(&commit);
    let mut out = std::io::stdout();
    let _ = out.write_all(&atelier_script::encode_outcome(&outcome));
    let _ = out.flush();
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
