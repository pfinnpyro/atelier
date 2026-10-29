//! Finds every action a document carries, JavaScript above all, so the
//! inspector can show what a file would try to do. Nothing here runs them.

use std::collections::HashSet;

use atelier_syntax::{Dict, Document, ObjRef, Object};

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
                    found.action(&format!("{name}, activate"), owner, action, 0);
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

struct Finder<'a> {
    doc: &'a Document,
    out: Vec<Action>,
    seen: HashSet<ObjRef>,
}

impl Finder<'_> {
    fn triggers(&mut self, dict: &Dict, owner: Option<ObjRef>, label: &str, keys: &[(&str, &str)]) {
        let Ok(Object::Dict(aa)) = self.doc.get_key(dict, b"AA") else {
            return;
        };
        for (key, what) in keys {
            if let Some(action) = aa.get(key.as_bytes()) {
                self.action(&format!("{label}, {what}"), owner, action, 0);
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
