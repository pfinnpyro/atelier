//! Atelier, a PDF viewer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
#[cfg(all(feature = "fips", windows))]
mod cng;
mod commands;
mod default_app;
mod icons;
mod instance;
mod keys;
#[cfg(target_os = "macos")]
mod macos;
mod memory;
#[cfg(target_os = "macos")]
mod menu_bar;
mod profiles;
mod recent;
mod sealing;
mod settings;
mod update;
mod worker;

use ::image;
use std::path::PathBuf;
use std::sync::Arc;

/// The argument that starts Atelier as the helper that runs form scripts.
pub const SCRIPT_HELPER: &str = "--form-script";

/// The files Atelier was started with. Old macOS versions pass a process
/// serial number to apps that Finder starts; it is not a file.
fn launch_paths(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<PathBuf> {
    args.into_iter()
        .filter(|a| !a.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from)
        .collect()
}

fn main() -> eframe::Result {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == SCRIPT_HELPER)
    {
        script_helper();
        return Ok(());
    }
    let paths = launch_paths(std::env::args_os().skip(1));
    let primary = match instance::claim() {
        instance::Claim::Primary(p) => Some(p),
        // File launches open tabs. An empty launch requests another window.
        instance::Claim::Taken if instance::hand_over(&paths) => {
            return Ok(());
        }
        _ => None,
    };
    let icon = Arc::new(load_icon());
    #[cfg(target_os = "macos")]
    macos::listen_for_apple_events();
    let options = eframe::NativeOptions {
        viewport: chrome::window(Some(icon.clone())),
        ..Default::default()
    };
    eframe::run_native(
        "Atelier",
        options,
        Box::new(move |cc| Ok(Box::new(app::Viewer::new(cc, paths, primary, Some(icon))))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_file() {
        let paths = launch_paths(["a.pdf", "b.pdf", "-psn_0_1"].map(Into::into));
        assert_eq!(paths, [PathBuf::from("a.pdf"), PathBuf::from("b.pdf")]);
    }
}
