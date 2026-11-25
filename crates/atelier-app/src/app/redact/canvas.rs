use atelier_raster::Point;
use atelier_view::Selection;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind};

use super::{Marks, Redaction, Viewer};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tool {
    Area,
    Text,
    Image,
    Table,
}

impl Tool {
    pub(super) const ALL: [Self; 4] = [Self::Area, Self::Text, Self::Image, Self::Table];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Area => "Area",
            Self::Text => "Text",
            Self::Image => "Image",
            Self::Table => "Table cells",
        }
    }

    pub(super) fn help(self) -> &'static str {
        match self {
            Self::Area => "Drag a rectangle over content to redact",
            Self::Text => "Drag to select text or double-click a word",
            Self::Image => "Click an image to redact it",
            Self::Table => "Click a detected cell or drag across several cells",
        }
    }
}

pub(super) struct Drag {
    page: usize,
    start: Pos2,
    end: Pos2,
}

impl Redaction {
    fn add(&mut self, zones: Marks) {
        let mut action = Vec::new();
        for (page, rects) in zones {
            let marks = self.marks.entry(page).or_default();
            let previous = marks.len();
            for rect in rects {
                if rect.is_positive() && !marks.contains(&rect) {
                    marks.push(rect);
                }
            }
            if previous != marks.len() {
                action.push((page, previous));
            }
        }
        if !action.is_empty() {
            self.history.push(action);
        }
    }

    pub(super) fn undo(&mut self) {
        if let Some(action) = self.history.pop() {
            for (page, count) in action {
                if let Some(marks) = self.marks.get_mut(&page) {
                    marks.truncate(count);
                }
            }
        }
    }

    pub(in crate::app) fn paint(&self, painter: &egui::Painter, page: usize, screen: Rect) {
        let paint = |rect: Rect, fill: Color32| {
            let to_screen = |p: Pos2| screen.min + p.to_vec2() * screen.size();
            let marked = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max));
            painter.rect_filled(marked, 0.0, fill);
            painter.rect_stroke(
                marked,
                0.0,
                Stroke::new(1.0, Color32::RED),
                StrokeKind::Inside,
            );
        };
        for &rect in self.marks.get(&page).into_iter().flatten() {
            paint(rect, Color32::from_black_alpha(210));
        }
        if let Some(d) = self.drag.as_ref().filter(|d| d.page == page) {
            let fill = if self.tool == Tool::Area {
                Color32::from_black_alpha(210)
            } else {
                Color32::from_black_alpha(40)
            };
            paint(Rect::from_two_pos(d.start, d.end), fill);
        }
    }
}

fn relative(rect: atelier_view::ViewRect, x: f64, y: f64) -> Pos2 {
    egui::pos2(
        ((x - rect.x) / rect.w).clamp(0.0, 1.0) as f32,
        ((y - rect.y) / rect.h).clamp(0.0, 1.0) as f32,
    )
}

fn zone(layout: &atelier_view::Layout, page: usize, points: [Point; 4]) -> Option<Rect> {
    let area = layout.page_rect(page)?;
    let matrix = layout.user_to_layout(page)?;
    let mut rect = Rect::NOTHING;
    for point in points {
        let p = matrix.apply(point);
        rect.extend_with(relative(area, p.x, p.y));
    }
    rect.is_positive().then_some(rect)
}

fn glyph_zones(
    layout: &atelier_view::Layout,
    page: usize,
    glyphs: &[atelier_render::TextGlyph],
) -> Vec<Rect> {
    let (Some(area), Some(matrix)) = (layout.page_rect(page), layout.user_to_layout(page)) else {
        return vec![];
    };
    let mut zones: Vec<Rect> = Vec::new();
    let tolerance = |length| (layout.zoom() * 2.0 / length) as f32;
    let mut previous_vertical = false;
    for glyph in glyphs {
        let Some(rect) = zone(layout, page, glyph.quad) else {
            continue;
        };
        let direction = matrix.apply_vector(glyph.baseline.1 - glyph.baseline.0);
        let vertical = direction.y.abs() > direction.x.abs();
        let axis = |p: Pos2| if vertical { (p.y, p.x) } else { (p.x, p.y) };
        if let Some(previous) = zones.last_mut().filter(|_| vertical == previous_vertical) {
            let limit = tolerance(if vertical { area.h } else { area.w });
            let (_, p) = axis(previous.center());
            let (_, c) = axis(rect.center());
            let (a0, pa) = axis(previous.min);
            let (a1, pb) = axis(previous.max);
            let (b0, ca) = axis(rect.min);
            let (b1, cb) = axis(rect.max);
            if (p - c).abs() < (pb - pa).min(cb - ca) * 0.25 && a0 <= b1 + limit && b0 <= a1 + limit
            {
                *previous = previous.union(rect);
                continue;
            }
        }
        zones.push(rect);
        previous_vertical = vertical;
    }
    zones
}

impl Viewer {
    pub(in crate::app) fn redaction_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        to_layout: impl Fn(Pos2) -> (f64, f64),
    ) -> bool {
        let Some(mut redaction) = self.redaction.take() else {
            return false;
        };
        if redaction.saving.is_none() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                redaction.drag = None;
                self.selection = None;
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(if redaction.tool == Tool::Text {
                    egui::CursorIcon::Text
                } else {
                    egui::CursorIcon::Crosshair
                });
            }
            if redaction.tool == Tool::Text {
                self.redaction_text_pointer(ui, response, &to_layout, &mut redaction);
            } else if let Some(open) = &self.open {
                let normalized = |p| {
                    let (x, y) = to_layout(p);
                    let (page, _, _) = open.layout.page_at(x, y)?;
                    Some((page, relative(open.layout.page_rect(page)?, x, y)))
                };
                if redaction.tool != Tool::Image
                    && response.drag_started_by(egui::PointerButton::Primary)
                {
                    redaction.drag = ui
                        .input(|i| i.pointer.press_origin())
                        .and_then(normalized)
                        .map(|(page, start)| Drag {
                            page,
                            start,
                            end: start,
                        });
                }
                if let Some(drag) = &mut redaction.drag
                    && let Some(area) = open.layout.page_rect(drag.page)
                    && let Some(p) = ui.input(|i| i.pointer.interact_pos())
                {
                    let (x, y) = to_layout(p);
                    drag.end = relative(area, x, y);
                }
                let selection = if response.drag_stopped_by(egui::PointerButton::Primary) {
                    redaction
                        .drag
                        .take()
                        .map(|d| (d.page, Rect::from_two_pos(d.start, d.end)))
                } else if response.clicked_by(egui::PointerButton::Primary)
                    && redaction.tool != Tool::Area
                {
                    response
                        .interact_pointer_pos()
                        .and_then(normalized)
                        .map(|(page, point)| (page, Rect::from_min_max(point, point)))
                } else {
                    None
                };
                if let Some((page, rect)) = selection {
                    let zones = self.redaction_object_zones(redaction.tool, page, rect);
                    if zones.is_empty() && redaction.tool != Tool::Area {
                        self.notice = Some(format!(
                            "No {} found here. Use Area to mark this content.",
                            redaction.tool.label().to_lowercase()
                        ));
                    }
                    redaction.add(Marks::from([(page, zones)]));
                }
            }
        }
        self.redaction = Some(redaction);
        true
    }

    fn redaction_text_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        to_layout: &impl Fn(Pos2) -> (f64, f64),
        redaction: &mut Redaction,
    ) {
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.selection = ui
                .input(|i| i.pointer.press_origin())
                .and_then(|p| {
                    let (x, y) = to_layout(p);
                    self.over_text(x, y).then(|| self.caret_at(x, y)).flatten()
                })
                .map(|anchor| Selection {
                    anchor,
                    focus: anchor,
                });
        }
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let (x, y) = to_layout(p);
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(focus) = self.caret_at(x, y)
                && let Some(selection) = &mut self.selection
            {
                selection.focus = focus;
            }
            if response.double_clicked()
                && self.over_text(x, y)
                && let Some(open) = &self.open
                && let Some((page, _, _)) = open.layout.page_at(x, y)
                && let (Some(text), Some(point)) =
                    (open.text.get(&page), open.layout.to_user(page, x, y))
            {
                self.selection = atelier_view::word_at(text, page, point);
            }
        }
        if (response.drag_stopped_by(egui::PointerButton::Primary) || response.double_clicked())
            && let Some(selection) = self.selection.take()
            && let Some(open) = &self.open
        {
            let mut marks = Marks::new();
            for page in selection.start().page..=selection.end().page {
                let extracted;
                let text: &atelier_render::PageText = if let Some(text) = open.text.get(&page) {
                    text
                } else {
                    let pulled = open
                        .doc
                        .pages()
                        .ok()
                        .and_then(|pages| pages.get(page))
                        .and_then(|page| atelier_render::extract_text(&open.doc, page).ok());
                    let Some(text) = pulled else {
                        self.notice = Some(
                            "Text could not be selected on every page. No mark was added.".into(),
                        );
                        return;
                    };
                    extracted = text;
                    &extracted
                };
                if let Some(range) = selection.range_on(page, text.glyphs.len()) {
                    marks.insert(page, glyph_zones(&open.layout, page, &text.glyphs[range]));
                }
            }
            redaction.add(marks);
        }
    }

    fn redaction_object_zones(&self, tool: Tool, index: usize, selection: Rect) -> Vec<Rect> {
        if tool == Tool::Area {
            return selection
                .is_positive()
                .then_some(selection)
                .into_iter()
                .collect();
        }
        let Some(open) = &self.open else {
            return vec![];
        };
        let Some(page) = open.doc.pages().ok().and_then(|pages| pages.get(index)) else {
            return vec![];
        };
        let to_zone = |points| zone(&open.layout, index, points);
        if tool == Tool::Image {
            let Some(area) = open.layout.page_rect(index) else {
                return vec![];
            };
            let click = selection.center();
            let Some(point) = open.layout.to_user(
                index,
                area.x + f64::from(click.x) * area.w,
                area.y + f64::from(click.y) * area.h,
            ) else {
                return vec![];
            };
            return atelier_render::extract_graphics(&open.doc, page)
                .ok()
                .into_iter()
                .flat_map(|g| g.images)
                .filter(|q| {
                    let (x, y) = (q[1] - q[0], q[3] - q[0]);
                    atelier_raster::Transform::new(x.x, x.y, y.x, y.y, q[0].x, q[0].y)
                        .invert()
                        .map(|inverse| inverse.apply(point))
                        .is_some_and(|p| (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y))
                })
                .filter_map(to_zone)
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .into_iter()
                .collect();
        }
        let empty = atelier_render::PageText::default();
        let text = open.text.get(&index).map_or(&empty, |text| text.as_ref());
        atelier_form::detect_page(&open.doc, page, text, &[])
            .tables
            .into_iter()
            .flat_map(|table| table.cells)
            .filter_map(|cell| {
                let r = cell.rect;
                let points = |i| {
                    [
                        Point::new(r.x0 + i, r.y0 + i),
                        Point::new(r.x1 - i, r.y0 + i),
                        Point::new(r.x1 - i, r.y1 - i),
                        Point::new(r.x0 + i, r.y1 - i),
                    ]
                };
                let cell = to_zone(points(0.0))?;
                let picked = if selection.is_positive() {
                    cell.intersects(selection)
                } else {
                    cell.contains(selection.center())
                };
                if !picked {
                    return None;
                }
                // Retain the surrounding rules when marking a cell's contents.
                let inset = 2.0_f64.min(r.width().min(r.height()) / 4.0);
                to_zone(points(inset))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_syntax::{Dict, Document, ObjRef, Object, Stream};
    use std::sync::Arc;

    #[test]
    fn table_zones_follow_canvas_zoom_and_keep_other_cells_and_rules() {
        let ctx = egui::Context::default();
        let mut viewer = Viewer::blank(
            ctx,
            crate::settings::Settings {
                check_updates: false,
                orient_on_open: false,
                ocr: false,
                ..Default::default()
            },
        );
        let doc = Document::open(super::super::super::quality_tests::fixture(2)).unwrap();
        let mut update = doc.update();
        let content = b"20 20 m 140 20 l S 20 70 m 140 70 l S 20 120 m 140 120 l S\n20 20 m 20 120 l S 80 20 m 80 120 l S 140 20 m 140 120 l S\nBT /Helv 10 Tf 28 94 Td (Private) Tj 60 0 Td (Keep) Tj ET";
        update.set(
            ObjRef::new(5, 0),
            Object::Stream(Stream {
                dict: Dict::new(),
                data: Arc::from(content.as_slice()),
            }),
        );
        viewer.open_bytes(update.to_bytes().unwrap(), "Table.pdf".into(), None);
        viewer.open_redaction();
        assert!(!viewer.organize.grid);
        let pick = Rect::from_min_max(egui::pos2(0.2, 0.5), egui::pos2(0.2, 0.5));
        let zones = viewer.redaction_object_zones(Tool::Table, 0, pick);
        let open = viewer.open.as_ref().unwrap();
        let page = &open.doc.pages().unwrap()[0];
        assert_eq!(
            zones.len(),
            1,
            "{:?}",
            atelier_render::extract_graphics(&open.doc, page).unwrap()
        );
        viewer.open.as_mut().unwrap().layout.set_zoom(2.5);
        assert_eq!(zones, viewer.redaction_object_zones(Tool::Table, 0, pick));
        let redaction = viewer.redaction.as_mut().unwrap();
        redaction.add(Marks::from([(0, zones)]));
        let copy = super::super::vector::vector_copy(&redaction.source, &redaction.marks).unwrap();
        let doc = Document::open(copy).unwrap();
        let pages = doc.pages().unwrap();
        assert_eq!(
            atelier_render::extract_text(&doc, &pages[0])
                .unwrap()
                .plain_text(),
            "Keep"
        );
        assert!(
            atelier_render::extract_text(&doc, &pages[1])
                .unwrap()
                .plain_text()
                .contains("Private")
        );
        let rendered = atelier_render::render_page(
            &doc,
            &pages[0],
            &atelier_render::RenderOptions {
                scale: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        let border = rendered.pixmap.pixel(80, 90);
        assert!(border[0] > 90 && border[0] < 170);
        redaction.undo();
        assert!(redaction.marks[&0].is_empty());
    }

    #[test]
    fn rotated_glyph_zones_stay_in_the_same_page_coordinates_when_zooming() {
        let doc = Document::open(super::super::super::quality_tests::fixture(1)).unwrap();
        let page = &doc.pages().unwrap()[0];
        let text = atelier_render::extract_text(&doc, page).unwrap();
        let mut geom = atelier_view::PageGeom::of(page);
        geom.rotate = 90;
        let mut layout = atelier_view::Layout::new(vec![geom], 0.75);
        let first_word = &text.glyphs[..7];
        let marks = glyph_zones(&layout, 0, first_word);
        assert_eq!(marks.len(), 1);
        assert!(marks[0].height() > marks[0].width());
        layout.set_zoom(3.2);
        assert_eq!(marks, glyph_zones(&layout, 0, first_word));
    }

    #[test]
    fn area_drag_on_the_main_canvas_creates_a_mark_that_follows_page_navigation() {
        let ctx = egui::Context::default();
        let mut viewer = Viewer::blank(
            ctx.clone(),
            crate::settings::Settings {
                check_updates: false,
                orient_on_open: false,
                ocr: false,
                ..Default::default()
            },
        );
        viewer.open_bytes(
            super::super::super::quality_tests::fixture(3),
            "Pages.pdf".into(),
            None,
        );
        viewer.zoom_mode = crate::app::ZoomMode::Custom;
        viewer.set_zoom(1.0, egui::Vec2::ZERO);
        viewer.open_redaction();
        let frame = |viewer: &mut Viewer, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(640.0, 260.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::no_frame().show(ui, |ui| viewer.page_view(ui));
                },
            );
            output.textures_delta.clear();
        };
        frame(&mut viewer, vec![]);
        let page = viewer.open.as_ref().unwrap().layout.page_rect(0).unwrap();
        let screen_x = (page.x + viewer.centre_shift()) as f32;
        let a = egui::pos2(screen_x + 20.0, page.y as f32 + 30.0);
        let b = a + egui::vec2(80.0, 30.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &mut viewer,
            vec![egui::Event::PointerMoved(a), button(a, true)],
        );
        frame(&mut viewer, vec![egui::Event::PointerMoved(b)]);
        frame(&mut viewer, vec![button(b, false)]);
        let mark = viewer.redaction.as_ref().unwrap().marks[&0][0];
        assert!((mark.width() - 0.4).abs() < 0.01);
        viewer.go_to_page(1);
        frame(&mut viewer, vec![]);
        assert_eq!(viewer.current_page(), 1);
        assert_eq!(viewer.redaction.as_ref().unwrap().marks[&0], vec![mark]);
    }
}
