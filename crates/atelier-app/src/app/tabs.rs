//! Several files open at once, one tab each. The file in front lives in
//! the viewer's own fields; the others are parked with what they need to
//! come back as they were (scroll position, zoom, search, picked pages).

use std::path::Path;

use atelier_view::Selection;
use eframe::egui::{self, Key, Modifiers, RichText, Vec2};

use super::sidebar::SidebarState;
use super::{Open, Pending, Search, Viewer, ZoomMode};
use crate::recent::same_file;

/// A file in a tab that is not in front.
pub(super) struct Parked {
    open: Open,
    zoom_mode: ZoomMode,
    offset: Vec2,
    search_input: String,
    search: Search,
    selection: Option<Selection>,
    sidebar: SidebarState,
    notice: Option<String>,
    /// Auto-orient was running (quietly, turning pages) and starts again.
    rerun: Option<(bool, bool)>,
}

impl Viewer {
    /// The tab in front, when a file is open.
    pub(super) fn active_tab(&self) -> Option<usize> {
        self.tabs.iter().position(Option::is_none)
    }

    /// Takes the file in front out of the viewer.
    fn park(&mut self) -> Option<Parked> {
        self.finish_editing(true);
        self.commit_ink();
        let mut open = self.open.take()?;
        self.pool.stop_search();
        self.pool.set_jobs(Vec::new());
        let rerun = open.orienting.take().map(|r| (r.quiet, r.turn));
        Some(Parked {
            open,
            zoom_mode: self.zoom_mode,
            offset: self.offset,
            search_input: std::mem::take(&mut self.search_input),
            search: std::mem::take(&mut self.search),
            selection: self.selection.take(),
            sidebar: self.sidebar.take_state(),
            notice: self.notice.take(),
            rerun,
        })
    }

    /// Puts a parked file in front.
    fn unpark(&mut self, p: Parked) {
        let mut open = p.open;
        // Work finished for it while it was parked was dropped; it is
        // asked for again.
        self.generation += 1;
        open.generation = self.generation;
        open.opened = self.generation;
        if open.outline.is_none() {
            open.outline_asked = false;
        }
        self.open = Some(open);
        self.zoom_mode = p.zoom_mode;
        self.scroll_to = Some(p.offset);
        self.offset = p.offset;
        self.search_input = p.search_input;
        self.search = p.search;
        self.selection = p.selection;
        self.sidebar.restore_state(p.sidebar);
        self.notice = p.notice;
        self.error = None;
        self.after_switch();
        if !self.search.query.is_empty() && !self.search.done {
            self.start_search(self.search.query.clone());
        }
        if let Some((quiet, turn)) = p.rerun {
            self.start_run(quiet, turn);
        }
    }

    /// Clears what belongs to the file that was in front.
    fn after_switch(&mut self) {
        self.selecting = false;
        self.menu = None;
        self.annotate.reset();
        self.inspector.reset();
        self.colors.reset();
        self.print = Default::default();
        self.update_title();
    }

    /// Makes room for a file about to open: the one in front is parked,
    /// and the new one gets the tab after it.
    pub(super) fn new_tab(&mut self) {
        match self.park() {
            Some(parked) => {
                let at = self.active_tab().unwrap_or(self.tabs.len());
                if at < self.tabs.len() {
                    self.tabs[at] = Some(parked);
                } else {
                    self.tabs.push(Some(parked));
                }
                self.tabs.insert(at + 1, None);
            }
            None => {
                self.tabs.retain(Option::is_some);
                self.tabs.push(None);
            }
        }
    }

    /// The tab that already has `path` open.
    pub(super) fn tab_with(&self, path: &Path) -> Option<usize> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let has = |o: &Open| {
            o.path.as_ref().is_some_and(|p| {
                let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
                same_file(&p, &path)
            })
        };
        self.tabs.iter().position(|t| match t {
            Some(p) => has(&p.open),
            None => self.open.as_ref().is_some_and(has),
        })
    }

    /// Brings tab `i` to the front.
    pub(super) fn switch_to(&mut self, i: usize) {
        let Some(active) = self.active_tab() else {
            return;
        };
        if i == active || i >= self.tabs.len() {
            return;
        }
        let Some(target) = self.tabs[i].take() else {
            return;
        };
        self.tabs[active] = self.park();
        self.unpark(target);
    }

    /// Closes tab `i`, first asking about unsaved changes.
    pub(super) fn request_close_tab(&mut self, i: usize) {
        let dirty = match self.tabs.get(i) {
            Some(Some(p)) => p.open.session.is_dirty(),
            Some(None) => self.is_dirty(),
            None => return,
        };
        if dirty {
            self.switch_to(i);
            self.confirm = Some(Pending::CloseTab);
        } else {
            self.close_tab(i);
        }
    }

    /// Closes tab `i` without asking.
    pub(super) fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs[i].is_some() {
            self.tabs.remove(i);
            return;
        }
        // The tab in front: drop its file and bring a neighbour forward.
        self.finish_editing(false);
        self.annotate.reset();
        self.open = None;
        self.pool.stop_search();
        self.pool.set_jobs(Vec::new());
        self.tabs.remove(i);
        self.search = Search::default();
        self.search_input.clear();
        self.selection = None;
        self.sidebar.reset();
        self.notice = None;
        if self.tabs.is_empty() {
            self.after_switch();
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Title("Atelier".into()));
            return;
        }
        let next = i.min(self.tabs.len() - 1);
        if let Some(p) = self.tabs[next].take() {
            self.unpark(p);
        }
    }

    /// For closing the window: brings forward a tab with unsaved changes
    /// and returns true, or returns false when there is none.
    pub(super) fn show_unsaved_tab(&mut self) -> bool {
        if self.is_dirty() {
            return true;
        }
        let dirty = self
            .tabs
            .iter()
            .position(|t| t.as_ref().is_some_and(|p| p.open.session.is_dirty()));
        match dirty {
            Some(i) => {
                self.switch_to(i);
                true
            }
            None => false,
        }
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab go through the tabs; Ctrl+W closes one.
    pub(super) fn tab_keys(&mut self, ctx: &egui::Context) {
        let Some(active) = self.active_tab() else {
            return;
        };
        let n = self.tabs.len();
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab)) {
            self.switch_to((active + n - 1) % n);
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Tab)) {
            self.switch_to((active + 1) % n);
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) {
            self.request_close_tab(active);
        }
    }

    /// The row of tabs under the toolbar.
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        if self.tabs.is_empty() {
            return;
        }
        let mut go = None;
        let mut close = None;
        egui::Panel::top("tabs").show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("tab-row")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, tab) in self.tabs.iter().enumerate() {
                            let (open, front) = match tab {
                                Some(p) => (&p.open, false),
                                None => match &self.open {
                                    Some(o) => (o, true),
                                    None => continue,
                                },
                            };
                            let mark = if open.session.is_dirty() { "• " } else { "" };
                            let mut name = format!("{mark}{}", open.name);
                            if name.chars().count() > 32 {
                                name = name.chars().take(30).collect::<String>() + "…";
                            }
                            let text = if front {
                                RichText::new(name).strong()
                            } else {
                                RichText::new(name)
                            };
                            let hover = open
                                .path
                                .as_ref()
                                .map_or(open.name.clone(), |p| p.display().to_string());
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let resp = ui.selectable_label(front, text).on_hover_text(hover);
                            if resp.clicked() {
                                go = Some(i);
                            }
                            if resp.middle_clicked() {
                                close = Some(i);
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text("Close (Ctrl+W)")
                                .clicked()
                            {
                                close = Some(i);
                            }
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.separator();
                        }
                    });
                });
        });
        if let Some(i) = close {
            self.request_close_tab(i);
        } else if let Some(i) = go {
            self.switch_to(i);
        }
    }
}
