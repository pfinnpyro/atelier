//! The viewer's side of the macOS menu bar and of files opened from Finder.

use std::path::PathBuf;

use eframe::egui::{self, Event, Key};

use super::Viewer;
use super::commands::key_press;
use crate::commands::Command;
use crate::macos::Incoming;
use crate::menu_bar::{MenuBar, MenuState};

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
                Incoming::Quit => self.quit(),
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
        // What is picked from the menu bar is for the window in front; a
        // window besides this one runs it in its own frame.
        let ctx = self.ctx.clone();
        let focused = self
            .windows
            .iter_mut()
            .find(|w| ctx.input_for(w.id, |i| i.viewport().focused == Some(true)));
        if let Some(w) = focused {
            w.viewer.mac.commands.append(&mut self.mac.commands);
        }
        self.mac_commands();
        let state = self.menu_state();
        if let Some(bar) = &mut self.mac.menu_bar {
            bar.sync(state);
        }
    }

    /// Runs what was picked from the menu bar for this window.
    pub(super) fn mac_commands(&mut self) {
        let commands = std::mem::take(&mut self.mac.commands);
        // While the unsaved-changes prompt is up, only quitting goes through,
        // as with the keyboard shortcuts.
        for c in commands {
            if self.confirm.is_none() || c == Command::Quit {
                self.run_command(c);
            }
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
        let all = || crate::commands::all();
        MenuState {
            disabled: all().filter(|c| !self.command_enabled(*c)).collect(),
            checked: all().filter(|c| self.command_checked(*c)).collect(),
            recent: self.recent.files.clone(),
        }
    }
}
