//! Interactive forms (ISO 32000-2, 12.7): reading the AcroForm field tree,
//! setting field values and drawing the appearances that show them.
//!
//! Actions attached to fields and the document, scripts above all, are
//! collected so they can be inspected. Nothing here runs them.

mod appearance;
mod da;
mod fill;
mod scripts;
mod session;
mod traits;
mod xfa;
mod xml;

use std::collections::{HashMap, HashSet};

use atelier_syntax::{Dict, Document, ObjRef, Object, Rect, Result};

pub use da::DefaultAppearance;
pub use fill::FillWarning;
pub use scripts::{Action, Hotspot, document_actions, hotspots};
pub use session::{Built, Session};
pub use traits::{ScriptTrait, script_traits};
pub use xfa::{Xfa, XfaScript, xfa};

/// Field flags (ISO 32000-2, tables 226, 229, 231 and 233).
pub mod flags {
    pub const READ_ONLY: u32 = 1 << 0;
    pub const REQUIRED: u32 = 1 << 1;
    pub const NO_EXPORT: u32 = 1 << 2;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSHBUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const SORT: u32 = 1 << 19;
    pub const FILE_SELECT: u32 = 1 << 20;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const DO_NOT_SPELL_CHECK: u32 = 1 << 22;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    pub const RICH_TEXT: u32 = 1 << 25;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    pub const COMMIT_ON_SEL_CHANGE: u32 = 1 << 26;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    PushButton,
    ComboBox,
    ListBox,
    Signature,
    /// No usable /FT.
    Unknown,
}

impl FieldKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::PushButton => "button",
            Self::ComboBox => "combo box",
            Self::ListBox => "list box",
            Self::Signature => "signature",
            Self::Unknown => "unknown",
        }
    }
}

/// A field value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    None,
    Text(String),
    /// A button state name: the on state of a checkbox or radio, or Off.
    State(String),
    /// The selected options of a list box (or a combo box).
    Choices(Vec<String>),
}

impl Value {
    /// The value as one line of text, for listing.
    pub fn display(&self) -> String {
        match self {
            Self::None => String::new(),
            Self::Text(t) | Self::State(t) => t.clone(),
            Self::Choices(c) => c.join(", "),
        }
    }

    /// Whether two values mean the same: no value, an empty text and the
    /// Off state are all "nothing", and one choice is the same as its text.
    pub fn same_as(&self, other: &Value) -> bool {
        fn norm(v: &Value) -> Value {
            match v {
                Value::Text(t) if t.is_empty() => Value::None,
                Value::State(s) if s == "Off" => Value::None,
                Value::Choices(c) if c.is_empty() => Value::None,
                Value::Choices(c) if c.len() == 1 => Value::Text(c[0].clone()),
                Value::State(s) => Value::Text(s.clone()),
                v => v.clone(),
            }
        }
        norm(self) == norm(other)
    }

    fn from_object(obj: &Object, doc: &Document) -> Self {
        match doc.resolve(obj).unwrap_or_default() {
            Object::String(s) => Self::Text(s.to_text()),
            Object::Name(n) => Self::State(n.as_str_lossy().into_owned()),
            Object::Stream(s) => doc
                .decode_stream(&s)
                .map(|d| Self::Text(atelier_syntax::text_from_bytes(&d.data)))
                .unwrap_or_default(),
            Object::Array(items) => Self::Choices(
                items
                    .iter()
                    .filter_map(|o| match doc.resolve(o).ok()? {
                        Object::String(s) => Some(s.to_text()),
                        Object::Name(n) => Some(n.as_str_lossy().into_owned()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => Self::None,
        }
    }
}

/// One entry of a choice field's /Opt.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceOption {
    /// The value stored in /V when this option is chosen.
    pub export: String,
    /// The text shown for it.
    pub display: String,
}

/// A widget annotation: one place on a page where a field appears.
#[derive(Debug, Clone)]
pub struct Widget {
    pub obj: ObjRef,
    /// Index of the page that lists it in /Annots.
    pub page: Option<usize>,
    /// Its position in that page's /Annots, the default Tab order.
    pub annot_index: Option<usize>,
    pub rect: Rect,
    /// Annotation flags (/F).
    pub annot_flags: u32,
    /// For checkboxes and radios: the name of the state that means on.
    pub on_state: Option<String>,
    /// /MK /R: rotation of the content, in degrees.
    pub rotation: u16,
}

impl Widget {
    /// Hidden or NoView: the widget is not shown.
    pub fn is_hidden(&self) -> bool {
        self.annot_flags & (2 | 32) != 0
    }
}

/// A terminal field: one value, shown by one or more widgets.
#[derive(Debug, Clone)]
pub struct Field {
    /// The field dictionary (for a single-widget field, also the widget).
    pub obj: ObjRef,
    /// The fully qualified name: partial names joined with dots.
    pub name: String,
    /// /TU: the name to show people.
    pub tooltip: Option<String>,
    pub kind: FieldKind,
    pub flags: u32,
    pub value: Value,
    pub default_value: Value,
    pub options: Vec<ChoiceOption>,
    pub max_len: Option<usize>,
    /// /DA, inherited, falling back to the form's.
    pub da: Option<String>,
    /// /Q: 0 left, 1 centred, 2 right.
    pub quadding: u8,
    /// /TI: the first visible option of a list box.
    pub top_index: usize,
    pub widgets: Vec<Widget>,
}

impl Field {
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    pub fn is_read_only(&self) -> bool {
        self.has_flag(flags::READ_ONLY)
    }

    /// The text shown for the current value of a choice field.
    pub fn display_value(&self) -> String {
        let shown = |v: &str| {
            self.options
                .iter()
                .find(|o| o.export == v)
                .map_or_else(|| v.to_string(), |o| o.display.clone())
        };
        match &self.value {
            Value::Text(t) if !self.options.is_empty() => shown(t),
            Value::Choices(c) => c.iter().map(|v| shown(v)).collect::<Vec<_>>().join(", "),
            v => v.display(),
        }
    }

    /// Every on state its widgets offer, in widget order, without repeats.
    pub fn states(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for w in &self.widgets {
            if let Some(s) = &w.on_state {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        out
    }
}

/// A document's interactive form.
#[derive(Debug, Clone)]
pub struct Form {
    pub fields: Vec<Field>,
    /// /NeedAppearances: the file asks viewers to redraw field appearances.
    pub need_appearances: bool,
    /// The form also has an XFA description, which Atelier does not use.
    pub has_xfa: bool,
    /// /SigFlags.
    pub sig_flags: u32,
    /// The form's default resources (/DR).
    pub resources: Dict,
    /// The form's default appearance (/DA).
    pub da: Option<String>,
    /// /CO: fields whose calculate scripts run, in order.
    pub calculation_order: Vec<ObjRef>,
}

/// Attributes a field inherits from its ancestors (ISO 32000-2, 12.7.4.1).
#[derive(Clone, Default)]
struct Inherited {
    name: String,
    ft: Option<Vec<u8>>,
    ff: Option<u32>,
    v: Option<Object>,
    dv: Option<Object>,
    da: Option<String>,
    q: Option<u8>,
    opt: Option<Object>,
    max_len: Option<usize>,
}

/// Deeper field trees than this are treated as broken.
const MAX_DEPTH: usize = 64;
/// More fields than this are ignored, as a guard against hostile files.
const MAX_FIELDS: usize = 100_000;

impl Form {
    /// Reads the document's AcroForm, or None when it has no fields.
    pub fn load(doc: &Document) -> Result<Option<Self>> {
        let catalog = doc.catalog()?;
        let Ok(Object::Dict(acro)) = doc.get_key(&catalog, b"AcroForm") else {
            return Ok(None);
        };
        let resources = doc
            .get_key(&acro, b"DR")
            .ok()
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let da = string_of(doc, &acro, b"DA");
        let mut form = Form {
            fields: Vec::new(),
            need_appearances: doc
                .get_key(&acro, b"NeedAppearances")
                .ok()
                .and_then(|o| o.as_bool())
                .unwrap_or(false),
            has_xfa: acro.get(b"XFA").is_some_and(|x| !x.is_null()),
            sig_flags: doc
                .get_key(&acro, b"SigFlags")
                .ok()
                .and_then(|o| o.as_i64())
                .unwrap_or(0) as u32,
            resources,
            da: da.clone(),
            calculation_order: doc
                .get_key(&acro, b"CO")
                .ok()
                .and_then(|o| {
                    o.as_array()
                        .map(|a| a.iter().filter_map(Object::as_reference).collect())
                })
                .unwrap_or_default(),
        };
        let pages = widget_pages(doc);
        let roots: Vec<Object> = doc
            .get_key(&acro, b"Fields")
            .ok()
            .and_then(|o| o.as_array().map(<[Object]>::to_vec))
            .unwrap_or_default();
        let inherited = Inherited {
            da,
            ..Inherited::default()
        };
        let mut seen = HashSet::new();
        for root in roots {
            let Some(r) = root.as_reference() else {
                continue;
            };
            form.walk(doc, r, &inherited, &pages, &mut seen, 0);
        }
        Ok((!form.fields.is_empty()).then_some(form))
    }

    fn walk(
        &mut self,
        doc: &Document,
        r: ObjRef,
        parent: &Inherited,
        pages: &HashMap<ObjRef, (usize, usize)>,
        seen: &mut HashSet<ObjRef>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH || self.fields.len() >= MAX_FIELDS || !seen.insert(r) {
            return;
        }
        let Ok(Object::Dict(dict)) = doc.get(r) else {
            return;
        };
        let mut here = parent.clone();
        if let Some(t) = string_of(doc, &dict, b"T") {
            here.name = if here.name.is_empty() {
                t
            } else {
                format!("{}.{t}", here.name)
            };
        }
        if let Some(ft) = doc
            .get_key(&dict, b"FT")
            .ok()
            .and_then(|o| o.as_name().cloned())
        {
            here.ft = Some(ft.0);
        }
        if let Some(ff) = doc.get_key(&dict, b"Ff").ok().and_then(|o| o.as_i64()) {
            here.ff = Some(ff as u32);
        }
        for (key, slot) in [
            (&b"V"[..], &mut here.v),
            (b"DV", &mut here.dv),
            (b"Opt", &mut here.opt),
        ] {
            if let Some(v) = dict.get(key).filter(|v| !v.is_null()) {
                *slot = Some(v.clone());
            }
        }
        if let Some(da) = string_of(doc, &dict, b"DA") {
            here.da = Some(da);
        }
        if let Some(q) = doc.get_key(&dict, b"Q").ok().and_then(|o| o.as_i64()) {
            here.q = Some(q.clamp(0, 2) as u8);
        }
        if let Some(m) = doc.get_key(&dict, b"MaxLen").ok().and_then(|o| o.as_i64()) {
            here.max_len = usize::try_from(m).ok();
        }

        // Kids with a /T are fields; kids without one are this field's widgets.
        let kids: Vec<ObjRef> = doc
            .get_key(&dict, b"Kids")
            .ok()
            .and_then(|o| {
                o.as_array()
                    .map(|a| a.iter().filter_map(Object::as_reference).collect())
            })
            .unwrap_or_default();
        let mut widget_kids = Vec::new();
        let mut field_kids = Vec::new();
        for k in kids {
            match doc.get(k) {
                Ok(Object::Dict(d)) if d.get(b"T").is_some() => field_kids.push(k),
                Ok(Object::Dict(d)) => widget_kids.push((k, d)),
                _ => {}
            }
        }
        for k in field_kids {
            self.walk(doc, k, &here, pages, seen, depth + 1);
        }
        let mut widgets: Vec<Widget> = widget_kids
            .into_iter()
            .filter(|(k, _)| seen.insert(*k))
            .filter_map(|(k, d)| widget(doc, k, &d, pages))
            .collect();
        if widgets.is_empty() && (dict.get(b"Rect").is_some() || dict.is_type(b"Annot")) {
            widgets.extend(widget(doc, r, &dict, pages));
        }
        if widgets.is_empty() && dict.get(b"T").is_none() {
            return;
        }
        if widgets.is_empty() && !dict.contains_key(b"FT") && here.ft.is_none() {
            return;
        }
        self.fields.push(field(doc, r, &dict, here, widgets));
    }

    /// The field with this fully qualified name.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The field that a widget belongs to, and the widget's index in it.
    pub fn field_of_widget(&self, widget: ObjRef) -> Option<(usize, usize)> {
        self.fields.iter().enumerate().find_map(|(i, f)| {
            f.widgets
                .iter()
                .position(|w| w.obj == widget)
                .map(|j| (i, j))
        })
    }
}

fn field(doc: &Document, r: ObjRef, dict: &Dict, here: Inherited, widgets: Vec<Widget>) -> Field {
    let flags = here.ff.unwrap_or(0);
    let kind = match here.ft.as_deref() {
        Some(b"Tx") => FieldKind::Text,
        Some(b"Btn") if flags & flags::PUSHBUTTON != 0 => FieldKind::PushButton,
        Some(b"Btn") if flags & flags::RADIO != 0 => FieldKind::Radio,
        Some(b"Btn") => FieldKind::Checkbox,
        Some(b"Ch") if flags & flags::COMBO != 0 => FieldKind::ComboBox,
        Some(b"Ch") => FieldKind::ListBox,
        Some(b"Sig") => FieldKind::Signature,
        _ => FieldKind::Unknown,
    };
    let value = here
        .v
        .as_ref()
        .map(|v| Value::from_object(v, doc))
        .unwrap_or_default();
    let default_value = here
        .dv
        .as_ref()
        .map(|v| Value::from_object(v, doc))
        .unwrap_or_default();
    let options = here
        .opt
        .as_ref()
        .and_then(|o| doc.resolve(o).ok())
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default()
        .iter()
        .filter_map(|o| choice_option(doc, o))
        .collect();
    Field {
        obj: r,
        name: here.name,
        tooltip: string_of(doc, dict, b"TU"),
        kind,
        flags,
        value,
        default_value,
        options,
        max_len: here.max_len,
        da: here.da,
        quadding: here.q.unwrap_or(0),
        top_index: doc
            .get_key(dict, b"TI")
            .ok()
            .and_then(|o| o.as_i64())
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(0),
        widgets,
    }
}

fn choice_option(doc: &Document, o: &Object) -> Option<ChoiceOption> {
    let text = |o: &Object| match doc.resolve(o).ok()? {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(n.as_str_lossy().into_owned()),
        _ => None,
    };
    match doc.resolve(o).ok()? {
        Object::Array(pair) if pair.len() >= 2 => Some(ChoiceOption {
            export: text(&pair[0])?,
            display: text(&pair[1])?,
        }),
        other => {
            let t = text(&other)?;
            Some(ChoiceOption {
                export: t.clone(),
                display: t,
            })
        }
    }
}

fn widget(
    doc: &Document,
    r: ObjRef,
    dict: &Dict,
    pages: &HashMap<ObjRef, (usize, usize)>,
) -> Option<Widget> {
    let rect = doc
        .get_key(dict, b"Rect")
        .ok()
        .and_then(|o| o.as_array().and_then(Rect::from_array))?;
    let on_state = doc
        .get_key(dict, b"AP")
        .ok()
        .and_then(|ap| ap.as_dict().cloned())
        .and_then(|ap| {
            ["N", "D"].iter().find_map(|k| {
                // A stream here is a single appearance, not a set of states.
                let Ok(Object::Dict(states)) = doc.get_key(&ap, k.as_bytes()) else {
                    return None;
                };
                states
                    .iter()
                    .map(|(n, _)| n.as_str_lossy().into_owned())
                    .find(|n| n != "Off")
            })
        });
    let mk = doc
        .get_key(dict, b"MK")
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    let rotation = doc
        .get_key(&mk, b"R")
        .ok()
        .and_then(|o| o.as_i64())
        .map_or(0, |r| r.rem_euclid(360) as u16 / 90 * 90);
    Some(Widget {
        obj: r,
        page: pages.get(&r).map(|p| p.0),
        annot_index: pages.get(&r).map(|p| p.1),
        rect,
        annot_flags: doc
            .get_key(dict, b"F")
            .ok()
            .and_then(|o| o.as_i64())
            .unwrap_or(0) as u32,
        on_state,
        rotation,
    })
}

/// Every annotation that a page lists, mapped to the page's index and its
/// position in the page's /Annots.
fn widget_pages(doc: &Document) -> HashMap<ObjRef, (usize, usize)> {
    let mut out = HashMap::new();
    let Ok(pages) = doc.pages() else {
        return out;
    };
    for (i, page) in pages.iter().enumerate() {
        let Ok(annots) = doc.get_key(&page.dict, b"Annots") else {
            continue;
        };
        for (j, a) in annots.as_array().unwrap_or_default().iter().enumerate() {
            if let Some(r) = a.as_reference() {
                out.entry(r).or_insert((i, j));
            }
        }
    }
    out
}

fn string_of(doc: &Document, dict: &Dict, key: &[u8]) -> Option<String> {
    match doc.get_key(dict, key).ok()? {
        Object::String(s) => Some(s.to_text()),
        _ => None,
    }
}
