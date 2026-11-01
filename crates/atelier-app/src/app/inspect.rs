//! The inspector panel: the file at a glance, its objects and streams, its
//! form fields, and every script and action it carries. Scripts are shown
//! as text; approved form and click scripts run only through the helper sandbox.

use atelier_form::{Field, FieldKind, Value, flags};
use atelier_syntax::{Document, ObjRef, Object};
use atelier_view::{Fact, Picked, children, has_children, object_at, overview, preview, summary};
use eframe::egui::{self, Color32, CursorIcon, Pos2, RichText, Stroke, StrokeKind};

use super::Viewer;

/// Stream data shown at most, in bytes.
const PREVIEW_LIMIT: usize = 256 * 1024;
/// Object tree depth at most, against reference loops.
const MAX_DEPTH: usize = 48;
const RISKY: Color32 = Color32::from_rgb(230, 140, 60);
/// Orange at 15% opacity, premultiplied.
const PICK_FILL: Color32 = Color32::from_rgba_premultiplied(35, 21, 9, 38);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Overview,
    Objects,
    Fields,
    Actions,
    /// XFA and embedded files.
    Files,
}

#[derive(Default)]
pub(super) struct Inspector {
    pub open: bool,
    tab: Tab,
    /// The document generation the lists below were made for.
    made_for: Option<u64>,
    facts: Vec<Fact>,
    object_input: String,
    /// An object opened by number or picked on the page, shown above the
    /// trailer, and what it is.
    pinned: Option<(ObjRef, String)>,
    /// The pinned object's tree should open (once, when pinned).
    open_pinned: bool,
    /// Clicking the page picks an object instead of using it.
    pub picking: bool,
    /// What the pointer is over while picking: its page and the object.
    hover: Option<(usize, Picked)>,
    /// Decoded stream data being shown: its title and text.
    stream: Option<(String, String)>,
    /// Text being typed into a field's box, not yet committed.
    draft: Option<(usize, String)>,
}

impl Inspector {
    /// Forgets what was open for the previous file.
    pub fn reset(&mut self) {
        let open = self.open;
        let tab = self.tab;
        *self = Self::default();
        self.open = open;
        self.tab = tab;
    }

    /// Opens on XFA and embedded files.
    pub fn show_safety(&mut self) {
        self.tab = Tab::Files;
    }

    /// Opens on the scripts and actions.
    pub fn show_actions(&mut self) {
        self.tab = Tab::Actions;
    }

    fn pin(&mut self, obj: ObjRef, label: String) {
        self.pinned = Some((obj, label));
        self.open_pinned = true;
        self.stream = None;
    }
}

impl Viewer {
    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &self.open else { return };
        let doc = o.doc.clone();
        let generation = o.generation;
        let insp = &mut self.inspector;
        if insp.made_for != Some(generation) {
            insp.facts = overview(&doc);
            insp.made_for = Some(generation);
        }
        let mut jump_to_field = None;
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(400.0)
            .min_size(260.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let s = self.open.as_ref().map(|o| &o.safety);
                    let actions = s.map_or(0, |s| s.actions.len());
                    let scripts = s.map_or(0, |s| {
                        s.actions.iter().filter(|a| a.is_javascript()).count()
                            + s.xfa.as_ref().map_or(0, |x| x.scripts.len())
                    });
                    let files = s.map_or(0, |s| s.files.len());
                    let actions_label = if scripts > 0 {
                        format!("Actions ({scripts} scripts)")
                    } else {
                        format!("Actions ({actions})")
                    };
                    let files_label = format!("Files ({files})");
                    let insp = &mut self.inspector;
                    for (tab, label) in [
                        (Tab::Overview, "Overview".to_string()),
                        (Tab::Objects, "Objects".to_string()),
                        (Tab::Fields, "Fields".to_string()),
                        (Tab::Actions, actions_label),
                        (Tab::Files, files_label),
                    ] {
                        if ui.selectable_label(insp.tab == tab, label).clicked() {
                            insp.tab = tab;
                        }
                    }
                });
                ui.separator();
                match self.inspector.tab {
                    Tab::Overview => overview_tab(ui, &self.inspector.facts),
                    Tab::Objects => objects_tab(ui, &doc, &mut self.inspector),
                    Tab::Fields => jump_to_field = self.fields_tab(ui),
                    Tab::Actions => {
                        self.form_scripts_ui(ui);
                        self.actions_tab(ui);
                    }
                    Tab::Files => self.files_tab(ui),
                }
            });
        if let Some(at) = jump_to_field {
            self.focus_widget(at);
        }
    }

    /// The form's fields, each with a box to fill it in. Returns the
    /// widget picked to go to.
    fn fields_tab(&mut self, ui: &mut egui::Ui) -> Option<(usize, usize)> {
        let Some(fv) = self.open.as_ref().and_then(|o| o.form.as_ref()) else {
            if self
                .open
                .as_ref()
                .is_some_and(|o| atelier_form::document_has_xfa(&o.doc))
            {
                ui.colored_label(ui.visuals().warn_fg_color, "This PDF contains an XFA-only form. XFA layout, calculations and scripts are unsupported; no editable AcroForm fallback is available.");
            } else {
                ui.label("This file has no form fields.");
            }
            return None;
        };
        let form = &fv.form;
        if form.has_xfa {
            ui.colored_label(
                ui.visuals().hyperlink_color,
                "Hybrid XFA form: edits update only the AcroForm fallback. XFA data and calculations are not synchronized.",
            );
        }
        if form.need_appearances {
            ui.colored_label(
                ui.visuals().hyperlink_color,
                "The file asks viewers to redraw its fields; Atelier did.",
            );
        }
        let mut picked = None;
        let mut set = None;
        let draft = &mut self.inspector.draft;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("fields")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        for (i, f) in form.fields.iter().enumerate() {
                            let name = ui
                                .link(&f.name)
                                .on_hover_text(f.tooltip.as_deref().unwrap_or("Go to this field"));
                            if name.clicked() && !f.widgets.is_empty() {
                                picked = Some((i, 0));
                            }
                            let mut kind = f.kind.label().to_string();
                            if f.is_read_only() {
                                kind.push_str(", read-only");
                            }
                            if f.has_flag(flags::REQUIRED) {
                                kind.push_str(", required");
                            }
                            ui.label(RichText::new(kind).weak());
                            if let Some(v) = fill_box(ui, i, f, draft) {
                                set = Some((i, v));
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some((field, value)) = set {
            self.commit(field, value);
        }
        picked
    }
}

/// The box that fills in a field, matching its kind. Returns a new value
/// once one is picked, or once typing ends.
fn fill_box(
    ui: &mut egui::Ui,
    i: usize,
    f: &Field,
    draft: &mut Option<(usize, String)>,
) -> Option<Value> {
    const WIDTH: f32 = 170.0;
    if f.kind == FieldKind::PushButton {
        ui.label(RichText::new("has a script").weak())
            .on_hover_text(
                "Pressing it would run a script. Click it to review the script in Actions; \
             it runs only if you allow it.",
            );
        return None;
    }
    if !f.is_fillable() || f.is_read_only() {
        ui.label(f.display_value());
        return None;
    }
    let typed =
        f.kind == FieldKind::Text || f.kind == FieldKind::ComboBox && f.has_flag(flags::EDIT);
    if typed {
        let mut text = match draft {
            Some((d, t)) if *d == i => t.clone(),
            _ => f.value.display(),
        };
        let multiline = f.has_flag(flags::MULTILINE);
        let mut edit = if multiline {
            egui::TextEdit::multiline(&mut text).desired_rows(2)
        } else {
            egui::TextEdit::singleline(&mut text)
        }
        .id_salt(("fill", i))
        .password(f.has_flag(flags::PASSWORD))
        .desired_width(WIDTH);
        if let Some(m) = f.max_len.filter(|&m| m > 0) {
            edit = edit.char_limit(m);
        }
        let mut chosen = None;
        let resp = ui
            .horizontal(|ui| {
                let resp = ui.add(edit);
                // An editable combo box also offers its options.
                if f.kind == FieldKind::ComboBox && !f.options.is_empty() {
                    ui.menu_button("▾", |ui| {
                        for o in &f.options {
                            if ui.button(&o.display).clicked() {
                                chosen = Some(o.export.clone());
                                ui.close();
                            }
                        }
                    });
                }
                resp
            })
            .inner;
        if let Some(c) = chosen {
            *draft = None;
            return Some(Value::Text(c));
        }
        if resp.changed() {
            *draft = Some((i, text.clone()));
        }
        if resp.lost_focus() && draft.as_ref().is_some_and(|(d, _)| *d == i) {
            let (_, t) = draft.take()?;
            return Some(Value::Text(t));
        }
        return None;
    }
    match f.kind {
        FieldKind::Checkbox => {
            let on_state = f
                .states()
                .into_iter()
                .next()
                .unwrap_or_else(|| "Yes".into());
            let was = matches!(&f.value, Value::State(s) if *s == on_state);
            let mut on = was;
            ui.checkbox(&mut on, "");
            (on != was).then_some({
                if on {
                    Value::State(on_state)
                } else {
                    Value::None
                }
            })
        }
        FieldKind::Radio => {
            let current = match &f.value {
                Value::State(s) => Some(s.clone()),
                _ => None,
            };
            let mut pick = None;
            egui::ComboBox::from_id_salt(("fill", i))
                .width(WIDTH)
                .selected_text(current.clone().unwrap_or_else(|| "Off".into()))
                .show_ui(ui, |ui| {
                    if !f.has_flag(flags::NO_TOGGLE_TO_OFF)
                        && ui.selectable_label(current.is_none(), "Off").clicked()
                    {
                        pick = Some(Value::None);
                    }
                    for s in f.states() {
                        if ui
                            .selectable_label(current.as_ref() == Some(&s), &s)
                            .clicked()
                        {
                            pick = Some(Value::State(s));
                        }
                    }
                });
            pick.filter(|v| !v.same_as(&f.value))
        }
        FieldKind::ComboBox | FieldKind::ListBox => {
            let chosen: Vec<String> = match &f.value {
                Value::Text(t) => vec![t.clone()],
                Value::Choices(c) => c.clone(),
                _ => Vec::new(),
            };
            let multi = f.kind == FieldKind::ListBox && f.has_flag(flags::MULTI_SELECT);
            let shown = f.display_value();
            let mut pick = None;
            egui::ComboBox::from_id_salt(("fill", i))
                .width(WIDTH)
                .selected_text(if shown.is_empty() {
                    "(none)".to_string()
                } else {
                    shown
                })
                .show_ui(ui, |ui| {
                    for o in &f.options {
                        let on = chosen.contains(&o.export);
                        if multi {
                            let mut tick = on;
                            if ui.checkbox(&mut tick, &o.display).changed() {
                                let mut next: Vec<String> = f
                                    .options
                                    .iter()
                                    .filter(|x| {
                                        if x.export == o.export {
                                            tick
                                        } else {
                                            chosen.contains(&x.export)
                                        }
                                    })
                                    .map(|x| x.export.clone())
                                    .collect();
                                next.dedup();
                                pick = Some(Value::Choices(next));
                            }
                        } else if ui.selectable_label(on, &o.display).clicked() {
                            pick = Some(if f.kind == FieldKind::ListBox {
                                Value::Choices(vec![o.export.clone()])
                            } else {
                                Value::Text(o.export.clone())
                            });
                        }
                    }
                });
            pick
        }
        _ => {
            ui.label(f.display_value());
            None
        }
    }
}

fn overview_tab(ui: &mut egui::Ui, facts: &[Fact]) {
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("overview")
                .striped(true)
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in facts {
                        ui.label(RichText::new(*label).strong());
                        let risky = matches!(
                            *label,
                            "Open action"
                                | "Document actions"
                                | "Document scripts"
                                | "Embedded files"
                        );
                        if risky {
                            ui.colored_label(RISKY, value);
                        } else {
                            ui.label(value);
                        }
                        ui.end_row();
                    }
                });
        });
}

fn objects_tab(ui: &mut egui::Ui, doc: &Document, insp: &mut Inspector) {
    ui.horizontal(|ui| {
        ui.label("Object");
        let field = ui.add(
            egui::TextEdit::singleline(&mut insp.object_input)
                .hint_text("number")
                .desired_width(70.0),
        );
        let go = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Open").clicked() || go)
            && let Ok(n) = insp.object_input.trim().parse::<u32>()
        {
            let generation = match doc.xref_entry(n) {
                Some(atelier_syntax::XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            insp.pin(ObjRef::new(n, generation), String::new());
        }
        let pick = ui
            .add(egui::Button::selectable(insp.picking, "⌖ Point to select"))
            .on_hover_text("Click an annotation, field or page to show its object");
        if pick.clicked() {
            insp.picking = !insp.picking;
            insp.hover = None;
        }
    });
    if insp.picking {
        ui.colored_label(
            ui.visuals().hyperlink_color,
            "Click something on the page. Esc cancels.",
        );
    }
    let mut tree = Tree { doc, show: None };
    // With stream data open, the tree takes the top half.
    let tree_height = if insp.stream.is_some() {
        ui.available_height() * 0.5
    } else {
        f32::INFINITY
    };
    egui::ScrollArea::both()
        .id_salt("object-tree")
        .max_height(tree_height)
        .auto_shrink(false)
        .show(ui, |ui| {
            if let Some((r, label)) = &insp.pinned {
                let obj = doc.get(*r).unwrap_or(Object::Null);
                let title = if label.is_empty() {
                    r.to_string()
                } else {
                    format!("{r} ({label})")
                };
                // Picked objects open, and a new pick gets a fresh tree.
                let path = format!("pinned-{r}");
                let open = std::mem::take(&mut insp.open_pinned);
                node(ui, &mut tree, &title, &obj, path, 0, open);
            }
            let trailer = Object::Dict(doc.trailer().clone());
            node(
                ui,
                &mut tree,
                "Trailer",
                &trailer,
                "trailer".into(),
                0,
                false,
            );
        });
    if let Some((title, stream)) = tree.show {
        let text = match doc.decode_stream(&stream) {
            Ok(d) => preview(&d.data, PREVIEW_LIMIT),
            Err(e) => format!("Could not decode this stream: {e}"),
        };
        insp.stream = Some((title, text));
    }
    let mut close = false;
    if let Some((title, text)) = &insp.stream {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            if ui.small_button("Close").clicked() {
                close = true;
            }
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(text.clone());
            }
        });
        egui::ScrollArea::both()
            .id_salt("stream-data")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text.as_str())
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
    }
    if close {
        insp.stream = None;
    }
}

/// The object tree being drawn: the document, and the stream picked to
/// show.
struct Tree<'a> {
    doc: &'a Document,
    show: Option<(String, atelier_syntax::Stream)>,
}

/// One object in the tree. References open to the object they point at.
fn node(
    ui: &mut egui::Ui,
    tree: &mut Tree<'_>,
    label: &str,
    obj: &Object,
    path: String,
    depth: usize,
    open: bool,
) {
    let doc = tree.doc;
    if depth > MAX_DEPTH {
        ui.label("… (too deep)");
        return;
    }
    let (target, title) = match obj {
        Object::Reference(r) => {
            let t = doc.get(*r).unwrap_or(Object::Null);
            let s = format!("{label}  →  {r}  {}", summary(&t));
            (t, s)
        }
        other => (other.clone(), format!("{label}  {}", summary(other))),
    };
    if !has_children(&target) {
        ui.label(RichText::new(title).monospace());
        return;
    }
    egui::CollapsingHeader::new(RichText::new(title).monospace())
        .id_salt(&path)
        .open(open.then_some(true))
        .show(ui, |ui| {
            if let Object::Stream(s) = &target {
                let name = match obj {
                    Object::Reference(r) => format!("Stream {r}"),
                    _ => format!("Stream {label}"),
                };
                if ui.small_button("Show data").clicked() {
                    tree.show = Some((name, s.clone()));
                }
            }
            for (i, (key, child)) in children(&target).into_iter().enumerate() {
                node(
                    ui,
                    tree,
                    &key,
                    &child,
                    format!("{path}/{i}"),
                    depth + 1,
                    false,
                );
            }
        });
}

impl Viewer {
    /// The form's scripts that check and work out values, each blocked
    /// until allowed here.
    fn form_scripts_ui(&mut self, ui: &mut egui::Ui) {
        let names: Vec<String> = self
            .open
            .as_ref()
            .and_then(|o| o.form.as_ref())
            .map(|fv| fv.form.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default();
        let Some(state) = self.open.as_mut().and_then(|o| o.scripts.as_mut()) else {
            return;
        };
        let total = state.runnable().len();
        if total == 0 {
            return;
        }
        let allowed = state.allowed_count();
        egui::CollapsingHeader::new(
            RichText::new(format!("Form scripts: {allowed} of {total} allowed")).strong(),
        )
        .id_salt("form-scripts")
        .default_open(allowed == 0)
        .show(ui, |ui| {
            ui.label(
                "These scripts check what is typed and work out totals. They are blocked \
                 until you allow them. Allowed scripts run in a sandbox that can only read \
                 and set this form's fields: no files, no network, nothing else on your \
                 computer.",
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(allowed < total, egui::Button::new("Allow all"))
                    .clicked()
                {
                    state.allow_all();
                }
                if ui
                    .add_enabled(allowed > 0, egui::Button::new("Block all"))
                    .clicked()
                {
                    state.allowed.clear();
                }
            });
            let mut rows: Vec<(String, String)> = state
                .all
                .document
                .iter()
                .map(|(n, s)| {
                    (
                        format!("Document script \"{n}\" (defines helpers)"),
                        s.clone(),
                    )
                })
                .collect();
            for (i, f) in state.all.fields.iter().enumerate() {
                let name = names.get(i).map_or("?", String::as_str);
                for (what, s) in [
                    ("checks typing", &f.keystroke),
                    ("checks the value", &f.validate),
                    ("works out its value", &f.calculate),
                ] {
                    if let Some(s) = s {
                        rows.push((format!("Field {name}: {what}"), s.clone()));
                    }
                }
            }
            let mut toggle = None;
            egui::ScrollArea::vertical()
                .id_salt("form-script-list")
                .max_height(320.0)
                .show(ui, |ui| {
                    for (i, (label, source)) in rows.iter().enumerate() {
                        let mut on = state.is_allowed(source);
                        if ui.checkbox(&mut on, label.as_str()).changed() {
                            toggle = Some(source.clone());
                        }
                        egui::CollapsingHeader::new(RichText::new("Show the script").small())
                            .id_salt(("form-script", i))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut source.as_str())
                                        .code_editor()
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(1),
                                );
                            });
                    }
                });
            if let Some(s) = toggle {
                state.toggle(&s);
            }
            if state.all.fields.iter().any(|f| f.format.is_some()) {
                ui.label(
                    RichText::new(
                        "Formatting scripts (how a value looks, such as 1,234.50) are not run \
                         yet; values show as typed.",
                    )
                    .weak()
                    .small(),
                );
            }
        });
        ui.separator();
    }

    /// While picking: the object under the pointer, and a click pins it.
    /// Returns whether picking took the pointer.
    pub(super) fn pick_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Option<(f64, f64)>,
    ) -> bool {
        if !self.inspector.picking {
            return false;
        }
        if !self.inspector.open || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.inspector.picking = false;
            self.inspector.hover = None;
            return true;
        }
        let Some(o) = &self.open else { return false };
        self.inspector.hover = at.and_then(|(lx, ly)| {
            let (page, _, _) = o.layout.page_at(lx, ly)?;
            let user = o.layout.to_user(page, lx, ly)?;
            let p = o.doc.pages().ok()?.get(page)?;
            object_at(&o.doc, p, user.x, user.y).map(|picked| (page, picked))
        });
        if self.inspector.hover.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if response.clicked()
            && let Some((page, picked)) = self.inspector.hover.take()
        {
            let label = if picked.rect.is_none() {
                format!("page {}", page + 1)
            } else {
                picked.label
            };
            let insp = &mut self.inspector;
            insp.pin(picked.obj, label);
            insp.picking = false;
            insp.tab = Tab::Objects;
        }
        true
    }

    /// The outline of the object under the pointer while picking.
    pub(super) fn draw_pick(&self, ui: &egui::Ui, origin: Pos2) {
        let Some((page, picked)) = &self.inspector.hover else {
            return;
        };
        let rect = match &picked.rect {
            Some(r) => self.user_rect(*page, r, origin),
            None => self.open.as_ref().and_then(|o| {
                let r = o.layout.page_rect(*page)?;
                let shift = self.centre_shift();
                Some(egui::Rect::from_min_size(
                    origin + egui::Vec2::new((r.x + shift) as f32, r.y as f32),
                    egui::Vec2::new(r.w as f32, r.h as f32),
                ))
            }),
        };
        if let Some(r) = rect {
            let p = ui.painter();
            p.rect_filled(r, 0.0, PICK_FILL);
            p.rect_stroke(r, 0.0, Stroke::new(2.0, RISKY), StrokeKind::Outside);
        }
    }
}
