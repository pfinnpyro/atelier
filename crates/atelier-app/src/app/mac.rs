//! The viewer's side of the macOS menu bar and of files opened from Finder.

use std::path::PathBuf;

use eframe::egui::{self, Event, Key, Modifiers, ViewportCommand};

use super::annotate::Tool;
use super::settings_window::Window;
use super::{SaveTo, Viewer, ZoomMode};
use crate::macos::Incoming;
use crate::menu_bar::{Command, MenuBar, MenuState};

pub(super) struct Mac {
    menu_bar: Option<MenuBar>,
    /// Picked from the menu bar, to run this frame.
    commands: Vec<Command>,
    /// Sent by Finder, to open this frame.
    opened: Vec<PathBuf>,
    /// How much of the toolbar's start the traffic lights take, in points.
    pub(super) traffic_light_width: f32,
}

impl Default for Mac {
    fn default() -> Self {
        Self {
            menu_bar: None,
            commands: Vec::new(),
            opened: Vec::new(),
            traffic_light_width: TRAFFIC_LIGHT_WIDTH,
        }
    }
}

impl Mac {
    pub(super) fn has_menu_bar(&self) -> bool {
        self.menu_bar.is_some()
    }
}

/// The traffic lights' width with their margins, until they are measured.
const TRAFFIC_LIGHT_WIDTH: f32 = 78.0;
/// From the window's edge to the close button.
const TRAFFIC_LIGHT_MARGIN: f32 = 14.0;

/// The key press an Edit item stands for, which egui's text boxes and the
/// viewer's own shortcuts already handle.
fn key_press(key: Key, shift: bool) -> Event {
    let mut modifiers = Modifiers::COMMAND | Modifiers::MAC_CMD;
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
    /// Puts up the menu bar and lets Finder wake the viewer.
    pub(super) fn start_mac(&mut self) {
        crate::macos::set_waker(&self.ctx);
        self.mac.menu_bar = MenuBar::install();
    }

    /// Sorts out what arrived since the last frame, before egui sees the
    /// frame's input. Edit items become the key presses they stand for.
    pub(super) fn mac_input(&mut self, raw: &mut egui::RawInput) {
        for item in crate::macos::take() {
            match item {
                Incoming::Open(path) => self.mac.opened.push(path),
                Incoming::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
                Incoming::Menu(id) => match Command::from_id(&id) {
                    Some(Command::Undo) => raw.events.push(key_press(Key::Z, false)),
                    Some(Command::Redo) => raw.events.push(key_press(Key::Z, true)),
                    Some(Command::SelectAll) => raw.events.push(key_press(Key::A, false)),
                    Some(Command::Cut) => raw.events.push(Event::Cut),
                    Some(Command::Copy) => raw.events.push(Event::Copy),
                    Some(Command::Paste) => {
                        if let Some(text) = super::menu::clipboard_text() {
                            raw.events.push(Event::Paste(text));
                        }
                    }
                    Some(c) => self.mac.commands.push(c),
                    None => {}
                },
            }
        }
    }

    /// Opens what Finder sent and runs what was picked from the menu bar.
    pub(super) fn mac_frame(&mut self) {
        for path in std::mem::take(&mut self.mac.opened) {
            self.open_path(&path);
        }
        let commands = std::mem::take(&mut self.mac.commands);
        // While the unsaved-changes prompt is up, only quitting goes through,
        // as with the keyboard shortcuts.
        for c in commands {
            if self.confirm.is_none() || c == Command::Quit {
                self.run_command(c);
            }
        }
        let state = self.menu_state();
        if let Some(bar) = &mut self.mac.menu_bar {
            bar.sync(state);
        }
    }

    /// Centres the traffic lights on the toolbar's first row, and measures
    /// them so the toolbar starts after them.
    pub(super) fn place_traffic_lights(&mut self, frame: &eframe::Frame, row: egui::Rangef) {
        if !crate::chrome::integrated_mac() {
            return;
        }
        frame.set_traffic_lights_position(&self.ctx, row, TRAFFIC_LIGHT_MARGIN);
        use raw_window_handle::HasWindowHandle;
        let measured = frame
            .window_handle()
            .ok()
            .and_then(|h| eframe::WindowChromeMetrics::from_window_handle(&h.as_raw()))
            .map(|m| m.traffic_lights_size.x / self.ctx.zoom_factor());
        if let Some(width) = measured.filter(|w| w.is_finite() && *w > 0.0)
            && (width - self.mac.traffic_light_width).abs() > 0.5
        {
            self.mac.traffic_light_width = width;
            self.ctx.request_repaint();
        }
    }

    fn menu_state(&self) -> MenuState {
        let form = self.open.as_ref().and_then(|o| o.form.as_ref());
        let facing = self.open.as_ref().is_some_and(|o| o.layout.facing_pages());
        let tool = self.annotate.tool;
        MenuState {
            open: self.open.is_some(),
            selection: self.selection.is_some_and(|s| !s.is_empty()),
            has_form: form.is_some(),
            orienting: self.open.as_ref().is_some_and(|o| o.orienting.is_some()),
            checked: vec![
                (Command::Sidebar, self.settings.sidebar),
                (Command::Comments, self.annotate.comments_open),
                (Command::Inspector, self.inspector.open),
                (Command::Fields, form.is_some_and(|f| f.highlight)),
                (Command::Colours, self.colors.open),
                (Command::FitWidth, self.zoom_mode == ZoomMode::FitWidth),
                (Command::FitPage, self.zoom_mode == ZoomMode::FitPage),
                (Command::SideBySide, facing),
                (Command::Organize, self.organize.grid),
                (Command::Comment, tool == Tool::Note),
                (Command::Draw, tool == Tool::Draw),
                (Command::InkSignature, tool == Tool::Sign),
                (Command::DetectFields, self.reviewing_fields()),
            ],
            recent: self.recent.files.clone(),
        }
    }

    fn run_command(&mut self, c: Command) {
        let centre = self.view_size / 2.0;
        let pick_tool = |v: &mut Viewer, tool: Tool| {
            v.set_tool(if v.annotate.tool == tool {
                Tool::Select
            } else {
                tool
            });
        };
        match c {
            Command::About => self.window = Some(Window::About),
            Command::Settings => self.window = Some(Window::Settings),
            Command::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
            Command::Open => self.pick_file(),
            Command::OpenRecent(i) => {
                if let Some(path) = self.recent.files.get(i).cloned() {
                    self.open_recent(&path);
                }
            }
            Command::ClearRecent => self.recent.clear(),
            Command::CloseTab => {
                if let Some(active) = self.active_tab() {
                    self.request_close_tab(active);
                }
            }
            Command::Save => self.save(SaveTo::Overwrite),
            Command::SaveAs => self.save(SaveTo::As),
            Command::SaveCopy => self.save(SaveTo::Copy),
            Command::Encrypt => self.open_security(false),
            Command::Sign | Command::BackInk => self.open_security(true),
            Command::Print => self.open_print(),
            // Passed on as key presses in `mac_input`.
            Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Copy
            | Command::Paste
            | Command::SelectAll => {}
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
}
