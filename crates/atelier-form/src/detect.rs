//! Finding tables and the blanks of a form on a page: ruled grids, boxes,
//! lines to write on and check boxes, each with the label printed beside
//! it. What is found is only suggested; a person decides which become
//! fields.
//!
//! Everything is worked out the way the page is read (after /Rotate), then
//! turned back into the page's own coordinates.

use std::collections::{HashMap, HashSet};

use atelier_raster::{Pixmap, Point};
use atelier_render::{
    PageGraphics, PageLine, PageText, RenderOptions, extract_graphics, page_transform, render_page,
};
use atelier_syntax::{Document, Page, Rect};

use crate::new_field::{NewField, NewFieldKind};

/// Lines nearer than this are one line; ends this near touch.
const JOIN: f64 = 2.0;
/// Rulings and cells are capped so a page of hatching cannot stall.
const MAX_RULINGS: usize = 1500;
const MAX_CELLS: usize = 4000;
/// Cells smaller than this are the gap of a double border.
const MIN_CELL: f64 = 4.0;
/// A blank to write in is at least this wide and high.
const MIN_TEXT_W: f64 = 18.0;
const MIN_TEXT_H: f64 = 8.0;
/// How long a blank for initials may be: about a third of an inch to an
/// inch and a quarter.
const MIN_INITIALS_W: f64 = 10.0;
const MAX_INITIALS_W: f64 = 90.0;
/// Boxes taller than this take more than one line.
const MULTILINE_H: f64 = 34.0;
/// Characters printed as an empty check box.
const BOX_GLYPHS: &str = "☐□▢◻❏❐❑❒";

/// Why a field is suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An empty cell of a table.
    TableCell,
    /// An empty box, or the space beside or under a label in a box.
    Box,
    /// A line to write on.
    Underline,
    /// A small square.
    CheckBox,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::TableCell => "table cell",
            Self::Box => "box",
            Self::Underline => "line",
            Self::CheckBox => "check box",
        }
    }
}

/// A field the page seems to want.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub field: NewField,
    pub source: Source,
    /// The table it is a cell of.
    pub table: Option<usize>,
}

/// One cell of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub rect: Rect,
    /// Row and column, from the top left as read, and how many it spans.
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
}

/// A ruled grid of at least two rows and two columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub rect: Rect,
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<TableCell>,
}

impl Table {
    /// The table's text, a line per row with tabs between the columns, as
    /// spreadsheets paste it.
    pub fn to_tsv(&self) -> String {
        let mut grid = vec![vec![String::new(); self.cols]; self.rows];
        for c in &self.cells {
            if let Some(slot) = grid.get_mut(c.row).and_then(|r| r.get_mut(c.col)) {
                *slot = c.text.replace(['\t', '\n'], " ");
            }
        }
        grid.iter()
            .map(|r| r.join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What was found on one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detected {
    pub tables: Vec<Table>,
    /// Suggested fields, in reading order.
    pub fields: Vec<Suggestion>,
}

/// What detection looks at.
pub struct PageInput<'a> {
    pub text: &'a PageText,
    pub lines: &'a [PageLine],
    /// Where images sit; nothing is suggested over a picture.
    pub images: &'a [[Point; 4]],
    /// The visible part of the page.
    pub bounds: Rect,
    /// The page's /Rotate.
    pub rotate: u16,
    /// Boxes that already have a field.
    pub taken: &'a [Rect],
}

// ----- Geometry ----------------------------------------------------------

/// A point as read: the page turned by its /Rotate.
fn upright(p: Point, rotate: u16) -> Point {
    match rotate {
        90 => Point::new(p.y, -p.x),
        180 => Point::new(-p.x, -p.y),
        270 => Point::new(-p.y, p.x),
        _ => p,
    }
}

fn from_upright(p: Point, rotate: u16) -> Point {
    upright(p, (360 - rotate % 360) % 360)
}

fn rect_of(points: impl IntoIterator<Item = Point>) -> Rect {
    let mut r = Rect {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };
    for p in points {
        r.x0 = r.x0.min(p.x);
        r.y0 = r.y0.min(p.y);
        r.x1 = r.x1.max(p.x);
        r.y1 = r.y1.max(p.y);
    }
    r
}

fn turn_rect(r: &Rect, rotate: u16, back: bool) -> Rect {
    let f = if back { from_upright } else { upright };
    rect_of([
        f(Point::new(r.x0, r.y0), rotate),
        f(Point::new(r.x1, r.y1), rotate),
    ])
}

fn area(r: &Rect) -> f64 {
    (r.x1 - r.x0).max(0.0) * (r.y1 - r.y0).max(0.0)
}

fn overlap(a: &Rect, b: &Rect) -> f64 {
    a.intersect(b).map_or(0.0, |r| area(&r))
}

fn union(a: &Rect, b: &Rect) -> Rect {
    Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

fn inset(r: &Rect, by: f64) -> Rect {
    Rect {
        x0: r.x0 + by,
        y0: r.y0 + by,
        x1: r.x1 - by,
        y1: r.y1 - by,
    }
}

fn contains(r: &Rect, x: f64, y: f64) -> bool {
    x >= r.x0 && x <= r.x1 && y >= r.y0 && y <= r.y1
}

// ----- Text --------------------------------------------------------------

/// A run of text on one line with no wide gap in it: a label, or the
/// content of a cell.
#[derive(Debug, Clone)]
struct Phrase {
    rect: Rect,
    text: String,
    size: f64,
}

struct Glyph {
    rect: Rect,
    text: String,
    size: f64,
}

struct Words {
    phrases: Vec<Phrase>,
    /// Runs of underscores: lines to write on, typed as text.
    blanks: Vec<(f64, f64, f64)>,
    /// Characters that are an empty check box.
    boxes: Vec<Rect>,
}

fn words(text: &PageText, rotate: u16) -> Words {
    let mut glyphs: Vec<Glyph> = text
        .glyphs
        .iter()
        .filter(|g| !g.text.trim().is_empty())
        .map(|g| Glyph {
            rect: rect_of(g.quad.iter().map(|p| upright(*p, rotate))),
            text: g.text.clone(),
            size: g.size.max(1.0),
        })
        .filter(|g| g.rect.x0.is_finite() && g.rect.y0.is_finite())
        .collect();
    // Lines of text: glyphs whose middles are at about the same height.
    glyphs.sort_by(|a, b| {
        let (ya, yb) = (a.rect.y0 + a.rect.y1, b.rect.y0 + b.rect.y1);
        yb.total_cmp(&ya)
    });
    let mut rows: Vec<Vec<Glyph>> = Vec::new();
    let mut row_y = f64::INFINITY;
    for g in glyphs {
        let mid = (g.rect.y0 + g.rect.y1) / 2.0;
        let h = g.rect.y1 - g.rect.y0;
        if rows.is_empty() || row_y - mid > h * 0.5 {
            rows.push(Vec::new());
            row_y = mid;
        }
        rows.last_mut().expect("a row was just pushed").push(g);
    }
    let mut out = Words {
        phrases: Vec::new(),
        blanks: Vec::new(),
        boxes: Vec::new(),
    };
    for mut row in rows {
        row.sort_by(|a, b| a.rect.x0.total_cmp(&b.rect.x0));
        let mut phrase: Option<Phrase> = None;
        let mut blank: Option<(Rect, usize)> = None;
        let end_blank = |blank: &mut Option<(Rect, usize)>, out: &mut Words| {
            if let Some((r, n)) = blank.take()
                && (n >= 3 || r.x1 - r.x0 >= 15.0)
            {
                // An underscore sits a little under the baseline, which
                // is a fifth of the way up the glyph's box.
                out.blanks.push((r.y0 + (r.y1 - r.y0) * 0.1, r.x0, r.x1));
            }
        };
        for g in row {
            let is_blank = g.text.chars().all(|c| c == '_');
            let is_box = g.text.chars().all(|c| BOX_GLYPHS.contains(c));
            if is_blank {
                out.phrases.extend(phrase.take());
                blank = match blank.take() {
                    Some((r, n)) if g.rect.x0 - r.x1 < g.size * 0.5 => {
                        Some((union(&r, &g.rect), n + 1))
                    }
                    other => {
                        let mut other = other;
                        end_blank(&mut other, &mut out);
                        Some((g.rect, 1))
                    }
                };
                continue;
            }
            end_blank(&mut blank, &mut out);
            if is_box {
                out.phrases.extend(phrase.take());
                // The square stands on the baseline and is about as wide
                // as the glyph.
                let h = g.rect.y1 - g.rect.y0;
                let side = (g.rect.x1 - g.rect.x0).clamp(h * 0.5, h * 0.9);
                let cx = (g.rect.x0 + g.rect.x1) / 2.0;
                let y0 = g.rect.y0 + h * 0.15;
                out.boxes.push(Rect {
                    x0: cx - side / 2.0,
                    y0,
                    x1: cx + side / 2.0,
                    y1: y0 + side,
                });
                continue;
            }
            match &mut phrase {
                Some(p) if g.rect.x0 - p.rect.x1 <= p.size.max(g.size) => {
                    if g.rect.x0 - p.rect.x1 > p.size.max(g.size) * 0.15 {
                        p.text.push(' ');
                    }
                    p.text.push_str(&g.text);
                    p.rect = union(&p.rect, &g.rect);
                    p.size = p.size.max(g.size);
                }
                _ => {
                    out.phrases.extend(phrase.take());
                    phrase = Some(Phrase {
                        rect: g.rect,
                        text: g.text,
                        size: g.size,
                    });
                }
            }
        }
        end_blank(&mut blank, &mut out);
        out.phrases.extend(phrase);
    }
    out
}

// ----- Rulings -----------------------------------------------------------

/// A horizontal or vertical line: where it is across, and its two ends
/// along.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ruling {
    at: f64,
    from: f64,
    to: f64,
}

impl Ruling {
    fn len(&self) -> f64 {
        self.to - self.from
    }
}

/// Joins pieces of the same line: dashes, double strokes, and the rows of
/// pixels of a line read from a scan.
fn merge(mut lines: Vec<Ruling>) -> Vec<Ruling> {
    lines.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.from.total_cmp(&b.from)));
    let mut out: Vec<Ruling> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j].at - lines[i].at <= 1.2 {
            j += 1;
        }
        let at = lines[i..j].iter().map(|l| l.at).sum::<f64>() / (j - i) as f64;
        let mut group: Vec<Ruling> = lines[i..j].to_vec();
        group.sort_by(|a, b| a.from.total_cmp(&b.from));
        let mut current = Ruling { at, ..group[0] };
        for l in &group[1..] {
            if l.from <= current.to + 2.5 {
                current.to = current.to.max(l.to);
            } else {
                out.push(current);
                current = Ruling { at, ..*l };
            }
        }
        out.push(current);
        i = j;
    }
    out.retain(|l| l.len() >= 3.0);
    if out.len() > MAX_RULINGS {
        out.sort_by(|a, b| b.len().total_cmp(&a.len()));
        out.truncate(MAX_RULINGS);
    }
    out
}

/// The page's lines as read: horizontal ones (at a height, from left to
/// right) and vertical ones (at an x, from bottom to top).
fn rulings(lines: &[PageLine], rotate: u16) -> (Vec<Ruling>, Vec<Ruling>) {
    let (mut h, mut v) = (Vec::new(), Vec::new());
    for l in lines {
        let (a, b) = (upright(l.from, rotate), upright(l.to, rotate));
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        if dy <= 0.6 && dx > dy * 20.0 {
            h.push(Ruling {
                at: (a.y + b.y) / 2.0,
                from: a.x.min(b.x),
                to: a.x.max(b.x),
            });
        } else if dx <= 0.6 && dy > dx * 20.0 {
            v.push(Ruling {
                at: (a.x + b.x) / 2.0,
                from: a.y.min(b.y),
                to: a.y.max(b.y),
            });
        }
    }
    (merge(h), merge(v))
}

// ----- Cells and tables --------------------------------------------------

/// The smallest boxes the rulings enclose. Also says which horizontal
/// rulings meet a vertical one.
fn cells(h: &[Ruling], v: &[Ruling]) -> (Vec<Rect>, HashSet<usize>) {
    let mut points: HashSet<(usize, usize)> = HashSet::new();
    let mut on_h: Vec<Vec<usize>> = vec![Vec::new(); h.len()];
    let mut on_v: Vec<Vec<usize>> = vec![Vec::new(); v.len()];
    for (i, a) in h.iter().enumerate() {
        for (j, b) in v.iter().enumerate() {
            let crosses = b.at >= a.from - JOIN
                && b.at <= a.to + JOIN
                && a.at >= b.from - JOIN
                && a.at <= b.to + JOIN;
            if crosses {
                points.insert((i, j));
                on_h[i].push(j);
                on_v[j].push(i);
            }
        }
    }
    // Along each line, the crossings in reading order.
    for list in &mut on_h {
        list.sort_by(|a, b| v[*a].at.total_cmp(&v[*b].at));
    }
    for list in &mut on_v {
        list.sort_by(|a, b| h[*b].at.total_cmp(&h[*a].at));
    }
    let touched: HashSet<usize> = points.iter().map(|p| p.0).collect();
    let mut out = Vec::new();
    'corner: for &(top, left) in &points {
        let below = on_v[left]
            .iter()
            .filter(|&&b| h[b].at < h[top].at - MIN_CELL);
        for &bottom in below {
            let right = on_h[top]
                .iter()
                .filter(|&&r| v[r].at > v[left].at + MIN_CELL);
            for &right in right {
                if points.contains(&(bottom, right)) {
                    out.push(Rect {
                        x0: v[left].at,
                        y0: h[bottom].at,
                        x1: v[right].at,
                        y1: h[top].at,
                    });
                    if out.len() >= MAX_CELLS {
                        break 'corner;
                    }
                    continue 'corner;
                }
            }
        }
    }
    // The order of a hash set is not the same from run to run.
    out.sort_by(|a, b| b.y1.total_cmp(&a.y1).then(a.x0.total_cmp(&b.x0)));
    out.dedup();
    (out, touched)
}

/// Sorted values with near-equal ones made one.
fn edges(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    let mut out: Vec<f64> = Vec::new();
    for v in values {
        if out.last().is_none_or(|l| v - l > JOIN) {
            out.push(v);
        }
    }
    out
}

fn edge_index(edges: &[f64], v: f64) -> usize {
    edges
        .iter()
        .position(|e| (e - v).abs() <= JOIN)
        .unwrap_or(0)
}

/// Groups of cells that share sides: each cell's group number.
fn groups(cells: &[Rect]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..cells.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..cells.len() {
        for j in i + 1..cells.len() {
            let (a, b) = (&cells[i], &cells[j]);
            let beside = ((a.x1 - b.x0).abs() <= JOIN || (b.x1 - a.x0).abs() <= JOIN)
                && a.y1.min(b.y1) - a.y0.max(b.y0) > 1.0;
            let stacked = ((a.y1 - b.y0).abs() <= JOIN || (b.y1 - a.y0).abs() <= JOIN)
                && a.x1.min(b.x1) - a.x0.max(b.x0) > 1.0;
            if beside || stacked {
                let (ra, rb) = (find(&mut parent, i), find(&mut parent, j));
                parent[ra] = rb;
            }
        }
    }
    (0..cells.len()).map(|i| find(&mut parent, i)).collect()
}

// ----- Labels ------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// The nearest text on one side of a box.
fn label<'a>(phrases: &'a [Phrase], r: &Rect, side: Side) -> Option<&'a Phrase> {
    let mut best: Option<(&Phrase, f64)> = None;
    for p in phrases {
        let mid_y = (p.rect.y0 + p.rect.y1) / 2.0;
        let level = mid_y >= r.y0 - 3.0 && mid_y <= r.y1 + 3.0;
        let across = p.rect.x1.min(r.x1) - p.rect.x0.max(r.x0);
        let gap = match side {
            Side::Left if level => r.x0 - p.rect.x1,
            Side::Right if level => p.rect.x0 - r.x1,
            Side::Above if across > 0.0 && p.rect.x0 >= r.x0 - 20.0 => p.rect.y0 - r.y1,
            Side::Below if across > 0.0 => r.y0 - p.rect.y1,
            _ => continue,
        };
        let reach = match side {
            Side::Left => 60.0,
            Side::Right => 30.0,
            Side::Above | Side::Below => 15.0,
        };
        if gap >= -3.0 && gap <= reach && best.is_none_or(|(_, g)| gap < g) {
            best = Some((p, gap));
        }
    }
    best.map(|b| b.0)
}

/// What a label asks to be written beside it, when that is not plain
/// text: a signature, or initials. English labels only.
fn asked_for(label: &str) -> Option<NewFieldKind> {
    let lower = label.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(w));
    if has(&["initial", "initials", "initialed", "initialled"]) {
        Some(NewFieldKind::Initials)
    } else if has(&["date", "dated", "print", "printed", "name", "title"]) {
        // "Date signed", "Print name of signatory": typed, not signed.
        None
    } else if has(&["signature", "signatures", "sign", "signed", "signatory"]) {
        Some(NewFieldKind::Signature)
    } else {
        None
    }
}

/// The fewest single-letter changes that turn one word into the other.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let change = last + usize::from(ca != cb);
            last = row[j + 1];
            row[j + 1] = change.min(last + 1).min(row[j] + 1);
        }
    }
    row[b.len()]
}

/// Whether a word is a form of a verb, given the verb's stem: "certify",
/// "certifies" and "certified" for "certif". Longer stems allow one wrong
/// letter, for misspellings and misread scans.
fn is_form_of(word: &str, stem: &str) -> bool {
    if word.starts_with(stem) {
        return true;
    }
    let (w, s): (Vec<char>, Vec<char>) = (word.chars().collect(), stem.chars().collect());
    s.len() >= 6
        && (s.len() - 1..=s.len() + 1)
            .filter(|n| *n <= w.len())
            .any(|n| distance(&w[..n], &s) <= 1)
}

/// Whether text is a statement someone puts their initials against: "I
/// certify that…", "I understand…", "We hereby acknowledge…". With
/// `opening`, the statement has to begin the text. English only.
fn attests(text: &str, opening: bool) -> bool {
    const STEMS: [&str; 20] = [
        "certif",
        "understand",
        "understood",
        "acknowledg",
        "agree",
        "attest",
        "affirm",
        "declar",
        "authori",
        "consent",
        "confirm",
        "accept",
        "swear",
        "verif",
        "represent",
        "warrant",
        "waive",
        "release",
        "elect",
        "request",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    // List letters and numbers may come first: "a) I certify".
    let within = if opening { 3 } else { words.len() };
    words
        .iter()
        .enumerate()
        .take(within)
        .filter(|(_, w)| matches!(**w, "i" | "we" | "undersigned"))
        .any(|(i, _)| {
            // "I hereby certify", "I have read and understood".
            words[i + 1..]
                .iter()
                .take(5)
                .any(|w| matches!(*w, "read" | "aware") || STEMS.iter().any(|s| is_form_of(w, s)))
        })
}

/// A field name from a label: no dots (they join parent and child names),
/// no trailing colon, not too long.
fn name_from(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| if c == '.' || c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut name = words.join(" ");
    while name.ends_with([':', '?', '*', '-', '_', ' ']) {
        name.pop();
    }
    if name.chars().count() > 60 {
        name = name.chars().take(60).collect::<String>().trim().to_string();
    }
    name
}

// ----- Detection ---------------------------------------------------------

struct Found {
    rect: Rect,
    kind: NewFieldKind,
    label: Option<String>,
    source: Source,
    table: Option<usize>,
}

fn text_kind(r: &Rect) -> NewFieldKind {
    if r.y1 - r.y0 > MULTILINE_H {
        NewFieldKind::Multiline
    } else {
        NewFieldKind::Text
    }
}

fn is_check_size(r: &Rect) -> bool {
    let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
    (5.0..=24.0).contains(&w) && (5.0..=24.0).contains(&h) && w / h > 0.75 && w / h < 1.34
}

/// Finds the tables and suggested fields of a page.
pub fn detect(input: &PageInput<'_>) -> Detected {
    let rotate = input.rotate % 360 / 90 * 90;
    let page = turn_rect(&input.bounds, rotate, false);
    let page_area = area(&page).max(1.0);
    let words = words(input.text, rotate);
    let phrases = &words.phrases;
    let (h, v) = rulings(input.lines, rotate);
    let images: Vec<Rect> = input
        .images
        .iter()
        .map(|q| rect_of(q.iter().map(|p| upright(*p, rotate))))
        .collect();
    let pictured = |r: &Rect| images.iter().any(|i| overlap(i, r) > area(r) * 0.3);

    let (cell_rects, touched) = cells(&h, &v);
    let group = groups(&cell_rects);
    // The text of each cell.
    let mut inside: Vec<Vec<usize>> = vec![Vec::new(); cell_rects.len()];
    for (pi, p) in phrases.iter().enumerate() {
        let (cx, cy) = ((p.rect.x0 + p.rect.x1) / 2.0, (p.rect.y0 + p.rect.y1) / 2.0);
        // The smallest cell holding it, should boxes nest.
        let holder = cell_rects
            .iter()
            .enumerate()
            .filter(|(_, c)| contains(c, cx, cy))
            .min_by(|a, b| area(a.1).total_cmp(&area(b.1)));
        if let Some((ci, _)) = holder {
            inside[ci].push(pi);
        }
    }
    let cell_text = |ci: usize| -> String {
        // `inside` follows the phrases, which run top to bottom, left to right.
        inside[ci]
            .iter()
            .map(|&pi| phrases[pi].text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };

    // Tables: groups of cells in at least two rows and two columns.
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ci, g) in group.iter().enumerate() {
        members.entry(*g).or_default().push(ci);
    }
    let mut group_ids: Vec<usize> = members.keys().copied().collect();
    group_ids.sort_by(|a, b| {
        let (ra, rb) = (&cell_rects[members[a][0]], &cell_rects[members[b][0]]);
        rb.y1.total_cmp(&ra.y1).then(ra.x0.total_cmp(&rb.x0))
    });
    let mut tables: Vec<Table> = Vec::new();
    // Each cell's table, row and column.
    let mut placed: HashMap<usize, (usize, usize, usize)> = HashMap::new();
    for g in &group_ids {
        let list = &members[g];
        let xs = edges(
            list.iter()
                .flat_map(|&c| [cell_rects[c].x0, cell_rects[c].x1])
                .collect(),
        );
        let mut ys = edges(
            list.iter()
                .flat_map(|&c| [cell_rects[c].y0, cell_rects[c].y1])
                .collect(),
        );
        ys.reverse();
        if list.len() < 4 || xs.len() < 3 || ys.len() < 3 {
            continue;
        }
        let rect = list
            .iter()
            .map(|&c| cell_rects[c])
            .reduce(|a, b| union(&a, &b))
            .expect("a table has cells");
        let index = tables.len();
        let cells = list
            .iter()
            .map(|&c| {
                let r = &cell_rects[c];
                let (row, col) = (edge_index(&ys, r.y1), edge_index(&xs, r.x0));
                placed.insert(c, (index, row, col));
                TableCell {
                    rect: turn_rect(r, rotate, true),
                    row,
                    col,
                    row_span: edge_index(&ys, r.y0).saturating_sub(row).max(1),
                    col_span: edge_index(&xs, r.x1).saturating_sub(col).max(1),
                    text: cell_text(c),
                }
            })
            .collect();
        tables.push(Table {
            rect: turn_rect(&rect, rotate, true),
            rows: ys.len() - 1,
            cols: xs.len() - 1,
            cells,
        });
    }
    // The text of a table's cell at a row and column.
    let text_at = |table: usize, row: usize, col: usize| -> Option<String> {
        placed
            .iter()
            .find(|(_, p)| **p == (table, row, col))
            .map(|(c, _)| cell_text(*c))
            .filter(|t| !t.trim().is_empty())
    };

    let mut found: Vec<Found> = Vec::new();

    // Cells: empty ones are blanks; so is the room beside or under a
    // caption printed in the cell.
    struct Captioned {
        cell: usize,
        rect: Rect,
        label: String,
        sure: bool,
    }
    let mut captioned: Vec<Captioned> = Vec::new();
    let mut with_text: HashMap<usize, usize> = HashMap::new();
    for (ci, c) in cell_rects.iter().enumerate() {
        let (w, hgt) = (c.x1 - c.x0, c.y1 - c.y0);
        if pictured(c) {
            continue;
        }
        if inside[ci].is_empty() {
            let table = placed.get(&ci).copied();
            let source = if table.is_some() {
                Source::TableCell
            } else {
                Source::Box
            };
            if is_check_size(c) {
                found.push(Found {
                    rect: inset(c, 0.5),
                    kind: NewFieldKind::Checkbox,
                    label: None,
                    source: Source::CheckBox,
                    table: table.map(|t| t.0),
                });
            } else if w >= MIN_TEXT_W && hgt >= MIN_TEXT_H && area(c) <= page_area * 0.35 {
                let label = table.and_then(|(t, row, col)| {
                    let head = text_at(t, 0, col).filter(|_| row > 0);
                    let side = text_at(t, row, 0).filter(|_| col > 0);
                    match (side, head) {
                        (Some(s), Some(h)) => Some(format!("{s} {h}")),
                        (Some(s), None) => Some(s),
                        (None, Some(h)) => Some(format!("{h} {row}")),
                        (None, None) => None,
                    }
                });
                found.push(Found {
                    rect: inset(c, 1.0),
                    kind: text_kind(c),
                    label,
                    source,
                    table: table.map(|t| t.0),
                });
            }
            continue;
        }
        *with_text.entry(group[ci]).or_default() += 1;
        // One line of text only: a caption, not content.
        let texts: Vec<&Phrase> = inside[ci].iter().map(|&pi| &phrases[pi]).collect();
        let tb = texts
            .iter()
            .map(|p| p.rect)
            .reduce(|a, b| union(&a, &b))
            .expect("the cell has text");
        let size = texts.iter().map(|p| p.size).fold(0.0, f64::max);
        if tb.y1 - tb.y0 > size * 1.6 {
            continue;
        }
        let text = cell_text(ci);
        let asks = text.trim_end().ends_with([':', '?']);
        // Room under a caption set in the top of the box.
        let under = tb.y0 - c.y0 - 1.0;
        let over = c.y1 - tb.y1;
        if under >= 9.0_f64.max(size * 1.2) && over <= under * 0.35 {
            captioned.push(Captioned {
                cell: ci,
                rect: Rect {
                    x0: c.x0 + 1.0,
                    y0: c.y0 + 1.0,
                    x1: c.x1 - 1.0,
                    y1: tb.y0 - 1.0,
                },
                label: text,
                sure: asks,
            });
            continue;
        }
        // Room to the right of "Name:".
        let right = c.x1 - tb.x1 - 4.0;
        if asks && right >= 30.0_f64.max(w * 0.3) {
            captioned.push(Captioned {
                cell: ci,
                rect: Rect {
                    x0: tb.x1 + 3.0,
                    y0: c.y0 + 1.0,
                    x1: c.x1 - 1.0,
                    y1: c.y1 - 1.0,
                },
                label: text,
                sure: true,
            });
        }
    }
    // A caption without a colon counts only where most of the grid is
    // laid out that way; otherwise it is a table of data with short rows.
    let mut captions: HashMap<usize, usize> = HashMap::new();
    for c in &captioned {
        *captions.entry(group[c.cell]).or_default() += 1;
    }
    for c in captioned {
        let g = group[c.cell];
        let usual = captions[&g] * 10 >= with_text[&g] * 6;
        if (c.sure || usual) && c.rect.x1 - c.rect.x0 >= MIN_TEXT_W {
            found.push(Found {
                kind: text_kind(&c.rect),
                rect: c.rect,
                label: Some(c.label),
                source: Source::Box,
                table: placed.get(&c.cell).map(|t| t.0),
            });
        }
    }

    // Lines to write on: rules that meet no upright line, with nothing
    // printed just above them.
    let mut blanks: Vec<(Ruling, bool)> = h
        .iter()
        .enumerate()
        .filter(|(i, _)| !touched.contains(i))
        .map(|(_, l)| (*l, false))
        .collect();
    blanks.extend(
        words
            .blanks
            .iter()
            .map(|&(at, from, to)| (Ruling { at, from, to }, true)),
    );
    for (line, typed) in blanks {
        if line.len() < MIN_INITIALS_W {
            continue;
        }
        // The lowest text above the line caps the field's height.
        let mut room = f64::INFINITY;
        for p in phrases {
            let across = p.rect.x1.min(line.to) - p.rect.x0.max(line.from);
            if across > 1.0 && p.rect.y1 > line.at + 1.0 {
                room = room.min(p.rect.y0 - line.at);
            }
        }
        if room < 8.0 {
            continue;
        }
        let probe = Rect {
            x0: line.from,
            y0: line.at,
            x1: line.to,
            y1: line.at + 10.0,
        };
        let left = label(phrases, &probe, Side::Left);
        let below = label(phrases, &probe, Side::Below).filter(|p| {
            // A caption under a signature line is short; the first line
            // of a paragraph under a rule is not.
            p.text.chars().count() <= 40
                && p.rect.x1 - p.rect.x0 <= line.len() * 1.2
                && line.len() <= (page.x1 - page.x0) * 0.7
        });
        // A short blank against a statement ("____ I certify that…") is
        // for initials. The statement usually follows the blank; it may
        // also end just before it, or stand over it.
        let short = line.len() <= MAX_INITIALS_W;
        let after = label(phrases, &probe, Side::Right).filter(|p| short && attests(&p.text, true));
        let before = left.filter(|p| {
            short && attests(&p.text, false) && p.text.trim_end().ends_with(['.', ';', ')'])
        });
        let over = label(phrases, &probe, Side::Above)
            .filter(|p| short && left.is_none() && below.is_none() && attests(&p.text, true));
        let stated = after.or(before).or(over);
        if stated.is_none() && line.len() < MIN_TEXT_W {
            continue;
        }
        let named = stated.or(left).or(below);
        if named.is_none() && !typed {
            continue;
        }
        let size = named.map_or(10.0, |p| p.size);
        // A signature wants more room than a line of type, where there is some.
        let signed = named.and_then(|p| asked_for(&p.text)) == Some(NewFieldKind::Signature);
        let tallest = if signed { 26.0 } else { 18.0 };
        let wanted = if signed { size * 2.6 } else { size * 1.4 };
        let height = wanted.clamp(10.0, tallest).min(room - 1.0);
        let rect = Rect {
            x0: line.from,
            y0: line.at + 0.5,
            x1: line.to,
            y1: line.at + height,
        };
        if pictured(&rect) {
            continue;
        }
        found.push(Found {
            rect,
            kind: if stated.is_some() {
                NewFieldKind::Initials
            } else {
                NewFieldKind::Text
            },
            label: named.map(|p| p.text.clone()),
            source: Source::Underline,
            table: None,
        });
    }

    // Check boxes printed as characters.
    for b in &words.boxes {
        found.push(Found {
            rect: *b,
            kind: NewFieldKind::Checkbox,
            label: None,
            source: Source::CheckBox,
            table: None,
        });
    }

    // Labels for what has none yet.
    for f in &mut found {
        if f.label.is_some() {
            continue;
        }
        let sides: &[Side] = match f.kind {
            NewFieldKind::Checkbox => &[Side::Right, Side::Left],
            _ => &[Side::Left, Side::Above, Side::Below],
        };
        f.label = sides
            .iter()
            .find_map(|s| label(phrases, &f.rect, *s))
            .map(|p| p.text.clone());
    }

    // Blanks labelled for a signature or for initials are those.
    for f in &mut found {
        if !matches!(f.kind, NewFieldKind::Checkbox | NewFieldKind::Initials)
            && let Some(kind) = f.label.as_deref().and_then(asked_for)
        {
            f.kind = kind;
        }
    }

    // Leave out what already has a field, what lies over something found
    // earlier, and what falls off the page.
    let taken: Vec<Rect> = input
        .taken
        .iter()
        .map(|r| turn_rect(r, rotate, false))
        .collect();
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        let a = area(&f.rect);
        let fits = f.rect.x1 - f.rect.x0 >= 3.0 && f.rect.y1 - f.rect.y0 >= 3.0;
        let on_page = overlap(&f.rect, &page) >= a * 0.9;
        let free = taken.iter().all(|t| overlap(t, &f.rect) <= a * 0.2)
            && kept
                .iter()
                .all(|k| overlap(&k.rect, &f.rect) <= a.min(area(&k.rect)) * 0.5);
        if fits && on_page && free {
            kept.push(f);
        }
    }

    // Reading order: rows from the top, each from the left.
    kept.sort_by(|a, b| {
        let (ya, yb) = (a.rect.y0 + a.rect.y1, b.rect.y0 + b.rect.y1);
        yb.total_cmp(&ya)
    });
    let mut row = 0;
    let mut row_y = f64::INFINITY;
    let mut keyed: Vec<(usize, Found)> = Vec::new();
    for f in kept {
        let mid = (f.rect.y0 + f.rect.y1) / 2.0;
        if row_y - mid > 5.0 {
            row += 1;
            row_y = mid;
        }
        keyed.push((row, f));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.rect.x0.total_cmp(&b.1.rect.x0)));

    let mut used: HashSet<String> = HashSet::new();
    let mut counts = [0usize; 2];
    let fields = keyed
        .into_iter()
        .map(|(_, f)| {
            let check = f.kind == NewFieldKind::Checkbox;
            let tooltip = f
                .label
                .as_deref()
                .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|l| !l.is_empty());
            let mut name = f.label.as_deref().map(name_from).unwrap_or_default();
            if name.is_empty() {
                counts[usize::from(check)] += 1;
                name = if check {
                    format!("Check box {}", counts[1])
                } else {
                    format!("Text {}", counts[0])
                };
            }
            let mut unique = name.clone();
            let mut n = 2;
            while !used.insert(unique.clone()) {
                unique = format!("{name} {n}");
                n += 1;
            }
            Suggestion {
                field: NewField {
                    name: unique,
                    kind: f.kind,
                    rect: turn_rect(&f.rect, rotate, true),
                    tooltip,
                    mark: None,
                    font_size: 0.0,
                },
                source: f.source,
                table: f.table,
            }
        })
        .collect();
    Detected { tables, fields }
}

// ----- Scans -------------------------------------------------------------

/// Lines read from a picture of the page: long thin runs of dark pixels,
/// in user space. For scans, which have no drawn lines to go by. Lines
/// shorter than 14 points are left out (they would be strokes of letters),
/// so the small squares of check boxes are not found this way.
pub fn lines_in_pixmap(pixmap: &Pixmap, page: &Page, scale: f64) -> Vec<PageLine> {
    let (w, h) = pixmap.bounds();
    let Some(to_user) = page_transform(page, scale).0.invert() else {
        return Vec::new();
    };
    let data = pixmap.data();
    let dark = |x: isize, y: isize| -> bool {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            return false;
        }
        let i = (y as usize * w + x as usize) * 4;
        is_dark([data[i], data[i + 1], data[i + 2], data[i + 3]])
    };
    let min_len = (14.0 * scale).ceil() as usize;
    let beyond = (3.0 * scale).ceil() as isize;
    let gap = 2usize;
    let mut out = Vec::new();
    // Rows first, then columns: `along` runs the length of the line.
    for upright_lines in [false, true] {
        let (across_n, along_n) = if upright_lines { (w, h) } else { (h, w) };
        let at = |across: isize, along: isize| -> bool {
            if upright_lines {
                dark(across, along)
            } else {
                dark(along, across)
            }
        };
        for across in 0..across_n as isize {
            let mut start: Option<usize> = None;
            let mut last = 0usize;
            for along in 0..=along_n {
                let on = along < along_n && at(across, along as isize);
                if on {
                    start.get_or_insert(along);
                    last = along;
                    continue;
                }
                let Some(s) = start else { continue };
                if along < along_n && along - last <= gap {
                    continue;
                }
                start = None;
                if last + 1 - s < min_len {
                    continue;
                }
                // A line is light on both sides; the edge of a dark block
                // is not.
                let samples = 16;
                let clear = (0..samples)
                    .filter(|k| {
                        let p = (s + (last - s) * k / (samples - 1)) as isize;
                        !at(across - beyond, p) && !at(across + beyond, p)
                    })
                    .count();
                if clear * 10 < samples * 7 {
                    continue;
                }
                let mid = across as f64 + 0.5;
                let (a, b) = if upright_lines {
                    (
                        Point::new(mid, s as f64),
                        Point::new(mid, last as f64 + 1.0),
                    )
                } else {
                    (
                        Point::new(s as f64, mid),
                        Point::new(last as f64 + 1.0, mid),
                    )
                };
                if out.len() < 20_000 {
                    out.push(PageLine {
                        from: to_user.apply(a),
                        to: to_user.apply(b),
                        width: 1.0 / scale,
                    });
                }
            }
        }
    }
    out
}

/// Whether a pixel of a page drawn white counts as ink.
fn is_dark(px: [u8; 4]) -> bool {
    // Premultiplied over a white page, so these are the shown colours.
    u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114 < 140_000
}

/// The box around what is printed inside `r` (as read, like `r`) on a
/// picture of the page, or `None` when there is nothing but specks.
fn ink_in(pixmap: &Pixmap, page: &Page, scale: f64, r: &Rect) -> Option<Rect> {
    let to_device = page_transform(page, scale).0;
    let to_user = to_device.invert()?;
    let user = turn_rect(r, page.rotate, true);
    let device = rect_of([
        to_device.apply(Point::new(user.x0, user.y0)),
        to_device.apply(Point::new(user.x1, user.y1)),
    ]);
    let (w, h) = pixmap.bounds();
    let x0 = (device.x0.ceil().max(0.0) as usize).min(w);
    let x1 = (device.x1.floor().max(0.0) as usize).min(w);
    let y0 = (device.y0.ceil().max(0.0) as usize).min(h);
    let y1 = (device.y1.floor().max(0.0) as usize).min(h);
    let mut count = 0usize;
    let mut found: Option<[usize; 4]> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if is_dark(pixmap.pixel(x, y)) {
                count += 1;
                let b = found.get_or_insert([x, y, x, y]);
                *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
    }
    let b = found.filter(|_| count >= 6 && count * 2000 >= (x1 - x0) * (y1 - y0))?;
    let corner = |x: usize, y: usize| to_user.apply(Point::new(x as f64, y as f64));
    let inked = rect_of([corner(b[0], b[1]), corner(b[2] + 1, b[3] + 1)]);
    Some(turn_rect(&inked, page.rotate, false))
}

/// On a scan the text is not known (until OCR reads it), so what is
/// printed is judged from the picture: a suggestion over print is dropped,
/// unless the print is a caption along the top of a box, which then keeps
/// the room under it.
fn keep_clear(found: &mut Detected, pixmap: &Pixmap, page: &Page, scale: f64) {
    let rotate = page.rotate;
    found.fields.retain_mut(|s| {
        let r = turn_rect(&s.field.rect, rotate, false);
        // Clear of the box's own lines.
        let within = inset(&r, 2.0);
        if within.x1 <= within.x0 || within.y1 <= within.y0 {
            return true;
        }
        let Some(ink) = ink_in(pixmap, page, scale, &within) else {
            return true;
        };
        let under = ink.y0 - r.y0 - 1.0;
        let over = r.y1 - ink.y1;
        let line = ink.y1 - ink.y0;
        if line > 16.0 || under < 9.0_f64.max(line * 1.2) || over > under * 0.35 {
            return false;
        }
        let kept = Rect {
            y1: ink.y0 - 1.0,
            ..r
        };
        if matches!(s.field.kind, NewFieldKind::Text | NewFieldKind::Multiline) {
            s.field.kind = text_kind(&kept);
        }
        s.field.rect = turn_rect(&kept, rotate, true);
        true
    });
    // Number again the ones named for want of a label.
    let mut counts = [0usize; 2];
    for s in &mut found.fields {
        if s.field.tooltip.is_some() {
            continue;
        }
        let check = s.field.kind == NewFieldKind::Checkbox;
        counts[usize::from(check)] += 1;
        s.field.name = if check {
            format!("Check box {}", counts[1])
        } else {
            format!("Text {}", counts[0])
        };
    }
}

/// Whether pictures cover most of the page, as on a scan.
fn mostly_picture(graphics: &PageGraphics, bounds: &Rect) -> bool {
    let covered: f64 = graphics
        .images
        .iter()
        .map(|q| overlap(&rect_of(q.iter().copied()), bounds))
        .fold(0.0, f64::max);
    covered >= area(bounds) * 0.5
}

/// Finds the tables and suggested fields of a page of a document. `text`
/// is the page's text (what OCR read, for a scan) and `taken` the boxes of
/// the fields it already has.
pub fn detect_page(doc: &Document, page: &Page, text: &PageText, taken: &[Rect]) -> Detected {
    let mut graphics = extract_graphics(doc, page).unwrap_or_default();
    let (h, v) = rulings(&graphics.lines, page.rotate);
    let mut scan = None;
    if h.is_empty() && v.is_empty() && mostly_picture(&graphics, &page.crop_box) {
        // A scan: read the lines from the picture, which is then what the
        // blanks are in rather than something to keep clear of.
        let longest = page.crop_box.width().max(page.crop_box.height()).max(1.0);
        let options = RenderOptions {
            scale: 2.0_f64.min(3000.0 / longest),
            annotations: false,
            ..RenderOptions::default()
        };
        if let Ok(r) = render_page(doc, page, &options) {
            graphics.lines = lines_in_pixmap(&r.pixmap, page, options.scale);
            scan = Some((r.pixmap, options.scale));
        }
        graphics.images.clear();
    }
    let mut found = detect(&PageInput {
        text,
        lines: &graphics.lines,
        images: &graphics.images,
        bounds: page.crop_box,
        rotate: page.rotate,
        taken,
    });
    if let Some((pixmap, scale)) = &scan {
        keep_clear(&mut found, pixmap, page, *scale);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_render::TextGlyph;

    const PAGE: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 612.0,
        y1: 792.0,
    };

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> PageLine {
        PageLine {
            from: Point::new(x0, y0),
            to: Point::new(x1, y1),
            width: 1.0,
        }
    }

    /// The four sides of a box.
    fn boxed(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PageLine> {
        vec![
            line(x0, y0, x1, y0),
            line(x1, y0, x1, y1),
            line(x1, y1, x0, y1),
            line(x0, y1, x0, y0),
        ]
    }

    /// A grid with lines at these xs and ys.
    fn grid(xs: &[f64], ys: &[f64]) -> Vec<PageLine> {
        let (x0, x1) = (xs[0], xs[xs.len() - 1]);
        let (y0, y1) = (ys[0], ys[ys.len() - 1]);
        let mut out: Vec<PageLine> = xs.iter().map(|&x| line(x, y0, x, y1)).collect();
        out.extend(ys.iter().map(|&y| line(x0, y, x1, y)));
        out
    }

    /// Text in 10 point type with its baseline at `y`, 5 points a letter.
    fn text(s: &str, x: f64, y: f64) -> Vec<TextGlyph> {
        s.chars()
            .enumerate()
            .map(|(i, c)| {
                let x = x + i as f64 * 5.0;
                let p = Point::new;
                TextGlyph {
                    text: c.to_string(),
                    quad: [
                        p(x, y - 2.0),
                        p(x + 5.0, y - 2.0),
                        p(x + 5.0, y + 8.0),
                        p(x, y + 8.0),
                    ],
                    baseline: (p(x, y), p(x + 5.0, y)),
                    size: 10.0,
                    font: "F".into(),
                    invisible: false,
                }
            })
            .collect()
    }

    fn run(glyphs: Vec<TextGlyph>, lines: &[PageLine], rotate: u16, taken: &[Rect]) -> Detected {
        detect(&PageInput {
            text: &PageText { glyphs },
            lines,
            images: &[],
            bounds: PAGE,
            rotate,
            taken,
        })
    }

    fn names(d: &Detected) -> Vec<&str> {
        d.fields.iter().map(|f| f.field.name.as_str()).collect()
    }

    #[test]
    fn a_ruled_table_gives_cells_named_by_their_headers() {
        // Three columns, three rows: a header row, then two rows with a
        // label in the first column.
        let lines = grid(&[100.0, 200.0, 300.0, 400.0], &[600.0, 620.0, 640.0, 660.0]);
        let mut glyphs = text("Item", 105.0, 646.0);
        glyphs.extend(text("Qty", 205.0, 646.0));
        glyphs.extend(text("Price", 305.0, 646.0));
        glyphs.extend(text("Bolts", 105.0, 626.0));
        glyphs.extend(text("Nuts", 105.0, 606.0));
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(d.tables.len(), 1);
        let t = &d.tables[0];
        assert_eq!((t.rows, t.cols, t.cells.len()), (3, 3, 9));
        assert_eq!(t.to_tsv(), "Item\tQty\tPrice\nBolts\t\t\nNuts\t\t");
        assert_eq!(
            names(&d),
            ["Bolts Qty", "Bolts Price", "Nuts Qty", "Nuts Price"]
        );
        assert!(d.fields.iter().all(|f| f.source == Source::TableCell
            && f.table == Some(0)
            && f.field.kind == NewFieldKind::Text));
        // Inside the cell's borders.
        let r = d.fields[0].field.rect;
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (201.0, 621.0, 299.0, 639.0));
    }

    #[test]
    fn a_table_full_of_text_suggests_nothing() {
        let lines = grid(&[100.0, 250.0, 400.0], &[600.0, 620.0, 640.0]);
        let mut glyphs = text("Name", 105.0, 626.0);
        glyphs.extend(text("Town", 255.0, 626.0));
        glyphs.extend(text("Ada", 105.0, 606.0));
        glyphs.extend(text("Leeds", 255.0, 606.0));
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(d.tables.len(), 1);
        assert!(d.fields.is_empty(), "{:?}", names(&d));
    }

    #[test]
    fn lines_with_labels_become_text_fields() {
        let mut lines = vec![line(150.0, 500.0, 350.0, 500.0)];
        let mut glyphs = text("Full name:", 90.0, 502.0);
        // A signature line with its caption underneath.
        lines.push(line(100.0, 400.0, 300.0, 400.0));
        glyphs.extend(text("Signature", 100.0, 388.0));
        // A rule under a heading is not a blank.
        lines.push(line(72.0, 700.0, 540.0, 700.0));
        glyphs.extend(text("Application", 72.0, 703.0));
        // Nor is a rule with nothing to say what it is for.
        lines.push(line(72.0, 100.0, 540.0, 100.0));
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(names(&d), ["Full name", "Signature"]);
        let f = &d.fields[0];
        assert_eq!(f.source, Source::Underline);
        assert_eq!(f.field.tooltip.as_deref(), Some("Full name:"));
        let r = f.field.rect;
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (150.0, 500.5, 350.0, 514.0));
    }

    #[test]
    fn short_blanks_against_statements_are_for_initials() {
        let kinds = |d: &Detected| d.fields.iter().map(|f| f.field.kind).collect::<Vec<_>>();
        // Typed blanks before statements, one of them misspelt; a drawn
        // one after a statement; a drawn one under a statement.
        let mut glyphs = text("____", 90.0, 600.0);
        glyphs.extend(text("I certify that the above is true.", 115.0, 600.0));
        glyphs.extend(text("____", 90.0, 580.0));
        glyphs.extend(text("b) We hereby acknowlege the fees.", 115.0, 580.0));
        glyphs.extend(text("I understand this is final.", 90.0, 560.0));
        glyphs.extend(text("I have read the rules.", 90.0, 530.0));
        let lines = [
            line(230.0, 559.0, 270.0, 559.0),
            line(90.0, 510.0, 130.0, 510.0),
        ];
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(kinds(&d), [NewFieldKind::Initials; 4], "{:?}", names(&d));
        assert_eq!(d.fields[0].field.name, "I certify that the above is true");

        // A blank inside a statement is something to fill in, a long line
        // beside one is not for initials, and nor is a short one beside
        // other text.
        let mut glyphs = text("I certify that my age is", 90.0, 600.0);
        glyphs.extend(text("______", 215.0, 600.0));
        glyphs.extend(text("________________________", 90.0, 570.0));
        glyphs.extend(text("I agree to the terms.", 215.0, 570.0));
        glyphs.extend(text("____", 90.0, 540.0));
        glyphs.extend(text("Invoices are sent monthly.", 115.0, 540.0));
        let d = run(glyphs, &[], 0, &[]);
        assert_eq!(kinds(&d), [NewFieldKind::Text; 3], "{:?}", names(&d));
    }

    #[test]
    fn typed_underscores_are_blanks() {
        let mut glyphs = text("Date:", 90.0, 300.0);
        glyphs.extend(text("__________", 120.0, 300.0));
        let d = run(glyphs, &[], 0, &[]);
        assert_eq!(names(&d), ["Date"]);
        let r = d.fields[0].field.rect;
        assert_eq!((r.x0, r.x1), (120.0, 170.0));
        assert!(r.y0 < 300.0 && r.y1 > 308.0, "{r:?}");
    }

    #[test]
    fn small_squares_are_check_boxes_named_by_the_text_beside_them() {
        let mut lines = boxed(100.0, 300.0, 110.0, 310.0);
        lines.extend(boxed(100.0, 280.0, 110.0, 290.0));
        let mut glyphs = text("Yes", 115.0, 301.0);
        glyphs.extend(text("No", 115.0, 281.0));
        // A box character with its label.
        glyphs.extend(text("☐", 100.0, 261.0));
        glyphs.extend(text("Maybe", 112.0, 261.0));
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(names(&d), ["Yes", "No", "Maybe"]);
        assert!(
            d.fields
                .iter()
                .all(|f| f.field.kind == NewFieldKind::Checkbox && f.source == Source::CheckBox)
        );
        assert!(d.tables.is_empty());
    }

    #[test]
    fn boxes_with_a_caption_keep_the_room_beside_or_under_it() {
        // "Email:" at the left of a wide box.
        let mut lines = boxed(100.0, 500.0, 400.0, 520.0);
        let mut glyphs = text("Email:", 104.0, 506.0);
        // A caption in the top of a tall box.
        lines.extend(boxed(100.0, 420.0, 400.0, 470.0));
        glyphs.extend(text("Comments?", 104.0, 459.0));
        // An empty tall box takes several lines.
        lines.extend(boxed(100.0, 300.0, 400.0, 380.0));
        let d = run(glyphs, &lines, 0, &[]);
        assert_eq!(names(&d), ["Email", "Comments", "Text 1"]);
        let email = d.fields[0].field.rect;
        assert_eq!((email.x0, email.x1), (137.0, 399.0));
        let comments = &d.fields[1].field;
        assert_eq!((comments.rect.y0, comments.rect.y1), (421.0, 456.0));
        assert_eq!(comments.kind, NewFieldKind::Multiline);
        assert_eq!(d.fields[2].field.kind, NewFieldKind::Multiline);
    }

    #[test]
    fn places_with_a_field_already_are_left_alone() {
        let lines = vec![line(150.0, 500.0, 350.0, 500.0)];
        let glyphs = text("Full name:", 90.0, 502.0);
        let taken = [Rect {
            x0: 150.0,
            y0: 499.0,
            x1: 350.0,
            y1: 515.0,
        }];
        assert!(run(glyphs, &lines, 0, &taken).fields.is_empty());
    }

    #[test]
    fn turned_pages_are_read_the_way_they_are_shown() {
        // The same label and line as above, on a page turned a quarter:
        // what is read left to right runs up the page.
        let turn = |p: Point| from_upright(p, 90);
        let lines: Vec<PageLine> = [line(150.0, -300.0, 350.0, -300.0)]
            .iter()
            .map(|l| PageLine {
                from: turn(l.from),
                to: turn(l.to),
                width: 1.0,
            })
            .collect();
        let glyphs: Vec<TextGlyph> = text("Full name:", 90.0, -298.0)
            .into_iter()
            .map(|mut g| {
                g.quad = g.quad.map(turn);
                g.baseline = (turn(g.baseline.0), turn(g.baseline.1));
                g
            })
            .collect();
        let d = run(glyphs, &lines, 90, &[]);
        assert_eq!(names(&d), ["Full name"]);
        let r = d.fields[0].field.rect;
        // Upright (150, -299.5)-(350, -286) is user (286, 150)-(299.5, 350).
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (286.0, 150.0, 299.5, 350.0));
    }

    #[test]
    fn lines_are_read_from_a_picture_of_the_page() {
        let page = Page {
            obj_ref: None,
            dict: Default::default(),
            media_box: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 200.0,
                y1: 100.0,
            },
            crop_box: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 200.0,
                y1: 100.0,
            },
            rotate: 0,
            resources: Default::default(),
        };
        let scale = 2.0;
        let mut pixmap = Pixmap::filled(400, 200, atelier_raster::Color::WHITE);
        let mut paint = |x0: usize, y0: usize, x1: usize, y1: usize| {
            let data = pixmap.data_mut();
            for y in y0..y1 {
                for x in x0..x1 {
                    data[(y * 400 + x) * 4..(y * 400 + x) * 4 + 3].fill(0);
                }
            }
        };
        // A line two pixels thick at user y = 50, from x = 20 to 180.
        paint(40, 99, 360, 101);
        // A dark block, whose edges are not lines.
        paint(40, 20, 200, 60);
        // A short stroke, as of a letter.
        paint(300, 150, 310, 152);
        let lines = lines_in_pixmap(&pixmap, &page, scale);
        let (h, v) = rulings(&lines, 0);
        assert!(v.is_empty(), "{v:?}");
        assert_eq!(h.len(), 1, "{h:?}");
        assert!((h[0].at - 50.0).abs() < 0.6, "{h:?}");
        assert!((h[0].from - 20.0).abs() < 1.0 && (h[0].to - 180.0).abs() < 1.0);
    }
}
