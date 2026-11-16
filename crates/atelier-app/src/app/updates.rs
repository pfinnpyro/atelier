//! Updates in the window: a check when Atelier starts, a banner when a new
//! version is out, and the controls in About.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, ViewportCommand};

use super::Viewer;
use crate::update::{self, Installed, Release};

#[derive(Clone)]
pub(super) enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, Arc<AtomicU64>),
    Ready(Installed),
    Failed(String),
}

pub(super) struct Updater {
    status: Arc<Mutex<Status>>,
    /// The banner for a new version was closed.
    pub(super) dismissed: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(Status::Idle)),
            dismissed: false,
        }
    }
}

impl Updater {
    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set(status: &Mutex<Status>, s: Status) {
        *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Looks for a newer release on a thread of its own. A `quiet` check
    /// (at start) stays silent unless it finds one.
    pub fn check(&self, preview: bool, quiet: bool, wake: egui::Context) {
        let status = self.status.clone();
        if !quiet {
            Self::set(&status, Status::Checking);
        }
        let _ = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let found = update::fetch_releases()
                    .map(|rs| update::newest_for_platform(&rs, preview, &update::current()));
                let s = match found {
                    Ok(Some(r)) => Status::Available(r),
                    Ok(None) if quiet => Status::Idle,
                    Ok(None) => Status::UpToDate,
                    Err(_) if quiet => Status::Idle,
                    Err(e) => Status::Failed(e),
                };
                Self::set(&status, s);
                wake.request_repaint();
            });
    }

    fn install(&self, release: Release, wake: egui::Context) {
        let got = Arc::new(AtomicU64::new(0));
        let status = self.status.clone();
        Self::set(&status, Status::Downloading(release.clone(), got.clone()));
        let _ = std::thread::Builder::new()
            .name("update-install".into())
            .spawn(move || {
                let s = match update::install(&release, &got) {
                    Ok(done) => Status::Ready(done),
                    Err(e) => Status::Failed(e),
                };
                Self::set(&status, s);
                wake.request_repaint();
            });
    }
}

fn size_of(release: &Release) -> u64 {
    if cfg!(target_os = "linux") {
        return update::linux_asset_name()
            .and_then(|name| release.asset(name))
            .map(|asset| asset.2)
            .unwrap_or(1)
            .max(1);
    }
    if let Some(asset) = release.asset("atelier-macos-arm64.zip")
        && cfg!(target_os = "macos")
    {
        return asset.2.max(1);
    }
    // The program and the installer are much the same size; either names
    // the download well enough for a progress bar.
    release
        .assets
        .iter()
        .filter(|a| a.0.ends_with(".exe"))
        .map(|a| a.2)
        .max()
        .unwrap_or(0)
        .max(1)
}

impl Viewer {
    /// The bar along the bottom when a new version is out, and quitting
    /// once the installer has taken over.
    /// Follows an update on its way in: keeps the progress moving, and
    /// closes the window once the installer has taken over.
    pub(super) fn update_tick(&mut self, ctx: &egui::Context) {
        let status = self.updater.status();
        if matches!(status, Status::Ready(Installed::InstallerRunning)) {
            self.closing = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if let Status::Downloading(_, _) = status {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// The update symbol in the toolbar's status area, shown while a
    /// newer version is on offer. It opens what the update is and what
    /// to do about it; nothing covers the page.
    pub(super) fn update_tray(&mut self, ui: &mut egui::Ui) {
        let Status::Available(release) = self.updater.status() else {
            return;
        };
        if self.updater.dismissed {
            return;
        }
        let accent = ui.visuals().hyperlink_color;
        let button = egui::Button::new(crate::icons::Icon::Update.image().tint(accent))
            .min_size(egui::Vec2::splat(30.0));
        let response = ui
            .add(button)
            .on_hover_text(format!("Atelier {} is available", release.version));
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(220.0);
            ui.label(RichText::new(format!("Atelier {} is available.", release.version)).strong());
            if ui.button("What's new").clicked() {
                self.window = Some(super::settings_window::Window::About);
                ui.close();
            }
            if update::supported() {
                let dirty = self.update_has_unsaved_work();
                if ui
                    .add_enabled(!dirty, egui::Button::new("Update now"))
                    .on_disabled_hover_text("Save your changes first.")
                    .clicked()
                {
                    self.updater.install(release.clone(), self.ctx.clone());
                    self.window = Some(super::settings_window::Window::About);
                    ui.close();
                }
            } else if ui.button("Download").clicked() {
                update::open_page(&release.page);
                ui.close();
            }
            if ui.button("Not now").clicked() {
                self.updater.dismissed = true;
                ui.close();
            }
        });
    }

    /// The Updates part of About.
    pub(super) fn updates_ui(&mut self, ui: &mut egui::Ui) {
        let channel = if self.settings.preview {
            "preview"
        } else {
            "stable"
        };
        ui.label(format!("You are on the {channel} channel."));
        let status = self.updater.status();
        let busy = matches!(status, Status::Checking | Status::Downloading(..));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Check for updates"))
                .clicked()
            {
                self.updater
                    .check(self.settings.preview, false, self.ctx.clone());
            }
            if ui.link("All releases").clicked() {
                update::open_page(&format!("https://github.com/{}/releases", update::REPO));
            }
        });
        match status {
            Status::Idle => {}
            Status::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking GitHub…");
                });
            }
            Status::UpToDate => {
                ui.label("✔ You have the latest version.");
            }
            Status::Failed(e) => {
                ui.colored_label(egui::Color32::from_rgb(230, 120, 100), e);
            }
            Status::Available(release) => {
                ui.label(RichText::new(format!("Atelier {} is out.", release.version)).strong());
                notes(ui, &release);
                ui.horizontal(|ui| {
                    if update::supported() {
                        let dirty = self.update_has_unsaved_work();
                        if ui
                            .add_enabled(!dirty, egui::Button::new("Download and install"))
                            .on_disabled_hover_text("Save your changes first.")
                            .clicked()
                        {
                            self.updater.install(release.clone(), self.ctx.clone());
                        }
                    }
                    if ui.button("Open the release page").clicked() {
                        update::open_page(&release.page);
                    }
                });
            }
            Status::Downloading(release, got) => {
                let total = size_of(&release);
                let got = got.load(Ordering::SeqCst).min(total);
                ui.add(
                    egui::ProgressBar::new(got as f32 / total as f32).text(format!(
                        "Downloading {}: {:.1} of {:.1} MB",
                        release.version,
                        got as f64 / 1e6,
                        total as f64 / 1e6
                    )),
                );
            }
            Status::Ready(Installed::Swapped) => {
                ui.label("The new version is in place.");
                let dirty = self.update_has_unsaved_work();
                if ui
                    .add_enabled(!dirty, egui::Button::new("Restart Atelier"))
                    .on_disabled_hover_text("Save your changes first.")
                    .clicked()
                {
                    let file = self.open.as_ref().and_then(|o| o.path.clone());
                    match update::restart(file.as_deref()) {
                        Ok(()) => {
                            self.closing = true;
                            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        }
                        Err(e) => self.error = Some(format!("Could not restart: {e}")),
                    }
                }
            }
            Status::Ready(Installed::InstallerRunning) => {
                ui.label("The installer is updating Atelier…");
            }
        }
    }
}

/// The first lines of a release's notes.
fn notes(ui: &mut egui::Ui, release: &Release) {
    let text: String = release
        .notes
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .show(ui, |ui| ui.label(RichText::new(text).small()));
}
