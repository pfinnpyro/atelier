//! The macOS menu bar, built from the same [`crate::commands::MENUS`] as
//! the menu button other platforms keep at the start of the toolbar.
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

use crate::commands::{Command, Entry, MENUS};

/// What the menus show: which items can be used and which are ticked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuState {
    /// Items that cannot be used now.
    pub disabled: Vec<Command>,
    /// Ticked items.
    pub checked: Vec<Command>,
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

/// The key a shortcut ends in.
fn code(key: &str) -> Option<Code> {
    Some(match key {
        "A" => Code::KeyA,
        "C" => Code::KeyC,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "S" => Code::KeyS,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "=" => Code::Equal,
        "-" => Code::Minus,
        "," => Code::Comma,
        _ => return None,
    })
}

/// The menu-bar shortcut for a spec such as `Mod+Shift+S`. Only shortcuts
/// with Command are given to AppKit; the function keys, Home, End and the
/// like stay with the window, which already handles them and must not
/// lose them while a text box has the keys.
fn accelerator(spec: &str) -> Option<Accelerator> {
    let mut parts: Vec<&str> = spec.split('+').collect();
    let key = code(parts.pop()?)?;
    if !parts.contains(&"Mod") {
        return None;
    }
    let mut mods = Modifiers::empty();
    for p in parts {
        mods |= match p {
            "Mod" => Modifiers::META,
            "Shift" => Modifiers::SHIFT,
            "Alt" => Modifiers::ALT,
            "Ctrl" => Modifiers::CONTROL,
            _ => return None,
        };
    }
    Some(Accelerator::new(mods, key))
}

impl MenuBar {
    /// Builds the menu bar and puts it on the app. Call on the main thread
    /// once the app has finished launching (when the viewer is created).
    pub fn install() -> Option<MenuBar> {
        let mut bar = Builder {
            items: HashMap::new(),
            checks: HashMap::new(),
            recent: Submenu::new("Open Recent", true),
        };
        let separator = PredefinedMenuItem::separator;

        let app = Submenu::new("Atelier", true);
        app.append_items(&[
            &bar.item(Command::About, "About Atelier", ""),
            &bar.item(Command::CheckUpdates, "Check for Updates…", ""),
            &separator(),
            &bar.item(Command::Settings, "Settings…", "Mod+,"),
            &separator(),
            &PredefinedMenuItem::services(None),
            &separator(),
            &PredefinedMenuItem::hide(Some("Hide Atelier")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &separator(),
            &bar.item(Command::Quit, "Quit Atelier", "Mod+Q"),
        ])
        .ok()?;

        let menu = Menu::new();
        menu.append(&app).ok()?;
        let mut help = None;
        for def in MENUS {
            let sub = Submenu::new(def.title, true);
            bar.fill(&sub, def.entries)?;
            if def.title == "View" {
                sub.append_items(&[&separator(), &PredefinedMenuItem::fullscreen(None)])
                    .ok()?;
            }
            if def.title == "Help" {
                // The Window menu sits before Help.
                let window = Submenu::new("Window", true);
                window
                    .append_items(&[
                        &PredefinedMenuItem::minimize(None),
                        &PredefinedMenuItem::maximize(Some("Zoom")),
                        &separator(),
                        &PredefinedMenuItem::bring_all_to_front(None),
                    ])
                    .ok()?;
                menu.append(&window).ok()?;
                window.set_as_windows_menu_for_nsapp();
                help = Some(sub.clone());
            }
            menu.append(&sub).ok()?;
        }
        menu.init_for_nsapp();
        if let Some(help) = help {
            help.set_as_help_menu_for_nsapp();
        }

        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            crate::macos::push(crate::macos::Incoming::Menu(event.id().0.clone()));
        }));

        Some(MenuBar {
            _menu: menu,
            items: bar.items,
            checks: bar.checks,
            recent: bar.recent,
            last: None,
        })
    }

    /// Brings the items up to date with the viewer. Only what changed is
    /// sent to AppKit.
    pub fn sync(&mut self, state: MenuState) {
        if self.last.as_ref() == Some(&state) {
            return;
        }
        for (c, item) in &self.items {
            item.set_enabled(!state.disabled.contains(c));
        }
        for (c, item) in &self.checks {
            item.set_enabled(!state.disabled.contains(c));
            item.set_checked(state.checked.contains(c));
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

struct Builder {
    items: HashMap<Command, MenuItem>,
    checks: HashMap<Command, CheckMenuItem>,
    recent: Submenu,
}

impl Builder {
    fn item(&mut self, c: Command, text: &str, keys: &str) -> MenuItem {
        let item = MenuItem::with_id(c.id(), text, true, accelerator(keys));
        self.items.insert(c, item.clone());
        item
    }

    fn check(&mut self, c: Command, text: &str, keys: &str) -> CheckMenuItem {
        let item = CheckMenuItem::with_id(c.id(), text, true, false, accelerator(keys));
        self.checks.insert(c, item.clone());
        item
    }

    fn fill(&mut self, menu: &Submenu, entries: &'static [Entry]) -> Option<()> {
        for e in entries {
            match e {
                Entry::Item(c, text, keys) => menu.append(&self.item(*c, text, keys)),
                Entry::Check(c, text, keys) => menu.append(&self.check(*c, text, keys)),
                Entry::Separator => menu.append(&PredefinedMenuItem::separator()),
                Entry::Recent => menu.append(&self.recent),
                Entry::Sub(title, inner) => {
                    let sub = Submenu::new(*title, true);
                    self.fill(&sub, inner)?;
                    menu.append(&sub)
                }
            }
            .ok()?;
        }
        Some(())
    }
}
