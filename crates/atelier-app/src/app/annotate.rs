//! Marking up pages: highlighting selected text, pinning comments,
//! drawing freehand and signing. Each mark is added to the session like
//! any other change, so it is saved with the file and can be undone.

use std::collections::HashSet;
use std::path::PathBuf;

use atelier_form::{Markup, NewAnnot, Review, line_quads};
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

pub(super) fn display_date(date: &str) -> String {
    let raw = date.strip_prefix("D:").unwrap_or(date);
    if raw.len() >= 14 && raw.as_bytes()[..14].iter().all(u8::is_ascii_digit) {
        format!(
            "{}-{}-{} {}:{}:{} {}",
            &raw[..4],
            &raw[4..6],
            &raw[6..8],
            &raw[8..10],
            &raw[10..12],
            &raw[12..14],
            if raw.ends_with('Z') {
                "UTC"
            } else {
                &raw[14..]
            }
        )
    } else {
        date.to_owned()
    }
}

/// The comment being written.
#[derive(Clone)]
pub(super) struct NoteEditor {
    page: usize,
    at: Point,
    text: String,
    focus: bool,
    created: String,
    modified: String,
    author: String,
    subject: String,
    color: [f64; 3],
    state: Option<String>,
    target: Option<ReviewTarget>,
    highlights: Vec<(usize, Vec<[Point; 4]>)>,
    generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReviewTarget {
    Saved(usize),
    Added(usize),
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
    pub comments_open: bool,
    transient_comments: bool,
    drafts: Vec<NoteEditor>,
    comments_cache: Vec<NoteEditor>,
    comments_generation: Option<u64>,
    reveal_comment: bool,
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
            comments_open: false,
            transient_comments: false,
            drafts: Vec::new(),
            comments_cache: Vec::new(),
            comments_generation: None,
            reveal_comment: false,
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
        self.drafts.clear();
        self.comments_cache.clear();
        self.comments_generation = None;
        if self.transient_comments {
            self.comments_open = false;
        }
        self.transient_comments = false;
        self.reveal_comment = false;
    }

    fn stash_note(&mut self) {
        if let Some(note) = self.note.take() {
            self.drafts.retain(|draft| !same_comment(draft, &note));
            self.drafts.push(note);
        }
    }

    fn focus_comment(&mut self, note: NoteEditor) {
        if !self.comments_open {
            self.comments_open = true;
            self.transient_comments = true;
        }
        if self
            .note
            .as_ref()
            .is_some_and(|current| same_comment(current, &note))
        {
            return;
        }
        self.stash_note();
        let draft = self
            .drafts
            .iter()
            .position(|draft| same_comment(draft, &note));
        self.note = Some(draft.map_or(note, |i| self.drafts.remove(i)));
        self.reveal_comment = true;
    }

    pub fn toggle_comments(&mut self) {
        if self.comments_open {
            self.stash_note();
            self.comments_open = false;
        } else {
            self.comments_open = true;
        }
        self.transient_comments = false;
    }

    fn dismiss_comment(&mut self) {
        self.stash_note();
        if self.transient_comments {
            self.comments_open = false;
            self.transient_comments = false;
        }
    }
}

fn same_comment(a: &NoteEditor, b: &NoteEditor) -> bool {
    a.page == b.page && a.target == b.target && (a.target.is_some() || a.at == b.at)
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
        self.review_selection(None);
    }

    pub(super) fn review_selection(&mut self, strike: Option<bool>) {
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
                    markup: match strike {
                        Some(strike) => Markup::TextLine { quads, strike },
                        None => Markup::Highlight { quads },
                    },
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
        if self.annotate.strokes.is_empty() && self.undo_revision() {
            return;
        }
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
    fn comment_at(&self, lx: f64, ly: f64) -> Option<NoteEditor> {
        let o = self.open.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let point = o.layout.to_user(page, lx, ly)?;
        self.comments_on_page(page, Some(point))?.into_iter().next()
    }

    fn comments_on_page(&self, page: usize, point: Option<Point>) -> Option<Vec<NoteEditor>> {
        let o = self.open.as_ref()?;
        let p = point.unwrap_or(Point::new(0.0, 0.0));
        let pages = o.doc.pages().ok()?;
        let Object::Array(annots) = o.doc.get_key(&pages.get(page)?.dict, b"Annots").ok()? else {
            return Some(Vec::new());
        };
        let added = o.session.added_annots();
        let page_added: Vec<_> = added
            .iter()
            .enumerate()
            .filter(|(_, (on, _))| *on == page)
            .map(|(i, _)| i)
            .collect();
        let original_count = annots.len().saturating_sub(page_added.len());
        let mut notes = Vec::new();
        for (ordinal, a) in annots.iter().enumerate().rev().take(2000) {
            let Ok(d) = o.doc.resolve_dict(a) else {
                continue;
            };
            let kind = o.doc.get_key(&d, b"Subtype").ok()?;
            if !kind.as_name().is_some_and(|n| {
                matches!(
                    n.0.as_slice(),
                    b"Text"
                        | b"Highlight"
                        | b"Underline"
                        | b"StrikeOut"
                        | b"Squiggly"
                        | b"FreeText"
                        | b"Ink"
                )
            }) {
                continue;
            }
            let value = |key: &[u8]| {
                o.doc
                    .get_key(&d, key)
                    .ok()
                    .and_then(|v| {
                        if let Object::String(s) = v {
                            Some(s.to_text())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            };
            let text = value(b"Contents");
            if text.is_empty() && !kind.as_name().is_some_and(|n| *n == b"Text") {
                continue;
            }
            let rect = o
                .doc
                .get_key(&d, b"Rect")
                .ok()
                .and_then(|r| atelier_syntax::Rect::from_array(r.as_array()?));
            let mut hit = rect.is_some_and(|r| {
                p.x >= r.x0.min(r.x1)
                    && p.x <= r.x0.max(r.x1)
                    && p.y >= r.y0.min(r.y1)
                    && p.y <= r.y0.max(r.y1)
            });
            let quads: Vec<[Point; 4]> = o
                .doc
                .get_key(&d, b"QuadPoints")
                .ok()
                .and_then(|v| v.as_array().map(<[_]>::to_vec))
                .unwrap_or_default()
                .as_chunks::<8>()
                .0
                .iter()
                .filter_map(|q| {
                    let v: Option<Vec<_>> = q.iter().map(Object::as_f64).collect();
                    let v = v?;
                    Some([
                        Point::new(v[4], v[5]),
                        Point::new(v[6], v[7]),
                        Point::new(v[2], v[3]),
                        Point::new(v[0], v[1]),
                    ])
                })
                .collect();
            if !quads.is_empty() {
                hit = quads.iter().any(|q| super::inside(q, p));
            }
            if hit || point.is_none() {
                notes.push(NoteEditor {
                    page,
                    at: quads
                        .first()
                        .map(|q| q[2])
                        .or_else(|| rect.map(|r| Point::new(r.x0, r.y1)))
                        .unwrap_or(p),
                    text,
                    focus: false,
                    created: value(b"CreationDate"),
                    modified: value(b"M"),
                    author: value(b"T"),
                    subject: value(b"Subj"),
                    color: o
                        .doc
                        .get_key(&d, b"C")
                        .ok()
                        .and_then(|v| {
                            let a = v.as_array()?;
                            Some([
                                a.first()?.as_f64()?,
                                a.get(1)?.as_f64()?,
                                a.get(2)?.as_f64()?,
                            ])
                        })
                        .unwrap_or(NOTE_COLOR),
                    state: Some(value(b"State")).filter(|s| !s.is_empty()),
                    target: Some(if ordinal >= original_count {
                        ReviewTarget::Added(page_added[ordinal - original_count])
                    } else {
                        ReviewTarget::Saved(ordinal)
                    }),
                    highlights: if quads.is_empty() {
                        Vec::new()
                    } else {
                        vec![(page, quads)]
                    },
                    generation: o.generation,
                });
            }
        }
        Some(notes)
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
        if matches!(tool, Tool::Select | Tool::Note) {
            if let Some((lx, ly)) = at
                && response.hovered()
                && !self.selecting
                && let Some(note) = self.comment_at(lx, ly)
            {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if response.clicked() {
                    self.review_ui.changes = false;
                    self.annotate.focus_comment(note);
                    self.selection = None;
                    return true;
                }
                response.clone().on_hover_text_at_pointer(format!(
                    "{} · {}\n{}\nClick to open comment",
                    note.author,
                    display_date(&note.created),
                    note.text.chars().take(500).collect::<String>()
                ));
                if tool == Tool::Note {
                    return true;
                }
            }
            if tool == Tool::Select {
                if response.clicked() && at.is_some() {
                    self.annotate.dismiss_comment();
                }
                return false;
            }
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
                        self.comment_here(page, p);
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
        self.notice =
            Some("Ink signature added. This is not certificate-backed. Ctrl+Z to undo.".into());
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
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z)) {
            self.redo_revision();
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
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
            .add(egui::Button::selectable(
                tool == Tool::Sign,
                "Ink signature…",
            ))
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

    pub(super) fn comment_here(&mut self, page: usize, at: Point) {
        let mut highlights = Vec::new();
        if let Some(selection) = self.selection.filter(|s| !s.is_empty())
            && let Some(o) = &self.open
        {
            for page in selection.start().page..=selection.end().page {
                if let Some(text) = o.text.get(&page)
                    && let Some(range) = selection.range_on(page, text.glyphs.len())
                {
                    let quads = line_quads(&text.glyphs[range]);
                    if !quads.is_empty() {
                        highlights.push((page, quads));
                    }
                }
            }
        }
        let date = atelier_form::annotation_date();
        self.review_ui.changes = false;
        self.annotate.stash_note();
        self.annotate.comments_open = true;
        self.annotate.transient_comments = false;
        self.annotate.reveal_comment = true;
        self.annotate.note = Some(NoteEditor {
            page,
            at,
            text: String::new(),
            focus: true,
            created: date.clone(),
            modified: date,
            author: self.settings.review_author.clone(),
            subject: String::new(),
            color: if highlights.is_empty() {
                NOTE_COLOR
            } else {
                self.annotate.highlight
            },
            state: None,
            target: None,
            highlights,
            generation: self.open.as_ref().map_or(0, |o| o.generation),
        });
    }

    pub(super) fn comments_panel(&mut self, ui: &mut egui::Ui) {
        if !self.annotate.comments_open {
            return;
        }
        let generation = self.open.as_ref().map(|o| o.generation);
        if self.annotate.comments_generation != generation {
            let count = self.open.as_ref().map_or(0, |o| o.layout.page_count());
            self.annotate.comments_cache = (0..count)
                .flat_map(|page| {
                    self.comments_on_page(page, None)
                        .unwrap_or_default()
                        .into_iter()
                        .rev()
                })
                .collect();
            self.annotate.comments_generation = generation;
        }
        let mut notes = self.annotate.comments_cache.clone();
        for draft in &self.annotate.drafts {
            if let Some(saved) = notes.iter_mut().find(|n| same_comment(n, draft)) {
                *saved = draft.clone();
            } else {
                notes.push(draft.clone());
            }
        }
        if let Some(note) = &self.annotate.note {
            if let Some(saved) = notes.iter_mut().find(|n| same_comment(n, note)) {
                *saved = note.clone();
            } else {
                notes.push(note.clone());
            }
        }
        egui::Panel::right("review-comments")
            .resizable(true)
            .default_size(340.0)
            .min_size(260.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Comments");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::icons::button(
                            ui,
                            crate::icons::Icon::Close,
                            "Close comments pane",
                        )
                        .clicked()
                        {
                            self.annotate.toggle_comments();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.review_ui.changes, false, "Comments");
                    ui.selectable_value(&mut self.review_ui.changes, true, "Changes");
                });
                ui.separator();
                if self.review_ui.changes {
                    self.revision_content(ui);
                    return;
                }
                if notes.is_empty() {
                    ui.weak("Select text or use the comment tool to add a comment.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("comments-list")
                    .show(ui, |ui| {
                        for note in notes {
                            let selected = self
                                .annotate
                                .note
                                .as_ref()
                                .is_some_and(|n| same_comment(n, &note));
                            let dimmed = self.annotate.note.is_some() && !selected;
                            ui.push_id(
                                (
                                    note.page,
                                    note.target.map(|t| match t {
                                        ReviewTarget::Saved(i) => (0, i),
                                        ReviewTarget::Added(i) => (1, i),
                                    }),
                                    note.at.x.to_bits(),
                                    note.at.y.to_bits(),
                                ),
                                |ui| {
                                    ui.scope(|ui| {
                                        if dimmed {
                                            ui.multiply_opacity(0.45);
                                        }
                                        let card = egui::Frame::new()
                                            .inner_margin(10)
                                            .outer_margin(egui::Margin::symmetric(4, 5))
                                            .corner_radius(6)
                                            .fill(ui.visuals().faint_bg_color)
                                            .stroke(Stroke::new(
                                                if selected { 2.0 } else { 1.0 },
                                                if selected {
                                                    color32(note.color)
                                                } else {
                                                    ui.visuals()
                                                        .widgets
                                                        .noninteractive
                                                        .bg_stroke
                                                        .color
                                                },
                                            ))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.weak(format!(
                                                    "Page {} · {}",
                                                    note.page + 1,
                                                    note.state.as_deref().unwrap_or("Pending")
                                                ));
                                                if selected {
                                                    ui.strong(if note.target.is_some() {
                                                        "Review comment"
                                                    } else {
                                                        "New comment"
                                                    });
                                                    self.note_editor(ui);
                                                } else {
                                                    let label = format!(
                                                        "{}\n{}\n{}",
                                                        note.author,
                                                        display_date(&note.created),
                                                        note.text
                                                            .chars()
                                                            .take(280)
                                                            .collect::<String>()
                                                    );
                                                    ui.add(egui::Label::new(label).wrap());
                                                }
                                            });
                                        if !selected
                                            && card.response.interact(Sense::click()).clicked()
                                        {
                                            self.go_to_page(note.page);
                                            self.annotate.focus_comment(note.clone());
                                        }
                                        if selected && self.annotate.reveal_comment {
                                            card.response.scroll_to_me(Some(egui::Align::Center));
                                            self.annotate.reveal_comment = false;
                                        }
                                    });
                                },
                            );
                        }
                    });
            });
    }

    /// The page remains unchanged; only the focused comment gets a connector.
    pub(super) fn draw_comment_link(&self, ui: &egui::Ui, origin: Pos2) {
        if !self.annotate.comments_open {
            return;
        }
        let Some(note) = &self.annotate.note else {
            return;
        };
        let Some(o) = &self.open else { return };
        let Some(transform) = o.layout.user_to_layout(note.page) else {
            return;
        };
        let point = transform.apply(note.at);
        let anchor =
            origin + Vec2::new(point.x as f32 + self.centre_shift() as f32, point.y as f32);
        let clip = ui.clip_rect();
        if clip.contains(anchor) {
            let color = color32(note.color);
            ui.painter().line_segment(
                [anchor, Pos2::new(clip.right(), anchor.y)],
                Stroke::new(1.5, color),
            );
            ui.painter().circle_filled(anchor, 3.0, color);
        }
    }

    fn note_editor(&mut self, ui: &mut egui::Ui) {
        let Some(note) = &mut self.annotate.note else {
            return;
        };
        let current = self
            .open
            .as_ref()
            .is_some_and(|o| o.generation == note.generation);
        let mut save = false;
        let mut cancel = false;
        let mut decision = None;
        ui.scope(|ui| {
            ui.horizontal(|ui| {
                ui.label("Author");
                ui.add(
                    egui::TextEdit::singleline(&mut note.author)
                        .desired_width(ui.available_width()),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Subject");
                ui.add(
                    egui::TextEdit::singleline(&mut note.subject)
                        .desired_width(ui.available_width()),
                );
            });
            ui.weak(format!("Created {}", display_date(&note.created)));
            if note.target.is_some() {
                ui.weak(format!("Modified {}", display_date(&note.modified)));
            }
            if !note.highlights.is_empty() {
                ui.weak("Anchored to the selected text highlight.");
            }
            ui.label(format!(
                "Status: {}",
                note.state.as_deref().unwrap_or("Pending")
            ));
            ui.label("Note");
            let shortcut = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            let edit = ui.add(
                egui::TextEdit::multiline(&mut note.text)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .hint_text("Write a review note…"),
            );
            if note.focus {
                edit.request_focus();
                note.focus = false;
            }
            if !current {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "The document changed. Copy your note, then reopen the comment before saving.",
                );
            }
            let valid = current && !note.text.trim().is_empty() && !note.author.trim().is_empty();
            ui.horizontal(|ui| {
                save = ui
                    .add_enabled(
                        valid,
                        egui::Button::new(if note.target.is_some() {
                            "Save comment"
                        } else {
                            "Add comment"
                        }),
                    )
                    .clicked()
                    || (shortcut && valid);
                cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
            ui.add_enabled_ui(current && note.target.is_some(), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Accept").clicked() {
                        decision = Some("Accepted");
                    }
                    if ui.button("Reject").clicked() {
                        decision = Some("Rejected");
                    }
                });
            });
        });
        if let Some(state) = decision {
            note.state = Some(state.into());
            save = true;
        }
        if cancel {
            self.annotate.note = None;
        } else if save && let Some(note) = self.annotate.note.take() {
            let date = atelier_form::annotation_date();
            let review = Review {
                text: note.text.trim_end().to_owned(),
                author: note.author.trim().to_owned(),
                subject: note.subject.trim().to_owned(),
                created: if note.created.is_empty() {
                    date.clone()
                } else {
                    note.created
                },
                modified: date,
                state: note.state,
            };
            let Some(o) = &mut self.open else { return };
            let focus_page = note.highlights.first().map_or(note.page, |(page, _)| *page);
            let focus_target = note
                .target
                .unwrap_or(ReviewTarget::Added(o.session.added_annots().len()));
            let mut pages = HashSet::from([note.page]);
            match note.target {
                Some(ReviewTarget::Saved(ordinal)) => {
                    o.session.set_review(note.page, ordinal, review)
                }
                Some(ReviewTarget::Added(index)) => {
                    o.session.set_added_review(index, review);
                }
                None => {
                    self.settings.review_author = review.author.clone();
                    if note.highlights.is_empty() {
                        o.session.add_annot(
                            note.page,
                            NewAnnot {
                                markup: Markup::Note {
                                    at: note.at,
                                    review,
                                },
                                color: NOTE_COLOR,
                            },
                        );
                    } else {
                        for (page, quads) in note.highlights {
                            o.session.add_annot(
                                page,
                                NewAnnot {
                                    markup: Markup::Comment {
                                        quads,
                                        review: review.clone(),
                                    },
                                    color: self.annotate.highlight,
                                },
                            );
                            pages.insert(page);
                        }
                    }
                    if let Err(error) = self.settings.save() {
                        self.error = Some(format!("Could not save review identity: {error}"));
                    }
                }
            }
            self.selection = None;
            self.rebuild(&pages, false);
            self.annotate.note = self
                .comments_on_page(focus_page, None)
                .unwrap_or_default()
                .into_iter()
                .find(|note| note.target == Some(focus_target));
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

    fn review_viewer() -> Viewer {
        let doc = atelier_syntax::Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec()).unwrap();
        let mut session = atelier_form::Session::new(std::sync::Arc::new(doc));
        for x in [30.0, 120.0] {
            session.add_annot(
                0,
                NewAnnot {
                    markup: Markup::Note {
                        at: Point::new(x, 150.0),
                        review: Review {
                            text: format!("Comment {x}"),
                            author: "Ada".into(),
                            subject: String::new(),
                            created: "D:20261102120000Z".into(),
                            modified: "D:20261102120000Z".into(),
                            state: None,
                        },
                    },
                    color: NOTE_COLOR,
                },
            );
        }
        let saved = session.build().unwrap().doc;
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
        viewer.open_with_password(saved.data().to_vec(), "pane-test.pdf".into(), "", None);
        viewer
    }

    fn click_review_page(viewer: &mut Viewer, user: Point) {
        let point = viewer
            .open
            .as_ref()
            .unwrap()
            .layout
            .user_to_layout(0)
            .unwrap()
            .apply(user);
        let ctx = viewer.ctx.clone();
        let cursor = Pos2::new(100.0, 100.0);
        for events in [
            vec![egui::Event::PointerMoved(cursor)],
            vec![egui::Event::PointerButton {
                pos: cursor,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            vec![egui::Event::PointerButton {
                pos: cursor,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO,
                        Vec2::new(1000.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let (_, response) =
                        ui.allocate_exact_size(Vec2::new(500.0, 500.0), Sense::click_and_drag());
                    viewer.annot_pointer(ui, &response, Some((point.x, point.y)));
                },
            );
            output.textures_delta.clear();
        }
    }

    #[test]
    fn comment_clicks_open_swap_and_dismiss_a_transient_pane_without_losing_edits() {
        let mut viewer = review_viewer();
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        assert!(viewer.annotate.comments_open && viewer.annotate.transient_comments);
        let first = viewer.annotate.note.as_ref().unwrap().target;
        viewer.annotate.note.as_mut().unwrap().text = "Unsaved review edit".into();
        click_review_page(&mut viewer, Point::new(125.0, 145.0));
        assert!(viewer.annotate.comments_open && viewer.annotate.transient_comments);
        assert!(viewer.annotate.note.as_ref().unwrap().target != first);
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        assert_eq!(
            viewer.annotate.note.as_ref().unwrap().text,
            "Unsaved review edit"
        );
        click_review_page(&mut viewer, Point::new(50.0, 40.0));
        assert!(!viewer.annotate.comments_open);
        assert!(viewer.annotate.note.is_none());
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        assert_eq!(
            viewer.annotate.note.as_ref().unwrap().text,
            "Unsaved review edit"
        );
    }

    #[test]
    fn manually_opened_pane_stays_open_after_page_click_and_toggle_preserves_drafts() {
        let mut viewer = review_viewer();
        viewer.annotate.toggle_comments();
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        assert!(!viewer.annotate.transient_comments);
        viewer.annotate.note.as_mut().unwrap().text = "Draft".into();
        viewer.annotate.toggle_comments();
        assert!(!viewer.annotate.comments_open);
        viewer.annotate.toggle_comments();
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        assert_eq!(viewer.annotate.note.as_ref().unwrap().text, "Draft");
        click_review_page(&mut viewer, Point::new(50.0, 40.0));
        assert!(viewer.annotate.comments_open);
        assert!(viewer.annotate.note.is_none());
    }

    #[test]
    fn saving_from_the_pane_keeps_focus_and_updates_the_document() {
        let mut viewer = review_viewer();
        click_review_page(&mut viewer, Point::new(35.0, 145.0));
        let target = viewer.annotate.note.as_ref().unwrap().target;
        viewer.annotate.note.as_mut().unwrap().text = "Updated in pane".into();
        let ctx = viewer.ctx.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(1000.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ui| {
                ui.set_width(320.0);
                viewer.note_editor(ui);
            },
        );
        output.textures_delta.clear();
        let note = viewer.annotate.note.as_ref().unwrap();
        assert!(viewer.annotate.comments_open);
        assert!(note.target == target);
        assert_eq!(note.text, "Updated in pane");
        assert_eq!(note.generation, viewer.open.as_ref().unwrap().generation);
        assert!(
            viewer
                .comments_on_page(0, None)
                .unwrap()
                .iter()
                .any(|n| n.text == "Updated in pane")
        );
    }

    #[test]
    fn revision_decisions_refresh_the_view_and_do_not_create_extra_revisions() {
        let mut viewer = review_viewer();
        viewer
            .open
            .as_mut()
            .unwrap()
            .session
            .set_tracking(true, "Ada")
            .unwrap();
        viewer.rotate_pages(1);
        assert_eq!(
            viewer.open.as_ref().unwrap().doc.pages().unwrap()[0].rotate,
            90
        );
        assert_eq!(viewer.open.as_ref().unwrap().session.revisions().len(), 1);
        let before = viewer.generation;
        viewer.apply_revision_decision(1, atelier_form::RevisionState::Rejected);
        assert!(viewer.generation > before);
        assert_eq!(
            viewer.open.as_ref().unwrap().doc.pages().unwrap()[0].rotate,
            0
        );
        assert_eq!(viewer.open.as_ref().unwrap().session.revisions().len(), 1);
        viewer.apply_revision_decision(1, atelier_form::RevisionState::Accepted);
        assert_eq!(
            viewer.open.as_ref().unwrap().doc.pages().unwrap()[0].rotate,
            90
        );
        assert!(viewer.undo_revision());
        assert_eq!(
            viewer.open.as_ref().unwrap().doc.pages().unwrap()[0].rotate,
            0
        );
        viewer.redo_revision();
        assert_eq!(
            viewer.open.as_ref().unwrap().doc.pages().unwrap()[0].rotate,
            90
        );
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
    }

    #[test]
    fn docked_pane_leaves_only_the_layout_margin_beside_the_document() {
        let mut viewer = review_viewer();
        viewer.annotate.toggle_comments();
        let ctx = viewer.ctx.clone();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO,
                        Vec2::new(1100.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    viewer.comments_panel(ui);
                    egui::CentralPanel::no_frame().show(ui, |ui| {
                        viewer.view_size = ui.available_size();
                        let page = viewer.open.as_ref().unwrap().layout.page_rect(0).unwrap();
                        let gap =
                            f64::from(viewer.view_size.x) - viewer.centre_shift() - page.x - page.w;
                        assert!((gap - atelier_view::Layout::GAP).abs() <= 1.0);
                        assert_eq!(viewer.annotate.comments_cache.len(), 2);
                        assert!(viewer.view_size.x > 600.0 && viewer.view_size.x < 800.0);
                    });
                },
            );
            output.textures_delta.clear();
        }
    }

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
    #[test]
    fn saved_inline_comments_hit_the_highlight_not_the_gap() {
        let doc=atelier_syntax::Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF".to_vec()).unwrap();
        let mut session = atelier_form::Session::new(std::sync::Arc::new(doc));
        let p = Point::new;
        let quads = vec![
            [
                p(20.0, 130.0),
                p(80.0, 130.0),
                p(80.0, 145.0),
                p(20.0, 145.0),
            ],
            [
                p(20.0, 100.0),
                p(60.0, 100.0),
                p(60.0, 115.0),
                p(20.0, 115.0),
            ],
        ];
        session.add_annot(
            0,
            NewAnnot {
                markup: Markup::Comment {
                    quads,
                    review: Review {
                        text: "Check totals".into(),
                        author: "Ada".into(),
                        subject: "Totals".into(),
                        created: "D:20261101120000Z".into(),
                        modified: "D:20261101120000Z".into(),
                        state: None,
                    },
                },
                color: [1.0, 0.9, 0.0],
            },
        );
        let saved = session.build().unwrap().doc;
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
        viewer.open_with_password(saved.data().to_vec(), "review-test.pdf".into(), "", None);
        let transform = viewer
            .open
            .as_ref()
            .unwrap()
            .layout
            .user_to_layout(0)
            .unwrap();
        for point in [p(30.0, 135.0), p(30.0, 105.0)] {
            let point = transform.apply(point);
            let note = viewer
                .comment_at(point.x, point.y)
                .expect("saved highlight must open a review editor");
            assert_eq!(note.text, "Check totals");
            assert_eq!(note.author, "Ada");
            assert_eq!(note.subject, "Totals");
            assert!(matches!(note.target, Some(ReviewTarget::Saved(0))));
        }
        let gap = transform.apply(p(30.0, 122.0));
        assert!(
            viewer.comment_at(gap.x, gap.y).is_none(),
            "whitespace between highlight lines must remain selectable"
        );
    }
}
