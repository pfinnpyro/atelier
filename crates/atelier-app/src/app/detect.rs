//! Making a form of a printed one: Atelier looks for tables, boxes, lines
//! to write on and check boxes, and suggests a field for each. The
//! suggestions lie over the pages, where they can be moved, resized,
//! renamed, changed to another kind or dismissed before they are added;
//! dragging on a page draws one by hand. Fields in the file can be moved
//! and resized the same way, and those added since the last save renamed
//! or removed.

use std::collections::HashSet;

use atelier_form::{CheckMark, FieldKind, NewField, NewFieldKind, Source, Table};
use atelier_raster::Point;
use atelier_syntax::Rect as PdfRect;
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Stroke, StrokeKind, Vec2,
};

use super::Viewer;

/// A light green wash (premultiplied: 40, 170, 90 at 22% opacity).
const SUGGESTED: Color32 = Color32::from_rgba_premultiplied(9, 37, 20, 56);
const SUGGESTED_EDGE: Color32 = Color32::from_rgb(24, 130, 66);
const TABLE_EDGE: Color32 = Color32::from_rgb(132, 84, 196);
const PICKED: Color32 = Color32::from_rgb(40, 110, 230);
/// How near an edge of the picked box a press resizes it, in pixels.
const REACH: f64 = 5.0;
/// The smallest field, in points.
const MIN_SIDE: f64 = 4.0;

/// Text sizes to pick from, in points, besides sizing to fit.
pub(super) const SIZES: [f64; 11] = [6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 24.0];

/// How a text size reads in a menu; 0 sizes the text to fit its box.
pub(super) fn size_label(size: f64) -> String {
    if size > 0.0 {
        format!("{size} pt")
    } else {
        "Fit the box".into()
    }
}

/// A box to pick a text size from. Returns a size newly picked.
fn size_picker(ui: &mut egui::Ui, salt: &str, current: f64) -> Option<f64> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!("Text: {}", size_label(current).to_lowercase()))
        .show_ui(ui, |ui| {
            for size in std::iter::once(0.0).chain(SIZES) {
                if ui
                    .selectable_label(current == size, size_label(size))
                    .clicked()
                {
                    picked = Some(size);
                }
            }
        })
        .response
        .on_hover_text("The size of what is typed; fitted text shrinks to stay in the box");
    picked
}

/// A box to pick a check box's mark from. Returns a mark newly picked.
fn mark_picker(ui: &mut egui::Ui, salt: &str, current: Option<CheckMark>) -> Option<CheckMark> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!(
            "Mark: {}",
            current.map_or("its own", |m| m.label()).to_lowercase()
        ))
        .show_ui(ui, |ui| {
            for mark in CheckMark::ALL {
                if ui
                    .selectable_label(current == Some(mark), mark.label())
                    .clicked()
                {
                    picked = Some(mark);
                }
            }
        })
        .response
        .on_hover_text("What the box shows when ticked");
    picked
}

/// A field waiting to be added.
pub(super) struct Item {
    pub page: usize,
    pub field: NewField,
    /// What it was found from; `None` when drawn by hand.
    pub source: Option<Source>,
}

/// What is picked: a suggestion, or a widget of a field (field, widget).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    Suggestion(usize),
    Field(usize, usize),
}

/// Which sides of a box a drag moves; none of them moves the whole box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Grip {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Grip {
    fn cursor(self) -> CursorIcon {
        match (self.left || self.right, self.top || self.bottom) {
            (false, false) => CursorIcon::Move,
            (true, false) => CursorIcon::ResizeHorizontal,
            (false, true) => CursorIcon::ResizeVertical,
            _ if self.left == self.top => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }
}

/// A box in layout coordinates: left, top, right, bottom.
type Box4 = [f64; 4];

/// A field being drawn: its page and two corners, in layout coordinates.
type Drawing = (usize, (f64, f64), (f64, f64));

/// A box being moved or resized.
struct Drag {
    what: Picked,
    page: usize,
    grip: Grip,
    from: (f64, f64),
    start: Box4,
    now: Box4,
}

/// Suggested fields and tables of the open file, and what is being done
/// with them.
pub(super) struct Suggest {
    run: u64,
    pub running: bool,
    pages_done: usize,
    pages: usize,
    pub items: Vec<Item>,
    /// Tables found, with their page.
    pub tables: Vec<(usize, Table)>,
    pub picked: Option<Picked>,
    drag: Option<Drag>,
    drawing: Option<Drawing>,
    /// The name being typed for the picked box.
    name: String,
    name_of: Option<Picked>,
}

/// What a press on the review bar asks for.
enum Ask {
    AddAll,
    AddPage(usize),
    Add(usize),
    Dismiss(usize),
    Kind(usize, NewFieldKind),
    Rename(usize, String),
    Remove(usize),
    Size(usize, f64),
    Mark(usize, CheckMark),
    DocumentMark(Option<CheckMark>),
    MarkAll(CheckMark),
    CopyTable(usize),
    ShowTable(usize),
    Again,
    Close,
}

/// Which sides of `rect` a press at `p` takes hold of, or `None` when it
/// misses the box. Small boxes are moved from inside and resized from
/// just outside, so there is always somewhere to hold them by.
fn grip_at(rect: Box4, p: (f64, f64), reach: f64) -> Option<Grip> {
    let [x0, y0, x1, y1] = rect;
    if p.0 < x0 - reach || p.0 > x1 + reach || p.1 < y0 - reach || p.1 > y1 + reach {
        return None;
    }
    let inner = |size: f64| if size >= reach * 4.0 { reach } else { 0.0 };
    let (ix, iy) = (inner(x1 - x0), inner(y1 - y0));
    Some(Grip {
        left: p.0 <= x0 + ix,
        right: p.0 >= x1 - ix,
        top: p.1 <= y0 + iy,
        bottom: p.1 >= y1 - iy,
    })
}

/// `rect` after dragging `grip` by `(dx, dy)`, never smaller than `min`.
fn dragged(rect: Box4, grip: Grip, (dx, dy): (f64, f64), min: f64) -> Box4 {
    let [mut x0, mut y0, mut x1, mut y1] = rect;
    if grip == Grip::default() {
        return [x0 + dx, y0 + dy, x1 + dx, y1 + dy];
    }
    if grip.left {
        x0 = (x0 + dx).min(x1 - min);
    } else if grip.right {
        x1 = (x1 + dx).max(x0 + min);
    }
    if grip.top {
        y0 = (y0 + dy).min(y1 - min);
    } else if grip.bottom {
        y1 = (y1 + dy).max(y0 + min);
    }
    [x0, y0, x1, y1]
}

/// The kind of field a box drawn by hand most likely is.
fn kind_for(rect: &PdfRect, rotate: u16) -> NewFieldKind {
    let (mut w, mut h) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
    if rotate % 180 == 90 {
        std::mem::swap(&mut w, &mut h);
    }
    if w <= 24.0 && h <= 24.0 && w / h > 0.75 && w / h < 1.34 {
        NewFieldKind::Checkbox
    } else if h > 34.0 {
        NewFieldKind::Multiline
    } else {
        NewFieldKind::Text
    }
}

impl Viewer {
    /// Whether suggested fields are being reviewed, which takes over
    /// clicks on the page.
    pub(super) fn reviewing_fields(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.suggest.is_some())
    }

    /// Looks for tables and blanks on every page and opens the review
    /// bar. Suggestions not yet added are replaced.
    pub(super) fn start_detect(&mut self) {
        self.finish_editing(true);
        self.commit_ink();
        self.annotate.tool = super::annotate::Tool::Select;
        self.selection = None;
        let Some(o) = &mut self.open else { return };
        if atelier_form::document_has_xfa(&o.doc) {
            self.error = Some(
                "This file's form is XFA, which Atelier does not lay out; fields added to it \
                 would not be shown by apps that do."
                    .into(),
            );
            return;
        }
        let pages = o.layout.page_count();
        let mut taken: Vec<Vec<PdfRect>> = vec![Vec::new(); pages];
        for f in o.form.iter().flat_map(|fv| &fv.form.fields) {
            for w in &f.widgets {
                if let Some(t) = w.page.and_then(|p| taken.get_mut(p)) {
                    t.push(w.rect);
                }
            }
        }
        let ctx = self.ctx.clone();
        let run = self.pool.detect(
            o.doc.clone(),
            o.opened,
            o.ocr_texts.clone(),
            taken,
            move || ctx.request_repaint(),
        );
        o.suggest = Some(Suggest {
            run,
            running: true,
            pages_done: 0,
            pages,
            items: Vec::new(),
            tables: Vec::new(),
            picked: None,
            drag: None,
            drawing: None,
            name: String::new(),
            name_of: None,
        });
    }

    /// Ends the review; suggestions not added are dropped.
    pub(super) fn close_detect(&mut self) {
        if let Some(o) = &mut self.open
            && o.suggest.take().is_some()
        {
            self.pool.stop_detect();
        }
    }

    /// What a detection run found on one page.
    pub(super) fn detected(&mut self, run: u64, page: usize, found: atelier_form::Detected) {
        let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
            return;
        };
        if s.run != run {
            return;
        }
        s.pages_done = page + 1;
        s.tables.extend(found.tables.into_iter().map(|t| (page, t)));
        s.items.extend(found.fields.into_iter().map(|f| Item {
            page,
            field: f.field,
            source: Some(f.source),
        }));
    }

    /// A detection run has looked at every page.
    pub(super) fn detect_ended(&mut self, run: u64) {
        let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
            return;
        };
        if s.run != run {
            return;
        }
        s.running = false;
        if s.items.is_empty() {
            self.notice = Some(
                if s.tables.is_empty() {
                    "No tables or blanks to fill in were found. Drag on a page to draw a \
                     field yourself."
                } else {
                    "No blanks to fill in were found. Drag on a page to draw a field yourself."
                }
                .into(),
            );
        }
    }

    // ----- Geometry -----------------------------------------------------

    fn layout_box(&self, page: usize, r: &PdfRect) -> Option<Box4> {
        let t = self.open.as_ref()?.layout.user_to_layout(page)?;
        let (a, b) = (
            t.apply(Point::new(r.x0, r.y0)),
            t.apply(Point::new(r.x1, r.y1)),
        );
        Some([a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)])
    }

    fn user_box(&self, page: usize, b: Box4) -> Option<PdfRect> {
        let layout = &self.open.as_ref()?.layout;
        let (p, q) = (
            layout.to_user(page, b[0], b[1])?,
            layout.to_user(page, b[2], b[3])?,
        );
        Some(PdfRect {
            x0: p.x.min(q.x),
            y0: p.y.min(q.y),
            x1: p.x.max(q.x),
            y1: p.y.max(q.y),
        })
    }

    /// The page and box (in user space) of what is picked.
    fn picked_rect(&self, what: Picked) -> Option<(usize, PdfRect)> {
        let o = self.open.as_ref()?;
        match what {
            Picked::Suggestion(i) => {
                let item = o.suggest.as_ref()?.items.get(i)?;
                Some((item.page, item.field.rect))
            }
            Picked::Field(f, w) => {
                let w = o.form.as_ref()?.form.fields.get(f)?.widgets.get(w)?;
                Some((w.page?, w.rect))
            }
        }
    }

    /// The box of what is picked in layout coordinates, as now dragged.
    fn picked_box(&self, what: Picked) -> Option<(usize, Box4)> {
        let s = self.open.as_ref()?.suggest.as_ref()?;
        if let Some(d) = s.drag.as_ref().filter(|d| d.what == what) {
            return Some((d.page, d.now));
        }
        let (page, rect) = self.picked_rect(what)?;
        Some((page, self.layout_box(page, &rect)?))
    }

    /// What a press at a layout point takes hold of: the picked box by an
    /// edge, else the smallest suggestion under the point, else a field.
    fn hold_at(&self, at: (f64, f64)) -> Option<(Picked, Grip)> {
        let o = self.open.as_ref()?;
        let s = o.suggest.as_ref()?;
        if let Some(what) = s.picked
            && let Some((_, b)) = self.picked_box(what)
            && let Some(grip) = grip_at(b, at, REACH)
        {
            return Some((what, grip));
        }
        let (page, _, _) = o.layout.page_at(at.0, at.1)?;
        let user = o.layout.to_user(page, at.0, at.1)?;
        let inside =
            |r: &PdfRect| user.x >= r.x0 && user.x <= r.x1 && user.y >= r.y0 && user.y <= r.y1;
        let suggestion = s
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.page == page && inside(&it.field.rect))
            .min_by(|a, b| {
                let area = |r: &PdfRect| (r.x1 - r.x0) * (r.y1 - r.y0);
                area(&a.1.field.rect).total_cmp(&area(&b.1.field.rect))
            })
            .map(|(i, _)| Picked::Suggestion(i));
        let field = || {
            let (f, w) = o.form.as_ref()?.form.widget_at(page, user.x, user.y)?;
            Some(Picked::Field(f, w))
        };
        suggestion
            .or_else(field)
            .map(|what| (what, Grip::default()))
    }

    /// Whether a field's box may be moved.
    fn movable(&self, what: Picked) -> bool {
        match what {
            Picked::Suggestion(_) => true,
            Picked::Field(f, _) => self
                .open
                .as_ref()
                .and_then(|o| o.form.as_ref())
                .and_then(|fv| fv.form.fields.get(f))
                .is_some_and(|f| !f.is_read_only()),
        }
    }

    fn pick(&mut self, what: Option<Picked>) {
        if let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) {
            s.picked = what;
            s.drag = None;
        }
    }

    // ----- Changes ------------------------------------------------------

    /// A name no field and no other suggestion has.
    fn fresh_name(&self, base: &str) -> String {
        let Some(o) = &self.open else {
            return base.into();
        };
        let waiting: HashSet<&str> = o
            .suggest
            .iter()
            .flat_map(|s| &s.items)
            .map(|i| i.field.name.as_str())
            .collect();
        (1..)
            .map(|n| format!("{base} {n}"))
            .find(|n| !waiting.contains(n.as_str()) && o.session.unique_field_name(n) == *n)
            .expect("some number is free")
    }

    /// Makes fields of suggestions (by index) and drops them from the
    /// list. Returns how many were added.
    pub(super) fn add_suggestions(&mut self, which: &[usize]) -> usize {
        self.finish_editing(true);
        let preferred = self.settings.check_mark;
        let Some(o) = &mut self.open else { return 0 };
        // Signed files that may not change are refused before anything
        // is made of the suggestions.
        if let Err(e) = o.doc.signature_write_policy().check(false) {
            self.error = Some(e.to_string());
            return 0;
        }
        let Some(s) = &mut o.suggest else { return 0 };
        let mut which: Vec<usize> = which
            .iter()
            .copied()
            .filter(|i| *i < s.items.len())
            .collect();
        which.sort_unstable();
        which.dedup();
        let mut pages = HashSet::new();
        let mut added = Vec::new();
        let mut problem = None;
        for &i in &which {
            let item = &s.items[i];
            // A check box's own mark, else the document's, else the
            // one preferred in Settings.
            let mut field = item.field.clone();
            field.mark = field.mark.or(o.session.document_mark()).or(Some(preferred));
            match o.session.add_field(item.page, field) {
                Ok(_) => {
                    pages.insert(item.page);
                    added.push(i);
                }
                Err(e) => problem = Some(e.to_string()),
            }
        }
        if added.is_empty() {
            self.error = problem.map(|e| format!("Could not add the field: {e}"));
            return 0;
        }
        if !self.try_rebuild(&pages, false) {
            // The file would not take them: put things back as they were.
            if let Some(o) = &mut self.open {
                for _ in &added {
                    let last = o.session.original().map_or(0, |f| f.fields.len());
                    o.session.remove_added_field(last.saturating_sub(1));
                }
            }
            return 0;
        }
        if let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) {
            for &i in added.iter().rev() {
                s.items.remove(i);
            }
            // Go on to the suggestion after the last one added.
            let next = added.last().map_or(0, |l| l + 1 - added.len());
            s.picked = (next < s.items.len()).then_some(Picked::Suggestion(next));
            s.drag = None;
        }
        if let Some(e) = problem {
            self.error = Some(format!("Some fields could not be added: {e}"));
        }
        added.len()
    }

    fn dismiss_suggestion(&mut self, i: usize) {
        let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
            return;
        };
        if i < s.items.len() {
            s.items.remove(i);
            s.picked = (i < s.items.len()).then_some(Picked::Suggestion(i));
            s.drag = None;
        }
    }

    /// Takes away a field added since the last save.
    fn remove_added(&mut self, field: usize) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        let Some(page) = o.session.remove_added_field(field) else {
            return;
        };
        if let Some(fv) = &mut o.form {
            fv.focus = None;
        }
        if let Some(s) = &mut o.suggest {
            s.picked = None;
            s.drag = None;
        }
        self.rebuild(&HashSet::from([page]), false);
    }

    /// Sets the size of a field's text; 0 sizes it to fit the box.
    pub(super) fn set_text_size(&mut self, field: usize, size: f64) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        let Some(mut style) = o.session.field_style(field) else {
            return;
        };
        style.appearance.size = size;
        match o.session.set_field_style(field, style) {
            Ok(()) => {
                let pages = o.session.pages_of(field).into_iter().collect();
                self.rebuild(&pages, false);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Changes the mark a check box shows when ticked.
    pub(super) fn set_mark(&mut self, field: usize, mark: CheckMark) {
        let Some(o) = &mut self.open else { return };
        match o.session.set_check_mark(field, mark) {
            Ok(()) => {
                let pages = o.session.pages_of(field).into_iter().collect();
                self.rebuild(&pages, false);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Gives every check box of the document that can be filled in the
    /// same mark.
    fn set_every_mark(&mut self, mark: CheckMark) {
        let Some(o) = &mut self.open else { return };
        let changed = o.session.set_all_check_marks(mark);
        if changed == 0 {
            self.notice = Some("Every check box already shows that mark.".into());
            return;
        }
        let pages: HashSet<usize> = (0..o.layout.page_count()).collect();
        if self.try_rebuild(&pages, false) {
            self.notice = Some(format!(
                "{changed} check {} now {} a {}.",
                if changed == 1 { "box" } else { "boxes" },
                if changed == 1 { "shows" } else { "show" },
                mark.label().to_lowercase()
            ));
        }
    }

    /// Sets the mark this document's new check boxes get, or with `None`
    /// goes back to the one preferred in Settings.
    fn set_document_mark(&mut self, mark: Option<CheckMark>) {
        let Some(o) = &mut self.open else { return };
        o.session.set_document_mark(mark);
        // The setting is kept in the file.
        self.rebuild(&HashSet::new(), false);
    }

    fn rename_added(&mut self, field: usize, name: &str) {
        let Some(o) = &mut self.open else { return };
        let pages: HashSet<usize> = o.session.pages_of(field).into_iter().collect();
        match o.session.rename_added_field(field, name) {
            Ok(_) => {
                if let Some(s) = &mut o.suggest {
                    // Show the name the field really got.
                    s.name_of = None;
                }
                self.rebuild(&pages, false);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Puts a moved or resized box where it was dropped.
    fn drop_drag(&mut self) {
        let Some(d) = self
            .open
            .as_mut()
            .and_then(|o| o.suggest.as_mut())
            .and_then(|s| s.drag.take())
        else {
            return;
        };
        if d.now == d.start {
            return;
        }
        let Some(rect) = self.user_box(d.page, d.now) else {
            return;
        };
        let Some(o) = &mut self.open else { return };
        match d.what {
            Picked::Suggestion(i) => {
                if let Some(item) = o.suggest.as_mut().and_then(|s| s.items.get_mut(i)) {
                    item.field.rect = rect;
                }
            }
            Picked::Field(f, w) => {
                self.finish_editing(true);
                let Some(o) = &mut self.open else { return };
                match o.session.set_widget_bounds(f, w, rect) {
                    Ok(()) => {
                        let pages = o.session.pages_of(f).into_iter().collect();
                        self.rebuild(&pages, false);
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        }
    }

    /// Makes a suggestion of a box drawn by hand, and picks it.
    fn drop_drawing(&mut self) {
        let Some((page, a, b)) = self
            .open
            .as_mut()
            .and_then(|o| o.suggest.as_mut())
            .and_then(|s| s.drawing.take())
        else {
            return;
        };
        let drawn = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
        let Some(rect) = self.user_box(page, drawn) else {
            return;
        };
        if rect.x1 - rect.x0 < MIN_SIDE || rect.y1 - rect.y0 < MIN_SIDE {
            return;
        }
        let rotate = self
            .open
            .as_ref()
            .and_then(|o| o.layout.geom(page))
            .map_or(0, |g| g.rotate);
        let kind = kind_for(&rect, rotate);
        let name = self.fresh_name(if kind == NewFieldKind::Checkbox {
            "Check box"
        } else {
            "Text"
        });
        let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
            return;
        };
        s.items.push(Item {
            page,
            field: NewField {
                name,
                kind,
                rect,
                tooltip: None,
                mark: None,
                font_size: 0.0,
            },
            source: None,
        });
        s.picked = Some(Picked::Suggestion(s.items.len() - 1));
    }

    /// Scrolls what is picked into view.
    fn reveal_picked(&mut self) {
        let Some((page, rect)) = self
            .open
            .as_ref()
            .and_then(|o| o.suggest.as_ref())
            .and_then(|s| s.picked)
            .and_then(|what| self.picked_rect(what))
        else {
            return;
        };
        let Some(r) = self.user_rect(page, &rect, Pos2::ZERO) else {
            return;
        };
        let view = Rect::from_min_size(self.offset.to_pos2(), self.view_size).shrink(24.0);
        if !view.contains_rect(r) {
            self.scroll_to = Some(Vec2::new(
                (r.center().x - self.view_size.x / 2.0).max(0.0),
                (r.center().y - self.view_size.y / 2.0).max(0.0),
            ));
        }
    }

    // ----- Input --------------------------------------------------------

    /// Keys while reviewing: Tab and Shift+Tab step through the
    /// suggestions, Enter adds the picked one, Delete dismisses it (or
    /// removes a picked added field) and Esc lets go, then ends the review.
    pub(super) fn detect_keys(&mut self) {
        if !self.reviewing_fields() || self.ctx.text_edit_focused() {
            return;
        }
        let ctx = self.ctx.clone();
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let back = pressed(Modifiers::SHIFT, Key::Tab);
        if back || pressed(Modifiers::NONE, Key::Tab) {
            // Keep egui from moving focus to the toolbar.
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
                return;
            };
            let n = s.items.len();
            if n > 0 {
                let next = match s.picked {
                    Some(Picked::Suggestion(i)) if back => (i + n - 1) % n,
                    Some(Picked::Suggestion(i)) => (i + 1) % n,
                    _ if back => n - 1,
                    _ => 0,
                };
                s.picked = Some(Picked::Suggestion(next));
                s.drag = None;
                self.reveal_picked();
            }
            return;
        }
        let picked = self
            .open
            .as_ref()
            .and_then(|o| o.suggest.as_ref())
            .and_then(|s| s.picked);
        if pressed(Modifiers::NONE, Key::Enter)
            && let Some(Picked::Suggestion(i)) = picked
        {
            self.add_suggestions(&[i]);
            self.reveal_picked();
        }
        if pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace) {
            match picked {
                Some(Picked::Suggestion(i)) => {
                    self.dismiss_suggestion(i);
                    self.reveal_picked();
                }
                Some(Picked::Field(f, _)) => self.remove_added(f),
                None => {}
            }
        }
        if pressed(Modifiers::NONE, Key::Escape) {
            let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) else {
                return;
            };
            if s.drag.is_some() || s.drawing.is_some() {
                s.drag = None;
                s.drawing = None;
            } else if s.picked.is_some() {
                s.picked = None;
            } else {
                self.close_detect();
            }
        }
    }

    /// The pointer while reviewing: a click picks a box, a drag moves or
    /// resizes it, a drag on bare page draws a new one and a double click
    /// adds a suggestion. Returns whether the review took the pointer.
    pub(super) fn detect_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        origin: Pos2,
    ) -> bool {
        if !self.reviewing_fields() {
            return false;
        }
        let shift = self.centre_shift();
        let to_layout = |p: Pos2| (f64::from(p.x - origin.x) - shift, f64::from(p.y - origin.y));
        let zoom = self.zoom();

        let busy = self
            .open
            .as_ref()
            .and_then(|o| o.suggest.as_ref())
            .and_then(|s| s.drag.as_ref().map(|d| d.grip.cursor()));
        if let Some(icon) = busy {
            ui.ctx().set_cursor_icon(icon);
        } else if let Some(p) = response.hover_pos() {
            let at = to_layout(p);
            let icon = match self.hold_at(at) {
                Some((what, grip)) if self.movable(what) => grip.cursor(),
                Some(_) => CursorIcon::PointingHand,
                None => CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = ui
                .input(|i| i.pointer.press_origin())
                .or(response.hover_pos())
        {
            let at = to_layout(p);
            match self.hold_at(at) {
                Some((what, grip)) => {
                    self.pick(Some(what));
                    let held = self.picked_box(what).filter(|_| self.movable(what));
                    if let (Some((page, start)), Some(s)) =
                        (held, self.open.as_mut().and_then(|o| o.suggest.as_mut()))
                    {
                        s.drag = Some(Drag {
                            what,
                            page,
                            grip,
                            from: at,
                            start,
                            now: start,
                        });
                    }
                }
                None => {
                    self.pick(None);
                    let page = self
                        .open
                        .as_ref()
                        .and_then(|o| o.layout.page_at(at.0, at.1))
                        .map(|p| p.0);
                    if let (Some(page), Some(s)) =
                        (page, self.open.as_mut().and_then(|o| o.suggest.as_mut()))
                    {
                        s.drawing = Some((page, at, at));
                    }
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            let at = to_layout(p);
            if let Some(s) = self.open.as_mut().and_then(|o| o.suggest.as_mut()) {
                if let Some(d) = &mut s.drag {
                    let by = (at.0 - d.from.0, at.1 - d.from.1);
                    d.now = dragged(d.start, d.grip, by, MIN_SIDE * zoom);
                }
                if let Some(drawing) = &mut s.drawing {
                    drawing.2 = at;
                }
            }
            ui.ctx().request_repaint();
        }
        if response.drag_stopped() {
            self.drop_drag();
            self.drop_drawing();
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let what = self.hold_at(to_layout(p)).map(|h| h.0);
            self.pick(what);
            if response.double_clicked()
                && let Some(Picked::Suggestion(i)) = what
            {
                self.add_suggestions(&[i]);
            }
        }
        true
    }

    // ----- Drawing ------------------------------------------------------

    /// Tables, suggestions and the picked box, drawn over the pages.
    pub(super) fn draw_suggestions(&self, ui: &egui::Ui, origin: Pos2) {
        let Some(o) = &self.open else { return };
        let Some(s) = &o.suggest else { return };
        let shift = self.centre_shift();
        let on_screen = |b: Box4| {
            Rect::from_min_max(
                origin + Vec2::new((b[0] + shift) as f32, b[1] as f32),
                origin + Vec2::new((b[2] + shift) as f32, b[3] as f32),
            )
        };
        let view = Rect::from_min_size(origin + self.offset, self.view_size);
        let painter = ui.painter();
        for (page, table) in &s.tables {
            let Some(r) = self
                .layout_box(*page, &table.rect)
                .map(on_screen)
                .filter(|r| r.intersects(view))
            else {
                continue;
            };
            painter.rect_stroke(
                r.expand(2.0),
                0.0,
                Stroke::new(1.5, TABLE_EDGE),
                StrokeKind::Outside,
            );
            painter.text(
                r.left_top() + Vec2::new(-2.0, -4.0),
                Align2::LEFT_BOTTOM,
                format!("Table, {} × {}", table.rows, table.cols),
                FontId::proportional(11.0),
                TABLE_EDGE,
            );
        }
        for (i, item) in s.items.iter().enumerate() {
            let what = Picked::Suggestion(i);
            let Some(r) = self
                .picked_box(what)
                .map(|b| on_screen(b.1))
                .filter(|r| r.intersects(view))
            else {
                continue;
            };
            painter.rect_filled(r, 0.0, SUGGESTED);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, SUGGESTED_EDGE), StrokeKind::Inside);
            if r.height() >= 9.0 && r.width() >= 30.0 {
                painter
                    .with_clip_rect(r.intersect(painter.clip_rect()))
                    .text(
                        r.left_center() + Vec2::new(3.0, 0.0),
                        Align2::LEFT_CENTER,
                        &item.field.name,
                        FontId::proportional((r.height() * 0.7).clamp(8.0, 11.0)),
                        SUGGESTED_EDGE,
                    );
            }
        }
        if let Some(r) = s
            .picked
            .and_then(|what| self.picked_box(what))
            .map(|b| on_screen(b.1))
        {
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, PICKED), StrokeKind::Outside);
            let movable = s.picked.is_some_and(|what| self.movable(what));
            if movable && r.width().min(r.height()) >= 16.0 {
                for x in [r.left(), r.center().x, r.right()] {
                    for y in [r.top(), r.center().y, r.bottom()] {
                        if x == r.center().x && y == r.center().y {
                            continue;
                        }
                        let knob = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(6.0));
                        painter.rect_filled(knob, 1.0, Color32::WHITE);
                        painter.rect_stroke(
                            knob,
                            1.0,
                            Stroke::new(1.0, PICKED),
                            StrokeKind::Inside,
                        );
                    }
                }
            }
        }
        if let Some((_, a, b)) = s.drawing {
            let r = on_screen([a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]);
            painter.rect_filled(r, 0.0, SUGGESTED);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, PICKED), StrokeKind::Inside);
        }
    }

    /// The bar along the bottom while suggestions are reviewed.
    pub(super) fn fields_bar(&mut self, ui: &mut egui::Ui) {
        if !self.reviewing_fields() {
            return;
        }
        let page = self.current_page();
        let preferred = self.settings.check_mark;
        let Some(o) = &mut self.open else { return };
        let session = &o.session;
        let document_mark = session.document_mark();
        let form = o.form.as_ref().map(|fv| &fv.form);
        let Some(s) = &mut o.suggest else { return };
        // The name box follows what is picked.
        if s.name_of != s.picked {
            s.name_of = s.picked;
            s.name = match s.picked {
                Some(Picked::Suggestion(i)) => s.items.get(i).map(|it| it.field.name.clone()),
                Some(Picked::Field(f, _)) => form
                    .and_then(|form| form.fields.get(f))
                    .map(|f| f.name.clone()),
                None => None,
            }
            .unwrap_or_default();
        }
        let mut ask = None;
        egui::Panel::bottom("fields-tool").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let n = s.items.len();
                let here = s.items.iter().filter(|i| i.page == page).count();
                if s.running {
                    ui.spinner();
                    ui.label(format!(
                        "Looking for fields: page {} of {}…",
                        (s.pages_done + 1).min(s.pages),
                        s.pages
                    ));
                }
                ui.label(match n {
                    0 if s.running => String::new(),
                    0 => "No suggested fields. Drag on a page to draw one.".into(),
                    1 => "1 suggested field.".into(),
                    n => format!("{n} suggested fields."),
                });
                if ui
                    .add_enabled(n > 0, egui::Button::new(format!("Add all ({n})")))
                    .on_hover_text("Make a field of every suggestion")
                    .clicked()
                {
                    ask = Some(Ask::AddAll);
                }
                if here > 0 && here < n && ui.button(format!("Add this page's ({here})")).clicked()
                {
                    ask = Some(Ask::AddPage(page));
                }
                if !s.tables.is_empty() {
                    ui.menu_button(format!("Tables ({})", s.tables.len()), |ui| {
                        for (i, (page, table)) in s.tables.iter().enumerate() {
                            let label =
                                format!("Page {}: {} × {}", page + 1, table.rows, table.cols);
                            ui.horizontal(|ui| {
                                if ui.button(label).on_hover_text("Go to this table").clicked() {
                                    ask = Some(Ask::ShowTable(i));
                                    ui.close();
                                }
                                if ui
                                    .small_button("Copy")
                                    .on_hover_text(
                                        "Copy the table's text, to paste into a spreadsheet",
                                    )
                                    .clicked()
                                {
                                    ask = Some(Ask::CopyTable(i));
                                    ui.close();
                                }
                            });
                        }
                    });
                }
                ui.menu_button("Check marks", |ui| {
                    ui.label("New check boxes in this document show a");
                    let mine = format!("{} (from Settings)", preferred.label().to_lowercase());
                    if ui.radio(document_mark.is_none(), mine).clicked() {
                        ask = Some(Ask::DocumentMark(None));
                        ui.close();
                    }
                    for mark in CheckMark::ALL {
                        if ui
                            .radio(document_mark == Some(mark), mark.label().to_lowercase())
                            .on_hover_text("Kept in this file")
                            .clicked()
                        {
                            ask = Some(Ask::DocumentMark(Some(mark)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    let mark = document_mark.unwrap_or(preferred);
                    if ui
                        .button(format!(
                            "Give every check box a {}",
                            mark.label().to_lowercase()
                        ))
                        .on_hover_text(
                            "Draws every check box in this document that can be filled in \
                             again with this mark, in place of the look it has",
                        )
                        .clicked()
                    {
                        ask = Some(Ask::MarkAll(mark));
                        ui.close();
                    }
                });
                if ui
                    .add_enabled(!s.running, egui::Button::new("Look again"))
                    .on_hover_text("Search the pages again; suggestions not added are replaced")
                    .clicked()
                {
                    ask = Some(Ask::Again);
                }
                ui.separator();
                if ui
                    .button("Done (Esc)")
                    .on_hover_text("Go back to filling in; suggestions not added are dropped")
                    .clicked()
                {
                    ask = Some(Ask::Close);
                }
            });
            ui.horizontal_wrapped(|ui| match s.picked {
                Some(Picked::Suggestion(i)) if i < s.items.len() => {
                    ui.label("Name");
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut s.name)
                            .id_salt("suggested-name")
                            .desired_width(180.0),
                    );
                    if name.changed() {
                        s.items[i].field.name = s.name.clone();
                    }
                    let mut kind = s.items[i].field.kind;
                    egui::ComboBox::from_id_salt("suggested-kind")
                        .selected_text(kind.label())
                        .show_ui(ui, |ui| {
                            for k in NewFieldKind::ALL {
                                ui.selectable_value(&mut kind, k, k.label());
                            }
                        });
                    if kind != s.items[i].field.kind {
                        ask = Some(Ask::Kind(i, kind));
                    }
                    if kind == NewFieldKind::Checkbox {
                        let shown = s.items[i].field.mark.or(document_mark).unwrap_or(preferred);
                        if let Some(mark) = mark_picker(ui, "suggested-mark", Some(shown)) {
                            s.items[i].field.mark = Some(mark);
                        }
                    } else if kind.is_text()
                        && let Some(size) =
                            size_picker(ui, "suggested-size", s.items[i].field.font_size)
                    {
                        s.items[i].field.font_size = size;
                    }
                    if ui.button("Add field (Enter)").clicked() {
                        ask = Some(Ask::Add(i));
                    }
                    if ui.button("Dismiss (Del)").clicked() {
                        ask = Some(Ask::Dismiss(i));
                    }
                    ui.weak(match s.items[i].source {
                        Some(source) => format!(
                            "Found from a {}. Drag it to move it, its edges to resize it.",
                            source.label()
                        ),
                        None => {
                            "Drawn by hand. Drag it to move it, its edges to resize it.".to_string()
                        }
                    });
                }
                Some(Picked::Field(f, _)) => {
                    let added = session.is_added_field(f);
                    let field = form.and_then(|form| form.fields.get(f));
                    let locked = field.is_none_or(|f| f.is_read_only());
                    ui.label("Field");
                    let kind = field.map(|f| f.kind);
                    let typed = matches!(
                        kind,
                        Some(FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox)
                    );
                    if added {
                        let name = ui.add(
                            egui::TextEdit::singleline(&mut s.name)
                                .id_salt("added-name")
                                .desired_width(180.0),
                        );
                        if name.lost_focus()
                            && field.is_some_and(|f| f.name != s.name)
                            && !s.name.trim().is_empty()
                        {
                            ask = Some(Ask::Rename(f, s.name.clone()));
                        }
                        if ui.button("Remove (Del)").clicked() {
                            ask = Some(Ask::Remove(f));
                        }
                        ui.weak("Drag it to move it, its edges to resize it.");
                    } else {
                        ui.strong(field.map_or("", |f| f.name.as_str()));
                        ui.weak(if locked {
                            "This field is read-only, so it stays where it is."
                        } else {
                            "Drag it to move it, its edges to resize it. Fields saved in \
                             the file keep their names."
                        });
                    }
                    if !locked && kind == Some(FieldKind::Checkbox) {
                        if let Some(mark) = mark_picker(ui, "field-mark", session.check_mark(f)) {
                            ask = Some(Ask::Mark(f, mark));
                        }
                    } else if !locked && typed {
                        let size = session.field_style(f).map_or(0.0, |s| s.appearance.size);
                        if let Some(size) = size_picker(ui, "field-size", size) {
                            ask = Some(Ask::Size(f, size));
                        }
                    }
                }
                _ => {
                    ui.weak(
                        "Click a box to pick it (Tab goes to the next), double-click to add \
                         it, or drag on the page to draw a field.",
                    );
                }
            });
        });
        match ask {
            Some(Ask::AddAll) => {
                let all: Vec<usize> = (0..s.items.len()).collect();
                self.add_suggestions(&all);
            }
            Some(Ask::AddPage(page)) => {
                let here: Vec<usize> = s
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| it.page == page)
                    .map(|(i, _)| i)
                    .collect();
                self.add_suggestions(&here);
            }
            Some(Ask::Add(i)) => {
                self.add_suggestions(&[i]);
                self.reveal_picked();
            }
            Some(Ask::Dismiss(i)) => {
                self.dismiss_suggestion(i);
                self.reveal_picked();
            }
            Some(Ask::Kind(i, kind)) => s.items[i].field.kind = kind,
            Some(Ask::Rename(f, name)) => self.rename_added(f, &name),
            Some(Ask::Remove(f)) => self.remove_added(f),
            Some(Ask::Size(f, size)) => self.set_text_size(f, size),
            Some(Ask::Mark(f, mark)) => self.set_mark(f, mark),
            Some(Ask::DocumentMark(mark)) => self.set_document_mark(mark),
            Some(Ask::MarkAll(mark)) => self.set_every_mark(mark),
            Some(Ask::CopyTable(i)) => {
                let text = s.tables[i].1.to_tsv();
                self.ctx.copy_text(text);
                self.notice = Some("Copied the table; paste it into a spreadsheet.".into());
            }
            Some(Ask::ShowTable(i)) => {
                let (page, rect) = (s.tables[i].0, s.tables[i].1.rect);
                if let Some(r) = self.user_rect(page, &rect, Pos2::ZERO) {
                    self.scroll_to = Some(Vec2::new(
                        (r.center().x - self.view_size.x / 2.0).max(0.0),
                        (r.top() - 40.0).max(0.0),
                    ));
                }
            }
            Some(Ask::Again) => self.start_detect(),
            Some(Ask::Close) => self.close_detect(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use atelier_form::{FieldKind, Value};

    /// A page with "Name:" and a line to write on, and a square to tick
    /// with "Agree" beside it.
    const PAGE: &str = "BT /F1 10 Tf 40 162 Td (Name:) Tj ET\n\
        0.5 w 80 160 m 180 160 l S\n\
        40 120 10 10 re S\n\
        BT /F1 10 Tf 56 121 Td (Agree) Tj ET";

    fn file_of(content: &str) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f\r\n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }

    fn viewer() -> Viewer {
        viewer_of(PAGE)
    }

    fn viewer_of(content: &str) -> Viewer {
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
        viewer.open_with_password(file_of(content), "blank-form.pdf".into(), "", None);
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        viewer
    }

    /// Runs detection and waits for the worker to finish.
    fn detect(viewer: &mut Viewer) {
        viewer.start_detect();
        for _ in 0..2000 {
            viewer.receive();
            let s = viewer.open.as_ref().unwrap().suggest.as_ref().unwrap();
            if !s.running {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("detection did not finish");
    }

    fn suggest(viewer: &Viewer) -> &Suggest {
        viewer.open.as_ref().unwrap().suggest.as_ref().unwrap()
    }

    #[test]
    fn suggestions_become_fields_that_can_be_filled_and_changed() {
        let mut viewer = viewer();
        assert!(viewer.open.as_ref().unwrap().form.is_none());
        detect(&mut viewer);
        let names: Vec<&str> = suggest(&viewer)
            .items
            .iter()
            .map(|i| i.field.name.as_str())
            .collect();
        assert_eq!(names, ["Name", "Agree"]);

        // One at a time: the next suggestion is then picked.
        assert_eq!(viewer.add_suggestions(&[0]), 1);
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        assert_eq!(suggest(&viewer).items.len(), 1);
        assert_eq!(suggest(&viewer).picked, Some(Picked::Suggestion(0)));
        let all: Vec<usize> = (0..suggest(&viewer).items.len()).collect();
        assert_eq!(viewer.add_suggestions(&all), 1);
        assert!(suggest(&viewer).items.is_empty());

        // The file now has a form, which fills in as usual.
        let o = viewer.open.as_ref().unwrap();
        let form = &o.form.as_ref().unwrap().form;
        assert_eq!(form.fields.len(), 2);
        assert_eq!(form.fields[1].kind, FieldKind::Checkbox);
        assert_eq!(o.form.as_ref().unwrap().tab.len(), 2);
        assert!(o.session.is_dirty());
        viewer.commit(0, Value::Text("Ada".into()));
        viewer.commit(1, Value::State("Yes".into()));
        let reopened =
            atelier_syntax::Document::open(viewer.open.as_ref().unwrap().doc.data().to_vec())
                .unwrap();
        let saved = atelier_form::Form::load(&reopened).unwrap().unwrap();
        assert_eq!(saved.fields[0].value, Value::Text("Ada".into()));
        assert_eq!(saved.fields[1].value, Value::State("Yes".into()));

        // Looking again leaves the places that now have fields alone.
        detect(&mut viewer);
        assert!(suggest(&viewer).items.is_empty());

        // Added fields are renamed and removed until saved.
        viewer.rename_added(0, "Agree");
        let name = |v: &Viewer, i: usize| {
            v.open.as_ref().unwrap().form.as_ref().unwrap().form.fields[i]
                .name
                .clone()
        };
        assert_eq!(name(&viewer, 0), "Agree 2");
        viewer.remove_added(0);
        assert_eq!(name(&viewer, 0), "Agree");
        viewer.remove_added(0);
        let o = viewer.open.as_ref().unwrap();
        assert!(o.form.is_none() && !o.session.is_dirty());
        viewer.close_detect();
        assert!(!viewer.reviewing_fields());
    }

    #[test]
    fn boxes_are_moved_resized_and_drawn_in_page_coordinates() {
        let mut viewer = viewer();
        detect(&mut viewer);
        let before = suggest(&viewer).items[0].field.rect;
        let (page, start) = viewer.picked_box(Picked::Suggestion(0)).unwrap();
        let zoom = viewer.zoom();
        // Dragged right and down by 10 points; the page's y runs up.
        let s = viewer.open.as_mut().unwrap().suggest.as_mut().unwrap();
        s.picked = Some(Picked::Suggestion(0));
        s.drag = Some(Drag {
            what: Picked::Suggestion(0),
            page,
            grip: Grip::default(),
            from: (0.0, 0.0),
            start,
            now: dragged(start, Grip::default(), (10.0 * zoom, 10.0 * zoom), 1.0),
        });
        viewer.drop_drag();
        let after = suggest(&viewer).items[0].field.rect;
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(near(after.x0, before.x0 + 10.0) && near(after.x1, before.x1 + 10.0));
        assert!(near(after.y0, before.y0 - 10.0) && near(after.y1, before.y1 - 10.0));

        // A box drawn by hand is a suggestion like the others.
        let corner = |x: f64, y: f64| {
            let p = viewer
                .open
                .as_ref()
                .unwrap()
                .layout
                .user_to_layout(0)
                .unwrap()
                .apply(Point::new(x, y));
            (p.x, p.y)
        };
        let (a, b) = (corner(40.0, 60.0), corner(52.0, 72.0));
        viewer
            .open
            .as_mut()
            .unwrap()
            .suggest
            .as_mut()
            .unwrap()
            .drawing = Some((0, a, b));
        viewer.drop_drawing();
        let s = suggest(&viewer);
        assert_eq!(s.items.len(), 3);
        assert_eq!(s.picked, Some(Picked::Suggestion(2)));
        let drawn = &s.items[2];
        assert_eq!(drawn.field.kind, NewFieldKind::Checkbox);
        assert_eq!(drawn.field.name, "Check box 1");
        assert!(drawn.source.is_none());
        assert!(near(drawn.field.rect.x0, 40.0) && near(drawn.field.rect.y1, 72.0));

        // An added field moves through the session, and so is saved.
        assert_eq!(viewer.add_suggestions(&[2]), 1);
        let (page, start) = viewer.picked_box(Picked::Field(0, 0)).unwrap();
        let s = viewer.open.as_mut().unwrap().suggest.as_mut().unwrap();
        s.drag = Some(Drag {
            what: Picked::Field(0, 0),
            page,
            grip: Grip {
                right: true,
                ..Grip::default()
            },
            from: (0.0, 0.0),
            start,
            now: dragged(
                start,
                Grip {
                    right: true,
                    ..Grip::default()
                },
                (8.0 * zoom, 0.0),
                1.0,
            ),
        });
        viewer.drop_drag();
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        let o = viewer.open.as_ref().unwrap();
        let rect = o.form.as_ref().unwrap().form.fields[0].widgets[0].rect;
        assert!(near(rect.x0, 40.0) && near(rect.x1, 60.0), "{rect:?}");
    }

    /// Runs a frame for each batch of events over a page area with the
    /// review bar, as the window does, and returns the area's corner.
    fn frames(viewer: &mut Viewer, batches: Vec<Vec<egui::Event>>) -> Pos2 {
        let ctx = viewer.ctx.clone();
        let mut origin = Pos2::ZERO;
        for events in batches {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 900.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    viewer.fields_bar(ui);
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(800.0, 600.0),
                        egui::Sense::click_and_drag(),
                    );
                    origin = rect.min;
                    viewer.draw_suggestions(ui, rect.min);
                    viewer.detect_pointer(ui, &response, rect.min);
                },
            );
            output.textures_delta.clear();
        }
        origin
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn the_pointer_picks_moves_and_draws_boxes() {
        let mut viewer = viewer();
        detect(&mut viewer);
        let origin = frames(&mut viewer, vec![Vec::new()]);
        let shift = viewer.centre_shift();
        let on_screen = |v: &Viewer, x: f64, y: f64| {
            let p = v
                .open
                .as_ref()
                .unwrap()
                .layout
                .user_to_layout(0)
                .unwrap()
                .apply(Point::new(x, y));
            origin + Vec2::new((p.x + shift) as f32, p.y as f32)
        };
        // A click on the line to write the name on picks its suggestion.
        let name = on_screen(&viewer, 130.0, 166.0);
        frames(
            &mut viewer,
            vec![
                vec![egui::Event::PointerMoved(name)],
                vec![press(name, true)],
                vec![press(name, false)],
            ],
        );
        assert_eq!(suggest(&viewer).picked, Some(Picked::Suggestion(0)));

        // Dragging it moves it; the others stay.
        let before = suggest(&viewer).items[0].field.rect;
        let zoom = viewer.zoom();
        let to = name + Vec2::new(40.0, 0.0);
        frames(
            &mut viewer,
            vec![
                vec![press(name, true)],
                vec![egui::Event::PointerMoved(name + Vec2::new(20.0, 0.0))],
                vec![egui::Event::PointerMoved(to)],
                vec![press(to, false)],
            ],
        );
        let after = suggest(&viewer).items[0].field.rect;
        assert!(
            (after.x0 - before.x0 - 40.0 / zoom).abs() < 0.01,
            "{before:?} {after:?}"
        );
        assert!((after.x1 - after.x0 - (before.x1 - before.x0)).abs() < 0.01);
        assert_eq!((after.y0, after.y1), (before.y0, before.y1));
        assert_eq!(suggest(&viewer).items.len(), 2);

        // Dragging on bare page draws a field, which is then picked.
        let (a, b) = (
            on_screen(&viewer, 40.0, 60.0),
            on_screen(&viewer, 140.0, 46.0),
        );
        frames(
            &mut viewer,
            vec![
                vec![egui::Event::PointerMoved(a)],
                vec![press(a, true)],
                vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
                vec![egui::Event::PointerMoved(b)],
                vec![press(b, false)],
            ],
        );
        let s = suggest(&viewer);
        assert_eq!(s.items.len(), 3);
        assert_eq!(s.picked, Some(Picked::Suggestion(2)));
        let drawn = &s.items[2].field;
        assert_eq!(drawn.kind, NewFieldKind::Text);
        let r = drawn.rect;
        assert!(
            (r.x0 - 40.0).abs() < 0.5 && (r.x1 - 140.0).abs() < 0.5,
            "{r:?}"
        );
        assert!(
            (r.y0 - 46.0).abs() < 0.5 && (r.y1 - 60.0).abs() < 0.5,
            "{r:?}"
        );

        // A double click adds the suggestion under it. (Some time passes
        // first, or it would count as a third click after the ones above.)
        let agree = on_screen(&viewer, 45.0, 125.0);
        frames(&mut viewer, vec![Vec::new(); 60]);
        frames(
            &mut viewer,
            vec![
                vec![egui::Event::PointerMoved(agree)],
                vec![press(agree, true)],
                vec![press(agree, false)],
                vec![press(agree, true)],
                vec![press(agree, false)],
            ],
        );
        assert_eq!(
            suggest(&viewer).items.len(),
            2,
            "{:?} {:?}",
            viewer.error,
            suggest(&viewer).picked
        );
        let o = viewer.open.as_ref().unwrap();
        let form = &o.form.as_ref().unwrap().form;
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].name, "Agree");
        // The bar and the overlay draw with a field picked, too.
        viewer.pick(Some(Picked::Field(0, 0)));
        frames(&mut viewer, vec![Vec::new()]);
    }

    /// A line to sign, a line for a date and a square to tick.
    const SIGN: &str = "BT /F1 10 Tf 20 162 Td (Signature:) Tj ET
        0.5 w 80 160 m 180 160 l S
        BT /F1 10 Tf 20 102 Td (Date:) Tj ET
        50 100 m 150 100 l S
        40 60 10 10 re S
        BT /F1 10 Tf 56 61 Td (Agree) Tj ET";

    #[test]
    fn signatures_marks_and_text_sizes_can_be_set() {
        let mut viewer = viewer_of(SIGN);
        viewer.settings.check_mark = CheckMark::Cross;
        detect(&mut viewer);
        let kinds: Vec<NewFieldKind> = suggest(&viewer)
            .items
            .iter()
            .map(|i| i.field.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                NewFieldKind::Signature,
                NewFieldKind::Text,
                NewFieldKind::Checkbox
            ]
        );
        // The bar draws with each kind of suggestion picked.
        for i in 0..3 {
            viewer.pick(Some(Picked::Suggestion(i)));
            frames(&mut viewer, vec![Vec::new()]);
        }
        assert_eq!(viewer.add_suggestions(&[0, 1, 2]), 3);
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        fn session(v: &Viewer) -> &atelier_form::Session {
            &v.open.as_ref().unwrap().session
        }
        // The check box took the mark preferred in Settings.
        assert_eq!(session(&viewer).check_mark(2), Some(CheckMark::Cross));
        viewer.set_mark(2, CheckMark::Square);
        assert_eq!(session(&viewer).check_mark(2), Some(CheckMark::Square));
        // The document's own mark wins over the preference from then on.
        viewer.set_document_mark(Some(CheckMark::Dot));
        let s = viewer.open.as_mut().unwrap().suggest.as_mut().unwrap();
        s.items.push(Item {
            page: 0,
            field: NewField::new(
                "Other",
                NewFieldKind::Checkbox,
                PdfRect {
                    x0: 100.0,
                    y0: 60.0,
                    x1: 110.0,
                    y1: 70.0,
                },
            ),
            source: None,
        });
        assert_eq!(viewer.add_suggestions(&[0]), 1);
        assert_eq!(session(&viewer).check_mark(3), Some(CheckMark::Dot));
        viewer.set_every_mark(CheckMark::Star);
        assert_eq!(session(&viewer).check_mark(2), Some(CheckMark::Star));
        assert_eq!(session(&viewer).check_mark(3), Some(CheckMark::Star));

        // Text is fitted to its box until a size is set, and can go back.
        let size = |v: &Viewer| session(v).field_style(1).unwrap().appearance.size;
        assert_eq!(size(&viewer), 0.0);
        viewer.set_text_size(1, 9.0);
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
        assert_eq!(size(&viewer), 9.0);
        let da = |v: &Viewer| {
            v.open.as_ref().unwrap().form.as_ref().unwrap().form.fields[1]
                .da
                .clone()
                .unwrap()
        };
        assert!(da(&viewer).contains("/Helv 9 Tf"), "{}", da(&viewer));
        viewer.set_text_size(1, 0.0);
        assert_eq!(size(&viewer), 0.0);
        // The bar draws with a check box and a text field picked.
        for f in [1, 2] {
            viewer.pick(Some(Picked::Field(f, 0)));
            frames(&mut viewer, vec![Vec::new()]);
        }
        viewer.close_detect();

        // A click on the signature field signs it in ink, sized to fit.
        let layout_point = |v: &Viewer, x: f64, y: f64| {
            let p = v
                .open
                .as_ref()
                .unwrap()
                .layout
                .user_to_layout(0)
                .unwrap()
                .apply(Point::new(x, y));
            (p.x, p.y)
        };
        let (lx, ly) = layout_point(&viewer, 130.0, 170.0);
        let (page, rect) = viewer.unsigned_at(lx, ly).unwrap();
        assert_eq!(page, 0);
        let (dx, dy) = layout_point(&viewer, 100.0, 105.0);
        assert!(viewer.unsigned_at(dx, dy).is_none());
        viewer.annotate.use_signature(vec![vec![
            Point::new(0.0, 0.0),
            Point::new(0.5, 0.4),
            Point::new(1.0, 0.1),
        ]]);
        assert!(viewer.sign_click(lx, ly));
        let o = viewer.open.as_ref().unwrap();
        let marks = o.session.added_annots();
        assert_eq!(marks.len(), 1);
        let [x0, y0, x1, y1] = marks[0].1.rect();
        assert!(
            x0 >= rect.x0 && x1 <= rect.x1 && y0 >= rect.y0 && y1 <= rect.y1,
            "{:?} in {rect:?}",
            [x0, y0, x1, y1]
        );
        assert!(x1 - x0 > 20.0);
        assert!(!viewer.sign_click(dx, dy));
    }

    #[test]
    fn edges_resize_and_the_middle_moves() {
        let b = [100.0, 100.0, 200.0, 140.0];
        assert_eq!(grip_at(b, (150.0, 120.0), 5.0), Some(Grip::default()));
        assert!(grip_at(b, (210.0, 120.0), 5.0).is_none());
        let corner = grip_at(b, (101.0, 139.0), 5.0).unwrap();
        assert!(corner.left && corner.bottom && !corner.right && !corner.top);
        assert_eq!(corner.cursor(), CursorIcon::ResizeNeSw);
        let right = grip_at(b, (203.0, 120.0), 5.0).unwrap();
        assert_eq!(right.cursor(), CursorIcon::ResizeHorizontal);
        assert_eq!(
            dragged(b, right, (20.0, 9.0), 4.0),
            [100.0, 100.0, 220.0, 140.0]
        );
        // A side never passes the one across from it.
        assert_eq!(
            dragged(b, right, (-500.0, 0.0), 4.0),
            [100.0, 100.0, 104.0, 140.0]
        );
        assert_eq!(
            dragged(b, Grip::default(), (5.0, -5.0), 4.0),
            [105.0, 95.0, 205.0, 135.0]
        );
        // A small box is moved from inside, resized from just outside.
        let small = [100.0, 100.0, 110.0, 110.0];
        assert_eq!(grip_at(small, (105.0, 105.0), 5.0), Some(Grip::default()));
        assert!(grip_at(small, (113.0, 105.0), 5.0).unwrap().right);
    }

    #[test]
    fn a_drawn_box_guesses_its_kind() {
        let r = |w: f64, h: f64| PdfRect {
            x0: 0.0,
            y0: 0.0,
            x1: w,
            y1: h,
        };
        assert_eq!(kind_for(&r(12.0, 12.0), 0), NewFieldKind::Checkbox);
        assert_eq!(kind_for(&r(120.0, 14.0), 0), NewFieldKind::Text);
        assert_eq!(kind_for(&r(120.0, 60.0), 0), NewFieldKind::Multiline);
        // On a page turned a quarter, a line of text runs up the page.
        assert_eq!(kind_for(&r(14.0, 120.0), 90), NewFieldKind::Text);
    }
}
