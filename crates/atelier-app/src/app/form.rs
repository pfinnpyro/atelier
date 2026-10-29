//! Filling in forms: field highlights, the editors that open over a field,
//! keyboard focus and Tab order, and rebuilding the document after each
//! change. Push buttons do nothing: their scripts are never run.

use std::collections::HashSet;

use atelier_form::{Field, FieldKind, Form, Session, Value, flags};
use atelier_raster::Point;
use eframe::egui::{
    self, Align, Color32, CursorIcon, FocusDirection, FontId, Key, Modifiers, Pos2, Rect, Stroke,
    StrokeKind, Vec2,
};

use super::{Open, Viewer};

/// A light blue wash (premultiplied: 60, 120, 255 at 16% opacity).
const FIELD_TINT: Color32 = Color32::from_rgba_premultiplied(10, 19, 41, 41);
const FOCUS_RING: Color32 = Color32::from_rgb(40, 110, 230);
/// Line spacing of list boxes, matching the drawn appearance.
const LIST_LINE: f64 = 1.15;

/// The form of the open document and what is being filled in.
pub(super) struct FormView {
    pub session: Session,
    /// Field values as now shown, including unsaved changes.
    pub form: Form,
    tab: Vec<(usize, usize)>,
    /// The widget with keyboard focus: (field, widget).
    pub focus: Option<(usize, usize)>,
    editor: Option<Editor>,
    /// A combo box's option list is open, since this frame.
    combo: Option<((usize, usize), u64)>,
    pub highlight: bool,
}

/// Text being typed into a field, not yet committed.
struct Editor {
    at: (usize, usize),
    text: String,
    multiline: bool,
    focus_pending: bool,
}

impl FormView {
    pub fn new(session: Session, form: Form, doc: &atelier_syntax::Document) -> Self {
        let tab = form.tab_order(doc);
        Self {
            session,
            form,
            tab,
            focus: None,
            editor: None,
            combo: None,
            highlight: true,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.session.is_dirty()
    }

    fn field(&self, at: (usize, usize)) -> Option<&Field> {
        self.form.fields.get(at.0)
    }

    /// The next (or previous) widget in Tab order from the focused one.
    fn step(&self, back: bool) -> Option<(usize, usize)> {
        let n = self.tab.len();
        if n == 0 {
            return None;
        }
        let i = match self
            .focus
            .and_then(|f| self.tab.iter().position(|t| *t == f))
        {
            None if back => n - 1,
            None => 0,
            Some(i) if back => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        Some(self.tab[i])
    }
}

/// What a click or key does to a field.
enum Act {
    Edit((usize, usize)),
    OpenCombo((usize, usize)),
    Set(usize, Value),
}

impl Viewer {
    /// Reads the form of a newly opened document. Files that ask viewers
    /// to redraw their fields get redrawn here, before anything is shown.
    pub(super) fn load_form(o: &mut Open) {
        let session = match Session::new(o.doc.clone()) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                o.warnings.insert(format!("Form: {e}"));
                return;
            }
        };
        match session.build() {
            Ok(built) => {
                o.warnings
                    .extend(built.warnings.iter().map(|w| format!("Form: {w}")));
                o.doc = built.doc;
                o.form = Some(FormView::new(session, built.form, &o.doc));
            }
            Err(e) => {
                o.warnings.insert(format!("Form: {e}"));
            }
        }
    }

    /// Records a field's new value and swaps in the rebuilt document. Tiles
    /// of the pages the field is on stay up until fresh ones arrive.
    fn commit(&mut self, field: usize, value: Value) {
        let Some(o) = &mut self.open else { return };
        let Some(fv) = &mut o.form else { return };
        if fv.form.fields[field].value.same_as(&value) {
            return;
        }
        fv.session.set(field, value);
        let built = match fv.session.build() {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Could not fill in the field: {e}"));
                return;
            }
        };
        if let Some(w) = built.warnings.first() {
            self.error = Some(w.to_string());
        }
        fv.form = built.form;
        fv.tab = fv.form.tab_order(&built.doc);
        o.doc = built.doc;
        let pages: HashSet<usize> = fv.session.pages_of(field).into_iter().collect();
        let stale: Vec<_> = pages
            .iter()
            .flat_map(|p| o.tiles.page_tiles(*p).map(|(k, _)| *k).collect::<Vec<_>>())
            .collect();
        o.stale.extend(stale);
        o.failed.retain(|k| !pages.contains(&k.page));
        // Work queued for the old document is dropped when it comes back.
        self.generation += 1;
        o.generation = self.generation;
        self.update_title();
    }

    pub(super) fn update_title(&self) {
        let Some(o) = &self.open else { return };
        let dirty = o.form.as_ref().is_some_and(FormView::is_dirty);
        let mark = if dirty { "• " } else { "" };
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "{mark}{} - Atelier",
                o.name
            )));
    }

    /// The fillable, writable widget under a layout point.
    pub(super) fn widget_at(&self, lx: f64, ly: f64) -> Option<(usize, usize)> {
        let o = self.open.as_ref()?;
        let fv = o.form.as_ref()?;
        let (page, _, _) = o.layout.page_at(lx, ly)?;
        let user = o.layout.to_user(page, lx, ly)?;
        let at = fv.form.widget_at(page, user.x, user.y)?;
        let f = fv.field(at)?;
        (f.is_fillable() && !f.is_read_only()).then_some(at)
    }

    /// A widget's box on screen, given the content origin.
    fn widget_rect(&self, at: (usize, usize), origin: Pos2) -> Option<Rect> {
        let o = self.open.as_ref()?;
        let w = o.form.as_ref()?.field(at)?.widgets.get(at.1)?;
        let t = o.layout.user_to_layout(w.page?)?;
        let shift = self.centre_shift() as f32;
        let mut r = Rect::NOTHING;
        for (x, y) in [
            (w.rect.x0, w.rect.y0),
            (w.rect.x1, w.rect.y0),
            (w.rect.x1, w.rect.y1),
            (w.rect.x0, w.rect.y1),
        ] {
            let p = t.apply(Point::new(x, y));
            r.extend_with(origin + Vec2::new(p.x as f32 + shift, p.y as f32));
        }
        r.is_positive().then_some(r)
    }

    /// A click on the page: fills or opens the field under it. Returns
    /// whether a field took the click.
    pub(super) fn form_click(&mut self, lx: f64, ly: f64, ctrl: bool) -> bool {
        let Some(at) = self.widget_at(lx, ly) else {
            self.finish_editing(true);
            if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
                fv.focus = None;
                fv.combo = None;
            }
            return false;
        };
        let (Some(o), point) = (&self.open, (lx, ly)) else {
            return false;
        };
        let act = {
            let fv = o.form.as_ref().expect("widget_at found a form");
            let f = &fv.form.fields[at.0];
            match f.kind {
                FieldKind::ListBox => {
                    let page = f.widgets[at.1].page.unwrap_or(0);
                    o.layout
                        .to_user(page, point.0, point.1)
                        .and_then(|u| list_row(f, at.1, u.y))
                        .map(|row| Act::Set(at.0, list_pick(f, row, ctrl)))
                }
                _ => activate(f, at),
            }
        };
        if self
            .open
            .as_ref()
            .and_then(|o| o.form.as_ref())
            .is_some_and(|fv| fv.editor.as_ref().is_some_and(|e| e.at != at))
        {
            self.finish_editing(true);
        }
        if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
            fv.focus = Some(at);
        }
        if let Some(act) = act {
            self.act(act);
        }
        true
    }

    fn act(&mut self, act: Act) {
        let frame = self.ctx.cumulative_frame_nr();
        match act {
            Act::Set(field, value) => self.commit(field, value),
            Act::Edit(at) => {
                let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) else {
                    return;
                };
                if fv.editor.as_ref().is_some_and(|e| e.at == at) {
                    return;
                }
                let f = &fv.form.fields[at.0];
                fv.editor = Some(Editor {
                    at,
                    text: f.value.display(),
                    multiline: f.has_flag(flags::MULTILINE),
                    focus_pending: true,
                });
            }
            Act::OpenCombo(at) => {
                if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
                    fv.combo = match fv.combo {
                        Some((c, _)) if c == at => None,
                        _ => Some((at, frame)),
                    };
                }
            }
        }
    }

    /// Ends typing: commits the text, or throws it away.
    fn finish_editing(&mut self, keep: bool) {
        let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) else {
            return;
        };
        let Some(ed) = fv.editor.take() else { return };
        if keep {
            self.commit(ed.at.0, Value::Text(ed.text));
        }
    }

    /// Form keys, before the page's own: Tab and Shift+Tab move between
    /// fields, Space toggles, Enter edits, arrows move in list boxes.
    /// Returns whether a key was used.
    pub(super) fn form_keys(&mut self) -> bool {
        let ctx = self.ctx.clone();
        let Some(fv) = self.open.as_ref().and_then(|o| o.form.as_ref()) else {
            return false;
        };
        if fv.tab.is_empty() {
            return false;
        }
        let editing = fv.editor.is_some();
        let tab = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        let back_tab = ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab));
        if tab || back_tab {
            // Keep egui from moving focus to the toolbar.
            ctx.memory_mut(|m| m.move_focus(FocusDirection::None));
            self.finish_editing(true);
            let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) else {
                return true;
            };
            fv.combo = None;
            fv.focus = fv.step(back_tab);
            if let Some(at) = fv.focus {
                let f = &fv.form.fields[at.0];
                if f.kind == FieldKind::Text
                    || f.kind == FieldKind::ComboBox && f.has_flag(flags::EDIT)
                {
                    self.act(Act::Edit(at));
                }
                self.reveal_widget(at);
            }
            return true;
        }
        if editing || ctx.text_edit_focused() {
            return false;
        }
        let Some(at) = fv.focus else { return false };
        let f = &fv.form.fields[at.0];
        let pressed = |k| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let act = match f.kind {
            FieldKind::Checkbox | FieldKind::Radio if pressed(Key::Space) => activate(f, at),
            FieldKind::Text | FieldKind::ComboBox if pressed(Key::Enter) || pressed(Key::Space) => {
                activate(f, at)
            }
            FieldKind::ListBox => {
                let current = f.options.iter().position(|o| match &f.value {
                    Value::Text(t) => *t == o.export,
                    Value::Choices(c) => c.first().is_some_and(|c| *c == o.export),
                    _ => false,
                });
                let next = if pressed(Key::ArrowDown) {
                    Some(current.map_or(0, |c| (c + 1).min(f.options.len().saturating_sub(1))))
                } else if pressed(Key::ArrowUp) {
                    Some(current.map_or(0, |c| c.saturating_sub(1)))
                } else {
                    None
                };
                next.filter(|_| !f.options.is_empty())
                    .map(|n| Act::Set(at.0, list_pick(f, n, false)))
            }
            _ => None,
        };
        if pressed(Key::Escape) {
            if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
                fv.focus = None;
                fv.combo = None;
            }
            return true;
        }
        match act {
            Some(a) => {
                self.act(a);
                true
            }
            None => false,
        }
    }

    /// Scrolls a widget into view.
    fn reveal_widget(&mut self, at: (usize, usize)) {
        let Some(r) = self.widget_rect(at, Pos2::ZERO) else {
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

    /// The cursor over a field.
    pub(super) fn form_cursor(&self, lx: f64, ly: f64) -> Option<CursorIcon> {
        let at = self.widget_at(lx, ly)?;
        let f = self.open.as_ref()?.form.as_ref()?.field(at)?;
        Some(match f.kind {
            FieldKind::Text => CursorIcon::Text,
            _ => CursorIcon::PointingHand,
        })
    }

    /// Field tints and the focus ring, drawn over the pages.
    pub(super) fn draw_fields(&self, ui: &egui::Ui, origin: Pos2) {
        let Some(o) = &self.open else { return };
        let Some(fv) = &o.form else { return };
        let painter = ui.painter();
        let view = Rect::from_min_size(origin + self.offset, self.view_size);
        if fv.highlight {
            for &at in &fv.tab {
                if let Some(r) = self.widget_rect(at, origin).filter(|r| r.intersects(view)) {
                    painter.rect_filled(r, 0.0, FIELD_TINT);
                }
            }
        }
        if let Some(r) = fv.focus.and_then(|at| self.widget_rect(at, origin)) {
            painter.rect_stroke(
                r.expand(1.5),
                1.0,
                Stroke::new(1.5, FOCUS_RING),
                StrokeKind::Outside,
            );
        }
    }

    /// The text box over a field being typed in.
    pub(super) fn draw_editor(&mut self, ui: &mut egui::Ui, origin: Pos2) {
        let zoom = self.zoom() as f32;
        let Some(at) = self
            .open
            .as_ref()
            .and_then(|o| o.form.as_ref())
            .and_then(|fv| fv.editor.as_ref().map(|e| e.at))
        else {
            return;
        };
        let Some(rect) = self.widget_rect(at, origin) else {
            return;
        };
        let Some(o) = &mut self.open else { return };
        let Some(fv) = &mut o.form else { return };
        let f = &fv.form.fields[at.0];
        let da = atelier_form::DefaultAppearance::parse(
            f.da.as_deref().or(fv.form.da.as_deref()).unwrap_or(""),
        );
        let height = rect.height() / zoom;
        let size = if da.size > 0.0 {
            da.size as f32
        } else if f.has_flag(flags::MULTILINE) {
            12.0
        } else {
            (height * 0.7).clamp(4.0, 24.0)
        };
        let align = match f.quadding {
            1 => Align::Center,
            2 => Align::Max,
            _ => Align::Min,
        };
        let max_len = f.max_len.filter(|&m| m > 0);
        let password = f.has_flag(flags::PASSWORD);
        let Some(ed) = &mut fv.editor else { return };
        // Escape cancels; the text box would otherwise just lose focus.
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
        let mut edit = if ed.multiline {
            egui::TextEdit::multiline(&mut ed.text)
        } else {
            egui::TextEdit::singleline(&mut ed.text)
        }
        .id_salt(("field-editor", at))
        .font(FontId::proportional(size * zoom))
        .text_color(Color32::BLACK)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric((2.0 * zoom) as i8, 0))
        .align(egui::Align2([
            align,
            if ed.multiline {
                Align::Min
            } else {
                Align::Center
            },
        ]))
        .password(password)
        .lock_focus(true)
        .desired_width(rect.width());
        if let Some(m) = max_len {
            edit = edit.char_limit(m);
        }
        let resp = ui.place(rect, edit);
        ui.painter()
            .rect_stroke(rect, 0.0, Stroke::new(1.0, FOCUS_RING), StrokeKind::Outside);
        if ed.focus_pending {
            resp.request_focus();
            ed.focus_pending = false;
            return;
        }
        if resp.lost_focus() {
            let keep = !escape;
            self.finish_editing(keep);
        }
    }

    /// A combo box's option list, under the field.
    pub(super) fn combo_popup(&mut self, ctx: &egui::Context, origin: Pos2) {
        let Some((at, opened)) = self
            .open
            .as_ref()
            .and_then(|o| o.form.as_ref())
            .and_then(|fv| fv.combo)
        else {
            return;
        };
        let Some(rect) = self.widget_rect(at, origin) else {
            return;
        };
        let Some(f) = self
            .open
            .as_ref()
            .and_then(|o| o.form.as_ref())
            .and_then(|fv| fv.form.fields.get(at.0))
            .cloned()
        else {
            return;
        };
        let mut chosen = None;
        let area = egui::Area::new(egui::Id::unique(("combo", at)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom())
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(rect.width().max(120.0));
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for o in &f.options {
                                let on = match &f.value {
                                    Value::Text(t) => *t == o.export || *t == o.display,
                                    Value::Choices(c) => c.contains(&o.export),
                                    _ => false,
                                };
                                if ui.selectable_label(on, &o.display).clicked() {
                                    chosen = Some(o.export.clone());
                                }
                            }
                        });
                });
            });
        let close = chosen.is_some()
            || ctx.input(|i| i.key_pressed(Key::Escape))
            || area.response.clicked_elsewhere() && ctx.cumulative_frame_nr() > opened;
        if close && let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
            fv.combo = None;
        }
        if let Some(v) = chosen {
            self.commit(at.0, Value::Text(v));
        }
    }

    /// Toggles field highlighting.
    pub(super) fn toggle_highlight(&mut self) {
        if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
            fv.highlight = !fv.highlight;
        }
    }
}

/// What clicking (or Space on) a widget does.
fn activate(f: &Field, at: (usize, usize)) -> Option<Act> {
    match f.kind {
        FieldKind::Text => Some(Act::Edit(at)),
        FieldKind::ComboBox if f.has_flag(flags::EDIT) => Some(Act::Edit(at)),
        FieldKind::ComboBox => Some(Act::OpenCombo(at)),
        FieldKind::Checkbox | FieldKind::Radio => {
            let on = f.widgets[at.1]
                .on_state
                .clone()
                .unwrap_or_else(|| "Yes".into());
            let is_on = matches!(&f.value, Value::State(s) if *s == on);
            let can_turn_off =
                f.kind == FieldKind::Checkbox || !f.has_flag(flags::NO_TOGGLE_TO_OFF);
            if is_on && !can_turn_off {
                return None;
            }
            Some(Act::Set(
                at.0,
                if is_on { Value::None } else { Value::State(on) },
            ))
        }
        _ => None,
    }
}

/// The option row of a list box at a height in user space.
fn list_row(f: &Field, widget: usize, y: f64) -> Option<usize> {
    let w = f.widgets.get(widget)?;
    let top = w.rect.y0.max(w.rect.y1);
    let size = {
        let da = atelier_form::DefaultAppearance::parse(f.da.as_deref().unwrap_or(""));
        if da.size > 0.0 { da.size } else { 12.0 }
    };
    let row = ((top - 1.0 - y) / (size * LIST_LINE)).floor();
    if row < 0.0 {
        return None;
    }
    let row = f.top_index + row as usize;
    (row < f.options.len()).then_some(row)
}

/// The value after picking option `row` of a list box: that option alone,
/// or with Ctrl in a multi-select list, the selection with it toggled.
fn list_pick(f: &Field, row: usize, ctrl: bool) -> Value {
    let picked = f.options[row].export.clone();
    if !(ctrl && f.has_flag(flags::MULTI_SELECT)) {
        return Value::Choices(vec![picked]);
    }
    let mut current: Vec<String> = match &f.value {
        Value::Text(t) => vec![t.clone()],
        Value::Choices(c) => c.clone(),
        _ => Vec::new(),
    };
    if let Some(i) = current.iter().position(|c| *c == picked) {
        current.remove(i);
    } else {
        current.push(picked);
    }
    // Keep the options' order.
    let order = |v: &String| f.options.iter().position(|o| o.export == *v);
    current.sort_by_key(order);
    Value::Choices(current)
}
