//! Page-grid organization and split export, backed by the PDF page engine.
use super::Viewer;
use crate::worker::Job;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::collections::BTreeSet;

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
    dragging: Option<Vec<usize>>,
    generation: Option<u64>,
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
            self.organize.dragging = None;
            self.organize.generation = Some(o.generation);
        }
        let mut move_to = None;
        let mut rotate = false;
        let mut export = false;
        let mut print = false;
        let mut insert = false;
        let mut split = false;
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
        ui.weak("Select page checkboxes, then drag a page to move the selected group. Shift-click selects a range. Double-click a page to read it.");
        ui.separator();
        #[cfg(test)]
        {
            self.organize.cards.clear();
        }
        let mut wanted = Vec::new();
        let mut go = None;
        let mut landing = None;
        let selected = &mut self.organize.selected;
        let anchor = &mut self.organize.anchor;
        let dragging = &mut self.organize.dragging;
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        let width = ui.available_width();
        let columns = ((width / 184.0).floor() as usize).max(1);
        let cell_width = (width / columns as f32).min(210.0);
        let row_height = 248.0;
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
                let first = ((viewport.min.y / row_height).floor().max(0.0) as usize).min(rows);
                let last = ((viewport.max.y / row_height).ceil().max(0.0) as usize + 1).min(rows);
                for row in first..last {
                    for col in 0..columns {
                        let page = row * columns + col;
                        if page >= count {
                            break;
                        }
                        let rect = Rect::from_min_size(
                            area.min
                                + Vec2::new(
                                    col as f32 * cell_width + 8.0,
                                    row as f32 * row_height + 8.0,
                                ),
                            Vec2::new(cell_width - 16.0, row_height - 16.0),
                        );
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
                        let preview = rect.shrink2(Vec2::new(12.0, 12.0));
                        let preview = Rect::from_min_max(
                            preview.min + Vec2::new(0.0, 26.0),
                            preview.max - Vec2::new(0.0, 12.0),
                        );
                        if let Some(g) = o.layout.geom(page) {
                            let (pw, ph) = g.size();
                            let scale =
                                (preview.width() / pw as f32).min(preview.height() / ph as f32);
                            let image = Rect::from_center_size(
                                preview.center(),
                                Vec2::new(pw as f32 * scale, ph as f32 * scale),
                            );
                            ui.painter().rect_filled(image, 0.0, Color32::WHITE);
                            if let Some(texture) = o.thumbs.get(&page) {
                                ui.painter().image(
                                    texture.id(),
                                    image,
                                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            } else {
                                wanted.push(page);
                            }
                            if o.thumb_stale.contains(&page) {
                                wanted.push(page);
                            }
                        }
                        let body = Rect::from_min_max(rect.min + Vec2::new(0.0, 34.0), rect.max);
                        let response = ui
                            .interact(
                                body,
                                ui.scope_id().with(("grid-page", page)),
                                Sense::click_and_drag(),
                            )
                            .on_hover_text(format!("Page {} · drag to rearrange", page + 1));
                        let mut checked = picked;
                        let check = ui
                            .scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(Rect::from_min_size(
                                        rect.min + Vec2::new(8.0, 4.0),
                                        Vec2::new(100.0, 28.0),
                                    ))
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                |ui| {
                                    ui.add(egui::Checkbox::new(
                                        &mut checked,
                                        format!("Page {}", page + 1),
                                    ))
                                },
                            )
                            .inner;
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
                        if response.drag_started() {
                            if !selected.contains(&page) {
                                *selected = BTreeSet::from([page]);
                            }
                            *dragging = Some(selected.iter().copied().collect());
                        }
                        if dragging.is_some()
                            && let Some(pointer) = pointer
                            && rect.contains(pointer)
                        {
                            let after = pointer.x >= rect.center().x;
                            landing = Some(page + usize::from(after));
                            let x = if after {
                                rect.max.x + 3.0
                            } else {
                                rect.min.x - 3.0
                            };
                            ui.painter().line_segment(
                                [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
                                Stroke::new(3.0, ui.visuals().selection.bg_fill),
                            );
                        }
                    }
                }
                if dragging.is_some()
                    && let Some(pointer) = pointer
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let clip = ui.clip_rect();
                    if clip.contains(pointer) {
                        if landing.is_none() {
                            let row =
                                ((pointer.y - area.min.y) / row_height).floor().max(0.0) as usize;
                            let col =
                                ((pointer.x - area.min.x) / cell_width).floor().max(0.0) as usize;
                            let page = (row * columns + col.min(columns - 1)).min(count);
                            let after =
                                pointer.x - area.min.x - col as f32 * cell_width > cell_width / 2.0;
                            landing = Some((page + usize::from(after)).min(count));
                        }
                        if pointer.y > area.min.y + rows as f32 * row_height {
                            landing = Some(count);
                        }
                        if pointer.y < clip.min.y + 30.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, 10.0));
                        } else if pointer.y > clip.max.y - 30.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, -10.0));
                        }
                    }
                }
            });
        let jobs = wanted
            .into_iter()
            .filter_map(|page| {
                o.layout.geom(page).map(|g| Job::Thumb {
                    doc: o.doc.clone(),
                    generation: o.generation,
                    page,
                    scale: super::sidebar::thumb_scale(
                        g.size(),
                        f64::from(ui.ctx().pixels_per_point()),
                    ),
                })
            })
            .collect();
        self.pool.set_jobs(jobs);
        let selected: Vec<_> = self.organize.selected.iter().copied().collect();
        if self.organize.dragging.is_some() && ui.input(|i| i.pointer.any_released()) {
            let pages = self.organize.dragging.take().unwrap();
            if let Some(to) = landing {
                let moved = self.move_pages(&pages, to);
                self.organize.selected = moved.into_iter().collect();
            }
        } else if let Some(to) = move_to {
            let moved = self.move_pages(&selected, to);
            self.organize.selected = moved.into_iter().collect();
        }
        if rotate {
            self.rotate_some(&selected, 1);
        }
        if export {
            self.extract_pages(&selected);
        }
        if print {
            self.print_pages(&selected);
        }
        if let Some(page) = go {
            self.organize.grid = false;
            self.go_to_page(page);
        }
        self.organize.generation = self.open.as_ref().map(|o| o.generation);
        if insert {
            self.show_organize(Mode::Insert);
        }
        if split {
            self.show_organize(Mode::Split);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use eframe::egui::{Event, Modifiers, PointerButton, RawInput};

    fn frame(viewer: &mut Viewer, ctx: &egui::Context, events: Vec<Event>) {
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
        viewer.open_with_password(data, "grid-test.pdf".into(), "", None);
        assert!(viewer.open.is_some());
        frame(&mut viewer, &ctx, vec![]);
        frame(&mut viewer, &ctx, vec![]);
        viewer.organize.selected = BTreeSet::from([0, 2]);
        let start = viewer.organize.cards[0].center();
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
        frame(
            &mut viewer,
            &ctx,
            vec![Event::PointerMoved(start + Vec2::new(30.0, 0.0))],
        );
        assert!(
            viewer.organize.dragging.is_some(),
            "page card must capture the drag"
        );
        frame(&mut viewer, &ctx, vec![Event::PointerMoved(end)]);
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
