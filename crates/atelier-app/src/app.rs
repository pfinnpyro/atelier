//! The viewer window: toolbar, the scrolling page view, selection and
//! search. Pages arrive as tiles from the worker pool; the window only
//! composites them, so moving or resizing it never touches the document.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atelier_form::Session;
use atelier_raster::Point;
use atelier_render::PageText;
use atelier_syntax::{Document, Error};
use atelier_view::{
    Caret, Layout, PREVIEW_SCALE, PageGeom, SearchMatch, Selection, TileCache, TileKey, ViewRect,
    hit_test, scale_key, tiles_at_scale, tiles_for_view, word_at,
};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Event, FontId, Key, Modifiers, Pos2, Rect,
    Sense, Shape, Stroke, TextureHandle, TextureOptions, Vec2, ViewportCommand,
};

use crate::worker::{Done, Job, Pool};

mod annotate;
mod colors;
mod commands;
mod controls;
mod detect;
mod form;
mod identity;
mod inspect;
#[cfg(target_os = "macos")]
mod mac;
mod menu;
mod organize;
mod pages;
mod print;
mod profiles;
mod protect;
mod revisions;
mod safety;
mod scripts;
mod security;
mod security_provider;
mod settings_window;
mod sidebar;
mod signatures;
mod tabs;
mod updates;
#[cfg(windows)]
mod windows_identity;

use form::FormView;
use inspect::Inspector;
use menu::MenuTarget;
use settings_window::Window;
use sidebar::Sidebar;

use crate::settings::Settings;

const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 64.0;
/// Sharp tiles wait this long after the zoom last changed, so resizing
/// the window or pinching draws each page once, not once per step.
const ZOOM_SETTLE: std::time::Duration = std::time::Duration::from_millis(140);
/// Page pictures and page texts kept for the file in front.
const MAX_THUMBS: usize = 240;
const MAX_TEXTS: usize = 160;
const SELECTION: Color32 = Color32::from_rgba_premultiplied(0, 70, 160, 90);
const HIT: Color32 = Color32::from_rgba_premultiplied(120, 100, 0, 110);
const CURRENT_HIT: Color32 = Color32::from_rgba_premultiplied(160, 70, 0, 150);

/// Where Save writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveTo {
    /// Over the file it was opened from.
    Overwrite,
    /// To a new file, which then becomes the open file.
    As,
    /// To a new file, leaving the open file where it is.
    Copy,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ZoomMode {
    FitWidth,
    FitPage,
    Custom,
}

/// An open file and everything known about it.
struct Open {
    signatures: signatures::Signatures,
    doc: Arc<Document>,
    name: String,
    generation: u64,
    layout: Layout,
    tiles: TileCache<TextureHandle>,
    failed: HashSet<TileKey>,
    text: HashMap<usize, Arc<PageText>>,
    warnings: BTreeSet<String>,
    /// Where the file was read from, for Save.
    path: Option<PathBuf>,
    /// Changes made since opening or saving: field values and page turns.
    session: Session,
    /// The generation the file was opened with, which outlives edits.
    opened: u64,
    /// Auto-orient is working out which way up the pages go: pages done
    /// so far, and how many there are.
    orienting: Option<pages::Orienting>,
    /// The form, when the document has one.
    form: Option<FormView>,
    /// Small pictures of the pages for the sidebar.
    thumbs: HashMap<usize, TextureHandle>,
    /// Page pictures that show an old version of their page.
    thumb_stale: HashSet<usize>,
    /// The outline, once read: its entries, and whether they are headings
    /// found in the text rather than bookmarks.
    outline: Option<(Vec<atelier_view::OutlineItem>, bool)>,
    outline_asked: bool,
    /// Text OCR read from scanned pages.
    ocr_texts: crate::worker::OcrTexts,
    /// OCR text was handed to the session but the document has not been
    /// rebuilt with it yet.
    layers_pending: bool,
    /// Tiles that still show a field's old value; they stay on screen
    /// until their replacements arrive.
    stale: HashSet<TileKey>,
    /// XFA, scripts and embedded files, and the file as opened.
    safety: safety::Safety,
    /// The form's scripts, and which the person allowed to run.
    scripts: Option<scripts::ScriptState>,
    /// Suggested fields being reviewed, while they are.
    suggest: Option<detect::Suggest>,
    /// Who the file is encrypted for, when it was opened from one
    /// protected with OpenPGP. Saving seals it for them again.
    readers: Option<atelier_pgp::Readers>,
}

/// A file read and parsed on a thread of its own.
enum Loaded {
    Ready {
        doc: Box<Document>,
        name: String,
        path: Option<PathBuf>,
    },
    /// It needs a password; the bytes wait for one.
    Locked {
        data: Vec<u8>,
        name: String,
        path: Option<PathBuf>,
    },
    Failed(String),
}

/// A save that finished writing on a thread of its own.
struct Saved {
    /// The document as it was written.
    doc: Arc<Document>,
    path: PathBuf,
    to: SaveTo,
    result: Result<(), String>,
    /// What was waiting on the save: closing the tab or the window.
    then: Option<Pending>,
}

/// Something waiting on the unsaved-changes prompt.
#[derive(Clone, Copy)]
enum Pending {
    /// Closing the window.
    Close,
    /// Closing the tab in front.
    CloseTab,
}

#[derive(Default)]
struct Search {
    /// The query the results are for; editing the box does not change it
    /// until Enter is pressed.
    query: String,
    id: u64,
    hits: Vec<SearchMatch>,
    current: Option<usize>,
    pages_done: usize,
    done: bool,
}

pub struct Viewer {
    pool: Pool,
    ctx: egui::Context,
    open: Option<Open>,
    generation: u64,
    error: Option<String>,
    /// A message about something done, shown until dismissed.
    notice: Option<String>,
    zoom_mode: ZoomMode,
    zoom_input: String,
    organize: organize::Organize,
    /// The scroll offset as of the last frame.
    offset: Vec2,
    /// The page view's size as of the last frame.
    view_size: Vec2,
    /// A scroll offset to apply on the next frame.
    scroll_to: Option<Vec2>,
    page_input: String,
    search_input: String,
    search: Search,
    focus_search: bool,
    selection: Option<Selection>,
    selecting: bool,
    show_warnings: bool,
    /// A file waiting for its password.
    locked: Option<Locked>,
    /// The unsaved-changes prompt is open for this.
    confirm: Option<Pending>,
    /// Closing was confirmed; let the window go.
    closing: bool,
    inspector: Inspector,
    review_ui: revisions::ReviewUi,
    /// What the right-click menu is for.
    menu: Option<MenuTarget>,
    settings: Settings,
    /// The Settings or About window is open.
    window: Option<Window>,
    /// The OCR models being downloaded.
    ocr_download: Option<settings_window::Download>,
    colors: colors::Colors,
    print: print::PrintDialog,
    profiles: profiles::Profiles,
    updater: updates::Updater,
    sidebar: Sidebar,
    /// The open files in tab order; the one in front is `None`, its state
    /// being the viewer's own. Empty when no file is open.
    tabs: Vec<Option<tabs::Parked>>,
    recent: crate::recent::Recent,
    annotate: annotate::Annotate,
    security: security::SecurityUi,
    /// The memory limits in force.
    budgets: crate::memory::Budgets,
    /// Files being read, and how many.
    loaded: (
        std::sync::mpsc::Sender<Loaded>,
        std::sync::mpsc::Receiver<Loaded>,
    ),
    loading: usize,
    /// Saves being written, and how many.
    saved: (
        std::sync::mpsc::Sender<Saved>,
        std::sync::mpsc::Receiver<Saved>,
    ),
    saving: usize,
    /// The kind of setting Settings is showing.
    settings_pane: settings_window::Pane,
    /// The find bar floating over the page is open.
    find_bar: bool,
    /// The message shown as a toast: its text, whether it is an error,
    /// and when it came up.
    toast: Option<(String, bool, std::time::Instant)>,
    /// The notice and error last seen, to tell when a new one arrives.
    seen: (Option<String>, Option<String>),
    /// Key presses and edits picked from a menu, for the next frame's input.
    injected: Vec<Event>,
    /// When the zoom last changed.
    zoom_changed: Option<std::time::Instant>,
    /// The scale tiles were last drawn for.
    last_scale: u32,
    protect: protect::ProtectUi,
    #[cfg(target_os = "macos")]
    mac: mac::Mac,
}

/// An encrypted file the password prompt is open for.
struct Locked {
    data: Vec<u8>,
    name: String,
    path: Option<PathBuf>,
    password: String,
    /// The last password tried was wrong.
    wrong: bool,
    focus: bool,
}

impl Viewer {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        let mut v = Self::initialize(cc, Settings::load());
        v.ctx.set_theme(match v.settings.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        });
        crate::update::clean_up();
        #[cfg(target_os = "macos")]
        v.start_mac();
        if v.settings.check_updates {
            v.updater.check(v.settings.preview, true, v.ctx.clone());
        }
        if let Some(p) = path {
            v.open_path(&p);
        }
        v
    }

    fn initialize(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        let ctx = cc.egui_ctx.clone();
        egui_extras::install_image_loaders(&ctx);
        // Ctrl +/- zoom the document, not the interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        controls::style(&ctx);
        let wake = ctx.clone();
        let budgets = crate::memory::Budgets::of(settings.memory);
        atelier_render::set_image_cache_budget(budgets.images);
        Self {
            budgets,
            loaded: std::sync::mpsc::channel(),
            loading: 0,
            saved: std::sync::mpsc::channel(),
            saving: 0,
            injected: Vec::new(),
            settings_pane: Default::default(),
            find_bar: false,
            toast: None,
            seen: (None, None),
            zoom_changed: None,
            last_scale: 0,
            pool: Pool::new(move || wake.request_repaint()),
            ctx,
            open: None,
            generation: 0,
            error: None,
            notice: None,
            zoom_mode: ZoomMode::FitWidth,
            zoom_input: String::new(),
            organize: organize::Organize::default(),
            offset: Vec2::ZERO,
            view_size: Vec2::new(800.0, 600.0),
            scroll_to: None,
            page_input: String::new(),
            search_input: String::new(),
            search: Search::default(),
            focus_search: false,
            selection: None,
            selecting: false,
            show_warnings: false,
            locked: None,
            confirm: None,
            closing: false,
            inspector: Inspector::default(),
            review_ui: revisions::ReviewUi::default(),
            menu: None,
            settings,
            window: None,
            ocr_download: None,
            colors: colors::Colors::default(),
            print: print::PrintDialog::default(),
            profiles: profiles::Profiles::default(),
            updater: updates::Updater::default(),
            sidebar: Sidebar::default(),
            tabs: Vec::new(),
            recent: crate::recent::Recent::load(),
            annotate: annotate::Annotate::default(),
            security: security::SecurityUi::default(),
            protect: protect::ProtectUi::default(),
            #[cfg(target_os = "macos")]
            mac: mac::Mac::default(),
        }
    }

    /// Opens a file in a new tab, or brings forward the tab that already
    /// has it. The file is read and parsed on a thread of its own, so a
    /// big one does not hold up the window.
    fn open_path(&mut self, path: &Path) {
        if let Some(i) = self.tab_with(path) {
            self.switch_to(i);
            return;
        }
        let (path, name) = (path.to_path_buf(), display_name(path));
        let (tx, wake) = (self.loaded.0.clone(), self.ctx.clone());
        let work = move || {
            let loaded = match std::fs::read(&path) {
                Ok(data) => Self::parse(data, name, Some(path)),
                Err(e) => Loaded::Failed(format!("Could not read {}: {e}", path.display())),
            };
            let _ = tx.send(loaded);
            wake.request_repaint();
        };
        match std::thread::Builder::new().name("open".into()).spawn(work) {
            Ok(_) => self.loading += 1,
            Err(e) => self.error = Some(format!("Could not start opening the file: {e}")),
        }
    }

    /// Parses a file's bytes with no password, off the window's thread.
    fn parse(data: Vec<u8>, name: String, path: Option<PathBuf>) -> Loaded {
        let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let doc = Document::open_with_password(&data[..], b"")?;
            // Walks the page tree now, so the window need not.
            doc.pages()?;
            Ok::<_, Error>(doc)
        }));
        match parsed {
            Ok(Ok(doc)) => Loaded::Ready {
                doc: Box::new(doc),
                name,
                path,
            },
            Ok(Err(Error::Password)) => Loaded::Locked { data, name, path },
            Ok(Err(e)) => Loaded::Failed(format!("Could not open {name}: {e}")),
            Err(_) => Loaded::Failed(format!("Could not open {name}: it could not be read.")),
        }
    }

    /// Takes in the files that finished loading.
    fn finish_loads(&mut self) {
        while let Ok(loaded) = self.loaded.1.try_recv() {
            self.loading = self.loading.saturating_sub(1);
            match loaded {
                Loaded::Ready { doc, name, path } => {
                    // Opened twice in quick succession: show the first.
                    if let Some(i) = path.as_deref().and_then(|p| self.tab_with(p)) {
                        self.switch_to(i);
                    } else {
                        self.install(*doc, name, path, None);
                    }
                }
                Loaded::Locked { data, name, path } => {
                    self.locked = Some(Locked {
                        data,
                        name,
                        path,
                        password: String::new(),
                        wrong: false,
                        focus: true,
                    });
                }
                Loaded::Failed(e) => self.error = Some(e),
            }
        }
    }

    /// Opens a file in a new tab, or brings forward the tab that already
    /// has it.
    fn open_bytes(&mut self, data: Vec<u8>, name: String, path: Option<PathBuf>) {
        if let Some(i) = path.as_deref().and_then(|p| self.tab_with(p)) {
            self.switch_to(i);
            return;
        }
        self.open_with_password(data, name, "", path);
    }

    /// Opens a file from the recent list, dropping it from the list when
    /// it is gone.
    fn open_recent(&mut self, path: &Path) {
        if !path.is_file() {
            self.recent.remove(path);
            self.error = Some(format!(
                "{} is no longer there, so it was taken off the list.",
                path.display()
            ));
            return;
        }
        self.open_path(path);
    }

    fn is_dirty(&self) -> bool {
        self.annotate.has_pending_ink() || self.open.as_ref().is_some_and(|o| o.session.is_dirty())
    }

    fn open_with_password(
        &mut self,
        data: Vec<u8>,
        name: String,
        password: &str,
        path: Option<PathBuf>,
    ) {
        // Set when these bytes were decrypted from a protected file.
        let readers = self.protect.pending.take();
        // The bytes are kept only while a password is needed.
        let doc = match Document::open_with_password(&data[..], password.as_bytes()) {
            Ok(d) => d,
            Err(Error::Password) if readers.is_some() => {
                self.error = Some(format!(
                    "Could not open {name}: the document inside the OpenPGP protection also has a PDF password, which is not supported together."
                ));
                return;
            }
            Err(Error::Password) => {
                self.locked = Some(Locked {
                    data,
                    name,
                    path,
                    password: String::new(),
                    wrong: !password.is_empty(),
                    focus: true,
                });
                return;
            }
            Err(e) => {
                self.error = Some(format!("Could not open {name}: {e}"));
                return;
            }
        };
        self.install(doc, name, path, readers);
    }

    /// Shows a parsed document in a new tab. `readers` is who the file is
    /// encrypted for, when these are the bytes decrypted from one
    /// protected with OpenPGP.
    fn install(
        &mut self,
        doc: Document,
        name: String,
        path: Option<PathBuf>,
        readers: Option<atelier_pgp::Readers>,
    ) {
        // A file protected with OpenPGP is a cover page around the real
        // document, which is opened in its place once decrypted.
        match atelier_pgp::protected_payload(&doc) {
            Ok(None) => {}
            Ok(Some(_)) if readers.is_some() => {
                self.error = Some(format!(
                    "Could not open {name}: it is protected twice over, which is not supported."
                ));
                return;
            }
            Ok(Some(payload)) => {
                self.locked = None;
                self.open_protected(payload, name, path);
                return;
            }
            Err(e) => {
                self.error = Some(format!("Could not open {name}: {e}"));
                return;
            }
        }
        let pages = match doc.pages() {
            Ok(p) => p.iter().map(PageGeom::of).collect(),
            Err(e) => {
                self.error = Some(format!("Could not read the pages of {name}: {e}"));
                return;
            }
        };
        self.locked = None;
        self.new_tab();
        if let Some(p) = &path {
            self.recent.add(p);
        }
        self.notice = None;
        self.pool.stop_search();
        self.pool.set_jobs(Vec::new());
        self.generation += 1;
        let warnings = doc.warnings().into_iter().collect();
        let doc = Arc::new(doc);
        let mut open = Open {
            signatures: Default::default(),
            safety: safety::Safety::new(doc.clone()),
            session: Session::new(doc.clone()),
            opened: self.generation,
            orienting: None,
            thumbs: HashMap::new(),
            thumb_stale: HashSet::new(),
            outline: None,
            outline_asked: false,
            ocr_texts: Default::default(),
            layers_pending: false,
            doc,
            name,
            generation: self.generation,
            layout: Layout::new(pages, 1.0),
            tiles: TileCache::new(self.budgets.tiles),
            failed: HashSet::new(),
            text: HashMap::new(),
            warnings,
            path,
            form: None,
            stale: HashSet::new(),
            scripts: None,
            suggest: None,
            readers,
        };
        Self::load_form(&mut open);
        let blocked = open.scripts.as_ref().map_or(0, |s| s.runnable().len());
        self.open = Some(open);
        self.inspector.reset();
        self.sidebar.reset();
        self.colors.reset();
        self.update_title();
        self.error = None;
        self.zoom_mode = if self.settings.fit_page {
            ZoomMode::FitPage
        } else {
            ZoomMode::FitWidth
        };
        self.scroll_to = Some(Vec2::ZERO);
        if blocked > 0 {
            self.notice = Some(format!(
                "This form has {blocked} scripts that check values and work out totals. \
                 They are blocked; to let them run, review and allow them in the inspector \
                 (F12, Actions)."
            ));
        }
        self.search = Search::default();
        self.selection = None;
        self.selecting = false;
        self.apply_fit();
        if self.settings.orient_on_open {
            self.start_orient(true);
        } else if self.ocr_ready() {
            // Read the scans, but leave the pages as they are.
            self.start_ocr();
        }
    }

    fn pick_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    /// Saves the document with its changes, to its own file or (`choose`)
    /// to one the person picks. The file is written beside the target and
    /// then moved over it, so a failed write never leaves half a file.
    fn save(&mut self, to: SaveTo) {
        self.save_then(to, None);
    }

    /// [`Viewer::save`], going on to `then` once the file is written.
    /// The bytes are written on a thread of their own; the file counts as
    /// saved when that ends well.
    fn save_then(&mut self, to: SaveTo, then: Option<Pending>) {
        // Reject pending restricted edits before consuming ink/editor buffers or
        // presenting a save dialog. An unchanged copy is still safe to save.
        if let Some(o) = &self.open {
            let pending = self.is_dirty()
                || o.layers_pending
                || o.form.as_ref().is_some_and(|f| f.is_editing());
            if pending && let Err(error) = o.doc.signature_write_policy().check(false) {
                self.error = Some(error.to_string());
                return;
            }
        }
        self.commit_ink();
        self.finish_editing(true);
        // Text OCR read goes into the file too.
        if self.open.as_ref().is_some_and(|o| o.layers_pending) {
            self.rebuild(&HashSet::new(), false);
            if let Some(o) = &mut self.open {
                o.layers_pending = false;
            }
        }
        let Some(o) = &self.open else { return };
        let path = match (&o.path, to) {
            (Some(p), SaveTo::Overwrite) => Some(p.clone()),
            _ => {
                let name = if to == SaveTo::Copy {
                    let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name);
                    format!("{stem} (copy).pdf")
                } else {
                    o.name.clone()
                };
                rfd::FileDialog::new()
                    .add_filter("PDF documents", &["pdf"])
                    .set_file_name(name)
                    .save_file()
            }
        };
        let Some(path) = path else { return };
        // A document opened from a protected file is sealed again for
        // the same readers, and is never written out in the clear here.
        // The sealing is done now; only the writing is left to the thread.
        let doc = o.doc.clone();
        let sealed = if o.readers.is_some() {
            match self.bytes_to_save() {
                Ok(bytes) => Some(bytes.into_owned()),
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        } else {
            None
        };
        let (tx, wake) = (self.saved.0.clone(), self.ctx.clone());
        let work = move || {
            let bytes = sealed.as_deref().unwrap_or_else(|| doc.data());
            let result = write_replacing(&path, bytes)
                .map_err(|e| format!("Could not save {}: {e}", path.display()));
            let _ = tx.send(Saved {
                doc,
                path,
                to,
                result,
                then,
            });
            wake.request_repaint();
        };
        match std::thread::Builder::new().name("save".into()).spawn(work) {
            Ok(_) => self.saving += 1,
            Err(e) => self.error = Some(format!("Could not start saving: {e}")),
        }
    }

    /// Takes in the saves that finished writing.
    fn finish_saves(&mut self) {
        while let Ok(saved) = self.saved.1.try_recv() {
            self.saving = self.saving.saturating_sub(1);
            if let Err(e) = saved.result {
                self.error = Some(e);
                continue;
            }
            let name = display_name(&saved.path);
            if saved.to != SaveTo::Copy {
                self.recent.add(&saved.path);
            }
            // The file the save was of, in front or in another tab. It is
            // only marked saved if it has not changed since it was written.
            let front = self
                .open
                .as_ref()
                .is_some_and(|o| Arc::ptr_eq(&o.doc, &saved.doc));
            let Some(o) = self.open_showing(&saved.doc) else {
                self.notice = Some(format!(
                    "Saved {name}. Changes made while it was being written are not in the file yet."
                ));
                continue;
            };
            let xfa = o.safety.xfa.is_some() && o.session.is_dirty();
            if saved.to != SaveTo::Copy {
                o.name = name.clone();
                o.path = Some(saved.path);
            }
            // The changes are saved somewhere, so closing need not ask again.
            o.session.saved(o.doc.clone());
            if saved.to == SaveTo::Copy {
                self.notice = Some(format!("Saved a copy as {name}."));
            }
            if xfa {
                self.notice = Some(
                    "Saved. The XFA form is kept as it was, so Acrobat may show its own XFA data \
                     rather than these field values."
                        .into(),
                );
            }
            if front {
                self.update_title();
                if let Some(pending) = saved.then
                    && !self.is_dirty()
                {
                    self.proceed(pending);
                }
            }
        }
    }

    /// Asks whether to save changes before closing or opening another file.
    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.confirm else { return };
        let name = self
            .open
            .as_ref()
            .map_or("", |o| o.name.as_str())
            .to_string();
        let what = match pending {
            Pending::Close => "closing",
            Pending::CloseTab => "closing it",
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("unsaved"))
            .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(20.0))
            .show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label(format!("Save the changes to {name} before {what}?"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        choice = Some(2);
                    }
                });
            });
        let Some(choice) = choice else { return };
        let Some(pending) = self.confirm.take() else {
            return;
        };
        if choice == 2 {
            return;
        }
        if choice == 0 {
            // Closing goes ahead when the file is written; if saving is
            // cancelled or fails, the file stays open.
            self.save_then(SaveTo::Overwrite, Some(pending));
            return;
        }
        self.proceed(pending);
    }

    /// Closes the tab or the window, the unsaved changes having been
    /// saved or given up.
    fn proceed(&mut self, pending: Pending) {
        let ctx = self.ctx.clone();
        match pending {
            Pending::Close => {
                // Not saving drops the changes; other tabs may have some
                // too.
                if let Some(o) = &mut self.open {
                    o.session.revert();
                }
                if self.show_unsaved_tab() {
                    self.confirm = Some(Pending::Close);
                } else {
                    self.closing = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Pending::CloseTab => {
                if let Some(i) = self.active_tab() {
                    self.close_tab(i);
                }
            }
        }
    }

    // ----- Zoom and scrolling -------------------------------------------

    fn zoom(&self) -> f64 {
        self.open.as_ref().map_or(1.0, |o| o.layout.zoom())
    }

    /// Horizontal shift that centres the document when it is narrower
    /// than the view.
    fn centre_shift(&self) -> f64 {
        let Some(o) = &self.open else { return 0.0 };
        let spare = (f64::from(self.view_size.x) - o.layout.size().0).max(0.0);
        if self.annotate.comments_open {
            spare
        } else {
            spare / 2.0
        }
    }

    /// Changes the zoom, keeping the document point under `anchor` (a
    /// position in the view, from its top left) where it is.
    fn set_zoom(&mut self, zoom: f64, anchor: Vec2) {
        let shift = self.centre_shift();
        let Some(o) = &mut self.open else { return };
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if o.layout.page_count() == 0 || (zoom - o.layout.zoom()).abs() < 1e-9 {
            return;
        }
        let lx = f64::from(self.offset.x + anchor.x) - shift;
        let ly = f64::from(self.offset.y + anchor.y);
        let page = o
            .layout
            .page_at(lx, ly)
            .map_or_else(|| o.layout.page_at_y(ly), |(page, _, _)| page);
        let user = o.layout.to_user(page, lx, ly);
        o.layout.set_zoom(zoom);
        self.zoom_changed = Some(std::time::Instant::now());
        if let (Some(u), Some(t)) = (user, o.layout.user_to_layout(page)) {
            let p = t.apply(u);
            let shift = ((f64::from(self.view_size.x) - o.layout.size().0) / 2.0).max(0.0);
            self.scroll_to = Some(Vec2::new(
                (p.x + shift) as f32 - anchor.x,
                p.y as f32 - anchor.y,
            ));
        }
    }

    fn zoom_by(&mut self, factor: f64, anchor: Vec2) {
        self.zoom_mode = ZoomMode::Custom;
        self.set_zoom(self.zoom() * factor, anchor);
    }

    /// Re-fits the zoom to the view for the fit modes.
    fn apply_fit(&mut self) {
        let Some(o) = &self.open else { return };
        let (w, h) = (f64::from(self.view_size.x), f64::from(self.view_size.y));
        let zoom = match self.zoom_mode {
            ZoomMode::Custom => return,
            ZoomMode::FitWidth => o.layout.fit_width_zoom(w),
            ZoomMode::FitPage => o.layout.fit_page_zoom(self.current_page(), w, h),
        };
        // Keep the top of the view on the same part of the document.
        self.set_zoom(zoom, Vec2::ZERO);
    }

    /// The page in the middle of the view.
    fn current_page(&self) -> usize {
        self.open.as_ref().map_or(0, |o| {
            o.layout
                .page_at_y(f64::from(self.offset.y + self.view_size.y / 2.0))
        })
    }

    fn go_to_page(&mut self, page: usize) {
        let Some(o) = &self.open else { return };
        if let Some(r) = o
            .layout
            .page_rect(page.min(o.layout.page_count().saturating_sub(1)))
        {
            self.scroll_to = Some(Vec2::new(
                self.offset.x,
                (r.y - Layout::GAP / 2.0).max(0.0) as f32,
            ));
        }
    }

    fn scroll_by(&mut self, dy: f32) {
        let base = self.scroll_to.unwrap_or(self.offset);
        self.scroll_to = Some(Vec2::new(base.x, (base.y + dy).max(0.0)));
    }

    // ----- Search -------------------------------------------------------

    fn run_search(&mut self, backwards: bool) {
        let query = self.search_input.trim().to_string();
        if query.is_empty() {
            self.pool.stop_search();
            self.search = Search::default();
            return;
        }
        if query == self.search.query {
            self.step_hit(backwards);
            return;
        }
        self.start_search(query);
    }

    /// Searches every page for `query` afresh.
    pub(super) fn start_search(&mut self, query: String) {
        let Some(o) = &self.open else { return };
        let wake = self.ctx.clone();
        let id = self.pool.search(
            o.doc.clone(),
            o.generation,
            query.clone(),
            o.ocr_texts.clone(),
            move || wake.request_repaint(),
        );
        self.search = Search {
            query,
            id,
            ..Search::default()
        };
    }

    fn step_hit(&mut self, backwards: bool) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        let next = match self.search.current {
            None => 0,
            Some(c) if backwards => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
        };
        self.search.current = Some(next);
        self.reveal_hit(next);
    }

    /// Scrolls so a hit is in view, if it is not already.
    fn reveal_hit(&mut self, i: usize) {
        // A hit is on a page, so the page grid gives way to the pages.
        self.organize.grid = false;
        let shift = self.centre_shift() as f32;
        let Some(o) = &self.open else { return };
        let Some(hit) = self.search.hits.get(i) else {
            return;
        };
        let Some(text) = o.text.get(&hit.page) else {
            // Text arrives with the search; until then show the page.
            self.go_to_page(hit.page);
            return;
        };
        let Some(t) = o.layout.user_to_layout(hit.page) else {
            return;
        };
        let mut bounds = Rect::NOTHING;
        for g in &text.glyphs[hit.glyphs.clone()] {
            for q in g.quad {
                let p = t.apply(q);
                bounds.extend_with(Pos2::new(p.x as f32 + shift, p.y as f32));
            }
        }
        if !bounds.is_positive() {
            return;
        }
        let view = Rect::from_min_size(self.offset.to_pos2(), self.view_size).shrink(24.0);
        if !view.contains_rect(bounds) {
            let c = bounds.center();
            self.scroll_to = Some(Vec2::new(
                (c.x - self.view_size.x / 2.0).max(0.0),
                (c.y - self.view_size.y / 2.0).max(0.0),
            ));
        }
    }

    // ----- Results from the workers -------------------------------------

    fn receive(&mut self) {
        let ppp = f64::from(self.ctx.pixels_per_point());
        let mut reveal = None;
        let mut oriented = None;
        let mut detected = Vec::new();
        let mut detect_ended = None;
        while let Ok(done) = self.pool.done.try_recv() {
            let Some(o) = &mut self.open else { continue };
            match done {
                Done::Tile {
                    generation,
                    key,
                    size,
                    rgba,
                    warnings,
                } if generation == o.generation => {
                    // Drop tiles for a zoom we have since left, unless they
                    // are previews or can stand in for the wanted one.
                    if key.scale_milli != scale_key(o.layout.zoom(), ppp)
                        && key.scale_milli != PREVIEW_SCALE
                        && o.tiles.page_tiles(key.page).next().is_some()
                    {
                        continue;
                    }
                    let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
                    let tex = self.ctx.load_texture(
                        format!("p{}-{}-{}-{}", key.page, key.scale_milli, key.tx, key.ty),
                        image,
                        TextureOptions::LINEAR,
                    );
                    o.tiles.insert(key, tex, rgba.len());
                    o.stale.remove(&key);
                    o.warnings.extend(
                        warnings
                            .into_iter()
                            .map(|w| format!("Page {}: {w}", key.page + 1)),
                    );
                }
                Done::TileFailed {
                    generation,
                    key,
                    error,
                } if generation == o.generation => {
                    o.failed.insert(key);
                    o.warnings.insert(format!("Page {}: {error}", key.page + 1));
                }
                Done::Text {
                    generation,
                    page,
                    text,
                } if generation == o.generation => {
                    o.text.insert(page, text);
                }
                Done::SearchHits {
                    generation,
                    search,
                    page,
                    hits,
                    text,
                } if generation == o.generation && search == self.search.id => {
                    // Kept for the pages with hits, to draw and reveal them.
                    if !hits.is_empty() {
                        o.text.entry(page).or_insert(text);
                    }
                    self.search.pages_done = page + 1;
                    let first_new = self.search.hits.len();
                    self.search
                        .hits
                        .extend(hits.into_iter().map(|glyphs| SearchMatch { page, glyphs }));
                    // Jump to the first hit at or after the page in view.
                    if self.search.current.is_none()
                        && self.search.hits.len() > first_new
                        && page >= o.layout.page_at_y(f64::from(self.offset.y))
                    {
                        self.search.current = Some(first_new);
                        reveal = Some(first_new);
                    }
                }
                Done::SearchEnded { generation, search }
                    if generation == o.generation && search == self.search.id =>
                {
                    self.search.done = true;
                    // Nothing after the view: wrap to the first hit.
                    if self.search.current.is_none() && !self.search.hits.is_empty() {
                        self.search.current = Some(0);
                        reveal = Some(0);
                    }
                }
                Done::Thumb {
                    generation,
                    page,
                    size,
                    rgba,
                } if generation == o.generation => {
                    let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
                    let tex = self.ctx.load_texture(
                        format!("thumb{page}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    o.thumbs.insert(page, tex);
                    o.thumb_stale.remove(&page);
                    if o.thumbs.len() > MAX_THUMBS {
                        // Keep the page pictures nearest the one that came.
                        let reach = MAX_THUMBS / 3;
                        o.thumbs.retain(|p, _| p.abs_diff(page) <= reach);
                    }
                }
                Done::Outline {
                    opened,
                    items,
                    from_headings,
                } if opened == o.opened => {
                    o.outline = Some((items, from_headings));
                }
                Done::Oriented {
                    generation,
                    rotations,
                } => oriented = Some((generation, rotations)),
                Done::OcrPage { opened, page } if opened == o.opened => {
                    // Its text is now what OCR read.
                    o.text.remove(&page);
                    if self.settings.ocr_save {
                        let read = o
                            .ocr_texts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get(&page)
                            .cloned();
                        if let Some(text) = read {
                            o.session.set_text_layer(page, text);
                            o.layers_pending = true;
                        }
                    }
                }
                Done::OcrFailed { error } => {
                    self.error = Some(format!("OCR is off for now: {error}"));
                }
                Done::Probed {
                    generation,
                    page,
                    result,
                } if generation == o.generation => {
                    self.colors.waiting = false;
                    self.colors.last = Some((page, result));
                }
                Done::Spots { opened, spots } if opened == o.opened => {
                    self.colors.spots = Some(spots);
                }
                Done::Detected {
                    opened,
                    run,
                    page,
                    found,
                } if opened == o.opened => detected.push((run, page, found)),
                Done::DetectEnded { opened, run } if opened == o.opened => {
                    detect_ended = Some(run);
                }
                _ => {}
            }
        }
        // A long file's text is kept only around the page in view.
        let here = self.current_page();
        if let Some(o) = &mut self.open
            && o.text.len() > MAX_TEXTS
        {
            let sel = self.selection;
            o.text.retain(|p, _| {
                p.abs_diff(here) <= MAX_TEXTS / 4
                    || sel.is_some_and(|s| *p == s.anchor.page || *p == s.focus.page)
            });
        }
        for (run, page, found) in detected {
            self.detected(run, page, found);
        }
        if let Some(run) = detect_ended {
            self.detect_ended(run);
        }
        if let Some((opened, rotations)) = oriented {
            self.oriented(opened, &rotations);
        } else {
            self.orient_progress();
        }
        if let Some(i) = reveal {
            self.reveal_hit(i);
        }
    }

    // ----- Input --------------------------------------------------------

    fn keyboard(&mut self) {
        let ctx = self.ctx.clone();
        // Only a text box takes keys away from the page; a focused button or
        // the page itself does not.
        let typing = ctx.text_edit_focused();
        // Shortcuts match each key event with the modifiers held at that
        // moment; a quick Ctrl+F can be over before the frame ends.
        let shortcut = |k| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        let shift = ctx.input(|i| i.modifiers.shift);
        let pressed = |k| ctx.input(|i| i.key_pressed(k));
        let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)));
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
            self.save(SaveTo::As);
        }
        if shortcut(Key::S) {
            self.save(SaveTo::Overwrite);
        }
        if shortcut(Key::O) {
            self.pick_file();
        }
        if shortcut(Key::P) {
            self.open_print();
        }
        let turn = |k| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, k));
        if turn(Key::Plus) || turn(Key::Equals) {
            self.rotate_pages(1);
        }
        if turn(Key::Minus) {
            self.rotate_pages(-1);
        }
        if self.open.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F6)) {
            self.toggle_colors();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F4)) {
            self.toggle_sidebar();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F12)) {
            self.inspector.open = !self.inspector.open;
        }
        self.detect_keys();
        self.form_keys();
        if shortcut(Key::F) {
            self.request_find();
        }
        if pressed(Key::F3) {
            self.run_search(shift);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::G)) {
            self.run_search(true);
        }
        if shortcut(Key::G) {
            self.run_search(false);
        }
        if shortcut(Key::Q) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if shortcut(Key::Comma) {
            self.window = Some(Window::Settings);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowDown)) {
            self.run_command(crate::commands::Command::NextPage);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp)) {
            self.run_command(crate::commands::Command::PreviousPage);
        }
        if copy && !typing {
            self.copy_selection();
        }
        if typing {
            return;
        }
        let centre = self.view_size / 2.0;
        if shortcut(Key::Plus) || shortcut(Key::Equals) {
            self.zoom_by(1.25, centre);
        }
        if shortcut(Key::Minus) {
            self.zoom_by(0.8, centre);
        }
        if shortcut(Key::Num0) {
            self.zoom_mode = ZoomMode::Custom;
            self.set_zoom(1.0, centre);
        }
        let page_step = self.view_size.y * 0.9;
        if pressed(Key::PageDown) || (pressed(Key::Space) && !shift) {
            self.scroll_by(page_step);
        }
        if pressed(Key::PageUp) || (pressed(Key::Space) && shift) {
            self.scroll_by(-page_step);
        }
        if pressed(Key::ArrowDown) {
            self.scroll_by(48.0);
        }
        if pressed(Key::ArrowUp) {
            self.scroll_by(-48.0);
        }
        if pressed(Key::Home) {
            self.scroll_to = Some(Vec2::ZERO);
        }
        if pressed(Key::End) {
            let h = self.open.as_ref().map_or(0.0, |o| o.layout.size().1) as f32;
            self.scroll_to = Some(Vec2::new(self.offset.x, h));
        }
        if pressed(Key::Escape) {
            self.selection = None;
        }
    }

    fn request_find(&mut self) {
        if self.open.is_none() {
            return;
        }
        // Finding does not leave the page grid; showing a hit does.
        if self.settings.find_floating {
            self.find_bar = true;
        } else {
            self.settings.sidebar = true;
            self.sidebar.find();
        }
        self.focus_search = true;
    }

    fn toggle_sidebar(&mut self) {
        self.settings.sidebar = !self.settings.sidebar;
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("Could not save the settings: {e}"));
        }
    }

    fn copy_selection(&self) {
        let (Some(o), Some(sel)) = (&self.open, &self.selection) else {
            return;
        };
        let text = sel.text(|p| o.text.get(&p).map(|t| &**t));
        if !text.is_empty() {
            self.ctx.copy_text(text);
        }
    }

    /// The caret nearest a layout point, on the page nearest it.
    fn caret_at(&self, lx: f64, ly: f64) -> Option<Caret> {
        let o = self.open.as_ref()?;
        let page = o
            .layout
            .page_at(lx, ly)
            .map_or_else(|| o.layout.page_at_y(ly), |(page, _, _)| page);
        let text = o.text.get(&page)?;
        let user = o.layout.to_user(page, lx, ly)?;
        hit_test(text, page, user)
    }

    /// Whether a layout point is over text, for the I-beam cursor.
    fn over_text(&self, lx: f64, ly: f64) -> bool {
        let Some(o) = &self.open else { return false };
        let Some((page, _, _)) = o.layout.page_at(lx, ly) else {
            return false;
        };
        let (Some(text), Some(user)) = (o.text.get(&page), o.layout.to_user(page, lx, ly)) else {
            return false;
        };
        text.glyphs
            .iter()
            .any(|g| !g.text.is_empty() && inside(&g.quad, user))
    }

    // ----- Panels -------------------------------------------------------

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().interact_size.y = controls::HEIGHT;
        ui.horizontal_wrapped(|ui| {
            ui.set_row_height(controls::HEIGHT);
            crate::chrome::traffic_light_space(ui, self.traffic_light_width());
            // The menu bar has these on macOS.
            if !self.native_menu_bar() {
                self.main_menu(ui);
            }
            self.toolbar_buttons(ui);
        });
    }

    #[cfg(target_os = "macos")]
    fn native_menu_bar(&self) -> bool {
        self.mac.has_menu_bar()
    }

    #[cfg(not(target_os = "macos"))]
    fn native_menu_bar(&self) -> bool {
        false
    }

    #[cfg(target_os = "macos")]
    fn traffic_light_width(&self) -> f32 {
        self.mac.traffic_light_width
    }

    #[cfg(not(target_os = "macos"))]
    fn traffic_light_width(&self) -> f32 {
        0.0
    }

    /// Shows or hides the second page beside each page.
    fn toggle_facing(&mut self) {
        let facing = self.open.as_ref().is_some_and(|o| o.layout.facing_pages());
        let page = self.current_page();
        if let Some(o) = &mut self.open {
            o.layout.set_facing_pages(!facing);
        }
        self.apply_fit();
        self.go_to_page(page);
    }

    /// The toolbar's buttons after the menu.
    fn toolbar_buttons(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(
                crate::icons::Icon::Sidebar
                    .button()
                    .selected(self.settings.sidebar),
            )
            .on_hover_text(self.tip("Page pictures and outline", "F4"))
            .clicked()
        {
            self.toggle_sidebar();
        }
        let (open, recent) = controls::split_button(
            ui,
            "Open",
            controls::Icon::Open,
            self.settings.toolbar_display[0],
            "Recent files",
        );
        if open
            .on_hover_text(self.tip("Open a PDF", "Mod+O"))
            .clicked()
        {
            self.pick_file();
        }
        egui::Popup::menu(&recent).show(|ui| {
            if let Some(path) = self.recent_menu(ui) {
                self.open_recent(&path);
            }
        });
        ui.add_enabled_ui(self.open.is_some(), |ui| {
            let (save, options) = controls::split_button(
                ui,
                "Save",
                controls::Icon::Save,
                self.settings.toolbar_display[1],
                "Save options",
            );
            if save
                .on_hover_text(self.tip("Save changes", "Mod+S"))
                .clicked()
            {
                self.save(SaveTo::Overwrite);
            }
            egui::Popup::menu(&options).show(|ui| {
                if ui
                    .add(egui::Button::new("Save").shortcut_text(self.keys("Mod+S")))
                    .on_hover_text("Save over the file it was opened from")
                    .clicked()
                {
                    self.save(SaveTo::Overwrite);
                    ui.close();
                }
                if ui
                    .add(egui::Button::new("Save as…").shortcut_text(self.keys("Mod+Shift+S")))
                    .clicked()
                {
                    self.save(SaveTo::As);
                    ui.close();
                }
                if ui.button("Encrypt a copy…").clicked() {
                    self.open_security(false);
                    ui.close();
                }
                if ui
                    .button("Protect with OpenPGP…")
                    .on_hover_text(
                        "Encrypt the whole document for people's keys, and manage those keys",
                    )
                    .clicked()
                {
                    self.open_protect();
                    ui.close();
                }
                if ui.button("Sign with certificate…").clicked() {
                    self.open_security(true);
                    ui.close();
                }
                if ui.button("Save a copy…").clicked() {
                    self.save(SaveTo::Copy);
                    ui.close();
                }
            });
        });
        ui.separator();
        let enabled = self.open.is_some();
        ui.add_enabled_ui(enabled, |ui| {
            let centre = self.view_size / 2.0;
            if crate::icons::button(ui, crate::icons::Icon::Minus, "Zoom out")
                .on_hover_text(self.tip("Zoom out", "Mod+-"))
                .clicked()
            {
                self.zoom_by(0.8, centre);
            }
            let zoom = ui
                .add(
                    egui::TextEdit::singleline(&mut self.zoom_input)
                        .id_salt("custom-zoom")
                        .desired_width(54.0)
                        .align(egui::Align2::CENTER_CENTER),
                )
                .on_hover_text("Enter a zoom from 10% to 800%, then press Enter");
            if zoom.lost_focus() {
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    if let Some(value) = controls::parse_zoom(&self.zoom_input) {
                        self.zoom_mode = ZoomMode::Custom;
                        self.set_zoom(value, centre);
                    } else {
                        self.notice = Some("Enter a zoom between 10% and 800%.".into());
                    }
                }
                self.zoom_input = format!("{:.0}%", self.zoom() * 100.0);
            } else if !zoom.has_focus() {
                self.zoom_input = format!("{:.0}%", self.zoom() * 100.0);
            }
            if crate::icons::button(ui, crate::icons::Icon::Plus, "Zoom in")
                .on_hover_text(self.tip("Zoom in", "Mod+="))
                .clicked()
            {
                self.zoom_by(1.25, centre);
            }
            if controls::toolbar_button(
                ui,
                "Fit width",
                controls::Icon::FitWidth,
                self.settings.toolbar_display[2],
                self.zoom_mode == ZoomMode::FitWidth,
            )
            .clicked()
            {
                self.zoom_mode = ZoomMode::FitWidth;
                self.apply_fit();
            }
            if controls::toolbar_button(
                ui,
                "Fit page",
                controls::Icon::FitPage,
                self.settings.toolbar_display[3],
                self.zoom_mode == ZoomMode::FitPage,
            )
            .clicked()
            {
                self.zoom_mode = ZoomMode::FitPage;
                self.apply_fit();
            }
        });
        let facing = self.open.as_ref().is_some_and(|o| o.layout.facing_pages());
        let facing_button = ui
            .add_enabled_ui(self.open.is_some() && !self.organize.grid, |ui| {
                controls::toolbar_button(
                    ui,
                    "Side by side",
                    controls::Icon::SideBySide,
                    self.settings.toolbar_display[4],
                    facing,
                )
            })
            .inner;
        if facing_button
            .on_hover_text("Show two pages side by side")
            .clicked()
        {
            self.toggle_facing();
        }
        let organize_button = ui
            .add_enabled_ui(self.open.is_some(), |ui| {
                controls::toolbar_button(
                    ui,
                    "Organize",
                    controls::Icon::Organize,
                    self.settings.toolbar_display[5],
                    self.organize.grid,
                )
            })
            .inner;
        if organize_button
            .on_hover_text("Rearrange pages in a grid")
            .clicked()
        {
            self.toggle_page_grid();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::chrome::window_buttons(ui);
            if ui
                .add(
                    crate::icons::Icon::SidebarRight
                        .button()
                        .selected(self.inspector.open),
                )
                .on_hover_text("Toggle inspector sidebar")
                .clicked()
            {
                self.inspector.open = !self.inspector.open;
            }
            if ui
                .add(
                    crate::icons::Icon::Comments
                        .button()
                        .selected(self.annotate.comments_open),
                )
                .on_hover_text("Toggle comments pane")
                .clicked()
            {
                self.annotate.toggle_comments();
            }
            self.status_tray(ui);
            crate::chrome::drag_space(ui);
            if let Some(o) = &self.open
                && !o.warnings.is_empty()
            {
                let label = format!("⚠ {}", o.warnings.len());
                if ui
                    .selectable_label(self.show_warnings, label)
                    .on_hover_text("Problems found while reading this file")
                    .clicked()
                {
                    self.show_warnings = !self.show_warnings;
                }
            }
        });
    }

    /// The status symbols at the end of the toolbar: work going on, an
    /// update on offer, an error, a notice. Each opens what it stands
    /// for, so nothing has to sit over the page to be noticed.
    fn status_tray(&mut self, ui: &mut egui::Ui) {
        if self.loading > 0 || self.saving > 0 {
            let what = if self.saving > 0 {
                "Saving…"
            } else {
                "Opening…"
            };
            ui.add(egui::Spinner::new().size(16.0)).on_hover_text(what);
        }
        self.update_tray(ui);
        let error_colour = ui.visuals().error_fg_color;
        for is_error in [true, false] {
            let text = if is_error { &self.error } else { &self.notice };
            let Some(text) = text.clone() else { continue };
            let (icon, tip, tint) = if is_error {
                (
                    crate::icons::Icon::Warning,
                    "Something went wrong",
                    Some(error_colour),
                )
            } else {
                (crate::icons::Icon::Info, "Notice", None)
            };
            let mut button = icon.button();
            if let Some(tint) = tint {
                button = egui::Button::new(icon.image().tint(tint)).min_size(Vec2::splat(30.0));
            }
            let response = ui.add(button).on_hover_text(tip);
            let mut dismiss = false;
            egui::Popup::menu(&response).show(|ui| {
                ui.set_max_width(360.0);
                ui.label(&text);
                ui.horizontal(|ui| {
                    if ui.button("Dismiss").clicked() {
                        dismiss = true;
                        ui.close();
                    }
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                        ui.close();
                    }
                });
            });
            if dismiss {
                if is_error {
                    self.error = None;
                } else {
                    self.notice = None;
                }
            }
        }
    }

    /// A new notice or error comes up for a few seconds in the corner of
    /// the window, then stays behind its symbol in the toolbar.
    fn toasts(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        if self.seen.0 != self.notice {
            self.seen.0 = self.notice.clone();
            if let Some(text) = &self.notice {
                self.toast = Some((text.clone(), false, now));
            }
        }
        if self.seen.1 != self.error {
            self.seen.1 = self.error.clone();
            // With no file open the error is already in the middle of the
            // window.
            if let Some(text) = self.error.as_ref().filter(|_| self.open.is_some()) {
                self.toast = Some((text.clone(), true, now));
            }
        }
        let Some((text, is_error, since)) = self.toast.clone() else {
            return;
        };
        let lasts = std::time::Duration::from_secs(if is_error { 12 } else { 7 });
        let current = if is_error { &self.error } else { &self.notice };
        if since.elapsed() >= lasts || current.as_ref() != Some(&text) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(lasts - since.elapsed());
        let mut close = false;
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -16.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal_top(|ui| {
                            let (icon, colour) = if is_error {
                                (crate::icons::Icon::Warning, ui.visuals().error_fg_color)
                            } else {
                                (crate::icons::Icon::Info, ui.visuals().text_color())
                            };
                            ui.add(icon.image().tint(colour));
                            ui.add(egui::Label::new(&text).wrap());
                            let x = crate::icons::Icon::Close
                                .button()
                                .min_size(Vec2::splat(22.0))
                                .frame(false);
                            if ui.add(x).on_hover_text("Hide").clicked() {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            // Hidden, not dismissed: it stays behind its toolbar symbol.
            self.toast = None;
        }
    }

    /// Find as a bar floating over the top right of the pages.
    fn find_bar_ui(&mut self, ctx: &egui::Context, pages: Rect) {
        if !self.find_bar || !self.settings.find_floating || self.open.is_none() {
            self.find_bar = false;
            return;
        }
        let mut close = false;
        egui::Area::new(egui::Id::unique("find-bar"))
            .order(egui::Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(pages.right_top() + Vec2::new(-24.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.add(
                                crate::icons::Icon::Find
                                    .image()
                                    .tint(ui.visuals().weak_text_color()),
                            );
                            let search = ui.add(
                                egui::TextEdit::singleline(&mut self.search_input)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(220.0)
                                    .hint_text("Find in document…"),
                            );
                            if self.focus_search {
                                search.request_focus();
                                self.focus_search = false;
                            }
                            if search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                self.run_search(ui.input(|i| i.modifiers.shift));
                                search.request_focus();
                            }
                            if search.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                                close = true;
                            }
                            let n = self.search.hits.len();
                            let count = match (self.search.query.is_empty(), self.search.current) {
                                (true, _) => String::new(),
                                (false, Some(i)) => format!("{} of {n}", i + 1),
                                (false, None) if self.search.done => "No matches".into(),
                                (false, None) => "Searching…".into(),
                            };
                            ui.add_sized(
                                [74.0, 20.0],
                                egui::Label::new(egui::RichText::new(count).weak()),
                            );
                            let small = |icon: crate::icons::Icon| {
                                icon.button().min_size(Vec2::splat(24.0)).frame(false)
                            };
                            if ui
                                .add(small(crate::icons::Icon::Up))
                                .on_hover_text(format!(
                                    "Previous match ({})",
                                    self.keys("Shift+Enter")
                                ))
                                .clicked()
                            {
                                self.run_search(true);
                            }
                            if ui
                                .add(small(crate::icons::Icon::Down))
                                .on_hover_text(format!("Next match ({})", self.keys("Enter")))
                                .clicked()
                            {
                                self.run_search(false);
                            }
                            if ui
                                .add(small(crate::icons::Icon::Close))
                                .on_hover_text("Close (Esc)")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            self.find_bar = false;
            self.pool.stop_search();
            self.search = Search::default();
        }
    }

    /// Asks for the password of an encrypted file.
    fn password_window(&mut self, ctx: &egui::Context) {
        let Some(locked) = &mut self.locked else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("Password required")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} is protected. Enter its password to open it.",
                    locked.name
                ));
                ui.add_space(4.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut locked.password)
                        .password(true)
                        .hint_text("Password")
                        .desired_width(260.0),
                );
                if locked.focus {
                    field.request_focus();
                    locked.focus = false;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if locked.wrong {
                    ui.colored_label(ui.visuals().error_fg_color, "That password is not right.");
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Open").clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.locked = None;
        } else if submit && let Some(locked) = self.locked.take() {
            let password = locked.password;
            self.open_with_password(locked.data, locked.name, &password, locked.path);
            if let Some(l) = &mut self.locked {
                l.focus = true;
            }
        }
    }

    fn warnings_window(&mut self, ctx: &egui::Context) {
        let Some(o) = &self.open else { return };
        let mut open = self.show_warnings;
        egui::Window::new("Problems")
            .open(&mut open)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} was drawn, but these parts are not supported yet or were damaged:",
                    o.name
                ));
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for w in &o.warnings {
                            ui.label(w);
                        }
                    });
            });
        self.show_warnings = open;
    }

    // ----- The page view ------------------------------------------------

    fn page_view(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        if (avail - self.view_size).length() > 0.5 {
            self.view_size = avail;
            self.apply_fit();
        }
        if self.open.is_none() {
            self.empty_view(ui);
            return;
        }
        if self.held_view(ui) {
            return;
        }
        let Some(o) = &self.open else { return };
        let (lw, lh) = o.layout.size();
        let content = Vec2::new((lw as f32).max(avail.x), lh as f32);
        // Each file has its own scroll state, so a scroll still easing in
        // one tab does not carry over to another.
        let mut area = egui::ScrollArea::both()
            .id_salt(("pages", o.opened))
            .auto_shrink(false)
            .content_margin(0.0)
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            });
        if let Some(to) = self.scroll_to.take() {
            let max = (content - avail).max(Vec2::ZERO);
            area = area.scroll_offset(to.clamp(Vec2::ZERO, max));
        }
        area.show_viewport(ui, |ui, viewport| {
            self.offset = viewport.min.to_vec2();
            let (rect, response) = ui.allocate_exact_size(content, Sense::click_and_drag());
            self.draw_pages(ui, rect.min, viewport);
            self.draw_fields(ui, rect.min);
            self.draw_hotspots(ui, rect.min);
            self.draw_pick(ui, rect.min);
            self.draw_marks(ui, rect.min);
            self.draw_suggestions(ui, rect.min);
            self.pointer(ui, &response, rect.min);
            self.draw_comment_link(ui, rect.min);
            self.draw_editor(ui, rect.min);
            let ctx = ui.ctx().clone();
            self.combo_popup(&ctx, rect.min);
        });
    }

    fn empty_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, ui.visuals().panel_fill);
        let msg = self.error.clone().unwrap_or_else(|| {
            format!("Open a PDF with {}, or drop one here.", self.keys("Mod+O"))
        });
        if self.recent.files.is_empty() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                msg,
                FontId::proportional(16.0),
                ui.visuals().text_color(),
            );
            return;
        }
        let mut pick = None;
        let width = 460.0_f32.min(rect.width() - 32.0);
        let area = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.min.y + rect.height() * 0.42),
            Vec2::new(width, rect.height() * 0.7),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.label(
                egui::RichText::new(msg)
                    .size(16.0)
                    .color(ui.visuals().text_color()),
            );
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("Recent files")
                    .strong()
                    .color(ui.visuals().text_color()),
            );
            ui.add_space(4.0);
            for path in &self.recent.files {
                let name = display_name(path);
                let folder = path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let resp = ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(name).color(ui.visuals().text_color()),
                        )
                        .frame(false)
                        .truncate(),
                    )
                    .on_hover_text(path.display().to_string());
                ui.label(
                    egui::RichText::new(folder)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(4.0);
                if resp.clicked() {
                    pick = Some(path.clone());
                }
            }
        });
        if let Some(p) = pick {
            self.open_recent(&p);
        }
    }

    /// The recent files as menu entries; returns the one picked.
    fn recent_menu(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        if self.recent.files.is_empty() {
            ui.label(egui::RichText::new("No recent files").weak());
            return None;
        }
        let mut pick = None;
        for path in &self.recent.files {
            if ui
                .button(display_name(path))
                .on_hover_text(path.display().to_string())
                .clicked()
            {
                pick = Some(path.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Clear the list").clicked() {
            self.recent.clear();
            ui.close();
        }
        pick
    }

    fn draw_pages(&mut self, ui: &egui::Ui, origin: Pos2, viewport: Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let shift = self.centre_shift();
        let painter = ui.painter();
        painter.rect_filled(
            viewport.translate(origin.to_vec2()),
            0.0,
            ui.visuals().panel_fill,
        );
        let print_page = self.print_preview_page();
        let Some(o) = &mut self.open else { return };
        o.tiles.next_frame();
        let zoom = o.layout.zoom();
        let view = ViewRect::new(
            f64::from(viewport.min.x) - shift,
            f64::from(viewport.min.y),
            f64::from(viewport.width()),
            f64::from(viewport.height()),
        );
        let wanted = tiles_for_view(&o.layout, view, f64::from(ppp));
        let current_scale = scale_key(zoom, f64::from(ppp));
        let to_screen =
            |x: f64, y: f64| Pos2::new(origin.x + (x + shift) as f32, origin.y + y as f32);

        for page in o.layout.visible_pages(view.y, view.bottom()) {
            let Some(r) = o.layout.page_rect(page) else {
                continue;
            };
            // Snap the page to device pixels so tiles meet without seams.
            let min = to_screen(r.x, r.y);
            let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
            let page_rect = Rect::from_min_size(min, Vec2::new(r.w as f32, r.h as f32));
            painter.rect_filled(
                page_rect.translate(Vec2::new(2.0, 3.0)),
                0.0,
                Color32::from_black_alpha(90),
            );
            painter.rect_filled(page_rect, 0.0, Color32::WHITE);
            let clip = page_rect.intersect(painter.clip_rect());
            let page_painter = painter.with_clip_rect(clip);

            let missing = wanted
                .iter()
                .any(|(k, _)| k.page == page && !o.tiles.contains(k));
            let mut draw: Vec<(TileKey, egui::TextureId, Vec2)> = Vec::new();
            if missing {
                // Stand-ins from other zoom levels, coarsest first.
                let mut others: Vec<_> = o
                    .tiles
                    .page_tiles(page)
                    .filter(|(k, _)| k.scale_milli != current_scale)
                    .map(|(k, t)| (*k, t.id(), t.size_vec2()))
                    .collect();
                others.sort_by_key(|(k, _, _)| k.scale_milli);
                for (k, _, _) in &others {
                    o.tiles.get(k);
                }
                draw.extend(others);
            }
            for (k, _) in wanted.iter().filter(|(k, _)| k.page == page) {
                if let Some(t) = o.tiles.get(k) {
                    draw.push((*k, t.id(), t.size_vec2()));
                }
            }
            for (k, id, size) in draw {
                let s = (zoom / k.scale()) as f32;
                let x = (k.tx as usize * atelier_view::TILE_SIZE) as f32 * s;
                let y = (k.ty as usize * atelier_view::TILE_SIZE) as f32 * s;
                let tile = Rect::from_min_size(min + Vec2::new(x, y), size * s);
                if tile.intersects(clip) {
                    page_painter.image(
                        id,
                        tile,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }

            // Highlights: search hits, then the selection.
            let Some(t) = o.layout.user_to_layout(page) else {
                continue;
            };
            let quad = |q: &[Point; 4]| -> Vec<Pos2> {
                q.iter()
                    .map(|p| {
                        let p = t.apply(*p);
                        min + Vec2::new((p.x - r.x) as f32, (p.y - r.y) as f32)
                    })
                    .collect()
            };
            if let Some(text) = o.text.get(&page) {
                for (i, hit) in self.search.hits.iter().enumerate() {
                    if hit.page != page {
                        continue;
                    }
                    let colour = if Some(i) == self.search.current {
                        CURRENT_HIT
                    } else {
                        HIT
                    };
                    for g in text.glyphs.get(hit.glyphs.clone()).unwrap_or(&[]) {
                        page_painter.add(Shape::convex_polygon(
                            quad(&g.quad),
                            colour,
                            Stroke::NONE,
                        ));
                    }
                }
                if let Some(range) = self
                    .selection
                    .and_then(|s| s.range_on(page, text.glyphs.len()))
                {
                    for g in &text.glyphs[range] {
                        if !g.text.is_empty() {
                            page_painter.add(Shape::convex_polygon(
                                quad(&g.quad),
                                SELECTION,
                                Stroke::NONE,
                            ));
                        }
                    }
                }
            }
        }

        // Ask the workers for what is missing, most urgent first: sharp
        // tiles in view, quick previews of the pages in view, their text,
        // sharp tiles a screen above and two below, then previews further
        // out so fast scrolling shows something at once.
        let mut jobs = Vec::new();
        let tile_job = |(key, page_px): (TileKey, (usize, usize))| Job::Tile {
            doc: o.doc.clone(),
            generation: o.generation,
            key,
            page_px,
        };
        let needed = |o: &Open, k: &TileKey| {
            (!o.tiles.contains(k) || o.stale.contains(k)) && !o.failed.contains(k)
        };
        let mut queued: HashSet<TileKey> = HashSet::new();
        let mut add_tiles = |jobs: &mut Vec<Job>, tiles: Vec<(TileKey, (usize, usize))>| {
            for t in tiles {
                if needed(o, &t.0) && queued.insert(t.0) {
                    jobs.push(tile_job(t));
                }
            }
        };
        let previews = current_scale > 2 * PREVIEW_SCALE;
        // While the zoom is still changing (a window being resized, a
        // pinch) pages are shown stretched from the tiles there are;
        // sharp ones are drawn once it settles.
        if current_scale != self.last_scale {
            self.last_scale = current_scale;
            self.zoom_changed
                .get_or_insert_with(std::time::Instant::now);
        }
        let settling = match self.zoom_changed {
            Some(at) if at.elapsed() < ZOOM_SETTLE => {
                ui.ctx().request_repaint_after(ZOOM_SETTLE - at.elapsed());
                // A page with nothing to show yet is drawn at once.
                o.tiles.bytes() > 0
            }
            _ => {
                self.zoom_changed = None;
                false
            }
        };
        let around = |above: f64, below: f64| {
            ViewRect::new(
                view.x,
                view.y - view.h * above,
                view.w,
                view.h * (1.0 + above + below),
            )
        };
        // Quick previews of the pages in view come first: they cost a
        // fraction of a sharp tile and put the page on screen.
        if previews {
            add_tiles(&mut jobs, tiles_at_scale(&o.layout, view, PREVIEW_SCALE));
        }
        if !settling {
            add_tiles(&mut jobs, wanted.clone());
        }
        let mut pages: Vec<usize> = o.layout.visible_pages(view.y, view.bottom()).collect();
        if let Some(sel) = self.selection {
            pages.extend([sel.anchor.page, sel.focus.page]);
        }
        for page in pages {
            if !o.text.contains_key(&page)
                && !jobs
                    .iter()
                    .any(|j| matches!(j, Job::Text { page: p, .. } if *p == page))
            {
                jobs.push(Job::Text {
                    doc: o.doc.clone(),
                    generation: o.generation,
                    page,
                    ocr: o.ocr_texts.clone(),
                });
            }
        }
        // Page pictures for the sidebar and the print preview.
        let preview = print_page.filter(|p| !o.thumbs.contains_key(p) || o.thumb_stale.contains(p));
        if self.settings.sidebar || preview.is_some() {
            let sidebar = if self.settings.sidebar {
                &self.sidebar.wanted[..]
            } else {
                &[]
            };
            for &page in preview.iter().chain(sidebar) {
                if let Some(g) = o.layout.geom(page) {
                    jobs.push(Job::Thumb {
                        doc: o.doc.clone(),
                        generation: o.generation,
                        page,
                        scale: sidebar::thumb_scale(g.size(), f64::from(ppp)),
                    });
                }
            }
        }
        if !settling {
            add_tiles(
                &mut jobs,
                tiles_for_view(&o.layout, around(1.0, 2.0), f64::from(ppp)),
            );
        }
        if previews {
            add_tiles(
                &mut jobs,
                tiles_at_scale(&o.layout, around(3.0, 6.0), PREVIEW_SCALE),
            );
        }
        self.pool.set_jobs(jobs);
    }

    fn pointer(&mut self, ui: &egui::Ui, response: &egui::Response, origin: Pos2) {
        // Navigation takes priority over annotation and selection tools.
        if response.dragged_by(egui::PointerButton::Middle) {
            self.scroll_to = Some(self.offset - ui.input(|i| i.pointer.delta()));
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            ui.ctx().request_repaint();
            return;
        }
        if response.drag_started_by(egui::PointerButton::Middle)
            || response.drag_stopped_by(egui::PointerButton::Middle)
        {
            return;
        }
        let shift = self.centre_shift();
        let to_layout = |p: Pos2| (f64::from(p.x - origin.x) - shift, f64::from(p.y - origin.y));

        // Ctrl + wheel (or a pinch) zooms around the pointer.
        let zoom_delta = ui.input(|i| i.zoom_delta());
        if response.hovered()
            && (zoom_delta - 1.0).abs() > 1e-4
            && let Some(p) = response.hover_pos()
        {
            let anchor = p - (origin + self.offset);
            // egui gives about 1.5x a wheel notch; make it about 1.2x.
            self.zoom_by(f64::from(zoom_delta).sqrt(), anchor);
        }

        if self.detect_pointer(ui, response, origin) {
            return;
        }
        let at = response
            .interact_pointer_pos()
            .or(response.hover_pos())
            .map(to_layout);
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (lx, ly) = to_layout(p);
            self.menu = None;
            self.menu_target_at(lx, ly);
        }
        response.context_menu(|ui| self.context_menu(ui));
        if self.annot_pointer(ui, response, at) {
            return;
        }
        if self.color_pointer(ui, response, response.hover_pos().map(to_layout))
            || self.pick_pointer(ui, response, response.hover_pos().map(to_layout))
        {
            return;
        }
        if !self.selecting && !response.dragged() {
            self.safety_hover(response, response.hover_pos().map(to_layout));
        }
        if let Some(p) = response.hover_pos() {
            let (lx, ly) = to_layout(p);
            if let Some(icon) = self.form_cursor(lx, ly).filter(|_| !self.selecting) {
                ui.ctx().set_cursor_icon(icon);
            } else if !self.selecting && self.unsigned_at(lx, ly).is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            } else if self.over_text(lx, ly) || self.selecting {
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
        }

        if response.drag_started_by(egui::PointerButton::Primary) {
            let start = ui
                .input(|i| i.pointer.press_origin())
                .or(response.hover_pos());
            if let Some(p) = start.filter(|p| {
                let (lx, ly) = to_layout(*p);
                self.widget_at(lx, ly).is_none()
            }) {
                let (lx, ly) = to_layout(p);
                self.selection = self.caret_at(lx, ly).map(|c| Selection {
                    anchor: c,
                    focus: c,
                });
                self.selecting = self.selection.is_some();
            }
        }
        if self.selecting
            && response.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            let (lx, ly) = to_layout(p);
            if let (Some(c), Some(sel)) = (self.caret_at(lx, ly), &mut self.selection) {
                sel.focus = c;
            }
            // Dragging past the top or bottom scrolls.
            let view = Rect::from_min_size(origin + self.offset, self.view_size);
            if p.y > view.max.y {
                self.scroll_by((p.y - view.max.y).min(60.0));
            } else if p.y < view.min.y {
                self.scroll_by(-(view.min.y - p.y).min(60.0));
            }
            ui.ctx().request_repaint();
        }
        if response.drag_stopped() {
            self.selecting = false;
            if self.selection.is_some_and(|s| s.is_empty()) {
                self.selection = None;
            }
        }
        if response.clicked() {
            self.selection = None;
            if let Some(p) = response.interact_pointer_pos() {
                let (lx, ly) = to_layout(p);
                let ctrl = ui.input(|i| i.modifiers.command);
                let on_field = self.form_click(lx, ly, ctrl) || self.sign_click(lx, ly);
                self.safety_click(lx, ly);
                // A double click selects the word under it.
                if response.double_clicked()
                    && !on_field
                    && let Some(o) = &self.open
                {
                    let page = o
                        .layout
                        .page_at(lx, ly)
                        .map_or_else(|| o.layout.page_at_y(ly), |(page, _, _)| page);
                    if let (Some(text), Some(user)) =
                        (o.text.get(&page), o.layout.to_user(page, lx, ly))
                        && self.over_text(lx, ly)
                    {
                        self.selection = word_at(text, page, user);
                    }
                }
            }
        }
    }
}

impl eframe::App for Viewer {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        #[cfg(target_os = "macos")]
        self.mac_input(raw_input);
        raw_input.events.append(&mut self.injected);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_os = "macos")]
        self.mac_frame();
        let dropped = ctx.input(|i| i.raw.dropped_files.first().cloned());
        if let Some(f) = dropped {
            if f.path().is_file() {
                self.open_path(f.path());
            } else {
                match f.bytes() {
                    Ok(data) => self.open_bytes(data, display_name(f.path()), None),
                    Err(e) => self.error = Some(format!("Could not read the dropped file: {e}")),
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing && self.show_unsaved_tab()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm = Some(Pending::Close);
        }
        crate::chrome::resize_edges(&ctx);
        self.finish_loads();
        self.finish_saves();
        self.receive();
        if self.confirm.is_none() {
            self.annot_keys(&ctx);
            self.tab_keys(&ctx);
            self.keyboard();
        }

        let mut title_bar = egui::Rangef::new(0.0, 0.0);
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            let top = ui.cursor().top();
            title_bar = egui::Rangef::new(top, top + controls::HEIGHT);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        #[cfg(target_os = "macos")]
        self.place_traffic_lights(frame, title_bar);
        #[cfg(not(target_os = "macos"))]
        let _ = (frame, title_bar);
        self.organize_window(&ctx);
        self.tab_bar(ui);
        self.update_tick(&ctx);
        self.tool_bar(ui);
        self.fields_bar(ui);
        self.safety_notice(ui);
        if self.settings.sidebar {
            self.sidebar_panel(ui);
        }
        if self.inspector.open {
            self.inspector_panel(ui);
        }
        self.comments_panel(ui);
        let pages = egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.organize.grid {
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(12)
                    .show(ui, |ui| self.page_grid(ui));
            } else {
                self.page_view(ui);
            }
        });
        self.find_bar_ui(&ctx, pages.response.rect);
        self.toasts(&ctx);
        if self.show_warnings {
            self.warnings_window(&ctx);
        }
        self.password_window(&ctx);
        self.sealed_window(&ctx);
        self.protect_window(&ctx);
        self.settings_window(&ctx);
        self.colors_window(&ctx);
        self.profiles_window(&ctx);
        self.print_window(&ctx);
        self.download_progress(&ctx);
        self.pad_window(&ctx);
        self.confirm_window(&ctx);
        self.poll_signatures(&ctx);
        self.security_window(&ctx);
    }
}

/// Writes a file next to `path` and moves it over `path`.
fn write_replacing(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".atelier-saving");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Whether a point lies inside a glyph box.
fn inside(q: &[Point; 4], p: Point) -> bool {
    let ex = q[1] - q[0];
    let ey = q[3] - q[0];
    let d = p - q[0];
    let (lx, ly) = (ex.dot(ex), ey.dot(ey));
    if lx == 0.0 || ly == 0.0 {
        return false;
    }
    let u = d.dot(ex) / lx;
    let v = d.dot(ey) / ly;
    (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
