//! The Settings and About windows.

use eframe::egui::{self, RichText};

use super::Viewer;
use crate::default_app;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Window {
    Settings,
    About,
}

impl Viewer {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(which) = self.window else { return };
        let mut open = true;
        let title = match which {
            Window::Settings => "Settings",
            Window::About => "About Atelier",
        };
        let before = self.settings.clone();
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| match which {
                Window::Settings => self.settings_ui(ui),
                Window::About => about_ui(ui),
            });
        if !open {
            self.window = None;
        }
        if self.settings != before
            && let Err(e) = self.settings.save()
        {
            self.error = Some(format!("Could not save the settings: {e}"));
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Opening files");
        ui.horizontal(|ui| {
            ui.label("Start at");
            ui.selectable_value(&mut s.fit_page, true, "Fit page");
            ui.selectable_value(&mut s.fit_page, false, "Fit width");
        });
        ui.checkbox(
            &mut s.orient_on_open,
            "Turn sideways and upside-down pages upright",
        )
        .on_hover_text(
            "Uses the page's text, or for scans the look of the page. The turns are \
                 saved only if you save the file.",
        );
        ui.add_space(6.0);
        ui.heading("Scans");
        ui.checkbox(&mut s.ocr, "Read the text of scans (OCR)")
            .on_hover_text("Lets you search and copy text in scanned pages.");
        ui.add_space(6.0);
        ui.heading("Default app");
        if default_app::supported() {
            ui.label("Windows asks you to choose the default PDF app yourself.");
            if ui.button("Make Atelier the default PDF app…").clicked() {
                match default_app::offer() {
                    Ok(msg) => self.notice = Some(msg.into()),
                    Err(e) => self.error = Some(e),
                }
            }
        } else {
            ui.label("Choose the default PDF app in your desktop's settings.");
        }
    }
}

fn about_ui(ui: &mut egui::Ui) {
    ui.heading(RichText::new("Atelier").size(22.0));
    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
    ui.add_space(6.0);
    ui.label(
        "A PDF viewer, form filler and inspector, written from the ground up in Rust: \
         its own parser, fonts, renderer and writer.",
    );
    ui.add_space(6.0);
    ui.label(
        "Scripts in PDF files are never run. The inspector (F12) shows every script and \
         action a file carries.",
    );
}
