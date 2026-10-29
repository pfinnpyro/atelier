//! The Print dialog: a preview of the page on the paper, and the choices
//! (printer, pages, copies, orientation, colour, sizing, two-sided).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use atelier_print::{
    Duplex, Options, Orientation, Paper, Picture, Printer, Sizing, Source, landscape, parse_pages,
    place,
};
use atelier_render::{RenderOptions, render_page};
use atelier_syntax::Document;
use atelier_view::PageGeom;
use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Shadow, Stroke,
    StrokeKind, Vec2,
};

use super::Viewer;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    All,
    Current,
    Range,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeChoice {
    Fit,
    Actual,
    Shrink,
    Custom,
}

/// A print job under way.
struct Running {
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

/// The dialog's state. The choices are kept between prints.
pub(super) struct PrintDialog {
    pub open: bool,
    printers: Vec<Printer>,
    printer: usize,
    paper: Option<Paper>,
    which: Which,
    range: String,
    copies: u32,
    collate: bool,
    orientation: Orientation,
    color: bool,
    fit: SizeChoice,
    percent: f64,
    duplex: Duplex,
    /// Which of the chosen pages the preview shows.
    preview: usize,
    running: Option<Running>,
    error: Option<String>,
}

impl Default for PrintDialog {
    fn default() -> Self {
        Self {
            open: false,
            printers: Vec::new(),
            printer: 0,
            paper: None,
            which: Which::All,
            range: String::new(),
            copies: 1,
            collate: true,
            orientation: Orientation::Auto,
            color: true,
            fit: SizeChoice::Fit,
            percent: 100.0,
            duplex: Duplex::Off,
            preview: 0,
            running: None,
            error: None,
        }
    }
}

impl PrintDialog {
    fn sizing(&self) -> Sizing {
        match self.fit {
            SizeChoice::Fit => Sizing::Fit,
            SizeChoice::Actual => Sizing::Actual,
            SizeChoice::Shrink => Sizing::Shrink,
            SizeChoice::Custom => Sizing::Custom(self.percent),
        }
    }

    fn pages(&self, current: usize, count: usize) -> Result<Vec<usize>, String> {
        match self.which {
            Which::All => Ok((0..count).collect()),
            Which::Current => Ok(vec![current.min(count.saturating_sub(1))]),
            Which::Range => parse_pages(&self.range, count),
        }
    }

    fn select_printer(&mut self, i: usize) {
        self.printer = i;
        self.paper = self
            .printers
            .get(i)
            .and_then(|p| atelier_print::paper(&p.name));
    }
}

/// The open document, for printing from a thread of its own.
struct DocSource {
    doc: Arc<Document>,
}

impl Source for DocSource {
    fn page_size(&self, page: usize) -> (f64, f64) {
        self.doc
            .pages()
            .ok()
            .and_then(|p| p.get(page))
            .map_or((612.0, 792.0), |p| PageGeom::of(p).size())
    }

    fn draw(&self, page: usize, scale: f64) -> Option<Picture> {
        let p = self.doc.pages().ok()?.get(page)?;
        let options = RenderOptions {
            scale,
            ..RenderOptions::default()
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_page(&self.doc, p, &options)
        }))
        .ok()?
        .ok()?;
        Some(Picture {
            width: r.pixmap.width(),
            height: r.pixmap.height(),
            rgba: r.pixmap.data().to_vec(),
        })
    }

    fn pdf(&self) -> &[u8] {
        self.doc.data()
    }
}

/// Colours of the preview.
const DESK: Color32 = Color32::from_rgb(60, 62, 66);
const SHEET: Color32 = Color32::WHITE;
const MARGIN: Color32 = Color32::from_rgb(200, 205, 215);

impl Viewer {
    /// Opens the dialog, finding the printers afresh.
    pub(super) fn open_print(&mut self) {
        if self.open.is_none() {
            return;
        }
        self.finish_editing(true);
        let d = &mut self.print;
        d.open = true;
        d.error = None;
        d.preview = 0;
        let keep = d.printers.get(d.printer).map(|p| p.name.clone());
        d.printers = atelier_print::printers();
        let i = keep
            .and_then(|k| d.printers.iter().position(|p| p.name == k))
            .unwrap_or(0);
        d.select_printer(i);
    }

    /// The page the preview wants a picture of.
    pub(super) fn print_preview_page(&self) -> Option<usize> {
        if !self.print.open {
            return None;
        }
        let o = self.open.as_ref()?;
        let pages = self
            .print
            .pages(self.current_page(), o.layout.page_count())
            .ok()?;
        pages
            .get(self.print.preview.min(pages.len().saturating_sub(1)))
            .copied()
    }

    pub(super) fn print_window(&mut self, ctx: &egui::Context) {
        if !self.print.open {
            return;
        }
        self.print_progress(ctx);
        let mut open = true;
        egui::Window::new("Print")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(640.0, 420.0))
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(270.0, 410.0),
                        Layout::top_down(Align::Center),
                        |ui| self.print_preview(ui),
                    );
                    ui.separator();
                    ui.vertical(|ui| self.print_options(ui));
                });
            });
        if !open && self.print.running.is_none() {
            self.print.open = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.print.running.is_none() {
            self.print.open = false;
        }
    }

    fn print_preview(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &self.open else { return };
        let count = o.layout.page_count();
        let pages = self
            .print
            .pages(self.current_page(), count)
            .unwrap_or_default();
        let n = pages.len();
        if self.print.preview >= n {
            self.print.preview = n.saturating_sub(1);
        }
        let (desk, _) = ui.allocate_exact_size(Vec2::new(266.0, 360.0), Sense::hover());
        let p = ui.painter_at(desk);
        p.rect_filled(desk, CornerRadius::same(6), DESK);
        let Some(&page) = pages.get(self.print.preview) else {
            p.text(
                desk.center(),
                egui::Align2::CENTER_CENTER,
                "No pages",
                egui::FontId::proportional(14.0),
                Color32::LIGHT_GRAY,
            );
            return;
        };
        let size = o.layout.geom(page).map_or((612.0, 792.0), |g| g.size());
        let upright = self.print.paper.unwrap_or(Paper::LETTER);
        let turned = landscape(size, self.print.orientation);
        let paper = if turned { upright.turned() } else { upright };
        // The sheet, as big as fits on the desk.
        let room = desk.shrink(18.0);
        let k = (room.width() as f64 / paper.size.0).min(room.height() as f64 / paper.size.1);
        let sheet = Rect::from_center_size(
            desk.center(),
            Vec2::new((paper.size.0 * k) as f32, (paper.size.1 * k) as f32),
        );
        let at = |x: f64, y: f64| sheet.min + Vec2::new((x * k) as f32, (y * k) as f32);
        p.add(
            Shadow {
                offset: [0, 3],
                blur: 10,
                spread: 0,
                color: Color32::from_black_alpha(90),
            }
            .as_shape(sheet, CornerRadius::ZERO),
        );
        p.rect_filled(sheet, CornerRadius::ZERO, SHEET);
        let (l, t, r, b) = paper.printable;
        p.rect_stroke(
            Rect::from_min_max(at(l, t), at(r, b)),
            CornerRadius::ZERO,
            Stroke::new(1.0, MARGIN),
            StrokeKind::Inside,
        );
        let spot = place(size, &paper, self.print.sizing());
        let page_rect =
            Rect::from_min_max(at(spot.x, spot.y), at(spot.x + spot.w, spot.y + spot.h));
        let clip = p.with_clip_rect(sheet);
        match o.thumbs.get(&page) {
            Some(tex) => {
                clip.image(
                    tex.id(),
                    page_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                clip.rect_filled(page_rect, CornerRadius::ZERO, Color32::from_gray(235));
                clip.text(
                    page_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("Page {}", page + 1),
                    egui::FontId::proportional(13.0),
                    Color32::GRAY,
                );
            }
        }
        clip.rect_stroke(
            page_rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_gray(210)),
            StrokeKind::Outside,
        );
        if !self.print.color {
            let badge = Rect::from_min_size(desk.min + Vec2::new(8.0, 8.0), Vec2::new(104.0, 20.0));
            p.rect_filled(
                badge,
                CornerRadius::same(10),
                Color32::from_black_alpha(150),
            );
            p.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "Black & white",
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        // Paging through the preview.
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(62.0);
            if ui
                .add_enabled(self.print.preview > 0, egui::Button::new("◀"))
                .clicked()
            {
                self.print.preview -= 1;
            }
            ui.label(format!("Sheet {} of {n}", self.print.preview + 1));
            if ui
                .add_enabled(self.print.preview + 1 < n, egui::Button::new("▶"))
                .clicked()
            {
                self.print.preview += 1;
            }
        });
    }

    fn print_options(&mut self, ui: &mut egui::Ui) {
        let count = self.open.as_ref().map_or(0, |o| o.layout.page_count());
        let current = self.current_page();
        let busy = self.print.running.is_some();
        let d = &mut self.print;
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("print-options")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Printer").strong());
                    ui.horizontal(|ui| {
                        let name = d
                            .printers
                            .get(d.printer)
                            .map_or("No printers found".to_string(), |p| p.name.clone());
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("printer")
                            .width(190.0)
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for (i, p) in d.printers.iter().enumerate() {
                                    let label = if p.default {
                                        format!("{} (default)", p.name)
                                    } else {
                                        p.name.clone()
                                    };
                                    if ui.selectable_label(i == d.printer, label).clicked() {
                                        pick = Some(i);
                                    }
                                }
                            });
                        if let Some(i) = pick {
                            d.select_printer(i);
                        }
                        if cfg!(windows)
                            && ui
                                .small_button("⚙")
                                .on_hover_text("The printer's own settings")
                                .clicked()
                            && let Some(p) = d.printers.get(d.printer)
                            && let Err(e) = atelier_print::printer_properties(&p.name)
                        {
                            d.error = Some(e);
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("Pages").strong());
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut d.which, Which::All, format!("All {count}"));
                            ui.selectable_value(
                                &mut d.which,
                                Which::Current,
                                format!("Current ({})", current + 1),
                            );
                            ui.selectable_value(&mut d.which, Which::Range, "Range");
                        });
                        if d.which == Which::Range {
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut d.range)
                                    .hint_text("e.g. 1-3, 5")
                                    .desired_width(160.0),
                            );
                            if r.changed() {
                                d.preview = 0;
                            }
                            if let Err(e) = parse_pages(&d.range, count)
                                && !d.range.trim().is_empty()
                            {
                                ui.label(
                                    RichText::new(e)
                                        .small()
                                        .color(Color32::from_rgb(230, 120, 100)),
                                );
                            }
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("Copies").strong());
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut d.copies)
                                .range(1..=999)
                                .speed(0.1),
                        );
                        ui.add_enabled(
                            d.copies > 1,
                            egui::Checkbox::new(&mut d.collate, "Collate"),
                        )
                        .on_hover_text("Print whole copies one after another");
                    });
                    ui.end_row();

                    ui.label(RichText::new("Orientation").strong());
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.orientation, Orientation::Auto, "Auto")
                            .on_hover_text("Wide pages print landscape");
                        ui.selectable_value(&mut d.orientation, Orientation::Portrait, "Portrait");
                        ui.selectable_value(
                            &mut d.orientation,
                            Orientation::Landscape,
                            "Landscape",
                        );
                    });
                    ui.end_row();

                    ui.label(RichText::new("Colour").strong());
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.color, true, "Colour");
                        ui.selectable_value(&mut d.color, false, "Black & white");
                    });
                    ui.end_row();

                    ui.label(RichText::new("Size").strong());
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut d.fit, SizeChoice::Fit, "Fit")
                                .on_hover_text("Scale each page to fill the paper");
                            ui.selectable_value(&mut d.fit, SizeChoice::Actual, "Actual size");
                            ui.selectable_value(&mut d.fit, SizeChoice::Shrink, "Shrink to fit")
                                .on_hover_text("Actual size, smaller only if a page is too big");
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut d.fit, SizeChoice::Custom, "Custom");
                            ui.add_enabled(
                                d.fit == SizeChoice::Custom,
                                egui::DragValue::new(&mut d.percent)
                                    .range(10.0..=400.0)
                                    .suffix(" %")
                                    .speed(1.0),
                            );
                        });
                    });
                    ui.end_row();

                    ui.label(RichText::new("Two-sided").strong());
                    egui::ComboBox::from_id_salt("duplex")
                        .width(190.0)
                        .selected_text(match d.duplex {
                            Duplex::Off => "Off",
                            Duplex::LongEdge => "Flip on long edge",
                            Duplex::ShortEdge => "Flip on short edge",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.duplex, Duplex::Off, "Off");
                            ui.selectable_value(
                                &mut d.duplex,
                                Duplex::LongEdge,
                                "Flip on long edge",
                            );
                            ui.selectable_value(
                                &mut d.duplex,
                                Duplex::ShortEdge,
                                "Flip on short edge",
                            );
                        });
                    ui.end_row();
                });
        });
        ui.add_space(8.0);
        if let Some(e) = &self.print.error {
            ui.label(RichText::new(e).color(Color32::from_rgb(230, 120, 100)));
        }
        ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
            ui.horizontal(|ui| {
                if let Some(run) = &self.print.running {
                    if ui.button("Cancel printing").clicked() {
                        run.cancel.store(true, Ordering::SeqCst);
                    }
                    let (done, total) = (
                        run.done.load(Ordering::SeqCst),
                        run.total.load(Ordering::SeqCst).max(1),
                    );
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(200.0)
                            .text(format!(
                                "Sending sheet {} of {total}",
                                (done + 1).min(total)
                            )),
                    );
                } else {
                    let can =
                        !self.print.printers.is_empty() && self.print.pages(current, count).is_ok();
                    let print =
                        egui::Button::new(RichText::new("Print").strong().color(Color32::WHITE))
                            .fill(ui.visuals().selection.bg_fill)
                            .min_size(Vec2::new(90.0, 28.0));
                    if ui.add_enabled(can, print).clicked() {
                        self.start_print();
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(80.0, 28.0)))
                        .clicked()
                    {
                        self.print.open = false;
                    }
                }
            });
        });
    }

    fn start_print(&mut self) {
        let Some(o) = &self.open else { return };
        let count = o.layout.page_count();
        let current = self.current_page();
        let d = &mut self.print;
        let pages = match d.pages(current, count) {
            Ok(p) => p,
            Err(e) => {
                d.error = Some(e);
                return;
            }
        };
        let Some(printer) = d.printers.get(d.printer) else {
            return;
        };
        let options = Options {
            printer: printer.name.clone(),
            copies: d.copies.max(1),
            collate: d.collate,
            orientation: d.orientation,
            color: d.color,
            sizing: d.sizing(),
            duplex: d.duplex,
            pages,
        };
        let run = Running {
            done: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(1)),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let (done, total, cancel, result) = (
            run.done.clone(),
            run.total.clone(),
            run.cancel.clone(),
            run.result.clone(),
        );
        let source = DocSource { doc: o.doc.clone() };
        let title = o.name.clone();
        let wake = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("print".into())
            .spawn(move || {
                let progress = |n: usize, t: usize| {
                    done.store(n, Ordering::SeqCst);
                    total.store(t, Ordering::SeqCst);
                    wake.request_repaint();
                };
                let r = atelier_print::print(&options, &title, &source, &progress, &cancel);
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => {
                d.error = None;
                d.running = Some(run);
            }
            Err(e) => d.error = Some(format!("Could not start printing: {e}")),
        }
    }

    /// Checks on a job under way; once done, closes the dialog.
    fn print_progress(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.print.running else {
            return;
        };
        let finished = run.result.lock().unwrap_or_else(|e| e.into_inner()).take();
        match finished {
            None => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            Some(Ok(())) => {
                let sheets = run.total.load(Ordering::SeqCst);
                self.print.running = None;
                self.print.open = false;
                let s = if sheets == 1 { "" } else { "s" };
                self.notice = Some(format!("Sent {sheets} sheet{s} to the printer."));
            }
            Some(Err(e)) => {
                self.print.running = None;
                self.print.error = Some(e);
            }
        }
    }
}
