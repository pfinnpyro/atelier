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
pub(super) struct NoteEditor {
    page: usize,
    at: Point,
    text: String,
    focus: bool,
    created: String,
    modified: String,
    author: String,
    subject: String,
    target: Option<ReviewTarget>,
    highlights: Vec<(usize, Vec<[Point; 4]>)>,
    generation: u64,
}

#[derive(Clone, Copy)]
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
    erasing: bool,
    signature_name: String,
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
            erasing: false,
            signature_name: String::new(),
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
    pub(super) fn has_pending_ink(&self) -> bool {
        !self.strokes.is_empty() || self.drawing.is_some()
    }
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

/// Distance to an ink segment, including single-point dots.
fn stroke_hit(line: &[Point], p: Point, tolerance: f64) -> bool {
    if line.iter().any(|q| (p - *q).length() <= tolerance) {
        return true;
    }
    line.windows(2).any(|pair| {
        let a = pair[0];
        let b = pair[1];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let length = dx * dx + dy * dy;
        if length == 0.0 {
            return false;
        }
        let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0);
        (p - Point::new(a.x + t * dx, a.y + t * dy)).length() <= tolerance
    })
}

impl Viewer {
    pub(super) fn ink_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Your saved ink signature is stored on this computer as drawing data.");
        ui.label("Ink alone does not authenticate identity or protect the document.");
        ui.checkbox(
            &mut self.settings.certify_ink,
            "Offer certificate signing after placing ink",
        );
        if let Some(sig) = &self.annotate.signature {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(280.0, 90.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
            let height = sig
                .iter()
                .flatten()
                .map(|p| p.y)
                .fold(0.0_f64, f64::max)
                .max(0.01);
            let scale = 250.0_f64.min(65.0 / height) as f32;
            for line in sig {
                let points = line
                    .iter()
                    .map(|p| {
                        rect.left_center()
                            + Vec2::new(
                                12.0 + p.x as f32 * scale,
                                (p.y - height / 2.0) as f32 * scale,
                            )
                    })
                    .collect();
                ui.painter().with_clip_rect(rect).add(Shape::line(
                    points,
                    Stroke::new(1.8, ui.visuals().text_color()),
                ));
            }
        }
        if ui
            .add_enabled(
                self.open.is_some(),
                egui::Button::new("Back document ink with a certificate…"),
            )
            .clicked()
        {
            self.open_security(true);
        }
        let exists = self.annotate.signature.is_some();
        ui.horizontal(|ui| {
            if ui
                .button(if exists {
                    "Replace signature…"
                } else {
                    "Create signature…"
                })
                .clicked()
            {
                self.annotate.pad = Some(Pad::default());
            }
            if ui
                .add_enabled(exists, egui::Button::new("Delete saved signature"))
                .clicked()
            {
                let result = signature_path().map(std::fs::remove_file);
                match result {
                    Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
                        self.error = Some(format!("Could not delete the signature: {e}"));
                    }
                    _ => {
                        self.annotate.signature = None;
                        self.annotate.tool = Tool::Select;
                    }
                }
            }
        });
        ui.add_space(8.0);
        ui.label("Signature library");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.annotate.signature_name)
                    .hint_text("Signature name")
                    .desired_width(180.0),
            );
            if ui
                .add_enabled(exists, egui::Button::new("Save named copy"))
                .clicked()
            {
                let name = self.annotate.signature_name.trim();
                if name.is_empty()
                    || name.len() > 64
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                {
                    self.error = Some(
                        "Use a name of 1–64 letters, numbers, spaces, hyphens or underscores."
                            .into(),
                    );
                } else if let (Some(root), Some(sig)) = (
                    signature_path().and_then(|p| p.parent().map(|d| d.join("signatures"))),
                    &self.annotate.signature,
                ) {
                    let result = std::fs::create_dir_all(&root).and_then(|_| {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(root.join(format!("{name}.txt")))?;
                        file.write_all(signature_text(sig).as_bytes())
                    });
                    match result {
                        Ok(()) => self.annotate.signature_name.clear(),
                        Err(e) => self.error = Some(format!("Could not save named signature: {e}")),
                    }
                }
            }
        });
        if let Some(root) = signature_path().and_then(|p| p.parent().map(|d| d.join("signatures")))
        {
            let mut files: Vec<_> = std::fs::read_dir(&root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "txt"))
                .collect();
            files.sort();
            for path in files {
                ui.horizontal(|ui| {
                    ui.label(path.file_stem().unwrap_or_default().to_string_lossy());
                    if ui.button("Use").clicked() {
                        match std::fs::read_to_string(&path) {
                            Ok(text) => {
                                let sig = parse_signature(&text);
                                if !sig.is_empty()
                                    && let Some(active) = signature_path()
                                {
                                    match std::fs::write(active, signature_text(&sig)) {
                                        Ok(()) => self.annotate.signature = Some(sig),
                                        Err(e) => {
                                            self.error =
                                                Some(format!("Could not select signature: {e}"))
                                        }
                                    }
                                }
                            }
                            Err(e) => self.error = Some(format!("Could not read signature: {e}")),
                        }
                    }
                    if ui.button("Delete").clicked()
                        && let Err(e) = std::fs::remove_file(&path)
                    {
                        self.error = Some(format!("Could not delete signature: {e}"));
                    }
                });
            }
        }
    }

    /// Reopens an ink mark as editable strokes, preserving its appearance.
    pub(super) fn edit_ink(&mut self, index: usize, redraw: bool) {
        self.commit_ink();
        let mark = self.open.as_ref().and_then(|o| {
            o.session
                .added_annots()
                .get(index)
                .map(|(page, a)| (*page, (*a).clone()))
        });
        let Some((
            page,
            NewAnnot {
                markup: Markup::Ink { strokes, width },
                color,
            },
        )) = mark
        else {
            return;
        };
        self.remove_mark(index);
        self.annotate.strokes = if redraw { Vec::new() } else { strokes };
        self.annotate.strokes_page = Some(page);
        self.annotate.ink = color;
        self.annotate.width = width;
        self.annotate.tool = Tool::Draw;
        self.selection = None;
    }

    /// Picks a tool; the one in use is put down when picked again.
    pub(super) fn set_tool(&mut self, tool: Tool) {
        self.commit_ink();
        let a = &mut self.annotate;
        a.tool = if a.tool == tool { Tool::Select } else { tool };
        a.erasing = false;
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
                let geometry = match &a.markup {
                    Markup::Ink { strokes, width } => strokes
                        .iter()
                        .any(|line| stroke_hit(line, p, width / 2.0 + 4.0 / o.layout.zoom())),
                    _ => true,
                };
                *on == page && geometry && p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
            })
            .map(|(i, _)| i)
    }

    /// Hit tests saved review annotations, excluding form widgets and locked marks.
    pub(super) fn saved_mark_at(&self, lx: f64, ly: f64) -> Option<(usize, usize)> {
        let o = self.open.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let p = o.layout.to_user(page, lx, ly)?;
        let pages = o.doc.pages().ok()?;
        let Object::Array(annots) = o.doc.get_key(&pages.get(page)?.dict, b"Annots").ok()? else {
            return None;
        };
        let added = o
            .session
            .added_annots()
            .iter()
            .filter(|(on, _)| *on == page)
            .count();
        annots
            .iter()
            .take(annots.len().saturating_sub(added))
            .enumerate()
            .rev()
            .find_map(|(i, a)| {
                let d = o.doc.resolve_dict(a).ok()?;
                let kind = d.get(b"Subtype")?.as_name()?;
                if !matches!(
                    kind.0.as_slice(),
                    b"Ink"
                        | b"Text"
                        | b"Highlight"
                        | b"Underline"
                        | b"StrikeOut"
                        | b"Squiggly"
                        | b"FreeText"
                ) {
                    return None;
                }
                if d.get(b"F").and_then(Object::as_i64).unwrap_or(0) & (64 | 128) != 0 {
                    return None;
                }
                if *kind == b"Ink" {
                    let ink = self.saved_ink(page, i)?;
                    if let Markup::Ink { strokes, width } = ink.markup
                        && !strokes
                            .iter()
                            .any(|line| stroke_hit(line, p, width / 2.0 + 4.0 / o.layout.zoom()))
                    {
                        return None;
                    }
                }
                let r = o.doc.get_key(&d, b"Rect").ok()?;
                let r = atelier_syntax::Rect::from_array(r.as_array()?)?;
                (p.x >= r.x0.min(r.x1)
                    && p.x <= r.x0.max(r.x1)
                    && p.y >= r.y0.min(r.y1)
                    && p.y <= r.y0.max(r.y1))
                .then_some((page, i))
            })
    }

    pub(super) fn saved_ink(&self, page: usize, visible: usize) -> Option<NewAnnot> {
        let o = self.open.as_ref()?;
        let pages = o.doc.pages().ok()?;
        let Object::Array(list) = o.doc.get_key(&pages.get(page)?.dict, b"Annots").ok()? else {
            return None;
        };
        let d = o.doc.resolve_dict(list.get(visible)?).ok()?;
        let Object::Array(lines) = o.doc.get_key(&d, b"InkList").ok()? else {
            return None;
        };
        let strokes: Option<Vec<Vec<Point>>> = lines
            .iter()
            .map(|line| {
                let line = o.doc.resolve(line).ok()?;
                let values = line.as_array()?;
                if values.len() % 2 != 0 {
                    return None;
                }
                values
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| {
                        let p = Point::new(p[0].as_f64()?, p[1].as_f64()?);
                        (p.x.is_finite() && p.y.is_finite()).then_some(p)
                    })
                    .collect()
            })
            .collect();
        let color = o
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
            .unwrap_or(INKS[0]);
        let width = o
            .doc
            .get_key(&d, b"BS")
            .ok()
            .and_then(|v| o.doc.resolve_dict(&v).ok())
            .and_then(|d| d.get(b"W").and_then(Object::as_f64))
            .filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(1.5);
        Some(NewAnnot {
            markup: Markup::Ink {
                strokes: strokes?,
                width,
            },
            color,
        })
    }

    pub(super) fn edit_added_comment(&mut self, index: usize) {
        let Some(o) = &self.open else { return };
        let list = o.session.added_annots();
        let Some((page, a)) = list.get(index) else {
            return;
        };
        let Some(review) = a.review() else { return };
        let rect = a.rect();
        self.annotate.note = Some(NoteEditor {
            page: *page,
            at: Point::new(rect[0], rect[3]),
            text: review.text.clone(),
            focus: true,
            created: review.created.clone(),
            modified: review.modified.clone(),
            author: review.author.clone(),
            subject: review.subject.clone(),
            target: Some(ReviewTarget::Added(index)),
            highlights: Vec::new(),
            generation: o.generation,
        });
    }

    pub(super) fn edit_saved_comment(&mut self, page: usize, visible: usize) {
        let Some(o) = &self.open else { return };
        let Some(ordinal) = o.session.original_annot_ordinal(page, visible) else {
            return;
        };
        let Ok(pages) = o.doc.pages() else { return };
        let Ok(Object::Array(list)) = o.doc.get_key(&pages[page].dict, b"Annots") else {
            return;
        };
        let Some(target) = list.get(visible) else {
            return;
        };
        let Ok(d) = o.doc.resolve_dict(target) else {
            return;
        };
        let text = |key: &[u8]| {
            o.doc
                .get_key(&d, key)
                .ok()
                .and_then(|v| v.as_string().map(|s| s.to_text()))
                .unwrap_or_default()
        };
        self.annotate.note = Some(NoteEditor {
            page,
            at: Point::new(0.0, 0.0),
            text: text(b"Contents"),
            focus: true,
            created: text(b"CreationDate"),
            modified: text(b"M"),
            author: text(b"T"),
            subject: text(b"Subj"),
            target: Some(ReviewTarget::Saved(ordinal)),
            highlights: Vec::new(),
            generation: o.generation,
        });
    }

    pub(super) fn remove_saved_mark(&mut self, page: usize, visible: usize) {
        let Some(o) = &mut self.open else { return };
        if let Some(ordinal) = o.session.original_annot_ordinal(page, visible) {
            o.session.remove_saved_annot(page, ordinal);
            self.rebuild(&HashSet::from([page]), false);
        }
    }

    pub(super) fn edit_saved_ink(&mut self, page: usize, visible: usize, redraw: bool) {
        let Some(NewAnnot {
            markup: Markup::Ink { strokes, width },
            color,
        }) = self.saved_ink(page, visible)
        else {
            return;
        };
        self.commit_ink();
        self.remove_saved_mark(page, visible);
        self.annotate.strokes = if redraw { Vec::new() } else { strokes };
        self.annotate.strokes_page = Some(page);
        self.annotate.ink = color;
        self.annotate.width = width;
        self.annotate.tool = Tool::Draw;
        self.selection = None;
    }

    /// The text of a comment (or any annotation with text) under a layout
    /// point, from the document as it stands.
    fn comment_at(&self, lx: f64, ly: f64) -> Option<NoteEditor> {
        let o = self.open.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let p = o.layout.to_user(page, lx, ly)?;
        let pages = o.doc.pages().ok()?;
        let Object::Array(annots) = o.doc.get_key(&pages.get(page)?.dict, b"Annots").ok()? else {
            return None;
        };
        let added = o.session.added_annots();
        let page_added: Vec<_> = added
            .iter()
            .enumerate()
            .filter(|(_, (on, _))| *on == page)
            .map(|(i, _)| i)
            .collect();
        let original_count = annots.len().saturating_sub(page_added.len());
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
            if let Ok(Object::Array(points)) = o.doc.get_key(&d, b"QuadPoints") {
                hit = points.as_chunks::<8>().0.iter().any(|q| {
                    let numbers: Option<Vec<_>> = q.iter().map(Object::as_f64).collect();
                    numbers.is_some_and(|v| {
                        super::inside(
                            &[
                                Point::new(v[4], v[5]),
                                Point::new(v[6], v[7]),
                                Point::new(v[2], v[3]),
                                Point::new(v[0], v[1]),
                            ],
                            p,
                        )
                    })
                });
            }
            if hit {
                return Some(NoteEditor {
                    page,
                    at: p,
                    text,
                    focus: false,
                    created: value(b"CreationDate"),
                    modified: value(b"M"),
                    author: value(b"T"),
                    subject: value(b"Subj"),
                    target: Some(if ordinal >= original_count {
                        ReviewTarget::Added(page_added[ordinal - original_count])
                    } else {
                        ReviewTarget::Saved(o.session.original_annot_ordinal(page, ordinal)?)
                    }),
                    highlights: Vec::new(),
                    generation: o.generation,
                });
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
                && let Some(note) = self.comment_at(lx, ly)
            {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if response.clicked() {
                    self.annotate.note = Some(note);
                    self.selection = None;
                    return true;
                }
                response.clone().on_hover_text_at_pointer(format!(
                    "{} · {}\n{}\nClick to open comment",
                    note.author,
                    display_date(&note.created),
                    note.text.chars().take(500).collect::<String>()
                ));
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
                if self.annotate.erasing {
                    if response.hovered()
                        && ui.input(|i| i.pointer.primary_down())
                        && let Some((page, p)) = user
                    {
                        if self.annotate.strokes.is_empty()
                            && let Some((lx, ly)) = at
                        {
                            if let Some(index) = self.mark_at(lx, ly) {
                                self.edit_ink(index, false);
                            } else if let Some((on, ordinal)) = self.saved_mark_at(lx, ly) {
                                self.edit_saved_ink(on, ordinal, false);
                            }
                        }
                        if self.annotate.strokes_page == Some(page) {
                            let tolerance = 6.0 / zoom + self.annotate.width / 2.0;
                            self.annotate
                                .strokes
                                .retain(|line| !stroke_hit(line, p, tolerance));
                        }
                    }
                    return true;
                }
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
        self.notice = Some("Ink signature added. Certificate backing applies only after exporting a signed copy. Ctrl+Z to undo.".into());
        if self.settings.certify_ink {
            self.open_security(true);
        }
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
                        ui.selectable_value(&mut a.erasing, false, "Draw");
                        ui.selectable_value(&mut a.erasing, true, "Erase strokes");
                        if ui
                            .add_enabled(
                                !a.strokes.is_empty(),
                                egui::Button::new("Erase last stroke"),
                            )
                            .clicked()
                        {
                            a.strokes.pop();
                        }
                        if ui.button("Clear drawing").clicked() {
                            a.strokes.clear();
                            a.drawing = None;
                        }
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
        if ui.button("Back ink with a certificate…").clicked() {
            self.open_security(true);
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
        self.annotate.note = Some(NoteEditor {
            page,
            at,
            text: String::new(),
            focus: true,
            created: date.clone(),
            modified: date,
            author: self.settings.review_author.clone(),
            subject: String::new(),
            target: None,
            highlights,
            generation: self.open.as_ref().map_or(0, |o| o.generation),
        });
    }

    pub(super) fn note_window(&mut self, ctx: &egui::Context) {
        let Some(note) = &mut self.annotate.note else {
            return;
        };
        let current = self
            .open
            .as_ref()
            .is_some_and(|o| o.generation == note.generation);
        let mut save = false;
        let mut cancel = false;
        let mut open = true;
        egui::Window::new(if note.target.is_some() {
            "Review comment"
        } else {
            "New comment"
        })
        .id(egui::Id::unique("review-comment-editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(340.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Author");
                ui.text_edit_singleline(&mut note.author);
            });
            ui.horizontal(|ui| {
                ui.label("Subject");
                ui.text_edit_singleline(&mut note.subject);
            });
            ui.weak(format!("Created {}", display_date(&note.created)));
            if note.target.is_some() {
                ui.weak(format!("Modified {}", display_date(&note.modified)));
            }
            if !note.highlights.is_empty() {
                ui.weak("Anchored to the selected text highlight.");
            }
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
        });
        if cancel || !open {
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
            };
            let Some(o) = &mut self.open else { return };
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
    fn eraser_hits_segments_without_erasing_distant_strokes() {
        let line = [Point::new(0.0, 0.0), Point::new(100.0, 0.0)];
        assert!(stroke_hit(&line, Point::new(50.0, 2.0), 3.0));
        assert!(!stroke_hit(&line, Point::new(50.0, 8.0), 3.0));
        assert!(!stroke_hit(&line, Point::new(120.0, 0.0), 3.0));
        assert!(stroke_hit(&line[..1], Point::new(1.0, 0.0), 3.0));
        assert!(!stroke_hit(&[], Point::new(0.0, 0.0), 3.0));
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
