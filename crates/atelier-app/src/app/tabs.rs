//! Several files open at once, one tab each. The file in front lives in
//! the viewer's own fields; the others are parked with what they need to
//! come back as they were (scroll position, zoom, search, picked pages).

use std::path::{Path, PathBuf};

use atelier_view::Selection;
use eframe::egui::{self, Key, Modifiers, Pos2, Rect, Vec2};

use super::sidebar::SidebarState;
use super::{Open, Pending, Search, Viewer, ZoomMode};
use crate::recent::same_file;

/// A file in a tab that is not in front.
pub(super) struct Parked {
    pub(super) open: Open,
    zoom_mode: ZoomMode,
    offset: Vec2,
    current_page: usize,
    search_input: String,
    search: Search,
    selection: Option<Selection>,
    sidebar: SidebarState,
    notice: Option<String>,
    /// Auto-orient was running (quietly, turning pages) and starts again.
    rerun: Option<(bool, bool)>,
}

pub(super) struct ClosedTab {
    path: PathBuf,
    page: usize,
    mode: ZoomMode,
    zoom: f64,
}

impl Viewer {
    /// Updating/restarting must preserve work in every tab, including live text
    /// editors and OCR layers not yet handed to the saved document.
    pub(super) fn update_has_unsaved_work(&self) -> bool {
        let pending = |open: &Open| {
            open.session.is_dirty()
                || open.layers_pending
                || open.form.as_ref().is_some_and(|form| form.is_editing())
        };
        self.is_dirty()
            || self.open.as_ref().is_some_and(pending)
            || self.tabs.iter().flatten().any(|tab| pending(&tab.open))
            || self
                .windows
                .iter()
                .any(|w| w.viewer.update_has_unsaved_work())
    }

    /// The open file showing exactly `doc`, in front or in another tab.
    pub(super) fn open_showing(
        &mut self,
        doc: &std::sync::Arc<atelier_syntax::Document>,
    ) -> Option<&mut Open> {
        self.open
            .iter_mut()
            .chain(self.tabs.iter_mut().flatten().map(|tab| &mut tab.open))
            .find(|o| std::sync::Arc::ptr_eq(&o.doc, doc))
    }

    /// Puts new memory limits into force, for every open file.
    pub(super) fn apply_budgets(&mut self) {
        self.budgets = crate::memory::Budgets::of(self.settings.memory);
        atelier_render::set_image_cache_budget(self.budgets.images);
        if let Some(o) = &mut self.open {
            o.tiles.set_budget(self.budgets.tiles);
        }
        for tab in self.tabs.iter_mut().flatten() {
            tab.open.tiles.set_budget(self.budgets.parked);
        }
    }

    /// The tab in front, when a file is open.
    pub(super) fn active_tab(&self) -> Option<usize> {
        self.tabs.iter().position(Option::is_none)
    }

    /// Takes the file in front out of the viewer.
    pub(super) fn park(&mut self) -> Option<Parked> {
        self.finish_editing(true);
        self.commit_ink();
        let mut open = self.open.take()?;
        // What a detection run finds while the file is parked is dropped,
        // so it would never be seen to end.
        if let Some(s) = &mut open.suggest {
            s.running = false;
            self.pool.stop_detect();
        }
        self.pool.stop_search();
        self.pool.set_jobs(Vec::new());
        let rerun = open.orienting.take().map(|r| (r.quiet, r.turn));
        // A file in a background tab keeps only a slim set of drawn pages
        // around where it was, enough to show at once when it comes back;
        // its decoded images and rasterized glyphs are let go.
        let here = open
            .layout
            .page_at_y(f64::from(self.offset.y + self.view_size.y / 2.0));
        open.tiles.retain(|k| k.page.abs_diff(here) <= 1);
        open.tiles.set_budget(self.budgets.parked);
        let tiles = &open.tiles;
        open.stale.retain(|k| tiles.contains(k));
        open.thumbs.retain(|p, _| p.abs_diff(here) <= 8);
        open.text.retain(|p, _| p.abs_diff(here) <= 2);
        atelier_render::trim_cache(&open.doc);
        Some(Parked {
            open,
            zoom_mode: self.zoom_mode,
            offset: self.offset,
            current_page: here,
            search_input: std::mem::take(&mut self.search_input),
            search: std::mem::take(&mut self.search),
            selection: self.selection.take(),
            sidebar: self.sidebar.take_state(),
            notice: self.notice.take(),
            rerun,
        })
    }

    /// Puts a parked file in front.
    pub(super) fn unpark(&mut self, p: Parked) {
        let mut open = p.open;
        open.tiles.set_budget(self.budgets.tiles);
        // Work finished for it while it was parked was dropped; it is
        // asked for again.
        self.generation += 1;
        open.generation = self.generation;
        open.page_generations.clear();
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
    pub(super) fn after_switch(&mut self) {
        self.organize = Default::default();
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
            Some(Some(p)) => p.open.has_unsaved_changes(),
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
        let (open, mode, page) = match &self.tabs[i] {
            Some(p) => (Some(&p.open), p.zoom_mode, p.current_page),
            None => (self.open.as_ref(), self.zoom_mode, self.current_page()),
        };
        if let Some(open) = open
            && let Some(path) = &open.path
        {
            self.closed_tabs.retain(|tab| !same_file(&tab.path, path));
            if self.closed_tabs.len() == 20 {
                self.closed_tabs.remove(0);
            }
            self.closed_tabs.push(ClosedTab {
                path: path.clone(),
                page,
                mode,
                zoom: open.layout.zoom(),
            });
        }
        if self.tabs[i].is_some() {
            if let Some(mut parked) = self.tabs.remove(i) {
                parked.open.recovery.clear();
            }
            return;
        }
        // The tab in front: drop its file and bring a neighbour forward.
        self.finish_editing(false);
        self.annotate.reset();
        if let Some(mut open) = self.open.take() {
            open.recovery.clear();
        }
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

    pub(super) fn reopen_closed_tab(&mut self) {
        let Some(closed) = self.closed_tabs.pop() else {
            return;
        };
        if let Some(index) = self.tab_with(&closed.path) {
            self.switch_to(index);
            return;
        }
        let path = closed.path.clone();
        self.reopening.insert(path.clone(), closed);
        self.open_path(&path);
    }

    pub(super) fn restore_closed_tab(&mut self, closed: ClosedTab) {
        self.zoom_mode = closed.mode;
        self.set_zoom(closed.zoom, Vec2::ZERO);
        self.go_to_page(closed.page);
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
            .position(|t| t.as_ref().is_some_and(|p| p.open.has_unsaved_changes()));
        match dirty {
            Some(i) => {
                self.switch_to(i);
                true
            }
            None => false,
        }
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab go through the tabs; Ctrl+W closes one;
    /// Ctrl+Shift+Page Up and Page Down move the one in front.
    pub(super) fn tab_keys(&mut self, ctx: &egui::Context) {
        let shifted = Modifiers::COMMAND | Modifiers::SHIFT;
        if !cfg!(target_os = "macos") && ctx.input_mut(|i| i.consume_key(shifted, Key::T)) {
            self.reopen_closed_tab();
            return;
        }
        let Some(active) = self.active_tab() else {
            return;
        };
        let n = self.tabs.len();
        if ctx.input_mut(|i| i.consume_key(shifted, Key::Tab)) {
            self.switch_to((active + n - 1) % n);
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Tab)) {
            self.switch_to((active + 1) % n);
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) {
            self.request_close_tab(active);
        } else if ctx.input_mut(|i| i.consume_key(shifted, Key::PageUp)) {
            self.move_tab(active, active.saturating_sub(1));
        } else if ctx.input_mut(|i| i.consume_key(shifted, Key::PageDown)) {
            self.move_tab(active, active + 1);
        }
    }

    /// Puts tab `from` at `to`, the others closing up around it.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        let n = self.tabs.len();
        if from >= n || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to.min(n - 1), tab);
    }

    /// Moves tab `i` to a window of its own, its corner at `at` on
    /// screen. It goes as it is, unsaved changes, undo and all.
    pub(super) fn request_pop_out(&mut self, i: usize, at: Option<Pos2>) {
        if self.tabs.len() < 2 || self.saving > 0 {
            return;
        }
        if let Some(tab) = self.take_tab(i) {
            self.leaving.push((tab, at));
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    /// Takes tab `i` out of this window, as it is, bringing a neighbour
    /// forward if it was in front.
    pub(super) fn take_tab(&mut self, i: usize) -> Option<Parked> {
        if self.tabs.get(i)?.is_some() {
            return self.tabs.remove(i);
        }
        let parked = self.park()?;
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.after_switch();
            self.ctx.send_viewport_cmd_to(
                self.viewport,
                egui::ViewportCommand::Title("Atelier".into()),
            );
        } else {
            let next = i.min(self.tabs.len() - 1);
            if let Some(p) = self.tabs[next].take() {
                self.unpark(p);
            }
        }
        Some(parked)
    }

    /// Takes every tab out of this window, in order, with the place of
    /// the one that was in front.
    pub(super) fn take_all(&mut self) -> (Vec<Parked>, usize) {
        let front = self.active_tab().unwrap_or(0);
        if let Some(i) = self.active_tab() {
            self.tabs[i] = self.park();
        }
        let tabs = std::mem::take(&mut self.tabs)
            .into_iter()
            .flatten()
            .collect();
        (tabs, front)
    }

    /// Puts `tabs` after this window's own, the one at `front` in front.
    pub(super) fn adopt(&mut self, tabs: Vec<Parked>, front: usize) {
        if tabs.is_empty() {
            return;
        }
        if let Some(i) = self.active_tab() {
            self.tabs[i] = self.park();
        }
        let at = self.tabs.len() + front.min(tabs.len() - 1);
        self.tabs.extend(tabs.into_iter().map(Some));
        if let Some(p) = self.tabs[at].take() {
            self.unpark(p);
        }
    }

    pub(super) fn insert_tab(&mut self, tab: Parked, slot: usize) {
        if let Some(active) = self.active_tab() {
            self.tabs[active] = self.park();
        }
        self.tabs.insert(slot.min(self.tabs.len()), None);
        self.unpark(tab);
    }

    /// Whether any tab has changes not saved.
    pub(super) fn has_unsaved(&self) -> bool {
        self.is_dirty()
            || self
                .tabs
                .iter()
                .flatten()
                .any(|t| t.open.has_unsaved_changes())
    }

    /// Lets go of every file in this window, changes and all.
    pub(super) fn drop_all(&mut self) {
        self.finish_editing(false);
        self.annotate.reset();
        if let Some(mut open) = self.open.take() {
            open.recovery.clear();
        }
        for parked in self.tabs.iter_mut().flatten() {
            parked.open.recovery.clear();
        }
        self.pool.stop_search();
        self.pool.set_jobs(Vec::new());
        self.tabs.clear();
        self.search = Search::default();
        self.search_input.clear();
        self.selection = None;
        self.sidebar.reset();
        self.notice = None;
        self.after_switch();
    }

    /// The row of tabs under the toolbar, with drops for reordering,
    /// moving between Atelier windows, and detaching to a new window.
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        if self.tabs.is_empty() {
            self.tab_drag = None;
            let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(ui.available_width(), 96.0));
            self.tab_strip = TabStrip::on_screen(ui.ctx(), area, &[]);
            return;
        }
        let mut go = None;
        let mut close = None;
        let mut moved = None;
        let mut pop = None;
        let mut copy_path = None;
        #[cfg(windows)]
        let mut reveal = None;
        let mut reopen = false;
        let several = self.tabs.len() > 1;
        let close_tip = self.tip("Close tab", "Mod+W");
        // Each tab's place in the row, and the one being dragged.
        let mut places = Vec::with_capacity(self.tabs.len());
        let mut dragged = None;
        let mut dropped = None;
        let bar = egui::Panel::top("tabs").show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("tab-row")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let n = self.tabs.len();
                        for (i, tab) in self.tabs.iter().enumerate() {
                            let (open, front) = match tab {
                                Some(p) => (&p.open, false),
                                None => match &self.open {
                                    Some(o) => (o, true),
                                    None => continue,
                                },
                            };
                            let mark = if open.has_unsaved_changes() {
                                "• "
                            } else {
                                ""
                            };
                            let mut name = format!("{mark}{}", open.name);
                            if name.chars().count() > 32 {
                                name = name.chars().take(30).collect::<String>() + "…";
                            }
                            let hover = open
                                .path
                                .as_ref()
                                .map_or(open.name.clone(), |p| p.display().to_string());
                            let (tab, close_button) =
                                super::controls::document_tab(ui, i, &name, front, &close_tip);
                            places.push((i, tab.rect.union(close_button.rect)));
                            if tab.dragged() {
                                dragged = Some((i, name.clone()));
                            }
                            if tab.drag_stopped() {
                                dropped = Some(i);
                            }
                            let tab = if dragged.is_some() {
                                tab
                            } else {
                                tab.on_hover_text(hover)
                            };
                            if tab.clicked() {
                                go = Some(i);
                            }
                            if tab.middle_clicked() || close_button.clicked() {
                                close = Some(i);
                            }
                            tab.context_menu(|ui| {
                                let left = ui.add_enabled(i > 0, egui::Button::new("Move left"));
                                if left.clicked() {
                                    moved = Some((i, i - 1));
                                }
                                let right =
                                    ui.add_enabled(i + 1 < n, egui::Button::new("Move right"));
                                if right.clicked() {
                                    moved = Some((i, i + 1));
                                }
                                let window = ui
                                    .add_enabled(several, egui::Button::new("Move to new window"));
                                if window.clicked() {
                                    pop = Some((i, None));
                                }
                                if let Some(path) = &open.path {
                                    ui.separator();
                                    if ui.button("Copy File Path").clicked() {
                                        copy_path = Some(path.clone());
                                        ui.close();
                                    }
                                    #[cfg(windows)]
                                    if ui.button("Show in File Explorer").clicked() {
                                        reveal = Some(path.clone());
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                if ui.button("Close tab").clicked() {
                                    close = Some(i);
                                }
                                if ui
                                    .add_enabled(
                                        !self.closed_tabs.is_empty(),
                                        egui::Button::new("Reopen Closed Tab"),
                                    )
                                    .clicked()
                                {
                                    reopen = true;
                                    ui.close();
                                }
                            });
                        }
                    });
                });
        });
        if let Some(path) = copy_path {
            let path = std::path::absolute(&path).unwrap_or(path);
            #[cfg(windows)]
            let printable = super::commands::explorer_path(&path);
            #[cfg(not(windows))]
            let printable = path.as_os_str();
            ui.ctx().copy_text(printable.to_string_lossy().into_owned());
        }
        #[cfg(windows)]
        if let Some(path) = reveal {
            self.reveal_path(&path);
        }
        if reopen {
            self.reopen_closed_tab();
        }
        let strip = bar.response.rect;
        self.tab_strip = TabStrip::on_screen(ui.ctx(), strip, &places);
        if let Some((from, name)) = dragged {
            self.drag_tab(ui.ctx(), strip, &places, from, &name);
        } else if let Some(from) = dropped {
            if let Some(drag) = self.tab_drag.take()
                && !drag.cancelled
                && !ui.input(|i| i.key_pressed(Key::Escape))
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos()).or(drag.last)
            {
                match drag_target(ui.ctx(), strip, &places, pos) {
                    Target::Slot(slot) => moved = Some((from, slot_index(from, slot))),
                    Target::Window(Some(at)) => {
                        self.tab_drop = Some((from, at));
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    Target::Window(None) => pop = Some((from, None)),
                }
            }
        } else {
            self.tab_drag = None;
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        } else if let Some((i, at)) = pop {
            self.request_pop_out(i, at);
        } else if let Some((from, to)) = moved {
            self.move_tab(from, to);
        } else if let Some(i) = go {
            self.switch_to(i);
        }
    }

    /// Follows a tab being dragged and shows where it would go. Returns
    /// the screen point for its new window once it is dragged out of this
    /// one.
    fn drag_tab(
        &mut self,
        ctx: &egui::Context,
        strip: Rect,
        places: &[(usize, Rect)],
        from: usize,
        name: &str,
    ) {
        let drag = self.tab_drag.get_or_insert_with(TabDrag::default);
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            drag.cancelled = true;
        }
        if drag.cancelled {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.interact_pos()).or(drag.last) else {
            return;
        };
        drag.last = Some(pos);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let target = drag_target(ctx, strip, places, pos);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("tab-drag"),
        ));
        let visuals = ctx.global_style().visuals.clone();
        // A line in the gap the tab would go into.
        if let Target::Slot(slot) = target
            && slot_index(from, slot) != from
            && let Some((_, first)) = places.first()
        {
            let x = match slot {
                0 => first.left() - 2.0,
                s => places[(s - 1).min(places.len() - 1)].1.right() + 2.0,
            };
            painter.line_segment(
                [
                    egui::pos2(x, first.top() + 4.0),
                    egui::pos2(x, first.bottom() - 4.0),
                ],
                egui::Stroke::new(2.0, visuals.selection.bg_fill),
            );
        }
        // The tab under the pointer, saying what letting go there does.
        let label = if matches!(target, Target::Window(_)) {
            format!("{name}\nDrop to move tab")
        } else {
            name.to_owned()
        };
        let galley = painter.layout_no_wrap(
            label,
            egui::FontId::proportional(13.0),
            visuals.text_color(),
        );
        let ghost = Rect::from_min_size(pos + Vec2::new(12.0, 10.0), galley.size())
            .expand2(Vec2::new(10.0, 6.0));
        painter.rect(
            ghost,
            6.0,
            visuals.window_fill,
            visuals.window_stroke,
            egui::StrokeKind::Outside,
        );
        painter.galley(
            ghost.min + Vec2::new(10.0, 6.0),
            galley,
            visuals.text_color(),
        );
    }
}

pub(super) struct TabStrip {
    pub(super) rect: Rect,
    pub(super) centers: Vec<f32>,
}

impl TabStrip {
    fn on_screen(ctx: &egui::Context, local: Rect, places: &[(usize, Rect)]) -> Option<Self> {
        let window = ctx.input(|input| input.viewport().inner_rect)?;
        let origin = window.min.to_vec2();
        let scale = ctx.pixels_per_point();
        let point = |local: Pos2| ((local + origin).to_vec2() * scale).to_pos2();
        Some(Self {
            rect: Rect::from_min_max(point(local.min), point(local.max)),
            centers: places
                .iter()
                .map(|(_, rect)| point(rect.center()).x)
                .collect(),
        })
    }

    pub(super) fn slot(&self, point: Pos2) -> Option<usize> {
        self.rect
            .contains(point)
            .then(|| self.centers.iter().filter(|x| **x < point.x).count())
    }
}

/// How far from the row of tabs one is dragged to come out of it.
const DETACH: f32 = 40.0;

/// A tab being dragged.
#[derive(Default)]
pub(super) struct TabDrag {
    /// Where the pointer last was in the window.
    last: Option<Pos2>,
    /// Escape was pressed; letting go does nothing.
    cancelled: bool,
}

/// Where a dragged tab goes when it is let go.
enum Target {
    /// Before the tab at this place in the row, or after the last.
    Slot(usize),
    /// An external drop in physical screen pixels.
    Window(Option<Pos2>),
}

fn drag_target(ctx: &egui::Context, strip: Rect, places: &[(usize, Rect)], pos: Pos2) -> Target {
    if !ctx.content_rect().contains(pos)
        || pos.y < strip.top() - DETACH
        || pos.y > strip.bottom() + DETACH
    {
        let screen = ctx
            .input(|input| input.viewport().inner_rect)
            .map(|window| {
                ((window.min.to_vec2() + pos.to_vec2()) * ctx.pixels_per_point()).to_pos2()
            });
        Target::Window(screen)
    } else {
        Target::Slot(
            places
                .iter()
                .filter(|(_, rect)| rect.center().x < pos.x)
                .count(),
        )
    }
}

/// Where tab `from` ends up when let go before the tab at `slot`
/// (counting itself); past the end puts it last.
fn slot_index(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
}

#[cfg(test)]
mod order_tests {
    use super::slot_index;

    #[test]
    fn a_dragged_tab_lands_in_the_gap_it_was_let_go_in() {
        // Tabs 0 1 2 3: tab 1 let go before tab 3 lands third.
        assert_eq!(slot_index(1, 3), 2);
        assert_eq!(slot_index(2, 0), 0);
        // Either side of itself: no move.
        assert_eq!(slot_index(1, 1), 1);
        assert_eq!(slot_index(1, 2), 1);
        // After the last.
        assert_eq!(slot_index(0, 4), 3);
    }
}

#[cfg(test)]
mod update_guard_tests {
    use super::*;

    #[test]
    fn updates_detect_pending_work_in_active_and_parked_tabs() {
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(egui::Context::default()),
            crate::settings::Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        );
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec();
        viewer.open_with_password(pdf, "pending.pdf".into(), "", None);
        assert!(!viewer.update_has_unsaved_work());
        viewer.open.as_mut().unwrap().layers_pending = true;
        assert!(viewer.update_has_unsaved_work());
        let parked = viewer.park().unwrap();
        viewer.tabs.clear();
        viewer.tabs.push(Some(parked));
        assert!(viewer.update_has_unsaved_work());
        viewer.tabs[0].as_mut().unwrap().open.layers_pending = false;
        assert!(!viewer.update_has_unsaved_work());
    }
}
