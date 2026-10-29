//! The sidebar: small pictures of the pages (one by one, or as two-page
//! spreads) and the outline, from the file's bookmarks or from headings
//! found in its text.

use std::collections::HashSet;

use atelier_raster::Point;
use atelier_view::{Layout, OutlineItem};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use super::Viewer;

/// Page pictures are drawn this wide at most, in logical pixels, and
/// rendered at this size.
const THUMB_W: f32 = 150.0;
const THUMB_H: f32 = 200.0;
const ROW_GAP: f32 = 22.0;
const ACCENT: Color32 = Color32::from_rgb(40, 110, 230);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Pages,
    Outline,
}

#[derive(Default)]
pub(super) struct Sidebar {
    tab: Tab,
    /// Pages shown as two-page spreads, the first page on its own.
    spreads: bool,
    /// Page pictures the sidebar would show now and does not have.
    pub wanted: Vec<usize>,
    /// Outline entries folded shut, by index.
    collapsed: HashSet<usize>,
    /// The page in view as of the last frame, to follow it.
    followed: Option<usize>,
}

impl Sidebar {
    pub fn reset(&mut self) {
        self.collapsed.clear();
        self.followed = None;
        self.wanted.clear();
    }
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
            .default_size(THUMB_W + 40.0)
            .min_size(120.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let tab = &mut self.sidebar.tab;
                    ui.selectable_value(tab, Tab::Pages, "Pages");
                    ui.selectable_value(tab, Tab::Outline, "Outline");
                    if self.sidebar.tab == Tab::Pages {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.toggle_value(&mut self.sidebar.spreads, "Spreads")
                                .on_hover_text("Show pages side by side, as a book opens");
                        });
                    }
                });
                ui.separator();
                match self.sidebar.tab {
                    Tab::Pages => self.page_list(ui),
                    Tab::Outline => self.outline_list(ui),
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
        let mut go = None;
        let painter_bg = ui.visuals().extreme_bg_color;
        egui::ScrollArea::vertical()
            .id_salt("page-list")
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                let (area, _) = ui.allocate_exact_size(Vec2::new(width, total), Sense::hover());
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
                        let resp = ui
                            .interact(rect, ui.scope_id().with(("thumb", page)), Sense::click())
                            .on_hover_text(format!("Page {}", page + 1));
                        if resp.clicked() {
                            go = Some(page);
                        }
                        if page == current || resp.hovered() {
                            let stroke = if page == current {
                                Stroke::new(2.5, ACCENT)
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
            });
        self.sidebar.wanted = wanted;
        if let Some(p) = go {
            self.go_to_page(p);
            self.sidebar.followed = Some(p);
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
