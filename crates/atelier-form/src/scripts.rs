//! Finds every action a document carries, JavaScript above all, so the
//! inspector can show what a file would try to do. Nothing here runs them.

use std::collections::HashSet;

use atelier_syntax::{Dict, Document, ObjRef, Object, Rect};

/// An action found in the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// Where it was found, for people: "Document open", "Page 3 close",
    /// "Field total, calculate".
    pub location: String,
    /// The object that holds the trigger, when there is one.
    pub owner: Option<ObjRef>,
    /// The action type (/S): JavaScript, URI, Launch, SubmitForm...
    pub kind: String,
    /// JavaScript source, for JavaScript actions.
    pub javascript: Option<String>,
    /// What else the action names: a URI, a file, a submit address.
    pub target: Option<String>,
    /// A click on the annotation that owns it would set it off: mouse
    /// down or up, focus, or the annotation's own action.
    pub on_click: bool,
}

impl Action {
    pub fn is_javascript(&self) -> bool {
        self.kind == "JavaScript"
    }
}

/// Follow /Next chains and name trees no deeper or longer than this.
const MAX_DEPTH: usize = 32;
const MAX_ACTIONS: usize = 10_000;

/// Every action in the document: document-level scripts, open and close
/// actions, page actions, annotation and field actions.
pub fn document_actions(doc: &Document) -> Vec<Action> {
    let mut found = Finder {
        doc,
        out: Vec::new(),
        seen: HashSet::new(),
        click: false,
    };
    let Ok(catalog) = doc.catalog() else {
        return found.out;
    };

    // Document-level JavaScript: the /Names /JavaScript name tree.
    if let Ok(Object::Dict(names)) = doc.get_key(&catalog, b"Names") {
        if let Ok(Object::Dict(tree)) = doc.get_key(&names, b"JavaScript") {
            let mut entries = Vec::new();
            name_tree(doc, &tree, &mut entries, 0);
            for (name, action) in entries {
                found.action(&format!("Document script \"{name}\""), None, &action, 0);
            }
        }
    }
    if let Some(open) = catalog.get(b"OpenAction") {
        // An array here is a destination, not an action.
        if let Ok(Object::Dict(_)) = doc.resolve(open) {
            found.action("Document open", None, open, 0);
        }
    }
    found.triggers(
        &catalog,
        None,
        "Document",
        &[
            ("WC", "before closing"),
            ("WS", "before saving"),
            ("DS", "after saving"),
            ("WP", "before printing"),
            ("DP", "after printing"),
        ],
    );

    if let Ok(pages) = doc.pages() {
        for (i, page) in pages.iter().enumerate() {
            let label = format!("Page {}", i + 1);
            found.triggers(
                &page.dict,
                page.obj_ref,
                &label,
                &[("O", "open"), ("C", "close")],
            );
            let Ok(annots) = doc.get_key(&page.dict, b"Annots") else {
                continue;
            };
            for a in annots.as_array().unwrap_or_default() {
                let owner = a.as_reference();
                let Ok(Object::Dict(annot)) = doc.resolve(a) else {
                    continue;
                };
                let name = annot_label(doc, &annot, &label);
                if let Some(action) = annot.get(b"A") {
                    found.click = true;
                    found.action(&format!("{name}, activate"), owner, action, 0);
                    found.click = false;
                }
                found.triggers(
                    &annot,
                    owner,
                    &name,
                    &[
                        ("E", "mouse enter"),
                        ("X", "mouse exit"),
                        ("D", "mouse down"),
                        ("U", "mouse up"),
                        ("Fo", "focus"),
                        ("Bl", "blur"),
                        ("PO", "page open"),
                        ("PC", "page close"),
                        ("PV", "page visible"),
                        ("PI", "page invisible"),
                    ],
                );
            }
        }
    }

    // Field triggers live on field dictionaries, which may not be widgets.
    if let Ok(Object::Dict(acro)) = doc.get_key(&catalog, b"AcroForm") {
        if let Ok(Object::Array(fields)) = doc.get_key(&acro, b"Fields") {
            for f in fields {
                found.field(&f, String::new(), 0);
            }
        }
    }
    found.out
}

/// An annotation on a page with actions tied to it, its own or its
/// field's, so the viewer can point at what a click would set off.
#[derive(Debug, Clone, PartialEq)]
pub struct Hotspot {
    /// Page index.
    pub page: usize,
    pub rect: Rect,
    /// The annotation.
    pub annot: ObjRef,
    /// The indices into the action list of everything tied to it: the
    /// annotation's actions, then its fields'.
    pub actions: Vec<usize>,
}

impl Hotspot {
    /// Whether a point in the page's user space is inside it.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let r = &self.rect;
        x >= r.x0.min(r.x1) && x <= r.x0.max(r.x1) && y >= r.y0.min(r.y1) && y <= r.y0.max(r.y1)
    }

    /// The JavaScript among its actions.
    pub fn has_javascript(&self, actions: &[Action]) -> bool {
        self.actions
            .iter()
            .any(|&i| actions.get(i).is_some_and(Action::is_javascript))
    }
}

/// Every visible annotation that has actions in `actions` (as found by
/// [`document_actions`]), page by page, in /Annots order.
pub fn hotspots(doc: &Document, actions: &[Action]) -> Vec<Hotspot> {
    let mut out = Vec::new();
    if actions.is_empty() {
        return out;
    }
    let Ok(pages) = doc.pages() else { return out };
    let mut by_owner: std::collections::HashMap<ObjRef, Vec<usize>> = Default::default();
    for (i, a) in actions.iter().enumerate() {
        if let Some(o) = a.owner {
            by_owner.entry(o).or_default().push(i);
        }
    }
    for (page, p) in pages.iter().enumerate() {
        let Ok(annots) = doc.get_key(&p.dict, b"Annots") else {
            continue;
        };
        for a in annots.as_array().unwrap_or_default() {
            let Some(annot) = a.as_reference() else {
                continue;
            };
            let Ok(Object::Dict(dict)) = doc.get(annot) else {
                continue;
            };
            // Hidden or NoView annotations cannot be pointed at.
            let flags = doc
                .get_key(&dict, b"F")
                .ok()
                .and_then(|o| o.as_i64())
                .unwrap_or(0);
            if flags & (2 | 32) != 0 {
                continue;
            }
            let Some(rect) = doc
                .get_key(&dict, b"Rect")
                .ok()
                .and_then(|o| o.as_array().and_then(Rect::from_array))
            else {
                continue;
            };
            // The annotation, then its field and the field's ancestors.
            let mut owners = vec![annot];
            let mut node = dict;
            while owners.len() < MAX_DEPTH {
                let Some(parent) = node.get(b"Parent").and_then(Object::as_reference) else {
                    break;
                };
                if owners.contains(&parent) {
                    break;
                }
                owners.push(parent);
                match doc.get(parent) {
                    Ok(Object::Dict(d)) => node = d,
                    _ => break,
                }
            }
            let mut tied = Vec::new();
            for owner in &owners {
                tied.extend(by_owner.get(owner).into_iter().flatten());
            }
            if !tied.is_empty() {
                out.push(Hotspot {
                    page,
                    rect,
                    annot,
                    actions: tied,
                });
            }
        }
    }
    out
}

struct Finder<'a> {
    doc: &'a Document,
    out: Vec<Action>,
    seen: HashSet<ObjRef>,
    /// The actions being read would run on a click.
    click: bool,
}

/// Annotation triggers a click sets off.
const CLICK_TRIGGERS: &[&str] = &["D", "U", "Fo"];

impl Finder<'_> {
    fn triggers(&mut self, dict: &Dict, owner: Option<ObjRef>, label: &str, keys: &[(&str, &str)]) {
        let Ok(Object::Dict(aa)) = self.doc.get_key(dict, b"AA") else {
            return;
        };
        for (key, what) in keys {
            if let Some(action) = aa.get(key.as_bytes()) {
                self.click = CLICK_TRIGGERS.contains(key);
                self.action(&format!("{label}, {what}"), owner, action, 0);
                self.click = false;
            }
        }
    }

    fn field(&mut self, obj: &Object, parent: String, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let owner = obj.as_reference();
        if let Some(r) = owner {
            if !self.seen.insert(r) {
                return;
            }
        }
        let Ok(Object::Dict(dict)) = self.doc.resolve(obj) else {
            return;
        };
        let name = match self.doc.get_key(&dict, b"T") {
            Ok(Object::String(t)) if parent.is_empty() => t.to_text(),
            Ok(Object::String(t)) => format!("{parent}.{}", t.to_text()),
            _ => parent,
        };
        let label = format!("Field {name}");
        self.triggers(
            &dict,
            owner,
            &label,
            &[
                ("K", "keystroke"),
                ("F", "format"),
                ("V", "validate"),
                ("C", "calculate"),
            ],
        );
        if let Ok(Object::Array(kids)) = self.doc.get_key(&dict, b"Kids") {
            for k in kids {
                self.field(&k, name.clone(), depth + 1);
            }
        }
    }

    fn action(&mut self, location: &str, owner: Option<ObjRef>, obj: &Object, depth: usize) {
        if depth > MAX_DEPTH || self.out.len() >= MAX_ACTIONS {
            return;
        }
        let Ok(Object::Dict(action)) = self.doc.resolve(obj) else {
            return;
        };
        let kind = self
            .doc
            .get_key(&action, b"S")
            .ok()
            .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned()))
            .unwrap_or_else(|| "(none)".to_string());
        let javascript = (kind == "JavaScript")
            .then(|| self.text(&action, b"JS"))
            .flatten();
        let target = match kind.as_str() {
            "URI" => self.text(&action, b"URI"),
            "Launch" | "GoToR" | "GoToE" | "ImportData" => self.file(&action, b"F"),
            "SubmitForm" => self.file(&action, b"F"),
            "Named" => self
                .doc
                .get_key(&action, b"N")
                .ok()
                .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned())),
            _ => None,
        };
        self.out.push(Action {
            location: location.to_string(),
            owner,
            kind,
            javascript,
            target,
            on_click: self.click,
        });
        match self.doc.get_key(&action, b"Next") {
            Ok(Object::Array(next)) => {
                for n in next {
                    self.action(location, owner, &n, depth + 1);
                }
            }
            Ok(Object::Dict(_)) => {
                if let Some(n) = action.get(b"Next") {
                    self.action(location, owner, n, depth + 1);
                }
            }
            _ => {}
        }
    }

    /// A text string or stream entry, such as /JS.
    fn text(&self, dict: &Dict, key: &[u8]) -> Option<String> {
        match self.doc.get_key(dict, key).ok()? {
            Object::String(s) => Some(s.to_text()),
            Object::Stream(s) => self
                .doc
                .decode_stream(&s)
                .ok()
                .map(|d| atelier_syntax::text_from_bytes(&d.data)),
            _ => None,
        }
    }

    /// A file specification: a string, or a dictionary with /F or /UF.
    fn file(&self, dict: &Dict, key: &[u8]) -> Option<String> {
        match self.doc.get_key(dict, key).ok()? {
            Object::String(s) => Some(s.to_text()),
            Object::Dict(spec) => self.text(&spec, b"UF").or_else(|| self.text(&spec, b"F")),
            _ => None,
        }
    }
}

fn annot_label(doc: &Document, annot: &Dict, page: &str) -> String {
    let subtype = doc
        .get_key(annot, b"Subtype")
        .ok()
        .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned()))
        .unwrap_or_else(|| "Annotation".to_string());
    match doc.get_key(annot, b"T") {
        Ok(Object::String(t)) if subtype == "Widget" => format!("{page}, widget {}", t.to_text()),
        _ => format!("{page}, {} annotation", subtype.to_lowercase()),
    }
}

/// The entries of a name tree, in order.
fn name_tree(doc: &Document, node: &Dict, out: &mut Vec<(String, Object)>, depth: usize) {
    if depth > MAX_DEPTH || out.len() >= MAX_ACTIONS {
        return;
    }
    if let Ok(Object::Array(names)) = doc.get_key(node, b"Names") {
        for pair in names.chunks_exact(2) {
            let key = match doc.resolve(&pair[0]) {
                Ok(Object::String(s)) => s.to_text(),
                _ => String::new(),
            };
            out.push((key, pair[1].clone()));
        }
    }
    if let Ok(Object::Array(kids)) = doc.get_key(node, b"Kids") {
        for k in kids {
            if let Ok(Object::Dict(kid)) = doc.resolve(&k) {
                name_tree(doc, &kid, out, depth + 1);
            }
        }
    }
}

/// A field's own scripts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldScripts {
    pub keystroke: Option<String>,
    pub format: Option<String>,
    pub validate: Option<String>,
    pub calculate: Option<String>,
}

impl FieldScripts {
    pub fn is_empty(&self) -> bool {
        self.keystroke.is_none()
            && self.format.is_none()
            && self.validate.is_none()
            && self.calculate.is_none()
    }
}

/// The scripts a form would run while it is filled in. Reading them runs
/// nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormScripts {
    /// Document-level scripts, by name, in name order.
    pub document: Vec<(String, String)>,
    /// Each field's scripts, by field index.
    pub fields: Vec<FieldScripts>,
    /// Fields whose calculate scripts run, in order: the form's /CO, or
    /// when it has none, every field with one.
    pub calculation_order: Vec<usize>,
}

impl FormScripts {
    pub fn is_empty(&self) -> bool {
        self.document.is_empty() && self.fields.iter().all(FieldScripts::is_empty)
    }

    /// How many scripts there are.
    pub fn count(&self) -> usize {
        self.document.len()
            + self
                .fields
                .iter()
                .map(|f| {
                    [&f.keystroke, &f.format, &f.validate, &f.calculate]
                        .iter()
                        .filter(|s| s.is_some())
                        .count()
                })
                .sum::<usize>()
    }
}

/// The JavaScript of an action, when it is a JavaScript action.
fn javascript(doc: &Document, action: &Object) -> Option<String> {
    let Ok(Object::Dict(action)) = doc.resolve(action) else {
        return None;
    };
    let is_js = doc
        .get_key(&action, b"S")
        .ok()
        .is_some_and(|s| s.as_name().is_some_and(|n| *n == b"JavaScript"));
    if !is_js {
        return None;
    }
    match doc.get_key(&action, b"JS").ok()? {
        Object::String(s) => Some(s.to_text()),
        Object::Stream(s) => doc
            .decode_stream(&s)
            .ok()
            .map(|d| atelier_syntax::text_from_bytes(&d.data)),
        _ => None,
    }
}

/// The scripts of a form.
pub fn form_scripts(doc: &Document, form: &crate::Form) -> FormScripts {
    let mut out = FormScripts::default();
    let tree = doc
        .catalog()
        .ok()
        .and_then(|c| match doc.get_key(&c, b"Names") {
            Ok(Object::Dict(names)) => Some(names),
            _ => None,
        })
        .and_then(|names| match doc.get_key(&names, b"JavaScript") {
            Ok(Object::Dict(tree)) => Some(tree),
            _ => None,
        });
    if let Some(tree) = tree {
        let mut entries = Vec::new();
        name_tree(doc, &tree, &mut entries, 0);
        for (name, action) in entries {
            if let Some(js) = javascript(doc, &action) {
                out.document.push((name, js));
            }
        }
    }
    for field in &form.fields {
        let mut s = FieldScripts::default();
        let aa = match doc.get(field.obj) {
            Ok(Object::Dict(dict)) => match doc.get_key(&dict, b"AA") {
                Ok(Object::Dict(aa)) => Some(aa),
                _ => None,
            },
            _ => None,
        };
        if let Some(aa) = aa {
            let get = |key: &[u8]| aa.get(key).and_then(|a| javascript(doc, a));
            s.keystroke = get(b"K");
            s.format = get(b"F");
            s.validate = get(b"V");
            s.calculate = get(b"C");
        }
        out.fields.push(s);
    }
    let mut order: Vec<usize> = form
        .calculation_order
        .iter()
        .filter_map(|r| form.fields.iter().position(|f| f.obj == *r))
        .filter(|&i| out.fields[i].calculate.is_some())
        .collect();
    if form.calculation_order.is_empty() {
        order = (0..out.fields.len())
            .filter(|&i| out.fields[i].calculate.is_some())
            .collect();
    }
    order.dedup();
    out.calculation_order = order;
    out
}
