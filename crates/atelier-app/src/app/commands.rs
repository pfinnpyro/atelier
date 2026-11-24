//! Running what the menus hold, on every platform: the macOS menu bar
//! and the menu button elsewhere both come here.

use eframe::egui::{self, Event, Key, Modifiers};

use super::annotate::Tool;
use super::settings_window::Window;
use super::{SaveTo, Viewer, ZoomMode};
use crate::commands::{APP, Command, Entry, MENUS, shortcut_text};

/// The key press an Edit item stands for, which egui's text boxes and the
/// viewer's own shortcuts already handle.
pub(super) fn key_press(key: Key, shift: bool) -> Event {
    let mut modifiers = Modifiers::COMMAND
        | if cfg!(target_os = "macos") {
            Modifiers::MAC_CMD
        } else {
            Modifiers::CTRL
        };
    modifiers.shift = shift;
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

impl Viewer {
    /// A shortcut written for this platform and the person's taste:
    /// `Mod+S` reads Ctrl+S, Cmd+S or ⌘S.
    pub(super) fn keys(&self, spec: &str) -> String {
        shortcut_text(spec, self.settings.shortcut_glyphs)
    }

    /// A tooltip with its shortcut: "Open a PDF (⌘O)".
    pub(super) fn tip(&self, text: &str, spec: &str) -> String {
        format!("{text} ({})", self.keys(spec))
    }

    /// Whether a menu item can be used now.
    pub(super) fn command_enabled(&self, c: Command) -> bool {
        let open = self.open.is_some();
        match c {
            Command::Highlight => open && self.selection.is_some_and(|s| !s.is_empty()),
            Command::Fields => self.open.as_ref().is_some_and(|o| o.form.is_some()),
            Command::AutoOrient => self.open.as_ref().is_some_and(|o| o.orienting.is_none()),
            Command::SideBySide => open && !self.organize.grid,
            Command::PreviousTab | Command::NextTab => self.tabs.len() > 1,
            Command::ClearRecent => !self.recent.files.is_empty(),
            Command::ReopenClosedTab => !self.closed_tabs.is_empty(),
            #[cfg(windows)]
            Command::ShowInExplorer => self.open.as_ref().is_some_and(|o| o.path.is_some()),
            c => open || !c.needs_document(),
        }
    }

    /// Whether a menu item shows a tick.
    pub(super) fn command_checked(&self, c: Command) -> bool {
        let form = self.open.as_ref().and_then(|o| o.form.as_ref());
        let tool = self.annotate.tool;
        match c {
            Command::Sidebar => self.settings.sidebar,
            Command::Comments => self.annotate.comments_open,
            Command::Inspector => self.inspector.open,
            Command::Fields => form.is_some_and(|f| f.highlight),
            Command::Colours => self.colors.open,
            Command::FitWidth => self.open.is_some() && self.zoom_mode == ZoomMode::FitWidth,
            Command::FitPage => self.open.is_some() && self.zoom_mode == ZoomMode::FitPage,
            Command::SideBySide => self.open.as_ref().is_some_and(|o| o.layout.facing_pages()),
            Command::Organize => self.organize.grid,
            Command::Comment => tool == Tool::Note,
            Command::Draw => tool == Tool::Draw,
            Command::InkSignature => tool == Tool::Sign,
            Command::DetectFields => self.reviewing_fields(),
            Command::Profiles => self.profiles.open,
            _ => false,
        }
    }

    pub(super) fn run_command(&mut self, c: Command) {
        if !self.command_enabled(c) {
            return;
        }
        let centre = self.view_size / 2.0;
        let pick_tool = |v: &mut Viewer, tool: Tool| {
            v.set_tool(if v.annotate.tool == tool {
                Tool::Select
            } else {
                tool
            });
        };
        let page = self.current_page();
        match c {
            Command::About => self.window = Some(Window::About),
            Command::Settings => self.window = Some(Window::Settings),
            Command::CheckUpdates => {
                self.updater
                    .check(self.settings.preview, false, self.ctx.clone());
                self.updater.dismissed = false;
                self.window = Some(Window::About);
            }
            Command::Quit => self.quit(),
            Command::Open => self.pick_file(),
            Command::OpenRecent(i) => {
                if let Some(path) = self.recent.files.get(i).cloned() {
                    self.open_recent(&path);
                }
            }
            Command::ClearRecent => self.recent.clear(),
            #[cfg(windows)]
            Command::ShowInExplorer => {
                if let Some(path) = self.open.as_ref().and_then(|o| o.path.clone()) {
                    self.reveal_path(&path);
                }
            }
            Command::ReopenClosedTab => self.reopen_closed_tab(),
            Command::CloseTab => {
                if let Some(active) = self.active_tab() {
                    self.request_close_tab(active);
                }
            }
            Command::Save => self.save(SaveTo::Overwrite),
            Command::SaveAs => self.save(SaveTo::As),
            Command::SaveCopy => self.save(SaveTo::Copy),
            Command::Encrypt => self.open_security(false),
            // Also where keys are kept, so it needs no document.
            Command::Protect => self.open_protect(),
            Command::Sign | Command::BackInk => self.open_security(true),
            Command::Print => self.open_print(),
            // Passed on as the key presses they stand for, so whatever has
            // the keys (a text box, the page's marks) takes them.
            Command::Undo => self.injected.push(key_press(Key::Z, false)),
            Command::Redo => self.injected.push(key_press(Key::Z, true)),
            Command::SelectAll => self.injected.push(key_press(Key::A, false)),
            Command::Cut => self.injected.push(Event::Cut),
            Command::Copy => self.injected.push(Event::Copy),
            Command::Paste => {
                if let Some(text) = super::menu::clipboard_text() {
                    self.injected.push(Event::Paste(text));
                }
            }
            Command::Find => self.request_find(),
            Command::FindNext => self.run_search(false),
            Command::FindPrevious => self.run_search(true),
            Command::Highlight => self.highlight_selection(),
            Command::Comment => pick_tool(self, Tool::Note),
            Command::Draw => pick_tool(self, Tool::Draw),
            Command::InkSignature => pick_tool(self, Tool::Sign),
            Command::DetectFields => {
                if self.reviewing_fields() {
                    self.close_detect();
                } else {
                    self.start_detect();
                }
            }
            Command::Profiles => self.toggle_profiles(),
            Command::Sidebar => self.toggle_sidebar(),
            Command::Comments => self.annotate.toggle_comments(),
            Command::Inspector => self.inspector.open = !self.inspector.open,
            Command::Fields => self.toggle_highlight(),
            Command::Colours => self.toggle_colors(),
            Command::ZoomIn => self.zoom_by(1.25, centre),
            Command::ZoomOut => self.zoom_by(0.8, centre),
            Command::ActualSize => {
                self.zoom_mode = ZoomMode::Custom;
                self.set_zoom(1.0, centre);
            }
            Command::FitWidth => {
                self.zoom_mode = ZoomMode::FitWidth;
                self.apply_fit();
            }
            Command::FitPage => {
                self.zoom_mode = ZoomMode::FitPage;
                self.apply_fit();
            }
            Command::SideBySide => self.toggle_facing(),
            Command::Organize => self.toggle_page_grid(),
            Command::PreviousPage => self.go_to_page(page.saturating_sub(1)),
            Command::NextPage => self.go_to_page(page + 1),
            Command::FirstPage => self.go_to_page(0),
            Command::LastPage => self.go_to_page(usize::MAX),
            Command::PreviousTab | Command::NextTab => {
                if let Some(active) = self.active_tab() {
                    let n = self.tabs.len();
                    let step = if c == Command::NextTab { 1 } else { n - 1 };
                    self.switch_to((active + step) % n);
                }
            }
            Command::RotateLeft => self.rotate_pages(-1),
            Command::RotateRight => self.rotate_pages(1),
            Command::AutoOrient => self.auto_orient(),
            Command::Website => {
                crate::update::open_page(&format!("https://github.com/{}", crate::update::REPO));
            }
            Command::ReleaseNotes => crate::update::open_page(&format!(
                "https://github.com/{}/releases",
                crate::update::REPO
            )),
            Command::ReportIssue => crate::update::open_page(&format!(
                "https://github.com/{}/issues/new",
                crate::update::REPO
            )),
        }
    }

    #[cfg(windows)]
    pub(super) fn reveal_path(&mut self, path: &std::path::Path) {
        if let Err(error) = show_in_explorer(path) {
            self.error = Some(format!(
                "Could not show {} in File Explorer: {error}",
                path.display()
            ));
        }
    }

    /// The menu behind the button at the start of the toolbar, for
    /// platforms without a menu bar: the same menus, as submenus.
    pub(super) fn main_menu(&mut self, ui: &mut egui::Ui) {
        let mut picked = None;
        let response = ui
            .menu_button(
                crate::icons::Icon::Menu
                    .image()
                    .tint(ui.visuals().text_color()),
                |ui| {
                    ui.set_min_width(190.0);
                    for def in MENUS {
                        ui.menu_button(def.title, |ui| {
                            self.menu_entries(ui, def.entries, &mut picked);
                        });
                    }
                    ui.separator();
                    self.menu_entries(ui, APP, &mut picked);
                },
            )
            .response
            .on_hover_text("Menu");
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Menu"));
        if let Some(c) = picked {
            self.run_command(c);
        }
    }

    pub(super) fn horizontal_menu(&mut self, ui: &mut egui::Ui) {
        let mut picked = None;
        egui::MenuBar::new().ui(ui, |ui| {
            for def in MENUS {
                ui.menu_button(def.title, |ui| {
                    self.menu_entries(ui, def.entries, &mut picked)
                });
            }
            ui.menu_button("Atelier", |ui| self.menu_entries(ui, APP, &mut picked));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::chrome::window_buttons(ui);
                crate::chrome::drag_space(ui);
            });
        });
        if let Some(command) = picked {
            self.run_command(command);
        }
    }

    fn menu_entries(&self, ui: &mut egui::Ui, entries: &[Entry], picked: &mut Option<Command>) {
        ui.set_min_width(230.0);
        for e in entries {
            match e {
                Entry::Separator => {
                    ui.separator();
                }
                Entry::Sub(title, inner) => {
                    ui.menu_button(*title, |ui| self.menu_entries(ui, inner, picked));
                }
                Entry::Recent => {
                    ui.menu_button("Open Recent", |ui| {
                        ui.set_min_width(230.0);
                        for (i, path) in self.recent.files.iter().enumerate() {
                            if ui
                                .button(super::display_name(path))
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                *picked = Some(Command::OpenRecent(i));
                                ui.close();
                            }
                        }
                        if self.recent.files.is_empty() {
                            ui.weak("No recent files");
                        } else {
                            ui.separator();
                            if ui.button("Clear Menu").clicked() {
                                *picked = Some(Command::ClearRecent);
                                ui.close();
                            }
                        }
                    });
                }
                Entry::Item(c, text, keys) | Entry::Check(c, text, keys) => {
                    let ticked = matches!(e, Entry::Check(..)) && self.command_checked(*c);
                    let button =
                        egui::Button::selectable(ticked, *text).shortcut_text(self.keys(keys));
                    if ui.add_enabled(self.command_enabled(*c), button).clicked() {
                        *picked = Some(*c);
                        ui.close();
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn show_in_explorer(path: &std::path::Path) -> std::io::Result<()> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;

    let path = std::path::absolute(path)?;
    let (target, select) = if path.is_file() {
        (path.as_path(), true)
    } else if let Some(folder) = path.parent().filter(|folder| folder.is_dir()) {
        (folder, false)
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "The containing folder is unavailable",
        ));
    };
    let target = explorer_path(target);
    if target.encode_wide().any(|unit| unit == u16::from(b'"')) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "File Explorer cannot open this path",
        ));
    }
    // Explorer reads comma-delimited switches from the raw command line.
    let mut argument = OsString::from(if select { "/select,\"" } else { "\"" });
    argument.push(target);
    argument.push("\"");
    std::process::Command::new("explorer.exe")
        .raw_arg(argument)
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn explorer_path(path: &std::path::Path) -> std::borrow::Cow<'_, std::ffi::OsStr> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Prefix};

    // Recent files are canonicalized with Win32's verbatim prefix.
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return path.as_os_str().into();
    };
    let (mut wide, skip) = match prefix.kind() {
        Prefix::VerbatimDisk(_) => (Vec::new(), 4),
        Prefix::VerbatimUNC(..) => (vec![u16::from(b'\\'); 2], 8),
        _ => return path.as_os_str().into(),
    };
    wide.extend(path.as_os_str().encode_wide().skip(skip));
    std::ffi::OsString::from_wide(&wide).into()
}
