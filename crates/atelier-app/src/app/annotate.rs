//! Marking up pages: highlighting selected text, pinning comments,
//! drawing freehand and signing. Each mark is added to the session like
//! any other change, so it is saved with the file and can be undone.

use std::collections::HashSet;
use std::path::PathBuf;

use atelier_form::{Markup, NewAnnot, line_quads};
use atelier_raster::Point;
use atelier_syntax::Object;
use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Pos2, Sense, Shape, Stroke, Vec2};

use super::Viewer;
use crate::settings::Settings;

/// What a click or drag on the page does.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) enum Tool {
    /// Select text and fill fields, as usual.
    #[default]
    Select,
    /// A click pins a comment.
    Note,
    /// Drags draw freehand lines.
    Draw,
    /// A click places the saved signature.
    Sign,
}

/// Colours to pick from, by tool.
const HIGHLIGHTS: [[f64; 3]; 4] = [
    [1.0, 0.92, 0.23],
    [0.55, 0.93, 0.45],
    [1.0, 0.6, 0.8],
    [0.5, 0.8, 1.0],
];
const INKS: [[f64; 3]; 4] = [
    [0.05, 0.12, 0.45],
    [0.0, 0.0, 0.0],
    [0.8, 0.1, 0.1],
    [0.0, 0.45, 0.2],
];
const NOTE_COLOR: [f64; 3] = [1.0, 0.82, 0.25];
/// A placed signature's width, in points.
const SIGNATURE_WIDTH: f64 = 150.0;

/// The comment being written.
pub(super) struct NoteEditor {
    page: usize,
    at: Point,
    text: String,
    focus: bool,
}

/// The signature pad: strokes in its own box, 0 to 1 across.
#[derive(Default)]
pub(super) struct Pad {
    strokes: Vec<Vec<Pos2>>,
}

pub(super) struct Annotate {
    pub tool: Tool,
    highlight: [f64; 3],
    ink: [f64; 3],
    width: f64,
    /// Lines drawn and not yet added, on one page.
    strokes: Vec<Vec<Point>>,
    strokes_page: Option<usize>,
    /// The line being drawn.
    drawing: Option<Vec<Point>>,
    pub note: Option<NoteEditor>,
    pub pad: Option<Pad>,
    /// The saved signature: strokes in a box 1 wide, y down.
    signature: Option<Vec<Vec<Point>>>,
}

impl Default for Annotate {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            highlight: HIGHLIGHTS[0],
            ink: INKS[0],
            width: 1.5,
            strokes: Vec::new(),
            strokes_page: None,
            drawing: None,
            note: None,
            pad: None,
            signature: load_signature(),
        }
    }
}

impl Annotate {
    /// Drops work in progress, as when another file comes forward.
    pub fn reset(&mut self) {
        self.tool = Tool::Select;
        self.strokes.clear();
        self.strokes_page = None;
        self.drawing = None;
        self.note = None;
    }
}

fn color32(c: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

/// Where the signature is kept: `signature.txt` beside the settings, one
/// stroke per line as `x,y` pairs.
fn signature_path() -> Option<PathBuf> {
    Settings::path().map(|p| p.with_file_name("signature.txt"))
}

fn load_signature() -> Option<Vec<Vec<Point>>> {
    let text = std::fs::read_to_string(signature_path()?).ok()?;
    let strokes = parse_signature(&text);
    (!strokes.is_empty()).then_some(strokes)
}

fn parse_signature(text: &str) -> Vec<Vec<Point>> {
    text.lines()
        .map(|line| {
            line.split_whitespace()
                .filter_map(|pair| {
                    let (x, y) = pair.split_once(',')?;
                    Some(Point::new(x.parse().ok()?, y.parse().ok()?))
                })
                .filter(|p| p.x.is_finite() && p.y.is_finite())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn signature_text(strokes: &[Vec<Point>]) -> String {
    strokes
        .iter()
        .map(|s| {
            let pairs: Vec<String> = s.iter().map(|p| format!("{:.4},{:.4}", p.x, p.y)).collect();
            pairs.join(" ") + "\n"
        })
        .collect()
}

/// Pad strokes scaled into a box 1 wide (y down), keeping their shape.
fn normalise(strokes: &[Vec<Pos2>]) -> Vec<Vec<Point>> {
    let all = strokes.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in all {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let w = f64::from((x1 - x0).max(1.0));
    strokes
        .iter()
        .map(|s| {
            s.iter()
                .map(|p| Point::new(f64::from(p.x - x0) / w, f64::from(p.y - y0) / w))
                .collect()
        })
        .collect()
}

/// Leaves out points closer than `min` to the last one kept, so a slow
/// drag does not make thousands of points.
fn thin(stroke: &[Point], min: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    for (i, &p) in stroke.iter().enumerate() {
        let last = i + 1 == stroke.len();
        if out.last().is_none_or(|q| (p - *q).length() >= min) || last {
            out.push(p);
        }
    }
    out
}

impl Viewer {
    /// Picks a tool; the one in use is put down when picked again.
    pub(super) fn set_tool(&mut self, tool: Tool) {
        self.commit_ink();
        let a = &mut self.annotate;
        a.tool = if a.tool == tool { Tool::Select } else { tool };
        a.note = None;
        if a.tool == Tool::Sign && a.signature.is_none() {
            a.pad = Some(Pad::default());
            a.tool = Tool::Select;
        }
        if a.tool != Tool::Select {
            self.selection = None;
        }
    }

    /// Highlights the selected text.
    pub(super) fn highlight_selection(&mut self) {
        let Some(sel) = self.selection.filter(|s| !s.is_empty()) else {
            self.notice = Some("Select some text first, then highlight it.".into());
            return;
        };
        self.finish_editing(true);
        let color = self.annotate.highlight;
        let Some(o) = &mut self.open else { return };
        let mut pages = HashSet::new();
        for page in sel.start().page..=sel.end().page {
            let Some(text) = o.text.get(&page) else {
                continue;
            };
            let Some(range) = sel.range_on(page, text.glyphs.len()) else {
                continue;
            };
            let quads = line_quads(&text.glyphs[range]);
            if quads.is_empty() {
                continue;
            }
            o.session.add_annot(
                page,
                NewAnnot {
                    markup: Markup::Highlight { quads },
                    color,
                },
            );
            pages.insert(page);
        }
        self.selection = None;
        if !pages.is_empty() {
            self.rebuild(&pages, false);
        }
    }

    /// Adds the lines drawn so far as one ink annotation.
    pub(super) fn commit_ink(&mut self) {
        let a = &mut self.annotate;
        a.drawing = None;
        let (Some(page), false) = (a.strokes_page.take(), a.strokes.is_empty()) else {
            a.strokes.clear();
            return;
        };
        let strokes = std::mem::take(&mut a.strokes);
        let annot = NewAnnot {
            markup: Markup::Ink {
                strokes,
                width: a.width,
            },
            color: a.ink,
        };
        if let Some(o) = &mut self.open {
            o.session.add_annot(page, annot);
            self.rebuild(&HashSet::from([page]), false);
        }
    }

    /// Takes back the newest mark (Ctrl+Z), or the line just drawn.
    pub(super) fn undo_mark(&mut self) {
        let a = &mut self.annotate;
        if a.strokes.pop().is_some() {
            if a.strokes.is_empty() {
                a.strokes_page = None;
            }
            return;
        }
        let Some(o) = &mut self.open else { return };
        match o.session.undo_annot() {
            Some(page) => self.rebuild(&HashSet::from([page]), false),
            None => self.notice = Some("No marks to take back.".into()),
        }
    }

    /// Removes an added mark.
    pub(super) fn remove_mark(&mut self, index: usize) {
        let Some(o) = &mut self.open else { return };
        if let Some(page) = o.session.remove_annot(index) {
            self.rebuild(&HashSet::from([page]), false);
        }
    }

    /// The added mark under a layout point, newest first.
    pub(super) fn mark_at(&self, lx: f64, ly: f64) -> Option<usize> {
        let o = self.open.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let p = o.layout.to_user(page, lx, ly)?;
        o.session
            .added_annots()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (on, a))| {
                let r = a.rect();
                *on == page && p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
            })
            .map(|(i, _)| i)
    }

    /// The text of a comment (or any annotation with text) under a layout
    /// point, from the document as it stands.
    fn comment_at(&self, lx: f64, ly: f64) -> Option<String> {
        let o = self.open.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let p = o.layout.to_user(page, lx, ly)?;
        let doc = &o.doc;
        let pages = doc.pages().ok()?;
        let Ok(Object::Array(annots)) = doc.get_key(&pages.get(page)?.dict, b"Annots") else {
            return None;
        };
        for a in annots.iter().rev().take(2000) {
            let Ok(d) = doc.resolve_dict(a) else { continue };
            let popup = doc
                .get_key(&d, b"Subtype")
                .ok()
                .is_some_and(|s| s.as_name().is_some_and(|n| *n == b"Popup"));
            let Ok(Object::String(text)) = doc.get_key(&d, b"Contents") else {
                continue;
            };
            let rect = doc
                .get_key(&d, b"Rect")
                .ok()
                .and_then(|r| atelier_syntax::Rect::from_array(r.as_array()?));
            let inside = rect.is_some_and(|r| {
                p.x >= r.x0.min(r.x1)
                    && p.x <= r.x0.max(r.x1)
                    && p.y >= r.y0.min(r.y1)
                    && p.y <= r.y0.max(r.y1)
            });
            let text = text.to_text();
            if inside && !popup && !text.trim().is_empty() {
                return Some(text);
            }
        }
        None
    }

    /// Handles the pointer while a tool is in use, and shows comments
    /// under it. Returns true when the pointer was used.
    pub(super) fn annot_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Option<(f64, f64)>,
    ) -> bool {
        let tool = self.annotate.tool;
        if tool == Tool::Select {
            if let Some((lx, ly)) = at
                && response.hovered()
                && !self.selecting
                && let Some(text) = self.comment_at(lx, ly)
            {
                let mut text = text;
                if text.chars().count() > 600 {
                    text = text.chars().take(600).collect::<String>() + "…";
                }
                response.clone().on_hover_text_at_pointer(text);
            }
            return false;
        }
        let Some(o) = &self.open else { return false };
        let user = at.and_then(|(lx, ly)| {
            let (page, _, _) = o.layout.page_at(lx, ly)?;
            Some((page, o.layout.to_user(page, lx, ly)?))
        });
        match tool {
            Tool::Select => false,
            Tool::Note | Tool::Sign => {
                if user.is_some() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if response.clicked()
                    && let Some((page, p)) = user
                {
                    if tool == Tool::Note {
                        self.annotate.note = Some(NoteEditor {
                            page,
                            at: p,
                            text: String::new(),
                            focus: true,
                        });
                    } else {
                        self.place_signature(page, p);
                    }
                }
                true
            }
            Tool::Draw => {
                if user.is_some() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                let zoom = o.layout.zoom();
                let a = &mut self.annotate;
                if response.drag_started_by(egui::PointerButton::Primary)
                    && let Some((page, p)) = user
                {
                    // Lines on another page start a new mark.
                    if a.strokes_page.is_some_and(|q| q != page) {
                        self.commit_ink();
                    }
                    let a = &mut self.annotate;
                    a.strokes_page = Some(page);
                    a.drawing = Some(vec![p]);
                } else if response.dragged_by(egui::PointerButton::Primary)
                    && let (Some(line), Some((page, p))) = (&mut a.drawing, user)
                    && a.strokes_page == Some(page)
                {
                    line.push(p);
                    ui.ctx().request_repaint();
                } else if response.drag_stopped()
                    && let Some(line) = a.drawing.take()
                {
                    // Points closer than half a screen pixel add nothing.
                    a.strokes.push(thin(&line, 0.5 / zoom));
                } else if response.clicked()
                    && let Some((page, p)) = user
                {
                    if a.strokes_page.is_some_and(|q| q != page) {
                        self.commit_ink();
                    }
                    let a = &mut self.annotate;
                    a.strokes_page = Some(page);
                    a.strokes.push(vec![p]);
                }
                true
            }
        }
    }

    /// Places the saved signature centred on a point.
    fn place_signature(&mut self, page: usize, at: Point) {
        let Some(sig) = self.annotate.signature.clone() else {
            return;
        };
        let height = sig.iter().flatten().map(|p| p.y).fold(0.0_f64, f64::max);
        let s = SIGNATURE_WIDTH;
        let strokes = sig
            .iter()
            .map(|st| {
                st.iter()
                    .map(|p| Point::new(at.x + (p.x - 0.5) * s, at.y + (height / 2.0 - p.y) * s))
                    .collect()
            })
            .collect();
        let annot = NewAnnot {
            markup: Markup::Ink {
                strokes,
                width: 1.6,
            },
            color: self.annotate.ink,
        };
        let Some(o) = &mut self.open else { return };
        o.session.add_annot(page, annot);
        self.annotate.tool = Tool::Select;
        self.rebuild(&HashSet::from([page]), false);
        self.notice = Some("Signed. Right-click the signature to remove it, or Ctrl+Z.".into());
    }

    /// Draws the lines being drawn, and the signature under the pointer
    /// while placing it.
    pub(super) fn draw_marks(&self, ui: &egui::Ui, origin: Pos2) {
        let Some(o) = &self.open else { return };
        let shift = self.centre_shift() as f32;
        let a = &self.annotate;
        let zoom = o.layout.zoom() as f32;
        let to_screen = |page: usize, p: Point| -> Option<Pos2> {
            let q = o.layout.user_to_layout(page)?.apply(p);
            Some(origin + Vec2::new(q.x as f32 + shift, q.y as f32))
        };
        let painter = ui.painter();
        if let Some(page) = a.strokes_page {
            let stroke = Stroke::new((a.width as f32 * zoom).max(1.0), color32(a.ink));
            for line in a.strokes.iter().chain(a.drawing.iter()) {
                let pts: Vec<Pos2> = line.iter().filter_map(|p| to_screen(page, *p)).collect();
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], stroke.width / 2.0, stroke.color);
                } else {
                    painter.add(Shape::line(pts, stroke));
                }
            }
        }
        if a.tool == Tool::Sign
            && let Some(sig) = &a.signature
            && let Some(hover) = ui.ctx().pointer_hover_pos()
        {
            let s = SIGNATURE_WIDTH as f32 * zoom;
            let height = sig.iter().flatten().map(|p| p.y).fold(0.0, f64::max) as f32;
            let stroke = Stroke::new(1.6 * zoom, color32(a.ink).gamma_multiply(0.6));
            for line in sig {
                let pts: Vec<Pos2> = line
                    .iter()
                    .map(|p| {
                        hover + Vec2::new((p.x as f32 - 0.5) * s, (p.y as f32 - height / 2.0) * s)
                    })
                    .collect();
                painter.add(Shape::line(pts, stroke));
            }
        }
    }

    /// Keys while marking up: Enter adds the lines drawn, Esc puts the tool
    /// down, Ctrl+Z takes back the newest mark.
    pub(super) fn annot_keys(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.undo_mark();
        }
        if self.annotate.tool == Tool::Select {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.commit_ink();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.commit_ink();
            self.annotate.tool = Tool::Select;
        }
    }

    /// The bar along the bottom while a tool is in use.
    pub(super) fn tool_bar(&mut self, ui: &mut egui::Ui) {
        let tool = self.annotate.tool;
        if tool == Tool::Select || self.open.is_none() {
            return;
        }
        let mut done = false;
        let mut redo_signature = false;
        egui::Panel::bottom("tool").show(ui, |ui| {
            ui.horizontal(|ui| {
                let a = &mut self.annotate;
                match tool {
                    Tool::Note => {
                        ui.label("Comment: click where it goes.");
                    }
                    Tool::Draw => {
                        ui.label("Draw: drag on a page.");
                        swatches(ui, &INKS, &mut a.ink);
                        ui.add(
                            egui::Slider::new(&mut a.width, 0.5..=8.0)
                                .text("pt")
                                .max_decimals(1),
                        );
                    }
                    Tool::Sign => {
                        ui.label("Sign: click where your signature goes.");
                        swatches(ui, &INKS, &mut a.ink);
                        redo_signature = ui.button("Draw a new signature…").clicked();
                    }
                    Tool::Select => {}
                }
                ui.separator();
                let label = if tool == Tool::Draw {
                    "Done (Enter)"
                } else {
                    "Done (Esc)"
                };
                done = ui.button(label).clicked();
            });
        });
        if redo_signature {
            self.annotate.pad = Some(Pad::default());
            self.annotate.tool = Tool::Select;
        }
        if done {
            self.commit_ink();
            self.annotate.tool = Tool::Select;
        }
    }

    /// The Mark menu's items.
    pub(super) fn mark_menu(&mut self, ui: &mut egui::Ui) {
        let has_selection = self.selection.is_some_and(|s| !s.is_empty());
        ui.horizontal(|ui| {
            ui.label("Highlight");
            swatches(ui, &HIGHLIGHTS, &mut self.annotate.highlight);
        });
        if ui
            .add_enabled(
                has_selection,
                egui::Button::new("Highlight the selected text"),
            )
            .on_disabled_hover_text("Select some text first")
            .clicked()
        {
            self.highlight_selection();
            ui.close();
        }
        ui.separator();
        let tool = self.annotate.tool;
        if ui
            .add(egui::Button::selectable(
                tool == Tool::Note,
                "Add a comment",
            ))
            .clicked()
        {
            self.set_tool(Tool::Note);
            ui.close();
        }
        if ui
            .add(egui::Button::selectable(tool == Tool::Draw, "Draw"))
            .clicked()
        {
            self.set_tool(Tool::Draw);
            ui.close();
        }
        if ui
            .add(egui::Button::selectable(tool == Tool::Sign, "Sign…"))
            .on_hover_text("Draw your signature once, then click to place it")
            .clicked()
        {
            self.set_tool(Tool::Sign);
            ui.close();
        }
        ui.separator();
        let any = self
            .open
            .as_ref()
            .is_some_and(|o| !o.session.added_annots().is_empty());
        if ui
            .add_enabled(
                any,
                egui::Button::new("Undo the last mark").shortcut_text("Ctrl+Z"),
            )
            .clicked()
        {
            self.undo_mark();
            ui.close();
        }
    }

    /// The comment being written, by the spot it goes.
    pub(super) fn note_window(&mut self, ctx: &egui::Context) {
        let Some(note) = &mut self.annotate.note else {
            return;
        };
        let mut add = false;
        let mut cancel = false;
        egui::Window::new("Comment")
            .collapsible(false)
            .resizable(false)
            .default_pos(
                ctx.pointer_interact_pos()
                    .unwrap_or(Pos2::new(200.0, 200.0)),
            )
            .show(ctx, |ui| {
                // Taken before the text box, which would make it a new line.
                let shortcut = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut note.text)
                        .desired_rows(4)
                        .desired_width(260.0)
                        .hint_text("Type your comment"),
                );
                if note.focus {
                    edit.request_focus();
                    note.focus = false;
                }
                ui.horizontal(|ui| {
                    add = ui
                        .add_enabled(
                            !note.text.trim().is_empty(),
                            egui::Button::new("Add").shortcut_text("Ctrl+Enter"),
                        )
                        .clicked()
                        || (shortcut && !note.text.trim().is_empty());
                    cancel =
                        ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                });
            });
        if cancel {
            self.annotate.note = None;
        } else if add && let Some(note) = self.annotate.note.take() {
            let Some(o) = &mut self.open else { return };
            o.session.add_annot(
                note.page,
                NewAnnot {
                    markup: Markup::Note {
                        at: note.at,
                        text: note.text.trim_end().to_string(),
                    },
                    color: NOTE_COLOR,
                },
            );
            self.rebuild(&HashSet::from([note.page]), false);
        }
    }

    /// The signature pad.
    pub(super) fn pad_window(&mut self, ctx: &egui::Context) {
        let Some(pad) = &mut self.annotate.pad else {
            return;
        };
        let mut use_it = false;
        let mut cancel = false;
        let ink = color32(self.annotate.ink);
        egui::Window::new("Your signature")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("Sign in the box with the mouse, pen or a finger.");
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(420.0, 160.0), Sense::drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 4.0, Color32::WHITE);
                p.hline(
                    rect.x_range().shrink(20.0),
                    rect.max.y - 36.0,
                    Stroke::new(1.0, Color32::from_gray(190)),
                );
                if resp.drag_started() {
                    pad.strokes.push(Vec::new());
                }
                if resp.dragged()
                    && let (Some(pos), Some(line)) =
                        (resp.interact_pointer_pos(), pad.strokes.last_mut())
                {
                    let pos = pos.clamp(rect.min, rect.max);
                    if line.last().is_none_or(|q| q.distance(pos) > 0.8) {
                        line.push(pos);
                    }
                }
                for line in &pad.strokes {
                    p.add(Shape::line(line.clone(), Stroke::new(2.2, ink)));
                }
                ui.horizontal(|ui| {
                    let any = pad.strokes.iter().any(|s| s.len() > 1);
                    use_it = ui
                        .add_enabled(any, egui::Button::new("Use this signature"))
                        .on_hover_text("Kept on this computer only, for signing again")
                        .clicked();
                    if ui.button("Clear").clicked() {
                        pad.strokes.clear();
                    }
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.annotate.pad = None;
        } else if use_it && let Some(pad) = self.annotate.pad.take() {
            let strokes: Vec<Vec<Pos2>> =
                pad.strokes.into_iter().filter(|s| !s.is_empty()).collect();
            let sig = normalise(&strokes);
            if let Some(path) = signature_path() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&path, signature_text(&sig)) {
                    self.error = Some(format!("Could not keep the signature: {e}"));
                }
            }
            self.annotate.signature = Some(sig);
            self.annotate.tool = Tool::Sign;
            self.selection = None;
        }
    }
}

/// A row of colour buttons.
fn swatches(ui: &mut egui::Ui, colours: &[[f64; 3]], current: &mut [f64; 3]) {
    for c in colours {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
        ui.painter().rect_filled(rect.shrink(1.0), 3.0, color32(*c));
        if *c == *current {
            ui.painter().rect_stroke(
                rect,
                3.0,
                Stroke::new(2.0, ui.visuals().strong_text_color()),
                egui::StrokeKind::Inside,
            );
        }
        if resp.clicked() {
            *current = *c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_survive_a_round_trip() {
        let strokes = normalise(&[
            vec![Pos2::new(10.0, 20.0), Pos2::new(110.0, 40.0)],
            vec![Pos2::new(60.0, 30.0)],
        ]);
        assert!((strokes[0][1].x - 1.0).abs() < 1e-9);
        let back = parse_signature(&signature_text(&strokes));
        assert_eq!(back.len(), 2);
        assert!((back[0][1].y - 0.2).abs() < 1e-3);
        assert!(parse_signature("1,nan\n\n").is_empty());
    }

    #[test]
    fn thinning_keeps_the_ends() {
        let line: Vec<Point> = (0..100)
            .map(|i| Point::new(f64::from(i) * 0.01, 0.0))
            .collect();
        let t = thin(&line, 0.25);
        assert_eq!(t.first(), line.first());
        assert_eq!(t.last(), line.last());
        assert!(t.len() < 10);
    }
}
