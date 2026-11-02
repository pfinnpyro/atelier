//! The sidebar: small pictures of the pages (one by one, or as two-page
//! spreads) and the outline, from the file's bookmarks or from headings
//! found in its text. Pages are picked here to move (by dragging), turn,
//! remove or save on their own, and other files' pages are added here.

use std::collections::{BTreeSet, HashSet};

use atelier_raster::Point;
use atelier_view::{Layout, OutlineItem};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use super::Viewer;

/// Page pictures are drawn this wide at most, in logical pixels, and
/// rendered at this size.
const THUMB_W: f32 = 150.0;
const THUMB_H: f32 = 200.0;
const ROW_GAP: f32 = 22.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Pages,
    Outline,
    Search,
}

#[derive(Default)]
pub(super) struct Sidebar {
    tab: Tab,
    search_visible: bool,
    /// Pages shown as two-page spreads, the first page on its own.
    spreads: bool,
    /// Page pictures the sidebar would show now and does not have.
    pub wanted: Vec<usize>,
    /// Outline entries folded shut, by index.
    collapsed: HashSet<usize>,
    /// The page in view as of the last frame, to follow it.
    followed: Option<usize>,
    /// Pages picked in the list.
    pub selected: BTreeSet<usize>,
    /// Where a Shift+click range starts.
    anchor: Option<usize>,
}

/// What the sidebar keeps for a file while another tab is in front.
pub(super) struct SidebarState {
    search_visible: bool,
    collapsed: HashSet<usize>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
}

impl Sidebar {
    pub fn take_state(&mut self) -> SidebarState {
        let state = SidebarState {
            search_visible: self.search_visible,
            collapsed: std::mem::take(&mut self.collapsed),
            selected: std::mem::take(&mut self.selected),
            anchor: self.anchor.take(),
        };
        self.reset();
        state
    }

    pub fn restore_state(&mut self, state: SidebarState) {
        self.reset();
        self.search_visible = state.search_visible;
        self.collapsed = state.collapsed;
        self.selected = state.selected;
        self.anchor = state.anchor;
    }

    pub fn find(&mut self) {
        self.search_visible = true;
        self.tab = Tab::Search;
    }

    pub fn reset(&mut self) {
        self.search_visible = false;
        self.tab = Tab::Pages;
        self.collapsed.clear();
        self.followed = None;
        self.wanted.clear();
        self.selected.clear();
        self.anchor = None;
    }
}

/// What the page list asks for, done once the list is drawn.
enum PageAction {
    Go(usize),
    Move(Vec<usize>, usize),
    Rotate(Vec<usize>, i32),
    Delete(Vec<usize>),
    Organize(Vec<usize>),
}

/// The menu for a right click on a page picture. `pages` are the pages it
/// acts on; `at` is the page clicked.
fn page_menu(ui: &mut egui::Ui, pages: &[usize], count: usize) -> Option<PageAction> {
    let n = pages.len();
    let these = if n == 1 {
        "this page".to_string()
    } else {
        format!("these {n} pages")
    };
    let mut action = None;
    if ui.button(format!("Rotate {these} clockwise")).clicked() {
        action = Some(PageAction::Rotate(pages.to_vec(), 1));
    }
    if ui
        .button(format!("Rotate {these} counterclockwise"))
        .clicked()
    {
        action = Some(PageAction::Rotate(pages.to_vec(), -1));
    }
    ui.separator();
    if ui
        .add_enabled(
            n < count,
            egui::Button::new(format!("Remove {these}")).shortcut_text("Delete"),
        )
        .clicked()
    {
        action = Some(PageAction::Delete(pages.to_vec()));
    }
    ui.separator();
    if ui.button("Organize selected pages…").clicked() {
        action = Some(PageAction::Organize(pages.to_vec()));
    }
    if action.is_some() {
        ui.close();
    }
    action
}

/// The rows of the page list: one page each, or spreads (the first page
/// alone, then pairs).
fn rows(count: usize, spreads: bool) -> Vec<Vec<usize>> {
    if !spreads {
        return (0..count).map(|p| vec![p]).collect();
    }
    let mut out = Vec::new();
    if count > 0 {
        out.push(vec![0]);
    }
    let mut p = 1;
    while p < count {
        out.push((p..(p + 2).min(count)).collect());
        p += 2;
    }
    out
}

/// The scale (device pixels per point) a page picture is rendered at.
pub(super) fn thumb_scale(size: (f64, f64), pixels_per_point: f64) -> f64 {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    (f64::from(THUMB_W) * 1.4 / w).min(f64::from(THUMB_H) * 1.4 / h) * pixels_per_point
}

impl Viewer {
    pub(super) fn sidebar_panel(&mut self, ui: &mut egui::Ui) {
        if self.open.is_none() {
            return;
        }
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .min_size(200.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                let current = self.current_page();
                let count = self.open.as_ref().map_or(0, |o| o.layout.page_count());
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(current > 0, crate::icons::Icon::Left.button())
                        .on_hover_text("Previous page")
                        .clicked()
                    {
                        self.go_to_page(current - 1);
                    }
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.page_input)
                            .desired_width(48.0)
                            .align(egui::Align2::CENTER_CENTER)
                            .hint_text("Page"),
                    );
                    if resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Ok(n) = self.page_input.trim().parse::<usize>()
                    {
                        self.go_to_page(n.saturating_sub(1));
                    }
                    if !resp.has_focus() {
                        self.page_input = if count > 0 {
                            (current + 1).to_string()
                        } else {
                            String::new()
                        };
                    }
                    ui.label(format!("/ {count}"));
                    if ui
                        .add_enabled(current + 1 < count, crate::icons::Icon::Right.button())
                        .on_hover_text("Next page")
                        .clicked()
                    {
                        self.go_to_page(current + 1);
                    }
                    if crate::icons::button(ui, crate::icons::Icon::Find, "Find (Ctrl+F)").clicked()
                    {
                        self.request_find();
                    }
                });
                if self.sidebar.search_visible {
                    egui::Frame::new()
                        .fill(ui.visuals().extreme_bg_color)
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .corner_radius(7)
                        .inner_margin(4)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let width = (ui.available_width() - 88.0).max(60.0);
                                let search = ui.add(
                                    egui::TextEdit::singleline(&mut self.search_input)
                                        .frame(egui::Frame::NONE)
                                        .desired_width(width)
                                        .hint_text("Search document…"),
                                );
                                if self.focus_search {
                                    search.request_focus();
                                    self.focus_search = false;
                                }
                                if search.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    self.run_search(ui.input(|i| i.modifiers.shift));
                                    search.request_focus();
                                }
                                if ui
                                    .add_sized([24.0, 24.0], egui::Button::new("↑").frame(false))
                                    .on_hover_text("Previous match (Shift+Enter)")
                                    .clicked()
                                {
                                    self.run_search(true);
                                }
                                if ui
                                    .add_sized([24.0, 24.0], egui::Button::new("↓").frame(false))
                                    .on_hover_text("Next match (Enter, F3)")
                                    .clicked()
                                {
                                    self.run_search(false);
                                }
                                if ui
                                    .add_sized(
                                        [24.0, 24.0],
                                        crate::icons::Icon::Close
                                            .button()
                                            .min_size(egui::Vec2::splat(24.0))
                                            .frame(false),
                                    )
                                    .on_hover_text("Close search (Esc)")
                                    .clicked()
                                    || (search.has_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Escape)))
                                {
                                    self.sidebar.search_visible = false;
                                }
                            });
                        });
                    if !self.search.query.is_empty() {
                        let n = self.search.hits.len();
                        let label = match (self.search.current, self.search.done) {
                            (_, false) => format!("{n} found, searching…"),
                            (_, true) if n == 0 => "No matches".to_string(),
                            (Some(c), true) => format!("{} of {n}", c + 1),
                            (None, true) => format!("{n} found"),
                        };
                        ui.label(label);
                    }
                    if !self.sidebar.search_visible {
                        self.pool.stop_search();
                        self.search = Default::default();
                        self.search_input.clear();
                        self.sidebar.tab = Tab::Pages;
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    let tab = &mut self.sidebar.tab;
                    ui.selectable_value(tab, Tab::Pages, "Pages");
                    ui.selectable_value(tab, Tab::Outline, "Outline");
                    if self.sidebar.search_visible {
                        ui.selectable_value(tab, Tab::Search, "Results");
                    }
                });
                if self.sidebar.tab == Tab::Pages {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.sidebar.spreads, "Thumbnail spreads");
                        ui.weak(format!("{count} pages"));
                    });
                }
                ui.separator();
                match self.sidebar.tab {
                    Tab::Pages => self.page_list(ui),
                    Tab::Outline => self.outline_list(ui),
                    Tab::Search => {
                        if self.search.hits.is_empty() {
                            ui.label("Search the document text with the field above.");
                        } else {
                            let hits: Vec<_> = self
                                .search
                                .hits
                                .iter()
                                .enumerate()
                                .map(|(i, hit)| (i, hit.page))
                                .collect();
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for (i, page) in hits {
                                    let label = format!("Match {} · page {}", i + 1, page + 1);
                                    if ui
                                        .selectable_label(self.search.current == Some(i), label)
                                        .clicked()
                                    {
                                        self.search.current = Some(i);
                                        self.reveal_hit(i);
                                    }
                                }
                            });
                        }
                    }
                }
            });
    }

    fn page_list(&mut self, ui: &mut egui::Ui) {
        let current = self.current_page();
        let Some(o) = &self.open else { return };
        let count = o.layout.page_count();
        let rows = rows(count, self.sidebar.spreads);
        let width = ui.available_width().max(60.0);
        // Each row's height: its tallest page at the width it gets.
        let fit = |page: usize, w: f32| -> Vec2 {
            let (pw, ph) = o.layout.geom(page).map_or((612.0, 792.0), |g| g.size());
            let s = (w / pw as f32).min(THUMB_H / ph as f32 * (w / THUMB_W).max(1.0));
            Vec2::new(pw as f32 * s, ph as f32 * s)
        };
        let cell = || {
            let w = if self.sidebar.spreads {
                (width - 8.0) / 2.0
            } else {
                width - 8.0
            };
            w.min(if self.sidebar.spreads {
                THUMB_W
            } else {
                THUMB_W * 1.4
            })
        };
        let heights: Vec<f32> = rows
            .iter()
            .map(|r| {
                let w = cell();
                r.iter().map(|&p| fit(p, w).y).fold(0.0, f32::max) + ROW_GAP
            })
            .collect();
        let total: f32 = heights.iter().sum::<f32>() + 8.0;
        let follow = self.sidebar.followed != Some(current);
        self.sidebar.followed = Some(current);
        let mut wanted = Vec::new();
        let mut action: Option<PageAction> = None;
        let selected = &mut self.sidebar.selected;
        selected.retain(|&p| p < count);
        let anchor = &mut self.sidebar.anchor;
        // Pages are dragged to a new place one row at a time, so not in
        // spreads.
        let can_drag = !self.sidebar.spreads;
        let mut drop_line: Option<(f32, usize)> = None;
        let painter_bg = ui.visuals().extreme_bg_color;
        egui::ScrollArea::vertical()
            .id_salt("page-list")
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                let (area, _) = ui.allocate_exact_size(Vec2::new(width, total), Sense::hover());
                // Where a dragged page would land: before the first row
                // whose middle is below the pointer.
                let pointer = ui.ctx().pointer_interact_pos();
                let mut tops = Vec::with_capacity(rows.len());
                let mut y = 4.0;
                for h in &heights {
                    tops.push(area.min.y + y);
                    y += h;
                }
                let landing = |py: f32| -> (f32, usize) {
                    for (r, row) in rows.iter().enumerate() {
                        let mid = tops[r] + (heights[r] - ROW_GAP) / 2.0;
                        if py < mid {
                            return (tops[r] - 2.0, row[0]);
                        }
                    }
                    (area.min.y + y - ROW_GAP + 2.0, count)
                };
                let mut y = 4.0;
                for (r, row) in rows.iter().enumerate() {
                    let h = heights[r];
                    let row_rect = Rect::from_min_size(
                        Pos2::new(area.min.x, area.min.y + y),
                        Vec2::new(width, h),
                    );
                    let has_current = row.contains(&current);
                    if has_current && follow {
                        ui.scroll_to_rect(row_rect, None);
                    }
                    let visible = y + h >= viewport.min.y - 200.0 && y <= viewport.max.y + 200.0;
                    y += h;
                    if !visible {
                        continue;
                    }
                    let w = cell();
                    let sizes: Vec<Vec2> = row.iter().map(|&p| fit(p, w)).collect();
                    let row_w: f32 = sizes.iter().map(|s| s.x).sum::<f32>();
                    // Spreads meet in the middle; a lone first page sits on
                    // the right, where a book's first page is.
                    let mut x = if self.sidebar.spreads && row.len() == 1 && row[0] == 0 {
                        row_rect.center().x
                    } else {
                        row_rect.center().x - row_w / 2.0
                    };
                    let mut row_hit = Rect::NOTHING;
                    for (&page, size) in row.iter().zip(&sizes) {
                        let rect = Rect::from_min_size(Pos2::new(x, row_rect.min.y), *size);
                        x += size.x;
                        row_hit = row_hit.union(rect);
                        let p = ui.painter();
                        p.rect_filled(
                            rect.translate(Vec2::new(1.5, 2.0)),
                            0.0,
                            Color32::from_black_alpha(80),
                        );
                        p.rect_filled(rect, 0.0, Color32::WHITE);
                        match o.thumbs.get(&page) {
                            Some(t) => {
                                p.image(
                                    t.id(),
                                    rect,
                                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                                if o.thumb_stale.contains(&page) {
                                    wanted.push(page);
                                }
                            }
                            None => {
                                p.rect_filled(rect.shrink(1.0), 0.0, painter_bg);
                                wanted.push(page);
                            }
                        }
                        let sense = if can_drag {
                            Sense::click_and_drag()
                        } else {
                            Sense::click()
                        };
                        let resp = ui
                            .interact(rect, ui.scope_id().with(("thumb", page)), sense)
                            .on_hover_text(format!("Page {}", page + 1));
                        let picked = selected.contains(&page);
                        if resp.clicked() {
                            let m = ui.input(|i| i.modifiers);
                            if m.shift {
                                let from = anchor.unwrap_or(current);
                                *selected = (from.min(page)..=from.max(page)).collect();
                            } else if m.command {
                                if !selected.remove(&page) {
                                    selected.insert(page);
                                }
                                *anchor = Some(page);
                            } else {
                                *selected = BTreeSet::from([page]);
                                *anchor = Some(page);
                                action = Some(PageAction::Go(page));
                            }
                        }
                        if resp.secondary_clicked() && !picked {
                            *selected = BTreeSet::from([page]);
                            *anchor = Some(page);
                        }
                        // The pages a drag or the menu acts on: the picked
                        // ones when this page is one of them.
                        let targets: Vec<usize> = if picked && selected.len() > 1 {
                            selected.iter().copied().collect()
                        } else {
                            vec![page]
                        };
                        resp.context_menu(|ui| {
                            if let Some(a) = page_menu(ui, &targets, count) {
                                action = Some(a);
                            }
                        });
                        if resp.dragged() {
                            if let Some(p) = pointer {
                                drop_line = Some(landing(p.y));
                                // Scroll when held near the top or bottom.
                                let clip = ui.clip_rect();
                                if p.y < clip.min.y + 30.0 {
                                    ui.scroll_with_delta(Vec2::new(0.0, 8.0));
                                } else if p.y > clip.max.y - 30.0 {
                                    ui.scroll_with_delta(Vec2::new(0.0, -8.0));
                                }
                            }
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        }
                        if resp.drag_stopped()
                            && let Some(p) = pointer
                        {
                            let (_, to) = landing(p.y);
                            action = Some(PageAction::Move(targets.clone(), to));
                        }
                        if picked {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                ui.visuals().selection.bg_fill.gamma_multiply(0.25),
                            );
                        }
                        if page == current || resp.hovered() || picked {
                            let stroke = if page == current || picked {
                                Stroke::new(2.5, ui.visuals().selection.bg_fill)
                            } else {
                                Stroke::new(1.0, ui.visuals().widgets.hovered.fg_stroke.color)
                            };
                            ui.painter()
                                .rect_stroke(rect, 0.0, stroke, StrokeKind::Outside);
                        }
                    }
                    let label = match row.as_slice() {
                        [a, b] => format!("{}–{}", a + 1, b + 1),
                        [a] => format!("{}", a + 1),
                        _ => String::new(),
                    };
                    ui.painter().text(
                        Pos2::new(row_hit.center().x, row_hit.max.y + 3.0),
                        egui::Align2::CENTER_TOP,
                        label,
                        egui::FontId::proportional(11.0),
                        if has_current {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().weak_text_color()
                        },
                    );
                }
                if let Some((y, _)) = drop_line {
                    ui.painter().hline(
                        area.x_range().shrink(6.0),
                        y,
                        Stroke::new(3.0, ui.visuals().selection.bg_fill),
                    );
                }
            });
        // Delete removes the picked pages while the pointer is over the
        // list and nothing is being typed.
        if action.is_none()
            && !selected.is_empty()
            && selected.len() < count
            && ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| ui.max_rect().contains(p))
            && ui.memory(|m| m.focused().is_none())
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
        {
            action = Some(PageAction::Delete(selected.iter().copied().collect()));
        }
        self.sidebar.wanted = wanted;
        match action {
            Some(PageAction::Go(p)) => {
                self.go_to_page(p);
                self.sidebar.followed = Some(p);
            }
            Some(PageAction::Move(pages, to)) => {
                let moved = self.move_pages(&pages, to);
                self.sidebar.selected = moved.iter().copied().collect();
                self.sidebar.anchor = moved.first().copied();
            }
            Some(PageAction::Rotate(pages, quarters)) => self.rotate_some(&pages, quarters),
            Some(PageAction::Delete(pages)) => self.delete_pages(&pages),
            Some(PageAction::Organize(pages)) => {
                self.sidebar.selected = pages.into_iter().collect();
                self.organize_pages(self.sidebar.selected.clone());
            }
            None => {}
        }
    }

    fn outline_list(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &mut self.open else { return };
        if !o.outline_asked {
            o.outline_asked = true;
            let wake = self.ctx.clone();
            self.pool
                .outline(o.doc.clone(), o.opened, o.ocr_texts.clone(), move || {
                    wake.request_repaint()
                });
        }
        let Some((items, from_headings)) = &o.outline else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the outline…");
            });
            return;
        };
        if items.is_empty() {
            ui.label("This file has no bookmarks, and no headings stood out in its text.");
            return;
        }
        if *from_headings {
            ui.label(
                RichText::new("No bookmarks: these are headings found by their font size.")
                    .weak()
                    .small(),
            );
        }
        let items = items.clone();
        let mut go = None;
        let collapsed = &mut self.sidebar.collapsed;
        egui::ScrollArea::both()
            .id_salt("outline")
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut hide_below: Option<usize> = None;
                for (i, item) in items.iter().enumerate() {
                    if let Some(d) = hide_below {
                        if item.depth > d {
                            continue;
                        }
                        hide_below = None;
                    }
                    let has_kids = items.get(i + 1).is_some_and(|n| n.depth > item.depth);
                    let shut = collapsed.contains(&i);
                    ui.horizontal(|ui| {
                        ui.add_space(item.depth as f32 * 14.0);
                        if has_kids {
                            let arrow = if shut { "▸" } else { "▾" };
                            if ui.small_button(arrow).clicked() {
                                if shut {
                                    collapsed.remove(&i);
                                } else {
                                    collapsed.insert(i);
                                }
                            }
                        } else {
                            ui.add_space(18.0);
                        }
                        let title = if item.title.is_empty() {
                            "(untitled)"
                        } else {
                            &item.title
                        };
                        let text = if item.depth == 0 {
                            RichText::new(title).strong()
                        } else {
                            RichText::new(title)
                        };
                        let resp = ui.add(egui::Button::new(text).frame(false).truncate());
                        let resp = match item.page {
                            Some(p) => resp.on_hover_text(format!("Page {}", p + 1)),
                            None => resp,
                        };
                        if resp.clicked() {
                            go = Some(item.clone());
                        }
                    });
                    if has_kids && shut {
                        hide_below = Some(item.depth);
                    }
                }
            });
        if let Some(item) = go {
            self.go_to_item(&item);
        }
    }

    /// Scrolls to an outline entry: its page, at its height when given.
    fn go_to_item(&mut self, item: &OutlineItem) {
        let Some(page) = item.page else { return };
        let Some(o) = &self.open else { return };
        let page = page.min(o.layout.page_count().saturating_sub(1));
        let (Some(top), Some(t), Some(g)) =
            (item.top, o.layout.user_to_layout(page), o.layout.geom(page))
        else {
            self.go_to_page(page);
            return;
        };
        let p = t.apply(Point::new(g.crop.x0, top));
        let r = o.layout.page_rect(page);
        // Keep the spot on the page, a little below the top of the view.
        let y = (p.y - 16.0).max(r.map_or(0.0, |r| r.y - Layout::GAP / 2.0));
        self.scroll_to = Some(Vec2::new(self.offset.x, y.max(0.0) as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::rows;

    #[test]
    fn spreads_put_the_first_page_alone() {
        assert_eq!(rows(5, true), vec![vec![0], vec![1, 2], vec![3, 4]]);
        assert_eq!(rows(4, true), vec![vec![0], vec![1, 2], vec![3]]);
        assert_eq!(rows(2, false), vec![vec![0], vec![1]]);
    }
}
