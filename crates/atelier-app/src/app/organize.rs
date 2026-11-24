//! Page-grid organization and split export, backed by the PDF page engine.
use super::Viewer;
use crate::icons::Icon;
use crate::worker::Job;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::collections::BTreeSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

const ZOOM_LEVELS: [u16; 6] = [75, 100, 125, 150, 175, 200];
const DEFAULT_ZOOM: usize = 1;
const MAX_PREVIEW_EDGE: f32 = 512.0;

struct PageDrag {
    pages: Vec<usize>,
    page: usize,
    preview: Rect,
    texture: Option<egui::TextureHandle>,
}

impl PageDrag {
    fn paint(&self, ui: &egui::Ui, pointer: Pos2) {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            ui.scope_id().with("organize-drag-preview"),
        ));
        let preview = self.preview.translate(pointer.to_vec2());
        painter.rect_filled(
            preview.translate(Vec2::splat(4.0)),
            3.0,
            Color32::from_black_alpha(85),
        );
        paint_preview(&painter, preview, self.texture.as_ref());
        painter.rect_stroke(
            preview,
            2.0,
            Stroke::new(2.0, ui.visuals().selection.bg_fill),
            StrokeKind::Outside,
        );
        if self.pages.len() > 1 {
            let caption =
                Rect::from_min_size(preview.left_top() + Vec2::splat(5.0), Vec2::new(60.0, 21.0));
            painter.rect_filled(caption, 4.0, ui.visuals().extreme_bg_color);
            painter.text(
                caption.center(),
                egui::Align2::CENTER_CENTER,
                format!("{} pages", self.pages.len()),
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
        }
    }
}

fn page_drag_handle(ui: &egui::Ui, card: Rect, page: usize) -> egui::Response {
    let rect = Rect::from_min_size(card.right_top() + Vec2::new(-30.0, 5.0), Vec2::splat(24.0));
    let handle = ui
        .interact(rect, ui.scope_id().with(("grid-move", page)), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to move this page or its selection");
    let grip = ui.style().interact(&handle);
    ui.painter().rect_filled(rect, 4.0, grip.weak_bg_fill);
    for y in [-5.0, 0.0, 5.0] {
        for x in [-2.5, 2.5] {
            ui.painter()
                .circle_filled(rect.center() + Vec2::new(x, y), 1.25, grip.fg_stroke.color);
        }
    }
    handle
}

fn page_actions(ui: &mut egui::Ui, card: Rect, page: usize, can_delete: bool) -> (bool, bool) {
    let footer = Rect::from_min_max(
        card.left_bottom() + Vec2::new(6.0, -30.0),
        card.right_bottom() - Vec2::new(6.0, 6.0),
    );
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("grid-actions", page))
            .max_rect(footer)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    ui.spacing_mut().item_spacing.x = 4.0;
    let size = Vec2::new((footer.width() - 4.0) / 2.0, footer.height());
    let delete = ui
        .add_enabled(can_delete, egui::Button::new("Delete").min_size(size))
        .on_hover_text("Delete only this page")
        .clicked();
    let export = ui
        .add(egui::Button::new("Export…").min_size(size))
        .on_hover_text("Save only this page as a PDF")
        .clicked();
    (delete, export)
}

fn paint_preview(painter: &egui::Painter, rect: Rect, texture: Option<&egui::TextureHandle>) {
    if let Some(texture) = texture.filter(|texture| texture.size() != [1, 1]) {
        painter.rect_filled(rect, 1.0, Color32::WHITE);
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.rect_filled(rect, 1.0, Color32::from_gray(232));
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if texture.is_some() {
                "Preview unavailable"
            } else {
                "Rendering…"
            },
            egui::FontId::proportional(12.0),
            Color32::from_gray(72),
        );
    }
}

struct BlankReview {
    document: u64,
    generation: u64,
    count: usize,
    checked: usize,
    unchecked: usize,
    candidates: Vec<(usize, PageContent)>,
    selected: BTreeSet<usize>,
    updates: mpsc::Receiver<Option<PageContent>>,
    cancelled: Arc<AtomicBool>,
    done: bool,
    visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageContent {
    Blank,
    Streaks,
    Content,
}

impl Drop for BlankReview {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Split,
    Insert,
}
#[derive(Clone, Copy, PartialEq, Default)]
enum InsertPosition {
    Beginning,
    #[default]
    End,
    BeforeSelection,
    AfterSelection,
}
impl InsertPosition {
    fn resolve(self, count: usize, selection: &BTreeSet<usize>) -> Option<usize> {
        match self {
            Self::Beginning => Some(0),
            Self::End => Some(count),
            Self::BeforeSelection => selection.first().copied().filter(|&p| p < count),
            Self::AfterSelection => selection
                .last()
                .copied()
                .filter(|&p| p < count)
                .map(|p| p + 1),
        }
    }
}

#[derive(Default)]
pub(super) struct Organize {
    mode: Option<Mode>,
    input: String,
    error: Option<String>,
    document: u64,
    insert_path: Option<std::path::PathBuf>,
    insert_position: InsertPosition,
    insert_selection: BTreeSet<usize>,
    pub grid: bool,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    dragging: Option<PageDrag>,
    zoom: Option<usize>,
    generation: Option<u64>,
    blanks: Option<BlankReview>,
    #[cfg(test)]
    cards: Vec<Rect>,
}

impl Viewer {
    pub(super) fn toggle_page_grid(&mut self) {
        if self.open.is_none() {
            return;
        }
        self.finish_editing(true);
        self.commit_ink();
        self.organize.grid = !self.organize.grid;
        if self.organize.grid {
            self.organize.selected = self.sidebar.selected.clone();
            self.organize.generation = self.open.as_ref().map(|o| o.generation);
        }
        self.organize.dragging = None;
        self.annotate.note = None;
    }

    pub(super) fn organize_pages(&mut self, pages: BTreeSet<usize>) {
        if !self.organize.grid {
            self.toggle_page_grid();
        }
        self.organize.selected = pages;
        self.organize.generation = self.open.as_ref().map(|o| o.generation);
    }

    pub(super) fn show_organize(&mut self, mode: Mode) {
        let Some(o) = &self.open else { return };
        self.organize.mode = Some(mode);
        self.organize.input = (self.current_page() + 1)
            .min(o.layout.page_count().saturating_sub(1))
            .max(1)
            .to_string();
        self.organize.error = None;
        self.organize.document = o.opened;
        self.organize.insert_path = None;
        self.organize.insert_position = InsertPosition::End;
        self.organize.insert_selection = self.organize.selected.clone();
    }

    pub(super) fn organize_window(&mut self, ctx: &egui::Context) {
        self.blank_review(ctx);
        let mut dialog = std::mem::take(&mut self.organize);
        if dialog.mode.is_none() {
            self.organize = dialog;
            return;
        }
        let Some(o) = &self.open else { return };
        if o.opened != dialog.document {
            dialog.mode = None;
            self.organize = dialog;
            return;
        }
        let count = o.layout.page_count();
        let mut open = true;
        let mut apply = false;
        let insert = dialog.mode == Some(Mode::Insert);
        egui::Window::new(if insert { "Insert PDF" } else { "Split PDF" })
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                if insert {
                    ui.label("PDF to insert");
                    ui.horizontal(|ui| {
                        ui.label(
                            dialog
                                .insert_path
                                .as_ref()
                                .map_or("Choose a PDF…".into(), |p| super::display_name(p)),
                        );
                        if ui.button("Browse…").clicked() {
                            dialog.insert_path = rfd::FileDialog::new()
                                .add_filter("PDF documents", &["pdf"])
                                .pick_file();
                        }
                    });
                    ui.separator();
                    ui.label("Insert pages at");
                    ui.radio_value(
                        &mut dialog.insert_position,
                        InsertPosition::Beginning,
                        "Beginning of document",
                    );
                    ui.radio_value(
                        &mut dialog.insert_position,
                        InsertPosition::End,
                        "End of document",
                    );
                    ui.add_enabled_ui(!dialog.insert_selection.is_empty(), |ui| {
                        ui.radio_value(
                            &mut dialog.insert_position,
                            InsertPosition::BeforeSelection,
                            "Before selected pages",
                        );
                        ui.radio_value(
                            &mut dialog.insert_position,
                            InsertPosition::AfterSelection,
                            "After selected pages",
                        );
                    });
                    if dialog.insert_selection.is_empty() {
                        ui.weak("Select pages in Organize to insert before or after a selection.");
                    } else {
                        ui.weak(format!(
                            "Selected pages: {}",
                            super::pages::page_list_name(
                                &dialog.insert_selection.iter().copied().collect::<Vec<_>>()
                            )
                        ));
                    }
                    apply = ui
                        .add_enabled(
                            dialog.insert_path.is_some()
                                && dialog
                                    .insert_position
                                    .resolve(count, &dialog.insert_selection)
                                    .is_some(),
                            egui::Button::new("Insert PDF"),
                        )
                        .clicked();
                } else {
                    ui.label("Split after page");
                    ui.text_edit_singleline(&mut dialog.input);
                    ui.weak(format!(
                        "{count} pages · writes two new PDFs; the source stays open."
                    ));
                    ui.weak("Exported parts omit document-level forms and bookmarks.");
                    apply = ui.button("Choose output folder…").clicked();
                }
                if let Some(error) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        if apply {
            if insert {
                if let (Some(path), Some(at)) = (
                    dialog.insert_path.as_ref(),
                    dialog
                        .insert_position
                        .resolve(count, &dialog.insert_selection),
                ) {
                    match self.insert_pdf_path(path, at) {
                        Ok(()) => {
                            open = false;
                            dialog.selected.clear();
                            dialog.generation = self.open.as_ref().map(|o| o.generation);
                        }
                        Err(error) => dialog.error = Some(error),
                    }
                }
            } else {
                match dialog.input.trim().parse::<usize>() {
                    Ok(after) if after > 0 && after < count => {
                        self.split_pdf(after);
                        open = false;
                    }
                    _ => {
                        dialog.error = Some(format!(
                            "Choose a split point between 1 and {}.",
                            count.saturating_sub(1)
                        ))
                    }
                }
            }
        }
        if !open {
            dialog.mode = None;
        }
        self.organize = dialog;
    }

    pub(super) fn page_grid(&mut self, ui: &mut egui::Ui) {
        if self.held_view(ui) {
            return;
        }
        let Some(o) = &self.open else { return };
        let count = o.layout.page_count();
        if self.organize.generation != Some(o.generation) {
            self.organize.selected.clear();
            self.organize.anchor = None;
            self.organize.dragging = None;
            self.organize.generation = Some(o.generation);
        }
        let mut move_to = None;
        let mut rotate = false;
        let mut export = false;
        let mut print = false;
        let mut insert = false;
        let mut split = false;
        let mut find_blanks = false;
        let mut show_blanks = false;
        let mut zoom = self.organize.zoom.unwrap_or(DEFAULT_ZOOM);
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading("Organize pages");
            ui.weak(format!(
                "{} of {count} selected",
                self.organize.selected.len()
            ));
            insert = ui.button("Insert PDF…").clicked();
            split = ui
                .add_enabled(count > 1, egui::Button::new("Split…"))
                .clicked();
            if self.organize.blanks.is_some() {
                show_blanks = ui.button("Review blank pages…").clicked();
            } else {
                find_blanks = ui.button("Find blank pages…").clicked();
            }
            if ui.button("Select all").clicked() {
                self.organize.selected = (0..count).collect();
            }
            if ui.button("Clear").clicked() {
                self.organize.selected.clear();
            }
            ui.add_enabled_ui(!self.organize.selected.is_empty(), |ui| {
                if ui.button("Move to start").clicked() {
                    move_to = Some(0);
                }
                if ui.button("Move to end").clicked() {
                    move_to = Some(count);
                }
                rotate = ui.button("Rotate").clicked();
                export = ui.button("Export…").clicked();
                print = ui
                    .button("Print selected…")
                    .on_hover_text("Print only the selected pages")
                    .clicked();
            });
            if ui.button("Done").clicked() {
                self.organize.grid = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.organize.dragging.is_none(), |ui| {
                ui.label("Preview zoom");
                if ui
                    .add_enabled(zoom > 0, Icon::Minus.button())
                    .on_hover_text("Smaller page previews")
                    .clicked()
                {
                    zoom -= 1;
                }
                egui::ComboBox::from_id_salt("organize-zoom")
                    .width(62.0)
                    .selected_text(format!("{}%", ZOOM_LEVELS[zoom]))
                    .show_ui(ui, |ui| {
                        for (index, percent) in ZOOM_LEVELS.iter().enumerate() {
                            ui.selectable_value(&mut zoom, index, format!("{percent}%"));
                        }
                    });
                if ui
                    .add_enabled(zoom + 1 < ZOOM_LEVELS.len(), Icon::Plus.button())
                    .on_hover_text("Larger page previews")
                    .clicked()
                {
                    zoom += 1;
                }
            });
            ui.weak("Drag a handle to move the selected pages. Shift-click to select a range.");
        });
        self.organize.zoom = Some(zoom);
        ui.separator();
        #[cfg(test)]
        {
            self.organize.cards.clear();
        }
        let mut wanted = Vec::new();
        let mut go = None;
        let mut delete_page = None;
        let mut export_page = None;
        let mut landing = None;
        let selected = &mut self.organize.selected;
        let anchor = &mut self.organize.anchor;
        let dragging = &mut self.organize.dragging;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *dragging = None;
        }
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        let ppp = ui.ctx().pixels_per_point();
        let width = ui.available_width();
        let zoom = f32::from(ZOOM_LEVELS[zoom]) / 100.0;
        let columns = ((width / (184.0 * zoom)).floor() as usize).max(1);
        let cell_width = (width / columns as f32).clamp(134.0, 210.0 * zoom);
        let row_height = 76.0 + 172.0 * zoom;
        let rows = count.div_ceil(columns);
        egui::ScrollArea::vertical()
            .id_salt("organize-page-grid")
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                let (area, _) = ui.allocate_exact_size(
                    Vec2::new(width, rows as f32 * row_height + 32.0),
                    Sense::hover(),
                );
                let card_rect = |page: usize| {
                    Rect::from_min_size(
                        area.min
                            + Vec2::new(
                                (page % columns) as f32 * cell_width + 8.0,
                                (page / columns) as f32 * row_height + 8.0,
                            ),
                        Vec2::new(cell_width - 16.0, row_height - 16.0),
                    )
                };
                let first = ((viewport.min.y / row_height).floor().max(0.0) as usize).min(rows);
                let last = ((viewport.max.y / row_height).ceil().max(0.0) as usize + 1).min(rows);
                for row in first..last {
                    for col in 0..columns {
                        let page = row * columns + col;
                        if page >= count {
                            break;
                        }
                        let rect = card_rect(page);
                        #[cfg(test)]
                        {
                            self.organize.cards.push(rect);
                        }
                        let picked = selected.contains(&page);
                        let visuals = ui.visuals();
                        ui.painter().rect(
                            rect,
                            8.0,
                            if picked {
                                visuals.selection.bg_fill.gamma_multiply(0.25)
                            } else {
                                visuals.panel_fill
                            },
                            Stroke::new(
                                if picked { 2.0 } else { 1.0 },
                                if picked {
                                    visuals.selection.bg_fill
                                } else {
                                    visuals.widgets.noninteractive.bg_stroke.color
                                },
                            ),
                            StrokeKind::Inside,
                        );
                        let preview = Rect::from_min_max(
                            rect.min + Vec2::new(12.0, 36.0),
                            rect.max - Vec2::new(12.0, 38.0),
                        );
                        let mut image = preview;
                        let texture = o.thumbs.get(&page);
                        if let Some(g) = o.layout.geom(page) {
                            let (pw, ph) = g.size();
                            let size = Vec2::new(pw.max(1.0) as f32, ph.max(1.0) as f32);
                            let scale = (preview.width() / size.x).min(preview.height() / size.y);
                            image = Rect::from_center_size(preview.center(), size * scale);
                            // Use suitable resolution for this zoom and display without
                            // letting large previews exhaust the shared thumbnail cache.
                            let pixels = (image.size().max_elem() * ppp).min(MAX_PREVIEW_EDGE);
                            if o.thumb_stale.contains(&page)
                                || texture.is_none_or(|texture| {
                                    let edge = texture.size_vec2().max_elem();
                                    edge > 1.0 && edge < pixels * 0.9
                                })
                            {
                                wanted.push((page, f64::from(pixels / size.max_elem())));
                            }
                        }
                        paint_preview(ui.painter(), image, texture);
                        let response = ui
                            .interact(
                                preview,
                                ui.scope_id().with(("grid-page", page)),
                                Sense::click(),
                            )
                            .on_hover_text("Double-click to read this page");
                        let handle = page_drag_handle(ui, rect, page);
                        let mut checked = picked;
                        let mut header = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("grid-check", page))
                                .max_rect(Rect::from_min_size(
                                    rect.min + Vec2::new(6.0, 4.0),
                                    Vec2::new(rect.width() - 36.0, 28.0),
                                ))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        let check = header.checkbox(&mut checked, format!("Page {}", page + 1));
                        if check.changed() {
                            if ui.input(|i| i.modifiers.shift) {
                                let from = anchor.unwrap_or(page);
                                for item in from.min(page)..=from.max(page) {
                                    if checked {
                                        selected.insert(item);
                                    } else {
                                        selected.remove(&item);
                                    }
                                }
                            } else if checked {
                                selected.insert(page);
                            } else {
                                selected.remove(&page);
                            }
                            *anchor = Some(page);
                        }
                        if response.clicked() {
                            if ui.input(|i| i.modifiers.shift) {
                                let from = anchor.unwrap_or(page);
                                *selected = (from.min(page)..=from.max(page)).collect();
                            } else if ui.input(|i| i.modifiers.command) {
                                if !selected.remove(&page) {
                                    selected.insert(page);
                                }
                                *anchor = Some(page);
                            } else {
                                *selected = BTreeSet::from([page]);
                                *anchor = Some(page);
                            }
                        }
                        if response.double_clicked() {
                            go = Some(page);
                        }
                        if handle.drag_started_by(egui::PointerButton::Primary) {
                            if !selected.contains(&page) {
                                *selected = BTreeSet::from([page]);
                                *anchor = Some(page);
                            }
                            let grab = ui.input(|i| i.pointer.press_origin());
                            *dragging = Some(PageDrag {
                                pages: selected.iter().copied().collect(),
                                page,
                                preview: image
                                    .translate(-grab.unwrap_or(handle.rect.center()).to_vec2()),
                                texture: texture.cloned(),
                            });
                        }
                        let (delete, export) = page_actions(ui, rect, page, count > 1);
                        if delete {
                            delete_page = Some(page);
                        }
                        if export {
                            export_page = Some(page);
                        }
                    }
                }
                if let Some(drag) = dragging
                    && let Some(pointer) = pointer
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let clip = ui.clip_rect().intersect(area);
                    if clip.contains(pointer) {
                        let row = ((pointer.y - area.min.y) / row_height) as usize;
                        let col = ((pointer.x - area.min.x) / cell_width) as usize;
                        let page = (row * columns + col.min(columns - 1)).min(count - 1);
                        let target = card_rect(page);
                        let after = row >= rows || pointer.x >= target.center().x;
                        landing = Some(page + usize::from(after));
                        let edge = if after {
                            target.right() + 4.0
                        } else {
                            target.left() - 4.0
                        };
                        let color = ui.visuals().selection.bg_fill;
                        ui.painter().line_segment(
                            [
                                Pos2::new(edge, target.top()),
                                Pos2::new(edge, target.bottom()),
                            ],
                            Stroke::new(3.0, color),
                        );
                        for y in [target.top(), target.bottom()] {
                            ui.painter().circle_filled(Pos2::new(edge, y), 3.0, color);
                        }
                        if pointer.y < clip.min.y + 30.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, 10.0));
                            ui.ctx().request_repaint();
                        } else if pointer.y > clip.max.y - 30.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, -10.0));
                            ui.ctx().request_repaint();
                        }
                    }
                    if let Some(texture) = o.thumbs.get(&drag.page) {
                        drag.texture = Some(texture.clone());
                    } else if drag.texture.is_none()
                        && let Some(geom) = o.layout.geom(drag.page)
                    {
                        let (width, height) = geom.size();
                        let pixels = (drag.preview.size().max_elem() * ppp).min(MAX_PREVIEW_EDGE);
                        wanted.retain(|&(page, _)| page != drag.page);
                        wanted.insert(
                            0,
                            (drag.page, f64::from(pixels) / width.max(height).max(1.0)),
                        );
                    }
                    drag.paint(ui, pointer);
                }
            });
        let jobs = wanted
            .into_iter()
            .map(|(page, scale)| Job::Thumb {
                doc: o.doc.clone(),
                generation: o.page_generation(page),
                page,
                scale,
            })
            .collect();
        self.pool.set_jobs(jobs);
        let selected: Vec<_> = self.organize.selected.iter().copied().collect();
        if ui.input(|i| i.pointer.primary_released())
            && let Some(drag) = self.organize.dragging.take()
        {
            if let Some(to) = landing {
                let moved = self.move_pages(&drag.pages, to);
                self.organize.anchor = moved.first().copied();
                self.organize.selected = moved.into_iter().collect();
            }
        } else if let Some(to) = move_to {
            let moved = self.move_pages(&selected, to);
            self.organize.anchor = moved.first().copied();
            self.organize.selected = moved.into_iter().collect();
        } else if !ui.input(|i| i.pointer.primary_down()) {
            self.organize.dragging = None;
        }
        if rotate {
            self.rotate_some(&selected, 1);
        }
        if let Some(page) = export_page {
            self.extract_pages(&[page]);
        } else if export {
            self.extract_pages(&selected);
        }
        if print {
            self.print_pages(&selected);
        }
        if let Some(page) = go {
            self.organize.grid = false;
            self.go_to_page(page);
        }
        if let Some(page) = delete_page {
            self.delete_pages(&[page]);
            if self
                .open
                .as_ref()
                .is_some_and(|o| o.layout.page_count() < count)
            {
                self.organize.selected = selected
                    .into_iter()
                    .filter(|&p| p != page)
                    .map(|p| p - usize::from(p > page))
                    .collect();
                self.organize.anchor = self.organize.selected.first().copied();
            }
        }
        self.organize.generation = self.open.as_ref().map(|o| o.generation);
        if insert {
            self.show_organize(Mode::Insert);
        }
        if split {
            self.show_organize(Mode::Split);
        }
        if find_blanks {
            self.find_blank_pages();
        }
        if show_blanks && let Some(review) = &mut self.organize.blanks {
            review.visible = true;
            review.selected = review
                .candidates
                .iter()
                .map(|&(page, _)| page)
                .filter(|page| self.organize.selected.contains(page))
                .collect();
        }
    }

    fn find_blank_pages(&mut self) {
        let Some(open) = &self.open else { return };
        let doc = open.doc.clone();
        let viewport = self.viewport;
        let wake = self.ctx.clone();
        let (sender, updates) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let scan = std::thread::Builder::new()
            .name("blank-pages".into())
            .spawn(move || {
                for page in doc.pages().unwrap_or_default() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let blank = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        blank_page(&doc, page)
                    }))
                    .ok()
                    .flatten();
                    if sender.send(blank).is_err() {
                        break;
                    }
                    wake.request_repaint_of(viewport);
                }
                drop(sender);
                wake.request_repaint_of(viewport);
            });
        if let Err(error) = scan {
            self.error = Some(format!("Could not scan for blank pages: {error}"));
            return;
        }
        self.organize.blanks = Some(BlankReview {
            document: open.opened,
            generation: open.generation,
            count: open.layout.page_count(),
            checked: 0,
            unchecked: 0,
            candidates: Vec::new(),
            selected: BTreeSet::new(),
            updates,
            cancelled,
            done: false,
            visible: true,
        });
    }

    fn blank_review(&mut self, ctx: &egui::Context) {
        let Some(mut review) = self.organize.blanks.take() else {
            return;
        };
        let matches = self.open.as_ref().is_some_and(|open| {
            open.opened == review.document && open.generation == review.generation
        });
        if !matches || !self.organize.grid {
            if !matches && self.organize.grid {
                self.notice = Some("The document changed. Run the blank-page check again.".into());
            }
            return;
        }
        if !review.visible {
            self.organize.blanks = Some(review);
            return;
        }
        while !review.done {
            match review.updates.try_recv() {
                Ok(Some(content)) => {
                    if content != PageContent::Content {
                        review.candidates.push((review.checked, content));
                    }
                    if content == PageContent::Blank {
                        review.selected.insert(review.checked);
                    }
                    review.checked += 1;
                }
                Ok(None) => {
                    review.checked += 1;
                    review.unchecked += 1;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    review.unchecked += review.count.saturating_sub(review.checked);
                    review.done = true;
                    if review.selected.len() == review.count {
                        review.selected.remove(&0);
                    }
                }
            }
        }
        let mut close = false;
        let mut select = false;
        let mut remove = false;
        egui::Modal::new(egui::Id::unique("blank-pages-review")).show(ctx, |ui| {
            ui.heading("Blank pages");
            if review.done {
                if review.candidates.is_empty() {
                    ui.label("No likely blank pages found.");
                } else {
                    ui.label("Review the pages before removing them.");
                }
                if review.unchecked > 0 {
                    ui.weak(format!(
                        "{} pages could not be checked and will be kept.",
                        review.unchecked
                    ));
                }
            } else {
                ui.label(format!(
                    "Checked {} of {} pages",
                    review.checked, review.count
                ));
                ui.spinner();
            }
            if review.done && !review.candidates.is_empty() {
                let streaks = review.candidates.iter().filter_map(|&(page, content)| {
                    (content == PageContent::Streaks).then_some(page)
                });
                if streaks.clone().next().is_some()
                    && ui.button("Select all streak candidates").clicked()
                {
                    review.selected.extend(streaks);
                }
                let row_height = ui.spacing().interact_size.y;
                egui::ScrollArea::vertical().max_height(280.0).show_rows(
                    ui,
                    row_height,
                    review.candidates.len(),
                    |ui, visible| {
                        for &(page, content) in &review.candidates[visible] {
                            let mut selected = review.selected.contains(&page);
                            let label = if content == PageContent::Streaks {
                                format!("Page {} · likely scanner streaks", page + 1)
                            } else {
                                format!("Page {}", page + 1)
                            };
                            if ui.checkbox(&mut selected, label).changed() {
                                if selected {
                                    review.selected.insert(page);
                                } else {
                                    review.selected.remove(&page);
                                }
                            }
                        }
                    },
                );
                if review.selected.len() == review.count {
                    ui.weak("Keep at least one page.");
                }
            }
            ui.horizontal(|ui| {
                let has_selection = review.done && !review.selected.is_empty();
                select = ui
                    .add_enabled(has_selection, egui::Button::new("Select in organizer"))
                    .clicked();
                remove = ui
                    .add_enabled(
                        has_selection && review.selected.len() < review.count,
                        egui::Button::new(format!("Remove {} selected", review.selected.len())),
                    )
                    .clicked();
                close = ui
                    .button(if review.done { "Close" } else { "Cancel" })
                    .clicked();
            });
        });
        if remove {
            let pages: Vec<_> = review.selected.iter().copied().collect();
            self.delete_pages(&pages);
        } else if select {
            self.organize.selected = review.selected.clone();
            review.visible = false;
            self.organize.blanks = Some(review);
        } else if !close {
            self.organize.blanks = Some(review);
        }
    }

    fn split_pdf(&mut self, after: usize) {
        self.finish_editing(true);
        self.commit_ink();
        let Some(o) = &self.open else { return };
        let count = o.layout.page_count();
        let parts = match (
            o.session.extract(&(0..after).collect::<Vec<_>>()),
            o.session.extract(&(after..count).collect::<Vec<_>>()),
        ) {
            (Ok(a), Ok(b)) => [a, b],
            _ => {
                self.error = Some("Could not build the split PDFs.".into());
                return;
            }
        };
        let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name).to_owned();
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Save split PDFs in folder")
            .pick_folder()
        else {
            return;
        };
        let paths = [
            folder.join(format!("{stem}-part-1.pdf")),
            folder.join(format!("{stem}-part-2.pdf")),
        ];
        // Never overwrite existing split outputs implicitly.
        if paths.iter().any(|p| p.exists()) {
            self.error = Some("Split output already exists. Choose a different folder or rename the existing parts.".into());
            return;
        }
        for (index, (path, bytes)) in paths.iter().zip(parts.iter()).enumerate() {
            use std::io::Write;
            let result = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut file| {
                    file.write_all(bytes)?;
                    file.sync_all()
                });
            if let Err(error) = result {
                self.error = Some(format!(
                    "Could not save {}: {error}. {index} complete part(s) were saved; check the output folder before retrying.",
                    path.display()
                ));
                return;
            }
        }
        self.notice = Some(format!(
            "Split into {} and {}.",
            paths[0].display(),
            paths[1].display()
        ));
    }
}

fn blank_page(doc: &atelier_syntax::Document, page: &atelier_syntax::Page) -> Option<PageContent> {
    if page.dict.contains_key(b"AA") {
        return Some(PageContent::Content);
    }
    if page.dict.contains_key(b"Annots") {
        match doc.get_key(&page.dict, b"Annots").ok()? {
            atelier_syntax::Object::Array(items) if items.is_empty() => {}
            atelier_syntax::Object::Null => {}
            _ => return Some(PageContent::Content),
        }
    }
    if !atelier_render::extract_text(doc, page)
        .ok()?
        .glyphs
        .is_empty()
    {
        return Some(PageContent::Content);
    }
    let long_side = page.crop_box.width().max(page.crop_box.height()).max(1.0);
    let options = atelier_render::RenderOptions {
        scale: (0.9_f64).min(900.0 / long_side),
        ..Default::default()
    };
    let rendered = atelier_render::render_page(doc, page, &options).ok()?;
    if !rendered.warnings.is_empty() {
        return None;
    }
    let gray = atelier_view::gray_of_rgba(rendered.pixmap.data());
    Some(scan_marks(
        &gray,
        rendered.pixmap.width(),
        rendered.pixmap.height(),
    ))
}

#[derive(Clone, Copy, Default)]
struct InkTrace {
    count: usize,
    first: usize,
    last: usize,
}

impl InkTrace {
    fn mark(&mut self, position: usize) {
        if self.count == 0 {
            self.first = position;
        }
        self.last = position;
        self.count += 1;
    }
}

/// Marks narrow, near-edge-to-edge bands. The lower coverage threshold
/// groups the lighter edges of a streak with its denser center.
fn streak_bands(traces: &[InkTrace], length: usize) -> Vec<bool> {
    let end_margin = (length / 10).max(1);
    let spans_page = |trace: &InkTrace| {
        trace.count * 100 >= length * 45
            && trace.first < end_margin
            && trace.last >= length - end_margin
    };
    let max_width = (traces.len() / 80).max(2);
    let max_total = (traces.len() / 40).max(3);
    let mut mask = vec![false; traces.len()];
    let mut start = 0;
    let mut masked = 0;
    for band in traces.chunk_by(|a, b| spans_page(a) == spans_page(b)) {
        if spans_page(&band[0])
            && band.len() <= max_width
            && band.iter().any(|trace| trace.count * 100 >= length * 70)
        {
            masked += band.len();
            if masked > max_total {
                mask.fill(false);
                break;
            }
            mask[start..start + band.len()].fill(true);
        }
        start += band.len();
    }
    mask
}

fn scan_marks(gray: &[u8], width: usize, height: usize) -> PageContent {
    const MARK: u8 = 246;
    const MIN_INK: usize = 5;
    if gray
        .iter()
        .filter(|&&pixel| pixel < MARK)
        .take(MIN_INK)
        .count()
        < MIN_INK
    {
        return PageContent::Blank;
    }
    let mut columns = vec![InkTrace::default(); width];
    let mut rows = vec![InkTrace::default(); height];
    for (y, pixels) in gray.chunks_exact(width).enumerate() {
        for (x, &pixel) in pixels.iter().enumerate() {
            if pixel < MARK {
                columns[x].mark(y);
                rows[y].mark(x);
            }
        }
    }
    let columns = streak_bands(&columns, height);
    let rows = streak_bands(&rows, width);
    if !columns.iter().chain(&rows).any(|&band| band) {
        return PageContent::Content;
    }
    let remaining = gray
        .chunks_exact(width)
        .zip(rows)
        .filter(|(_, band)| !band)
        .flat_map(|(pixels, _)| pixels.iter().zip(&columns))
        .filter(|&(&pixel, &band)| !band && pixel < MARK)
        .take(MIN_INK)
        .count();
    if remaining < MIN_INK {
        PageContent::Streaks
    } else {
        PageContent::Content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use eframe::egui::{Event, Modifiers, PointerButton, RawInput};

    fn frame(
        viewer: &mut Viewer,
        ctx: &egui::Context,
        events: Vec<Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| viewer.page_grid(ui));
            },
        );
        output.textures_delta.clear();
        output.shapes
    }

    fn wait_for_thumbnail(viewer: &mut Viewer, page: usize, min_edge: f32) -> egui::TextureHandle {
        let ctx = viewer.ctx.clone();
        let start = std::time::Instant::now();
        loop {
            viewer.receive();
            if let Some(texture) = viewer.open.as_ref().unwrap().thumbs.get(&page)
                && texture.size_vec2().max_elem() >= min_edge
            {
                return texture.clone();
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "page preview did not arrive"
            );
            frame(viewer, &ctx, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn blank_page_review_finds_scanner_streaks_and_keeps_real_content() {
        use atelier_syntax::{Dict, Document, Name, Object, Stream};

        let doc = Document::open(super::super::quality_tests::fixture(9)).unwrap();
        let pages = doc.pages().unwrap();
        let mut changes = doc.update();
        let drawing = changes.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: Arc::from(&b"0.94 g 10 10 20 20 re f"[..]),
        }));
        let mut image = Dict::new();
        image.insert("Subtype", Object::Name(Name::from("Image")));
        image.insert("Width", Object::Integer(200));
        image.insert("Height", Object::Integer(200));
        image.insert("ColorSpace", Object::Name(Name::from("DeviceGray")));
        image.insert("BitsPerComponent", Object::Integer(8));
        let mut raster = vec![255; 200 * 200];
        for (y, row) in raster.as_chunks_mut::<200>().0.iter_mut().enumerate() {
            row[65] = 180;
            if y % 20 < 17 {
                row[130] = 220;
            }
        }
        let streaks = changes.add(Object::Stream(Stream {
            dict: image.clone(),
            data: raster.clone().into(),
        }));
        for y in 100..113 {
            raster[y * 200 + y] = 175;
        }
        let handwriting = changes.add(Object::Stream(Stream {
            dict: image,
            data: raster.into(),
        }));
        let show_image = changes.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: Arc::from(&b"q 200 0 0 200 0 0 cm /Scan Do Q"[..]),
        }));
        let short_rule = changes.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: Arc::from(&b"0 g 65 20 1 160 re f"[..]),
        }));
        let wide_rule = changes.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: Arc::from(&b"0 g 65 0 8 200 re f"[..]),
        }));
        for (index, page) in pages.iter().enumerate().skip(1) {
            let mut dict = page.dict.clone();
            let content = match index {
                2 => {
                    dict.insert("Annots", pages[0].dict.get(b"Annots").unwrap().clone());
                    None
                }
                3 => Some(drawing),
                4..=6 => {
                    let mut images = Dict::new();
                    images.insert(
                        "Scan",
                        Object::Reference(if index == 6 { handwriting } else { streaks }),
                    );
                    let mut resources = page.resources.clone();
                    resources.insert("XObject", Object::Dict(images));
                    dict.insert("Resources", Object::Dict(resources));
                    if index == 5 {
                        dict.insert("Rotate", Object::Integer(90));
                    }
                    Some(show_image)
                }
                7 => Some(short_rule),
                8 => Some(wide_rule),
                _ => None,
            };
            dict.insert("Contents", content.map_or(Object::Null, Object::Reference));
            changes.set(page.obj_ref.unwrap(), Object::Dict(dict));
        }
        let ctx = egui::Context::default();
        let mut viewer = Viewer::blank(
            ctx.clone(),
            Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Settings::default()
            },
        );
        viewer.open_bytes(changes.to_bytes().unwrap(), "Scan.pdf".into(), None);
        viewer.toggle_page_grid();
        viewer.find_blank_pages();
        let started = std::time::Instant::now();
        loop {
            let mut output = ctx.run_ui(RawInput::default(), |_| viewer.blank_review(&ctx));
            output.textures_delta.clear();
            let review = viewer.organize.blanks.as_ref().unwrap();
            if review.done {
                assert_eq!(
                    review.candidates,
                    [
                        (1, PageContent::Blank),
                        (4, PageContent::Streaks),
                        (5, PageContent::Streaks)
                    ]
                );
                assert_eq!(review.selected, BTreeSet::from([1]));
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        viewer.delete_pages(&[1]);
        assert_eq!(viewer.open.as_ref().unwrap().layout.page_count(), 8);
        assert!(viewer.is_dirty());
    }

    #[test]
    fn grid_drag_moves_the_checked_group_and_keeps_selection() {
        let ctx = egui::Context::default();
        let settings = Settings {
            check_updates: false,
            ocr: false,
            orient_on_open: false,
            ..Default::default()
        };
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            settings,
        );
        let data=b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n5 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj\n%%EOF".to_vec();
        viewer.open_bytes(data, "grid-test.pdf".into(), None);
        assert!(viewer.open.is_some());
        let texture = wait_for_thumbnail(&mut viewer, 0, 2.0).id();
        frame(&mut viewer, &ctx, vec![]);
        viewer.organize.selected = BTreeSet::from([0, 2]);
        let start = viewer.organize.cards[0].right_top() + Vec2::new(-18.0, 17.0);
        let end = viewer.organize.cards[2].right_center() - Vec2::new(5.0, 0.0);
        frame(&mut viewer, &ctx, vec![Event::PointerMoved(start)]);
        frame(
            &mut viewer,
            &ctx,
            vec![Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        let mid = start + Vec2::new(30.0, 0.0);
        let ghost = |shapes: Vec<egui::epaint::ClippedShape>| {
            shapes
                .into_iter()
                .rev()
                .find_map(|shape| match shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                        Some(mesh.calc_bounds())
                    }
                    _ => None,
                })
                .unwrap()
        };
        let first = ghost(frame(&mut viewer, &ctx, vec![Event::PointerMoved(mid)]));
        assert!(
            viewer.organize.dragging.is_some(),
            "page handle must capture the drag"
        );
        let last = ghost(frame(&mut viewer, &ctx, vec![Event::PointerMoved(end)]));
        assert!(((last.min - first.min) - (end - mid)).length() < 1.0);
        frame(
            &mut viewer,
            &ctx,
            vec![Event::PointerButton {
                pos: end,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        let widths: Vec<_> = viewer
            .open
            .as_ref()
            .unwrap()
            .doc
            .pages()
            .unwrap()
            .iter()
            .map(|p| p.media_box.width() as usize)
            .collect();
        assert_eq!(widths, [200, 100, 300]);
        assert_eq!(viewer.organize.selected, BTreeSet::from([1, 2]));
        assert!(viewer.organize.dragging.is_none());
    }

    #[test]
    fn auto_rotated_page_gets_a_fresh_preview_and_zoom_upgrades_its_texture() {
        let mut viewer = merged_viewer(super::super::quality_tests::fixture(2));
        let original = wait_for_thumbnail(&mut viewer, 0, 2.0);
        viewer.start_orient(true);
        viewer.turn_oriented(&[(0, Some((90, atelier_view::Basis::Image)))]);
        let open = viewer.open.as_mut().unwrap();
        open.orienting = None;
        assert_eq!(open.session.rotation(0), 90);
        assert_ne!(open.generation, open.page_generation(0));
        assert!(!open.thumbs.contains_key(&0));

        let rotated = wait_for_thumbnail(&mut viewer, 0, 2.0);
        assert_ne!(rotated.id(), original.id());
        let size = viewer.organize.cards[0].size();
        viewer.organize.zoom = Some(ZOOM_LEVELS.len() - 1);
        let zoomed = wait_for_thumbnail(&mut viewer, 0, rotated.size_vec2().max_elem() * 1.5);
        assert_ne!(zoomed.id(), rotated.id());
        assert!(viewer.organize.cards[0].width() > size.x);
    }

    #[test]
    fn card_delete_applies_to_that_page_and_keeps_other_selected_pages() {
        let mut viewer = merged_viewer(super::super::quality_tests::fixture(2));
        let ctx = viewer.ctx.clone();
        frame(&mut viewer, &ctx, vec![]);
        viewer.organize.selected.insert(1);
        for _ in 0..2 {
            let card = viewer.organize.cards[0];
            let pos = Pos2::new(card.left() + card.width() / 4.0, card.bottom() - 18.0);
            frame(&mut viewer, &ctx, vec![Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    &mut viewer,
                    &ctx,
                    vec![Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    }],
                );
            }
            let open = viewer.open.as_ref().unwrap();
            assert_eq!(open.layout.page_count(), 1);
            assert!(!open.doc.pages().unwrap()[0].dict.contains_key(b"Annots"));
            assert_eq!(viewer.organize.selected, BTreeSet::from([0]));
        }
    }
    #[test]
    fn printing_the_selection_opens_the_dialog_on_those_pages() {
        let mut viewer = merged_viewer(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n5 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj\n%%EOF".to_vec());
        viewer.print_pages(&[]);
        assert!(!viewer.print.open);
        viewer.organize.selected = BTreeSet::from([0, 2]);
        let selected: Vec<_> = viewer.organize.selected.iter().copied().collect();
        viewer.print_pages(&selected);
        assert!(viewer.print.open);
        assert_eq!(viewer.print_preview_page(), Some(0));
        viewer.print.open = false;
        viewer.print_pages(&[1, 2]);
        assert_eq!(viewer.print_preview_page(), Some(1));
    }

    #[test]
    fn insertion_positions_use_document_or_selection_boundaries() {
        let selected = BTreeSet::from([1, 3]);
        assert_eq!(InsertPosition::Beginning.resolve(5, &selected), Some(0));
        assert_eq!(InsertPosition::End.resolve(5, &selected), Some(5));
        assert_eq!(
            InsertPosition::BeforeSelection.resolve(5, &selected),
            Some(1)
        );
        assert_eq!(
            InsertPosition::AfterSelection.resolve(5, &selected),
            Some(4)
        );
        assert_eq!(
            InsertPosition::BeforeSelection.resolve(5, &BTreeSet::new()),
            None
        );
        assert_eq!(InsertPosition::AfterSelection.resolve(3, &selected), None);
    }
    fn merged_viewer(data: Vec<u8>) -> Viewer {
        let ctx = egui::Context::default();
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(ctx),
            Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        );
        viewer.open_with_password(data, "merge-test.pdf".into(), "", None);
        assert!(viewer.open.is_some());
        viewer
    }

    #[test]
    fn page_edits_refresh_hotspots_without_approving_inserted_scripts() {
        let data=b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] >> >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [6 0 R] >> endobj\n5 0 obj << /Subtype /Widget /FT /Tx /T (a) /Rect [10 10 50 30] /AA << /C << /S /JavaScript /JS (event.value = 1;) >> >> /A << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n6 0 obj << /Subtype /Widget /FT /Tx /T (b) /Rect [10 10 50 30] /AA << /C << /S /JavaScript /JS (event.value = 2;) >> >> >> endobj\n%%EOF".to_vec();
        let mut viewer = merged_viewer(data);
        let original = viewer.open.as_ref().unwrap().safety.original.clone();
        viewer
            .open
            .as_mut()
            .unwrap()
            .scripts
            .as_mut()
            .unwrap()
            .allowed
            .insert(super::super::scripts::script_id("event.value = 1;"));
        viewer.move_pages(&[0], 2);
        let open = viewer.open.as_ref().unwrap();
        let a = open
            .safety
            .actions
            .iter()
            .position(|a| a.javascript.as_deref() == Some("app.alert(1)"))
            .unwrap();
        assert!(
            open.safety
                .hotspots
                .iter()
                .any(|h| h.page == 1 && h.actions.contains(&a))
        );
        assert!(
            !open
                .safety
                .hotspots
                .iter()
                .any(|h| h.page == 0 && h.actions.contains(&a))
        );
        assert!(std::sync::Arc::ptr_eq(&original, &open.safety.original));
        viewer.open.as_mut().unwrap().session.set_hidden(0, true);
        viewer.rebuild(&std::collections::HashSet::from([1]), false);
        let open = viewer.open.as_ref().unwrap();
        assert!(
            !open.safety.hotspots.iter().any(|h| h.page == 1),
            "hidden widgets must not retain clickable script hotspots"
        );
        let inserted=atelier_syntax::Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >> endobj\n4 0 obj << /Subtype /Link /Rect [10 10 50 30] /A << /S /JavaScript /JS (app.alert(99)) >> >> endobj\n%%EOF".to_vec()).unwrap();
        let mapping = viewer
            .open
            .as_mut()
            .unwrap()
            .session
            .insert_file(std::sync::Arc::new(inserted), 0)
            .unwrap();
        viewer.rearranged(&mapping, Some(0));
        let open = viewer.open.as_ref().unwrap();
        let a = open
            .safety
            .actions
            .iter()
            .position(|a| a.javascript.as_deref() == Some("app.alert(99)"))
            .unwrap();
        assert!(
            open.safety
                .hotspots
                .iter()
                .any(|h| h.page == 0 && h.actions.contains(&a))
        );
        let state = open.scripts.as_ref().unwrap();
        assert!(!state.is_allowed("app.alert(99)"));
        assert!(state.is_allowed("event.value = 1;"));
        assert!(std::sync::Arc::ptr_eq(&original, &open.safety.original));
    }

    #[test]
    fn organizer_respects_dynamic_xfa_gate() {
        let data=b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm << /Fields [] /XFA 4 0 R >> >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n4 0 obj << /Length 11 >> stream\n<template/>\nendstream endobj\n%%EOF".to_vec();
        let mut viewer = merged_viewer(data);
        viewer.toggle_page_grid();
        let ctx = viewer.ctx.clone();
        frame(&mut viewer, &ctx, vec![]);
        assert!(viewer.open.as_ref().unwrap().safety.held);
        assert!(
            viewer.organize.cards.is_empty(),
            "the grid must not bypass the fallback-page warning"
        );
        let open = viewer.open.as_mut().unwrap();
        open.safety.held = false;
        open.safety.refresh(open.doc.clone());
        assert!(
            !open.safety.held,
            "a user's existing fallback-page choice should survive rebuilding"
        );
        frame(&mut viewer, &ctx, vec![]);
        assert_eq!(viewer.organize.cards.len(), 1);
    }
}
