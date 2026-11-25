//! Everything the menus can do, and the menus themselves. The macOS menu
//! bar and the menu button other platforms have are both built from
//! [`MENUS`], so they hold the same items in the same order under the
//! same names.

/// Something picked from a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    About,
    Settings,
    CheckUpdates,
    Quit,
    Open,
    OpenRecent(usize),
    ClearRecent,
    #[cfg(windows)]
    ShowInExplorer,
    CloseTab,
    ReopenClosedTab,
    Save,
    SaveAs,
    SaveCopy,
    Encrypt,
    Protect,
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
    RedactCopy,
    Organize,
    PreviousPage,
    NextPage,
    FirstPage,
    LastPage,
    PreviousTab,
    NextTab,
    RotateLeft,
    RotateRight,
    AutoOrient,
    Website,
    ReleaseNotes,
    ReportIssue,
}

impl Command {
    #[cfg(any(target_os = "macos", test))]
    pub fn id(self) -> String {
        match self {
            Command::OpenRecent(i) => format!("recent:{i}"),
            other => format!("{other:?}"),
        }
    }

    /// The command a menu item's id stands for.
    #[cfg(any(target_os = "macos", test))]
    pub fn from_id(id: &str) -> Option<Command> {
        if let Some(i) = id.strip_prefix("recent:") {
            return i.parse().ok().map(Command::OpenRecent);
        }
        if id == Command::ClearRecent.id() {
            return Some(Command::ClearRecent);
        }
        all().find(|c| c.id() == id)
    }

    /// Whether the command only makes sense with a file open.
    pub fn needs_document(self) -> bool {
        use Command::*;
        !matches!(
            self,
            About
                | Settings
                | CheckUpdates
                | Quit
                | Open
                | OpenRecent(_)
                | ClearRecent
                | ReopenClosedTab
                | Undo
                | Redo
                | Cut
                | Copy
                | Paste
                | SelectAll
                | Profiles
                | Sidebar
                | Inspector
                | Comments
                | Website
                | ReleaseNotes
                | ReportIssue
        )
    }
}

/// One line of a menu. A shortcut is written `Mod+Shift+S`, where `Mod`
/// is Command on the Mac and Ctrl elsewhere; empty for none.
pub enum Entry {
    Item(Command, &'static str, &'static str),
    /// An item that shows a tick while what it stands for is on.
    Check(Command, &'static str, &'static str),
    Separator,
    /// The recent files.
    Recent,
    Sub(&'static str, &'static [Entry]),
}

pub struct MenuDef {
    pub title: &'static str,
    pub entries: &'static [Entry],
}

use Command as C;
use Entry::{Check, Item, Recent, Separator, Sub};

const MARK: &[Entry] = &[
    Item(C::Highlight, "Highlight Selected Text", ""),
    Check(C::Comment, "Add a Comment", ""),
    Check(C::Draw, "Draw", ""),
    Check(C::InkSignature, "Ink Signature…", ""),
    Item(C::BackInk, "Back Ink with a Certificate…", ""),
];

/// The menus, in menu-bar order. The Mac adds its own application and
/// Window menus around them; elsewhere [`APP`] follows them.
pub const MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        entries: &[
            Item(C::Open, "Open…", "Mod+O"),
            Recent,
            #[cfg(windows)]
            Item(C::ShowInExplorer, "Show in File Explorer", ""),
            Separator,
            Item(C::CloseTab, "Close Tab", "Mod+W"),
            Item(C::ReopenClosedTab, "Reopen Closed Tab", "Mod+Shift+T"),
            Item(C::Save, "Save", "Mod+S"),
            Item(C::SaveAs, "Save As…", "Mod+Shift+S"),
            Item(C::SaveCopy, "Save a Copy…", ""),
            Item(C::RedactCopy, "Secure Redaction…", ""),
            Separator,
            Item(C::Encrypt, "Encrypt a Copy…", ""),
            Item(C::Protect, "Protect with OpenPGP…", ""),
            Item(C::Sign, "Sign with Certificate…", ""),
            Separator,
            Item(C::Print, "Print…", "Mod+P"),
        ],
    },
    MenuDef {
        title: "Edit",
        entries: &[
            Item(C::Undo, "Undo", "Mod+Z"),
            Item(C::Redo, "Redo", "Mod+Shift+Z"),
            Separator,
            Item(C::Cut, "Cut", "Mod+X"),
            Item(C::Copy, "Copy", "Mod+C"),
            Item(C::Paste, "Paste", "Mod+V"),
            Item(C::SelectAll, "Select All", "Mod+A"),
            Separator,
            Item(C::Find, "Find…", "Mod+F"),
            Item(C::FindNext, "Find Next", "Mod+G"),
            Item(C::FindPrevious, "Find Previous", "Mod+Shift+G"),
        ],
    },
    MenuDef {
        title: "View",
        entries: &[
            Check(C::Sidebar, "Sidebar", "F4"),
            Check(C::Comments, "Comments", ""),
            Check(C::Inspector, "Inspector", "F12"),
            Check(C::Colours, "Colours", "F6"),
            Check(C::Fields, "Highlight Form Fields", ""),
            Separator,
            Item(C::ZoomIn, "Zoom In", "Mod+="),
            Item(C::ZoomOut, "Zoom Out", "Mod+-"),
            Item(C::ActualSize, "Actual Size", "Mod+0"),
            Check(C::FitWidth, "Fit Width", ""),
            Check(C::FitPage, "Fit Page", ""),
            Separator,
            Check(C::SideBySide, "Side by Side", ""),
            Check(C::Organize, "Organize Pages", ""),
        ],
    },
    MenuDef {
        title: "Go",
        entries: &[
            Item(C::PreviousPage, "Previous Page", "Alt+Up"),
            Item(C::NextPage, "Next Page", "Alt+Down"),
            Item(C::FirstPage, "First Page", "Home"),
            Item(C::LastPage, "Last Page", "End"),
            Separator,
            Item(C::PreviousTab, "Previous Tab", "Mod+Shift+Tab"),
            Item(C::NextTab, "Next Tab", "Mod+Tab"),
        ],
    },
    MenuDef {
        title: "Tools",
        entries: &[
            Sub("Mark", MARK),
            Separator,
            Check(C::DetectFields, "Find Tables and Form Fields…", ""),
            Check(C::Profiles, "Fill from a Profile…", ""),
            Separator,
            Item(C::RotateLeft, "Rotate Pages Left", "Mod+Shift+-"),
            Item(C::RotateRight, "Rotate Pages Right", "Mod+Shift+="),
            Item(C::AutoOrient, "Auto-orient Pages", ""),
        ],
    },
    MenuDef {
        title: "Help",
        entries: &[
            Item(C::Website, "Atelier Website", ""),
            Item(C::ReleaseNotes, "Release Notes", ""),
            Item(C::ReportIssue, "Report an Issue…", ""),
        ],
    },
];

/// What the Mac keeps in its application menu; other platforms show
/// these at the end of the menu button's menu.
pub const APP: &[Entry] = &[
    Item(C::Settings, "Settings…", "Mod+,"),
    Item(C::CheckUpdates, "Check for Updates…", ""),
    Item(C::About, "About Atelier", ""),
    Separator,
    Item(C::Quit, "Quit Atelier", "Mod+Q"),
];

#[cfg(any(target_os = "macos", test))]
fn walk(entries: &'static [Entry], out: &mut Vec<Command>) {
    for e in entries {
        match e {
            Item(c, ..) | Check(c, ..) => out.push(*c),
            Sub(_, inner) => walk(inner, out),
            Separator | Recent => {}
        }
    }
}

/// Every command a menu holds (not the recent files).
#[cfg(any(target_os = "macos", test))]
pub fn all() -> impl Iterator<Item = Command> {
    let mut out = Vec::new();
    for m in MENUS {
        walk(m.entries, &mut out);
    }
    walk(APP, &mut out);
    out.into_iter()
}

/// How a shortcut is written for the person: `Mod+Shift+S` becomes
/// `Ctrl+Shift+S`, on the Mac `Cmd+Shift+S`, and with `glyphs` the keys'
/// symbols (`⌘⇧S` on the Mac, `⌃⇧S` elsewhere).
pub fn shortcut_text(spec: &str, glyphs: bool) -> String {
    shortcut_text_for(spec, glyphs, cfg!(target_os = "macos"))
}

fn shortcut_text_for(spec: &str, glyphs: bool, mac: bool) -> String {
    if spec.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = spec.split('+').collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let key = match (key[0], glyphs) {
        // "Mod+=" splits to an empty last part only for "+", never used.
        ("Up", true) => "↑",
        ("Down", true) => "↓",
        ("Left", true) => "←",
        ("Right", true) => "→",
        ("Tab", true) => "⇥",
        ("Enter", true) => "↩",
        ("-", _) => "−",
        (k, _) => k,
    };
    let mut out = String::new();
    // The Mac lists Control, Option, Shift, Command in that order.
    let order = |m: &&str| match *m {
        "Ctrl" => 0,
        "Alt" => 1,
        "Shift" => 2,
        _ => 3,
    };
    let mut mods = mods.to_vec();
    if mac {
        mods.sort_by_key(|m| order(m));
    }
    for m in mods {
        let name = match (m, mac, glyphs) {
            ("Mod", true, true) => "⌘",
            ("Mod", true, false) => "Cmd",
            ("Mod" | "Ctrl", _, true) => "⌃",
            ("Mod" | "Ctrl", _, false) => "Ctrl",
            ("Shift", _, true) => "⇧",
            ("Alt", true, true) => "⌥",
            ("Alt", true, false) => "Option",
            ("Alt", false, true) => "⎇",
            (other, _, _) => other,
        };
        out.push_str(name);
        if !glyphs {
            out.push('+');
        }
    }
    out.push_str(key);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for c in all() {
            assert_eq!(Command::from_id(&c.id()), Some(c));
        }
        assert_eq!(Command::from_id("recent:3"), Some(Command::OpenRecent(3)));
        assert_eq!(Command::from_id("nonsense"), None);
    }

    #[test]
    fn shortcuts_are_written_for_the_platform() {
        assert_eq!(
            shortcut_text_for("Mod+Shift+S", false, false),
            "Ctrl+Shift+S"
        );
        assert_eq!(shortcut_text_for("Mod+Shift+S", false, true), "Shift+Cmd+S");
        assert_eq!(shortcut_text_for("Mod+Shift+S", true, true), "⇧⌘S");
        assert_eq!(shortcut_text_for("Mod+Shift+S", true, false), "⌃⇧S");
        assert_eq!(shortcut_text_for("Alt+Down", true, true), "⌥↓");
        assert_eq!(shortcut_text_for("F4", true, true), "F4");
        assert_eq!(shortcut_text_for("", true, true), "");
    }
}
