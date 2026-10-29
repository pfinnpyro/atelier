//! The inspector panel: the file at a glance, its objects and streams, its
//! form fields, and every script and action it carries. Scripts are shown
//! as text and never run.

use atelier_form::{Action, FieldKind, document_actions};
use atelier_syntax::{Document, ObjRef, Object};
use atelier_view::{Fact, children, has_children, overview, preview, summary};
use eframe::egui::{self, Color32, RichText};

use super::Viewer;

/// Stream data shown at most, in bytes.
const PREVIEW_LIMIT: usize = 256 * 1024;
/// Object tree depth at most, against reference loops.
const MAX_DEPTH: usize = 48;
const RISKY: Color32 = Color32::from_rgb(230, 140, 60);
const NOTE: Color32 = Color32::from_rgb(140, 180, 255);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Overview,
    Objects,
    Fields,
    Actions,
}

#[derive(Default)]
pub(super) struct Inspector {
    pub open: bool,
    tab: Tab,
    /// The document generation the lists below were made for.
    made_for: Option<u64>,
    facts: Vec<Fact>,
    actions: Vec<Action>,
    object_input: String,
    /// An object opened by number, shown under the trailer.
    pinned: Option<ObjRef>,
    /// Decoded stream data being shown: its title and text.
    stream: Option<(String, String)>,
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
}

impl Viewer {
    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &self.open else { return };
        let doc = o.doc.clone();
        let generation = o.generation;
        let insp = &mut self.inspector;
        if insp.made_for != Some(generation) {
            insp.facts = overview(&doc);
            insp.actions = document_actions(&doc);
            insp.made_for = Some(generation);
        }
        let mut jump_to_field = None;
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(400.0)
            .min_size(260.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let insp = &mut self.inspector;
                    let scripts = insp.actions.iter().filter(|a| a.is_javascript()).count();
                    let actions_label = if scripts > 0 {
                        format!("Actions ({scripts} scripts)")
                    } else {
                        format!("Actions ({})", insp.actions.len())
                    };
                    for (tab, label) in [
                        (Tab::Overview, "Overview".to_string()),
                        (Tab::Objects, "Objects".to_string()),
                        (Tab::Fields, "Fields".to_string()),
                        (Tab::Actions, actions_label),
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
                    Tab::Actions => actions_tab(ui, &self.inspector.actions),
                }
            });
        if let Some(at) = jump_to_field {
            self.focus_widget(at);
        }
    }

    /// The form's fields; returns the widget picked to go to.
    fn fields_tab(&self, ui: &mut egui::Ui) -> Option<(usize, usize)> {
        let fv = self.open.as_ref()?.form.as_ref();
        let Some(fv) = fv else {
            ui.label("This file has no form fields.");
            return None;
        };
        let form = &fv.form;
        if form.has_xfa {
            ui.colored_label(
                NOTE,
                "The file also has an XFA form. Atelier uses its AcroForm.",
            );
        }
        if form.need_appearances {
            ui.colored_label(
                NOTE,
                "The file asks viewers to redraw its fields; Atelier did.",
            );
        }
        let mut picked = None;
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
                            if f.has_flag(atelier_form::flags::REQUIRED) {
                                kind.push_str(", required");
                            }
                            ui.label(RichText::new(kind).weak());
                            let value = match f.kind {
                                FieldKind::PushButton => {
                                    "(runs a script when pressed: never run)".into()
                                }
                                _ => f.display_value(),
                            };
                            ui.label(value);
                            ui.end_row();
                        }
                    });
            });
        picked
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
            insp.pinned = Some(ObjRef::new(n, generation));
        }
    });
    let mut show = None;
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
            if let Some(r) = insp.pinned {
                let obj = doc.get(r).unwrap_or(Object::Null);
                node(
                    ui,
                    doc,
                    &format!("{r}"),
                    &obj,
                    "pinned".into(),
                    0,
                    &mut show,
                );
            }
            let trailer = Object::Dict(doc.trailer().clone());
            node(ui, doc, "Trailer", &trailer, "trailer".into(), 0, &mut show);
        });
    if let Some((title, stream)) = show {
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

/// One object in the tree. References open to the object they point at.
fn node(
    ui: &mut egui::Ui,
    doc: &Document,
    label: &str,
    obj: &Object,
    path: String,
    depth: usize,
    show: &mut Option<(String, atelier_syntax::Stream)>,
) {
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
        .show(ui, |ui| {
            if let Object::Stream(s) = &target {
                let name = match obj {
                    Object::Reference(r) => format!("Stream {r}"),
                    _ => format!("Stream {label}"),
                };
                if ui.small_button("Show data").clicked() {
                    *show = Some((name, s.clone()));
                }
            }
            for (i, (key, child)) in children(&target).into_iter().enumerate() {
                node(
                    ui,
                    doc,
                    &key,
                    &child,
                    format!("{path}/{i}"),
                    depth + 1,
                    show,
                );
            }
        });
}

fn actions_tab(ui: &mut egui::Ui, actions: &[Action]) {
    ui.colored_label(
        NOTE,
        "Atelier never runs scripts. They are listed here so you can see what this file \
         would try to do.",
    );
    ui.separator();
    if actions.is_empty() {
        ui.label("This file has no scripts or actions.");
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for (i, a) in actions.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let risky = matches!(
                        a.kind.as_str(),
                        "JavaScript"
                            | "Launch"
                            | "URI"
                            | "SubmitForm"
                            | "ImportData"
                            | "GoToR"
                            | "GoToE"
                    );
                    let kind = RichText::new(&a.kind).strong();
                    if risky {
                        ui.colored_label(RISKY, kind);
                    } else {
                        ui.label(kind);
                    }
                    ui.label(&a.location);
                    if let Some(r) = a.owner {
                        ui.label(RichText::new(r.to_string()).weak());
                    }
                });
                if let Some(t) = &a.target {
                    ui.label(format!("→ {t}"));
                }
                if let Some(js) = &a.javascript {
                    egui::ScrollArea::vertical()
                        .id_salt(("script", i))
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut js.as_str())
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(1),
                            );
                        });
                }
                ui.separator();
            }
        });
}
