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
        // A window of the system's own, with its title bar and controls,
        // sized to its contents.
        let size = match which {
            Window::Settings => egui::vec2(700.0, 480.0),
            Window::About => egui::vec2(520.0, 440.0),
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_resizable(which == Window::Settings)
            .with_minimize_button(false)
            .with_maximize_button(false);
        let id = egui::ViewportId::from_hash_of(("atelier-window", title));
        ctx.show_viewport_immediate(id, builder, |ui, _class| {
            let close = ui.ctx().input(|i| {
                i.viewport().close_requested()
                    || (i.key_pressed(egui::Key::Escape) && !ui.ctx().text_edit_focused())
            });
            if close {
                open = false;
            }
            egui::CentralPanel::default().show(ui, |ui| match which {
                Window::Settings => self.settings_ui(ui),
                Window::About => {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            about_ui(ui);
                            ui.separator();
                            ui.heading("Updates");
                            self.updates_ui(ui);
                        });
                }
            });
        });
        if !open {
            self.profiles.settings_closed(ctx);
            self.window = None;
        }
        if self.settings != before
            && let Err(e) = self.settings.save()
        {
            self.error = Some(format!("Could not save the settings: {e}"));
        }
        if self.settings.memory != before.memory {
            self.apply_budgets();
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

    /// Settings: the kinds of setting down the side, the chosen kind's
    /// settings beside them.
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        // The list keeps its width; the pane takes what the window has
        // left, so nothing reaches past the window's edge.
        const LIST: f32 = 170.0;
        let space = ui.available_size();
        let gap = ui.spacing().item_spacing.x * 2.0 + 6.0;
        let (pane, height) = ((space.x - LIST - gap).max(240.0), space.y);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(LIST, height),
                egui::Layout::top_down_justified(egui::Align::Min),
                |ui| {
                    ui.set_width(LIST);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for pane in Pane::ALL {
                        let chosen = self.settings_pane == pane;
                        let label = egui::Button::selectable(chosen, pane.label())
                            .min_size(egui::vec2(LIST, 30.0));
                        if ui.add(label).clicked() {
                            self.settings_pane = pane;
                        }
                    }
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(pane, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings-pane")
                        .max_width(pane)
                        .max_height(height)
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            // Room for the scroll bar; text wraps inside.
                            ui.set_width(pane - 14.0);
                            ui.spacing_mut().item_spacing.y = 9.0;
                            match self.settings_pane {
                                Pane::General => self.general_settings(ui),
                                Pane::Appearance => self.appearance_settings(ui),
                                Pane::Forms => self.forms_settings(ui),
                                Pane::Profiles => self.profiles.manage_ui(ui),
                                Pane::Signatures => {
                                    ui.heading("Ink signatures");
                                    self.ink_settings_ui(ui);
                                }
                                Pane::Performance => self.performance_settings(ui),
                                Pane::Updates => self.update_settings(ui),
                            }
                        });
                },
            );
        });
    }

    fn general_settings(&mut self, ui: &mut egui::Ui) {
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
        section(ui, "Find");
        ui.horizontal(|ui| {
            ui.label("Find opens");
            ui.selectable_value(&mut s.find_floating, true, "As a bar over the page");
            ui.selectable_value(&mut s.find_floating, false, "In the sidebar");
        });
        section(ui, "Review identity");
        ui.horizontal(|ui| {
            ui.label("Name on new comments");
            ui.text_edit_singleline(&mut s.review_author)
                .on_hover_text("Saved as the author of new PDF comments");
        });
        section(ui, "Default app");
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
    }

    fn appearance_settings(&mut self, ui: &mut egui::Ui) {
        use crate::settings::ButtonDisplay;
        let s = &mut self.settings;
        ui.heading("Theme");
        egui::widgets::global_theme_preference_buttons(ui);
        s.theme = ui
            .ctx()
            .options(|o| match o.theme_preference {
                egui::ThemePreference::Light => "light",
                egui::ThemePreference::Dark => "dark",
                egui::ThemePreference::System => "system",
            })
            .into();
        section(ui, "Keyboard shortcuts");
        ui.checkbox(&mut s.shortcut_glyphs, "Show shortcuts as key symbols")
            .on_hover_text("In menus and tips: symbols such as ⇧ and ⌘ rather than key names");
        ui.weak(format!(
            "Save As reads {}",
            crate::commands::shortcut_text("Mod+Shift+S", s.shortcut_glyphs)
        ));
        section(ui, "Toolbar buttons");
        const MODES: [(ButtonDisplay, &str); 3] = [
            (ButtonDisplay::IconOnly, "Icons"),
            (ButtonDisplay::IconAndText, "Icons and text"),
            (ButtonDisplay::TextOnly, "Text"),
        ];
        ui.horizontal(|ui| {
            ui.label("Show all as");
            let all = s.toolbar_display[0];
            let same = s.toolbar_display.iter().all(|d| *d == all);
            for (mode, label) in MODES {
                if ui.selectable_label(same && all == mode, label).clicked() {
                    s.toolbar_display.fill(mode);
                }
            }
        });
        egui::CollapsingHeader::new("Choose for each button")
            .id_salt("toolbar-each")
            .show(ui, |ui| {
                egui::Grid::new("toolbar-button-display")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for (index, label) in crate::settings::TOOLBAR_ACTIONS.iter().enumerate() {
                            ui.label(*label);
                            ui.horizontal(|ui| {
                                for (mode, label) in MODES {
                                    ui.selectable_value(&mut s.toolbar_display[index], mode, label);
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
    }

    fn forms_settings(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Forms");
        ui.horizontal(|ui| {
            ui.label("Ticked check boxes show a");
            egui::ComboBox::from_id_salt("check-mark")
                .selected_text(s.check_mark.label())
                .show_ui(ui, |ui| {
                    for mark in atelier_form::CheckMark::ALL {
                        ui.selectable_value(&mut s.check_mark, mark, mark.label());
                    }
                });
        })
        .response
        .on_hover_text(
            "For check boxes Atelier adds. A document can keep a mark of its own, and \
             each check box can be given another (Find tables and form fields).",
        );
        section(ui, "Scans");
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
    }

    fn performance_settings(&mut self, ui: &mut egui::Ui) {
        use crate::memory::{Budgets, MemoryUse, host_memory};
        ui.heading("Memory");
        ui.label(
            "Drawn pages and decoded images are kept so scrolling and zooming need not \
             draw them again. More memory keeps more of them.",
        );
        ui.horizontal(|ui| {
            for mode in MemoryUse::ALL {
                ui.selectable_value(&mut self.settings.memory, mode, mode.label());
            }
        });
        ui.weak(format!(
            "Now: {}.",
            Budgets::of(self.settings.memory).describe()
        ));
        let found = match host_memory() {
            Some(bytes) => format!(
                "Automatic goes by this computer's {:.0} GB of memory.",
                bytes as f64 / (1u64 << 30) as f64
            ),
            None => "Automatic assumes 8 GB of memory on this system.".to_string(),
        };
        ui.weak(found);
        ui.weak(
            "Files in background tabs keep only a few drawn pages around where you \
             left them, whatever is chosen here.",
        );
    }

    fn update_settings(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Updates");
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
        ui.add_space(4.0);
        self.updates_ui(ui);
    }
}

/// A heading that starts a new group of settings.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.separator();
    ui.heading(title);
}

/// The kinds of setting, listed down the side of Settings.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Pane {
    #[default]
    General,
    Appearance,
    Forms,
    Profiles,
    Signatures,
    Performance,
    Updates,
}

impl Pane {
    const ALL: [Pane; 7] = [
        Pane::General,
        Pane::Appearance,
        Pane::Forms,
        Pane::Profiles,
        Pane::Signatures,
        Pane::Performance,
        Pane::Updates,
    ];

    fn label(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Appearance => "Appearance",
            Pane::Forms => "Forms and scans",
            Pane::Profiles => "Form profiles",
            Pane::Signatures => "Signatures",
            Pane::Performance => "Performance",
            Pane::Updates => "Updates",
        }
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
    if cfg!(feature = "fips") {
        ui.label(format!(
            "FIPS build: form profiles are encrypted by {}.",
            crate::sealing::MODULE
        ));
    }
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
