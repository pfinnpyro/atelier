//! The Settings and About windows.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use atelier_ocr::models;
use eframe::egui::{self, RichText};

use super::Viewer;
use crate::default_app;
use crate::settings::Settings;

/// The OCR models on their way: bytes so far, and the outcome once done.
pub(super) struct Download {
    got: Arc<AtomicU64>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

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
            .default_width(500.0)
            .default_height(560.0)
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(18.0))
            .vscroll(true)
            .max_height(620.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| match which {
                Window::Settings => self.settings_ui(ui),
                Window::About => {
                    about_ui(ui);
                    ui.separator();
                    ui.heading("Updates");
                    self.updates_ui(ui);
                }
            });
        if !open {
            self.window = None;
        }
        if self.settings != before
            && let Err(e) = self.settings.save()
        {
            self.error = Some(format!("Could not save the settings: {e}"));
        }
        if self.settings.ocr && !before.ocr {
            self.read_scans_now();
        }
    }

    /// Starts fetching the OCR models on a thread of its own.
    fn start_download(&mut self) {
        let Some(dir) = Settings::ocr_models_dir() else {
            self.error = Some("There is no folder to keep the OCR models in.".into());
            return;
        };
        let d = Download {
            got: Arc::new(AtomicU64::new(0)),
            result: Arc::new(Mutex::new(None)),
        };
        let (got, result) = (d.got.clone(), d.result.clone());
        let wake = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("ocr-download".into())
            .spawn(move || {
                let r = models::download(&dir, &got);
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => self.ocr_download = Some(d),
            Err(e) => self.error = Some(format!("Could not start the download: {e}")),
        }
    }

    /// Checks on the download, and once the models are in, reads the open
    /// file's scans.
    pub(super) fn download_progress(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.ocr_download else { return };
        let done = d.result.lock().unwrap_or_else(|e| e.into_inner()).take();
        match done {
            None => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            Some(Ok(())) => {
                self.ocr_download = None;
                self.notice = Some("The OCR models are ready.".into());
                self.read_scans_now();
            }
            Some(Err(e)) => {
                self.ocr_download = None;
                self.error = Some(e);
            }
        }
    }

    /// Reads the open file's scans now that OCR can, turning them upright
    /// too if pages are turned on opening. The turns count as changes.
    fn read_scans_now(&mut self) {
        if self.open.is_none() || !self.ocr_ready() {
            return;
        }
        if self.settings.orient_on_open {
            self.start_orient(false);
        } else {
            self.start_ocr();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Appearance");
        egui::widgets::global_theme_preference_buttons(ui);
        s.theme = ui
            .ctx()
            .options(|o| match o.theme_preference {
                egui::ThemePreference::Light => "light",
                egui::ThemePreference::Dark => "dark",
                egui::ThemePreference::System => "system",
            })
            .into();
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Toolbar buttons");
        ui.horizontal(|ui| {
            ui.label("Set all");
            for (mode, label) in [
                (crate::settings::ButtonDisplay::IconOnly, "Icons"),
                (
                    crate::settings::ButtonDisplay::IconAndText,
                    "Icons and text",
                ),
                (crate::settings::ButtonDisplay::TextOnly, "Text"),
            ] {
                if ui.button(label).clicked() {
                    s.toolbar_display.fill(mode);
                }
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("toolbar-button-display")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (index, label) in crate::settings::TOOLBAR_ACTIONS.iter().enumerate() {
                    ui.label(*label);
                    egui::ComboBox::from_id_salt(("toolbar-display", index))
                        .selected_text(match s.toolbar_display[index] {
                            crate::settings::ButtonDisplay::IconOnly => "Icon only",
                            crate::settings::ButtonDisplay::IconAndText => "Icon and text",
                            crate::settings::ButtonDisplay::TextOnly => "Text only",
                        })
                        .show_ui(ui, |ui| {
                            for (mode, label) in [
                                (crate::settings::ButtonDisplay::IconOnly, "Icon only"),
                                (crate::settings::ButtonDisplay::IconAndText, "Icon and text"),
                                (crate::settings::ButtonDisplay::TextOnly, "Text only"),
                            ] {
                                ui.selectable_value(&mut s.toolbar_display[index], mode, label);
                            }
                        });
                    ui.end_row();
                }
            });
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Review identity");
        ui.text_edit_singleline(&mut s.review_author)
            .on_hover_text("Saved as the author of new PDF comments");
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
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
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Scans");
        ui.checkbox(&mut s.ocr, "Read the text of scans (OCR)")
            .on_hover_text(
                "Reads the words in scanned pages so you can search and copy them, \
                 and uses them to turn the pages upright. Runs on this computer.",
            );
        self.ocr_models_ui(ui);
        let s = &mut self.settings;
        ui.add_enabled(
            s.ocr,
            egui::Checkbox::new(&mut s.ocr_save, "Save the text it reads into the file"),
        )
        .on_hover_text(
            "When you save, the words are added as invisible text, the way scanners \
                 make searchable PDFs, so other apps can search and copy them too.",
        );
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Updates");
        let s = &mut self.settings;
        ui.checkbox(
            &mut s.check_updates,
            "Check for updates when Atelier starts",
        )
        .on_hover_text("Asks GitHub for the list of releases; nothing else is sent.");
        ui.horizontal(|ui| {
            ui.label("Channel");
            ui.selectable_value(&mut s.preview, false, "Stable");
            ui.selectable_value(&mut s.preview, true, "Preview")
                .on_hover_text("Also offer test versions before they are released");
        });
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Default app");
        ui.label(default_app::guidance());
        if cfg!(target_os = "macos") {
            ui.hyperlink_to(
                "Apple's default-app guide",
                "https://support.apple.com/guide/mac-help/mh35597/mac",
            );
            if ui.button("Open PDF default-app controls…").clicked() {
                let path = self.open.as_ref().and_then(|o| o.path.clone()).or_else(|| {
                    rfd::FileDialog::new()
                        .add_filter("PDF documents", &["pdf"])
                        .pick_file()
                });
                if let Some(ref path) = path
                    && let Err(e) = default_app::reveal_pdf(path)
                {
                    self.error = Some(e);
                } else if path.is_some() {
                    self.notice = Some("In Get Info, expand Open with, choose Atelier, then click Change All… and Continue.".into());
                }
            }
        }
        if default_app::supported() && ui.button("Make Atelier the default PDF app…").clicked() {
            match default_app::offer() {
                Ok(msg) => self.notice = Some(msg.into()),
                Err(e) => self.error = Some(e),
            }
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Ink signatures");
        self.ink_settings_ui(ui);
    }
}

impl Viewer {
    /// Whether the OCR models are in, with a button to fetch them if not.
    fn ocr_models_ui(&mut self, ui: &mut egui::Ui) {
        let total = models::total_size();
        let mb = |b: u64| b as f64 / 1_000_000.0;
        ui.indent("ocr-models", |ui| {
            if let Some(d) = &self.ocr_download {
                let got = d.got.load(Ordering::SeqCst).min(total);
                ui.add(
                    egui::ProgressBar::new(got as f32 / total as f32).text(format!(
                        "Downloading: {:.1} of {:.1} MB",
                        mb(got),
                        mb(total)
                    )),
                );
            } else if Settings::ocr_models_dir().is_some_and(|d| models::installed(&d)) {
                ui.label(RichText::new("✔ OCR models installed").weak());
            } else {
                ui.label(format!(
                    "OCR needs two model files ({:.0} MB), fetched once from the ocrs project \
                     and checked against fixed checksums.",
                    mb(total)
                ));
                if ui.button("Download OCR models").clicked() {
                    self.start_download();
                }
            }
        });
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
        "Scripts in PDF files run only when you allow them, in a sandbox that reaches \
         nothing but the form's fields. The inspector (F12) shows every script and \
         action a file carries.",
    );
}
