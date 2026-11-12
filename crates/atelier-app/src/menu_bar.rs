//! The macOS menu bar. It takes the place of the menu button at the start
//! of the toolbar, which other platforms keep.
//!
//! Picking an item queues a [`Command`] through [`crate::macos`]; the viewer
//! runs it on its next frame. The Edit items with standard shortcuts (Undo,
//! Cut, Copy, Paste, Select All) are passed on as the key presses they
//! stand for, so a focused text box handles them just as before the menu
//! bar took the shortcut.

use std::collections::HashMap;
use std::path::PathBuf;

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// Something picked from the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    About,
    Settings,
    Quit,
    Open,
    OpenRecent(usize),
    ClearRecent,
    CloseTab,
    Save,
    SaveAs,
    SaveCopy,
    Encrypt,
    Sign,
    Print,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindNext,
    FindPrevious,
    Highlight,
    Comment,
    Draw,
    InkSignature,
    BackInk,
    DetectFields,
    Profiles,
    Sidebar,
    Inspector,
    Comments,
    Fields,
    Colours,
    ZoomIn,
    ZoomOut,
    ActualSize,
    FitWidth,
    FitPage,
    SideBySide,
    Organize,
    RotateLeft,
    RotateRight,
    AutoOrient,
    Website,
    ReleaseNotes,
    ReportIssue,
}

impl Command {
    fn id(self) -> String {
        match self {
            Command::OpenRecent(i) => format!("recent:{i}"),
            other => format!("{other:?}"),
        }
    }

    /// The command a menu item's id stands for.
    pub fn from_id(id: &str) -> Option<Command> {
        if let Some(i) = id.strip_prefix("recent:") {
            return i.parse().ok().map(Command::OpenRecent);
        }
        SIMPLE.iter().copied().find(|c| c.id() == id)
    }
}

/// Every command without a parameter, for looking ids up.
const SIMPLE: &[Command] = &[
    Command::About,
    Command::Settings,
    Command::Quit,
    Command::Open,
    Command::ClearRecent,
    Command::CloseTab,
    Command::Save,
    Command::SaveAs,
    Command::SaveCopy,
    Command::Encrypt,
    Command::Sign,
    Command::Print,
    Command::Undo,
    Command::Redo,
    Command::Cut,
    Command::Copy,
    Command::Paste,
    Command::SelectAll,
    Command::Find,
    Command::FindNext,
    Command::FindPrevious,
    Command::Highlight,
    Command::Comment,
    Command::Draw,
    Command::InkSignature,
    Command::BackInk,
    Command::DetectFields,
    Command::Profiles,
    Command::Sidebar,
    Command::Inspector,
    Command::Comments,
    Command::Fields,
    Command::Colours,
    Command::ZoomIn,
    Command::ZoomOut,
    Command::ActualSize,
    Command::FitWidth,
    Command::FitPage,
    Command::SideBySide,
    Command::Organize,
    Command::RotateLeft,
    Command::RotateRight,
    Command::AutoOrient,
    Command::Website,
    Command::ReleaseNotes,
    Command::ReportIssue,
];

/// Commands that need an open document.
const NEEDS_DOCUMENT: &[Command] = &[
    Command::CloseTab,
    Command::Save,
    Command::SaveAs,
    Command::SaveCopy,
    Command::Encrypt,
    Command::Sign,
    Command::Find,
    Command::FindNext,
    Command::FindPrevious,
    Command::Highlight,
    Command::Comment,
    Command::Draw,
    Command::InkSignature,
    Command::BackInk,
    Command::DetectFields,
    Command::Colours,
    Command::ZoomIn,
    Command::ZoomOut,
    Command::ActualSize,
    Command::FitWidth,
    Command::FitPage,
    Command::SideBySide,
    Command::Organize,
    Command::RotateLeft,
    Command::RotateRight,
    Command::AutoOrient,
];

/// What the menus show: which items can be used and which are ticked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuState {
    pub open: bool,
    pub selection: bool,
    pub has_form: bool,
    pub orienting: bool,
    /// Ticked items, by command.
    pub checked: Vec<(Command, bool)>,
    pub recent: Vec<PathBuf>,
}

pub struct MenuBar {
    /// The menu bar; AppKit only holds the native side.
    _menu: Menu,
    items: HashMap<Command, MenuItem>,
    checks: HashMap<Command, CheckMenuItem>,
    recent: Submenu,
    last: Option<MenuState>,
}

fn cmd(key: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Modifiers::META, key))
}

fn shift_cmd(key: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, key))
}

impl MenuBar {
    /// Builds the menu bar and puts it on the app. Call on the main thread
    /// once the app has finished launching (when the viewer is created).
    pub fn install() -> Option<MenuBar> {
        let mut bar = Builder::default();
        let separator = PredefinedMenuItem::separator;

        let app = Submenu::new("Atelier", true);
        app.append_items(&[
            &bar.item(Command::About, "About Atelier", None),
            &separator(),
            &bar.item(Command::Settings, "Settings…", cmd(Code::Comma)),
            &separator(),
            &PredefinedMenuItem::services(None),
            &separator(),
            &PredefinedMenuItem::hide(Some("Hide Atelier")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &separator(),
            &bar.item(Command::Quit, "Quit Atelier", cmd(Code::KeyQ)),
        ])
        .ok()?;

        let recent = Submenu::new("Open Recent", true);
        let file = Submenu::new("File", true);
        file.append_items(&[
            &bar.item(Command::Open, "Open…", cmd(Code::KeyO)),
            &recent,
            &separator(),
            &bar.item(Command::CloseTab, "Close Tab", cmd(Code::KeyW)),
            &bar.item(Command::Save, "Save", cmd(Code::KeyS)),
            &bar.item(Command::SaveAs, "Save As…", shift_cmd(Code::KeyS)),
            &bar.item(Command::SaveCopy, "Save a Copy…", None),
            &separator(),
            &bar.item(Command::Encrypt, "Encrypt a Copy…", None),
            &bar.item(Command::Sign, "Sign with Certificate…", None),
            &separator(),
            &bar.item(Command::Print, "Print…", cmd(Code::KeyP)),
        ])
        .ok()?;

        let mark = Submenu::new("Mark", true);
        mark.append_items(&[
            &bar.item(Command::Highlight, "Highlight Selected Text", None),
            &bar.check(Command::Comment, "Add a Comment", None),
            &bar.check(Command::Draw, "Draw", None),
            &bar.check(Command::InkSignature, "Ink Signature…", None),
            &bar.item(Command::BackInk, "Back Ink with a Certificate…", None),
        ])
        .ok()?;
        let edit = Submenu::new("Edit", true);
        edit.append_items(&[
            &bar.item(Command::Undo, "Undo", cmd(Code::KeyZ)),
            &bar.item(Command::Redo, "Redo", shift_cmd(Code::KeyZ)),
            &separator(),
            &bar.item(Command::Cut, "Cut", cmd(Code::KeyX)),
            &bar.item(Command::Copy, "Copy", cmd(Code::KeyC)),
            &bar.item(Command::Paste, "Paste", cmd(Code::KeyV)),
            &bar.item(Command::SelectAll, "Select All", cmd(Code::KeyA)),
            &separator(),
            &bar.item(Command::Find, "Find…", cmd(Code::KeyF)),
            &bar.item(Command::FindNext, "Find Next", cmd(Code::KeyG)),
            &bar.item(
                Command::FindPrevious,
                "Find Previous",
                shift_cmd(Code::KeyG),
            ),
            &separator(),
            &mark,
            &bar.check(Command::DetectFields, "Find Tables and Form Fields…", None),
            &bar.check(Command::Profiles, "Fill from a Profile…", None),
        ])
        .ok()?;

        let view = Submenu::new("View", true);
        view.append_items(&[
            &bar.check(Command::Sidebar, "Sidebar", None),
            &bar.check(Command::Comments, "Comments", None),
            &bar.check(Command::Inspector, "Inspector", None),
            &bar.check(Command::Fields, "Highlight Form Fields", None),
            &bar.check(Command::Colours, "Colours", None),
            &separator(),
            &bar.item(Command::ActualSize, "Actual Size", cmd(Code::Digit0)),
            &bar.item(Command::ZoomIn, "Zoom In", cmd(Code::Equal)),
            &bar.item(Command::ZoomOut, "Zoom Out", cmd(Code::Minus)),
            &bar.check(Command::FitWidth, "Fit Width", None),
            &bar.check(Command::FitPage, "Fit Page", None),
            &separator(),
            &bar.check(Command::SideBySide, "Side by Side", None),
            &bar.check(Command::Organize, "Organize Pages", None),
            &separator(),
            &bar.item(Command::RotateLeft, "Rotate Pages Left", None),
            &bar.item(Command::RotateRight, "Rotate Pages Right", None),
            &bar.item(Command::AutoOrient, "Auto-orient Pages", None),
            &separator(),
            &PredefinedMenuItem::fullscreen(None),
        ])
        .ok()?;

        let window = Submenu::new("Window", true);
        window
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ])
            .ok()?;

        let help = Submenu::new("Help", true);
        help.append_items(&[
            &bar.item(Command::Website, "Atelier Website", None),
            &bar.item(Command::ReleaseNotes, "Release Notes", None),
            &bar.item(Command::ReportIssue, "Report an Issue…", None),
        ])
        .ok()?;

        let menu = Menu::new();
        menu.append_items(&[&app, &file, &edit, &view, &window, &help])
            .ok()?;
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            crate::macos::push(crate::macos::Incoming::Menu(event.id().0.clone()));
        }));

        Some(MenuBar {
            _menu: menu,
            items: bar.items,
            checks: bar.checks,
            recent,
            last: None,
        })
    }

    /// Brings the items up to date with the viewer. Only what changed is
    /// sent to AppKit.
    pub fn sync(&mut self, state: MenuState) {
        if self.last.as_ref() == Some(&state) {
            return;
        }
        let enabled = |c: Command| match c {
            Command::Highlight => state.open && state.selection,
            Command::Fields => state.has_form,
            Command::AutoOrient => state.open && !state.orienting,
            c => state.open || !NEEDS_DOCUMENT.contains(&c),
        };
        for (c, item) in &self.items {
            item.set_enabled(enabled(*c));
        }
        for (c, item) in &self.checks {
            item.set_enabled(enabled(*c));
            let on = state.checked.iter().any(|&(k, v)| k == *c && v);
            item.set_checked(on);
        }
        if self.last.as_ref().map(|l| &l.recent) != Some(&state.recent) {
            self.rebuild_recent(&state.recent);
        }
        self.last = Some(state);
    }

    fn rebuild_recent(&self, files: &[PathBuf]) {
        for _ in 0..self.recent.items().len() {
            self.recent.remove_at(0);
        }
        for (i, path) in files.iter().enumerate() {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let _ = self.recent.append(&MenuItem::with_id(
                Command::OpenRecent(i).id(),
                name,
                true,
                None,
            ));
        }
        if !files.is_empty() {
            let _ = self.recent.append(&PredefinedMenuItem::separator());
        }
        let _ = self.recent.append(&MenuItem::with_id(
            Command::ClearRecent.id(),
            "Clear Menu",
            !files.is_empty(),
            None,
        ));
    }
}

#[derive(Default)]
struct Builder {
    items: HashMap<Command, MenuItem>,
    checks: HashMap<Command, CheckMenuItem>,
}

impl Builder {
    fn item(&mut self, c: Command, text: &str, key: Option<Accelerator>) -> MenuItem {
        let item = MenuItem::with_id(c.id(), text, true, key);
        self.items.insert(c, item.clone());
        item
    }

    fn check(&mut self, c: Command, text: &str, key: Option<Accelerator>) -> CheckMenuItem {
        let item = CheckMenuItem::with_id(c.id(), text, true, false, key);
        self.checks.insert(c, item.clone());
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for &c in SIMPLE {
            assert_eq!(Command::from_id(&c.id()), Some(c));
        }
        assert_eq!(Command::from_id("recent:3"), Some(Command::OpenRecent(3)));
        assert_eq!(Command::from_id("nonsense"), None);
    }
}
