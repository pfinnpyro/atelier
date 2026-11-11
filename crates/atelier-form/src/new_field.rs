//! Fields added in the viewer: a text box or a check box drawn onto a
//! page. Until the file is saved an added field can still be renamed,
//! moved or taken away again; it is filled in like any other field.

use atelier_syntax::{
    Dict, Document, Error, Name, ObjRef, Object, PdfString, Rect, Result, Update,
};

use crate::pages::PageSlot;
use crate::{Field, FieldKind, Form, Session, Value, Widget, flags};

/// The default appearance of an added field: Helvetica, black, at a size
/// in points (0 has viewers size the text to fit the box).
fn default_appearance(size: f64) -> String {
    let size = if size.is_finite() {
        size.clamp(0.0, 144.0)
    } else {
        0.0
    };
    format!("/Helv {} Tf 0 g", crate::da::num(size))
}
/// Where a document keeps its own settings for added fields.
const SETTINGS: &[u8] = b"AtelierForm";
/// The state name of a ticked check box.
const ON: &str = "Yes";

/// What kind of field to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewFieldKind {
    /// One line of text.
    Text,
    /// Text that wraps over several lines.
    Multiline,
    Checkbox,
    /// An empty signature field, to be signed.
    Signature,
    /// A short text field for initials, set in the middle of its box.
    Initials,
}

impl NewFieldKind {
    pub const ALL: [Self; 5] = [
        Self::Text,
        Self::Multiline,
        Self::Checkbox,
        Self::Signature,
        Self::Initials,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Multiline => "Text, several lines",
            Self::Checkbox => "Check box",
            Self::Signature => "Signature",
            Self::Initials => "Initials",
        }
    }

    /// Whether the field holds typed text, whose size can be set.
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::Multiline | Self::Initials)
    }
}

/// The mark a ticked check box shows: one of the ZapfDingbats characters
/// viewers know for /MK /CA, which Atelier draws as a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckMark {
    Check,
    Cross,
    /// A filled square with a margin around it.
    #[default]
    Square,
    Dot,
    Diamond,
    Star,
}

impl CheckMark {
    pub const ALL: [Self; 6] = [
        Self::Square,
        Self::Check,
        Self::Cross,
        Self::Dot,
        Self::Diamond,
        Self::Star,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Check => "Check",
            Self::Cross => "Cross",
            Self::Square => "Filled square",
            Self::Dot => "Dot",
            Self::Diamond => "Diamond",
            Self::Star => "Star",
        }
    }

    /// The name it is saved under, in settings and in a document.
    pub fn key(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Cross => "cross",
            Self::Square => "square",
            Self::Dot => "dot",
            Self::Diamond => "diamond",
            Self::Star => "star",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.key().eq_ignore_ascii_case(key.trim()))
    }

    /// The ZapfDingbats character code of the mark.
    pub fn code(self) -> u8 {
        match self {
            Self::Check => b'4',
            Self::Cross => b'8',
            Self::Square => b'n',
            Self::Dot => b'l',
            Self::Diamond => b'u',
            Self::Star => b'H',
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.code() == code)
    }
}

/// A field to add to a page.
#[derive(Debug, Clone, PartialEq)]
pub struct NewField {
    /// The field's name. Names are unique in a form, so the one a field
    /// ends up with may have a number added.
    pub name: String,
    pub kind: NewFieldKind,
    /// Where it goes, in the page's user space.
    pub rect: Rect,
    /// What to tell people the field is for (/TU).
    pub tooltip: Option<String>,
    /// For a check box, the mark it shows when ticked; `None` takes the
    /// document's setting, or failing that a check.
    pub mark: Option<CheckMark>,
    /// For text, the size in points; 0 sizes the text to fit the box.
    pub font_size: f64,
}

impl NewField {
    pub fn new(name: impl Into<String>, kind: NewFieldKind, rect: Rect) -> Self {
        Self {
            name: name.into(),
            kind,
            rect,
            tooltip: None,
            mark: None,
            font_size: 0.0,
        }
    }
}

/// A field added and not yet saved.
#[derive(Debug, Clone)]
pub(crate) struct Added {
    pub slot: PageSlot,
    pub field: NewField,
    /// How far the page was turned when the field was added, so its text
    /// reads the way the page does.
    pub rotation: u16,
}

fn invalid(message: &str) -> Error {
    Error::Structure(message.into())
}

/// A name without the dots that join parent and child names, trimmed.
fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '.' || c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn helvetica() -> Dict {
    let mut font = Dict::new();
    font.insert("Type", Object::Name(Name::new("Font")));
    font.insert("Subtype", Object::Name(Name::new("Type1")));
    font.insert("BaseFont", Object::Name(Name::new("Helvetica")));
    font.insert("Encoding", Object::Name(Name::new("WinAnsiEncoding")));
    font
}

/// A form's default resources with the font added fields are set in.
fn with_helvetica(doc: &Document, resources: &Dict) -> Dict {
    let mut resources = resources.clone();
    let mut fonts = doc
        .get_key(&resources, b"Font")
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    if !fonts.contains_key(b"Helv") {
        fonts.insert("Helv", Object::Dict(helvetica()));
    }
    resources.insert("Font", Object::Dict(fonts));
    resources
}

impl Added {
    /// The field as the form lists it before it is written.
    fn listed(&self, obj: ObjRef) -> Field {
        let check = self.field.kind == NewFieldKind::Checkbox;
        Field {
            obj,
            name: self.field.name.clone(),
            tooltip: self.field.tooltip.clone(),
            kind: match self.field.kind {
                NewFieldKind::Checkbox => FieldKind::Checkbox,
                NewFieldKind::Signature => FieldKind::Signature,
                _ => FieldKind::Text,
            },
            flags: if self.field.kind == NewFieldKind::Multiline {
                flags::MULTILINE
            } else {
                0
            },
            value: Value::None,
            default_value: Value::None,
            options: Vec::new(),
            max_len: None,
            da: Some(default_appearance(self.field.font_size)),
            quadding: u8::from(self.field.kind == NewFieldKind::Initials),
            top_index: 0,
            widgets: vec![Widget {
                obj,
                page: match self.slot {
                    PageSlot::Own(n) => Some(n),
                    PageSlot::Added { .. } => None,
                },
                annot_index: None,
                rect: self.field.rect,
                // Print.
                annot_flags: 4,
                on_state: check.then(|| ON.to_string()),
                rotation: self.rotation,
            }],
        }
    }

    /// The field and its one widget, as a single dictionary.
    fn dict(&self) -> Dict {
        let f = &self.field;
        let mut d = Dict::new();
        d.insert("Type", Object::Name(Name::new("Annot")));
        d.insert("Subtype", Object::Name(Name::new("Widget")));
        d.insert(
            "FT",
            Object::Name(Name::new(match f.kind {
                NewFieldKind::Checkbox => "Btn",
                NewFieldKind::Signature => "Sig",
                _ => "Tx",
            })),
        );
        d.insert("T", Object::String(PdfString::from_text(&f.name)));
        if let Some(t) = &f.tooltip {
            d.insert("TU", Object::String(PdfString::from_text(t)));
        }
        if f.kind == NewFieldKind::Multiline {
            d.insert("Ff", Object::Integer(i64::from(flags::MULTILINE)));
        }
        d.insert(
            "Rect",
            Object::Array(
                [f.rect.x0, f.rect.y0, f.rect.x1, f.rect.y1]
                    .into_iter()
                    .map(Object::Real)
                    .collect(),
            ),
        );
        d.insert("F", Object::Integer(4));
        if f.kind != NewFieldKind::Signature {
            d.insert(
                "DA",
                Object::String(PdfString::from_text(&default_appearance(f.font_size))),
            );
        }
        if f.kind == NewFieldKind::Initials {
            d.insert("Q", Object::Integer(1));
        }
        let mut mk = Dict::new();
        if self.rotation != 0 {
            mk.insert("R", Object::Integer(i64::from(self.rotation)));
        }
        if let Some(mark) = f.mark.filter(|_| f.kind == NewFieldKind::Checkbox) {
            mk.insert("CA", Object::String(PdfString::literal(vec![mark.code()])));
        }
        if !mk.is_empty() {
            d.insert("MK", Object::Dict(mk));
        }
        d
    }
}

impl Session {
    /// The object each added field will be, were the document built now:
    /// they are the first objects a build adds.
    fn renumber_added(&mut self) {
        let next = self.base.next_object_number();
        let own = self.own_fields;
        let Some(form) = &mut self.base_form else {
            return;
        };
        for (i, f) in form.fields.iter_mut().skip(own).enumerate() {
            let r = ObjRef::new(next + i as u32, 0);
            f.obj = r;
            for w in &mut f.widgets {
                w.obj = r;
            }
        }
    }

    /// A name no field has yet: `wanted`, or that with a number after it.
    pub fn unique_field_name(&self, wanted: &str) -> String {
        self.unique_name_but(wanted, None)
    }

    fn unique_name_but(&self, wanted: &str, but: Option<usize>) -> String {
        let wanted = clean_name(wanted);
        let wanted = if wanted.is_empty() {
            "Field".to_string()
        } else {
            wanted
        };
        let taken = |name: &str| {
            self.base_form.as_ref().is_some_and(|f| {
                f.fields.iter().enumerate().any(|(i, f)| {
                    // A field also may not share a name with the parent
                    // of other fields.
                    Some(i) != but && (f.name == name || f.name.split('.').next() == Some(name))
                })
            })
        };
        if !taken(&wanted) {
            return wanted;
        }
        (2..)
            .map(|n| format!("{wanted} {n}"))
            .find(|n| !taken(n))
            .expect("some number is free")
    }

    /// Adds a field to a page and returns its index in the form. The
    /// field's name is made unique.
    pub fn add_field(&mut self, page: usize, field: NewField) -> Result<usize> {
        let slot = *self
            .order
            .get(page)
            .ok_or_else(|| invalid("No such page"))?;
        if crate::document_has_xfa(&self.base) {
            return Err(invalid(
                "This file's form is XFA; fields added to it would not be shown",
            ));
        }
        if self.page(slot).is_none_or(|p| p.obj_ref.is_none()) {
            return Err(invalid("This page cannot take fields"));
        }
        let r = field.rect;
        let rect = Rect {
            x0: r.x0.min(r.x1),
            y0: r.y0.min(r.y1),
            x1: r.x0.max(r.x1),
            y1: r.y0.max(r.y1),
        };
        if ![rect.x0, rect.y0, rect.x1, rect.y1]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 1e7)
            || rect.x1 - rect.x0 < 1.0
            || rect.y1 - rect.y0 < 1.0
        {
            return Err(invalid("A field must be at least a point wide and high"));
        }
        let added = Added {
            slot,
            rotation: self.rotation(page),
            field: NewField {
                name: self.unique_field_name(&field.name),
                rect,
                tooltip: field.tooltip.filter(|t| !t.trim().is_empty()),
                kind: field.kind,
                mark: field.mark.or(self.document_mark),
                font_size: field.font_size,
            },
        };
        let base = self.base.clone();
        let form = self.base_form.get_or_insert_with(|| Form {
            fields: Vec::new(),
            need_appearances: false,
            has_xfa: false,
            sig_flags: 0,
            resources: Dict::new(),
            da: None,
            calculation_order: Vec::new(),
        });
        form.resources = with_helvetica(&base, &form.resources);
        let index = form.fields.len();
        form.fields.push(added.listed(ObjRef::new(0, 0)));
        self.touch(
            format!("Field: {} (added)", added.field.name),
            String::new(),
            format!("{} field", added.field.kind.label()),
            Some(page),
        );
        self.added.push(added);
        self.renumber_added();
        Ok(index)
    }

    /// Whether a field was added since the file was opened or saved, so
    /// it can still be renamed or removed.
    pub fn is_added_field(&self, field: usize) -> bool {
        field >= self.own_fields && field - self.own_fields < self.added.len()
    }

    /// How many fields were added and not yet saved.
    pub fn added_field_count(&self) -> usize {
        self.added.len()
    }

    pub(crate) fn added_slot(&self, field: usize) -> Option<PageSlot> {
        self.added
            .get(field.checked_sub(self.own_fields)?)
            .map(|a| a.slot)
    }

    /// Renames an added field. The name it gets is returned.
    pub fn rename_added_field(&mut self, field: usize, name: &str) -> Result<String> {
        if !self.is_added_field(field) {
            return Err(invalid(
                "Only fields added since the last save can be renamed",
            ));
        }
        let name = self.unique_name_but(name, Some(field));
        let i = field - self.own_fields;
        let before = std::mem::replace(&mut self.added[i].field.name, name.clone());
        if let Some(f) = self
            .base_form
            .as_mut()
            .and_then(|f| f.fields.get_mut(field))
        {
            f.name = name.clone();
        }
        if before != name {
            let page = self.pages_of(field).first().copied();
            self.touch(
                format!("Field name: {name} (added)"),
                before,
                name.clone(),
                page,
            );
        }
        Ok(name)
    }

    /// Takes an added field away again, with whatever was filled into it.
    /// Returns the page it was on. Fields after it move up by one.
    pub fn remove_added_field(&mut self, field: usize) -> Option<usize> {
        if !self.is_added_field(field) {
            return None;
        }
        let page = self.pages_of(field).first().copied();
        let name = self.added[field - self.own_fields].field.name.clone();
        self.touch(
            format!("Field: {name} (added)"),
            "field".into(),
            String::new(),
            page,
        );
        self.drop_added(field - self.own_fields);
        page
    }

    /// Forgets the `i`th added field and renumbers what named later ones.
    pub(crate) fn drop_added(&mut self, i: usize) {
        let field = self.own_fields + i;
        self.added.remove(i);
        if let Some(form) = &mut self.base_form {
            form.fields.remove(field);
        }
        let moved = |f: usize| if f > field { f - 1 } else { f };
        self.edits.retain(|(f, _)| *f != field);
        for (f, _) in &mut self.edits {
            *f = moved(*f);
        }
        self.hidden = std::mem::take(&mut self.hidden)
            .into_iter()
            .filter(|(f, _)| *f != field)
            .map(|(f, v)| (moved(f), v))
            .collect();
        self.styles = std::mem::take(&mut self.styles)
            .into_iter()
            .filter(|(f, _)| *f != field)
            .map(|(f, v)| (moved(f), v))
            .collect();
        self.marks = std::mem::take(&mut self.marks)
            .into_iter()
            .filter(|(f, _)| *f != field)
            .map(|(f, v)| (moved(f), v))
            .collect();
        self.bounds = std::mem::take(&mut self.bounds)
            .into_iter()
            .filter(|((f, _), _)| *f != field)
            .map(|((f, w), v)| ((moved(f), w), v))
            .collect();
        if self.added.is_empty() && self.own_fields == 0 {
            // The file had no form, and has none again.
            self.base_form = None;
        }
        self.renumber_added();
    }

    /// Writes the added fields into the update: their dictionaries (the
    /// first objects added, see [`Session::renumber_added`]), the form
    /// that lists them and their appearances. `form` gets the objects.
    pub(crate) fn write_added(&self, form: &mut Form, update: &mut Update<'_>) -> Result<()> {
        if self.added.is_empty() {
            return Ok(());
        }
        let mut refs = Vec::with_capacity(self.added.len());
        for (i, a) in self.added.iter().enumerate() {
            let r = update.add(Object::Dict(a.dict()));
            let f = &mut form.fields[self.own_fields + i];
            f.obj = r;
            f.widgets[0].obj = r;
            // No appearance yet: the first refresh draws one.
            f.widgets[0].on_state = None;
            refs.push(r);
        }
        let catalog_ref = match update.document().trailer().get(b"Root") {
            Some(Object::Reference(r)) => *r,
            _ => return Err(invalid("the catalog is not an object of its own")),
        };
        let Object::Dict(mut catalog) = update.get(catalog_ref)? else {
            return Err(invalid("the catalog is not a dictionary"));
        };
        // The form, the list of its fields: each in place, or made anew.
        let (mut acro, acro_ref) = match catalog.get(b"AcroForm") {
            Some(Object::Reference(r)) => match update.get(*r)? {
                Object::Dict(d) => (d, Some(*r)),
                _ => (Dict::new(), None),
            },
            Some(Object::Dict(d)) => (d.clone(), None),
            _ => (Dict::new(), None),
        };
        let (mut list, list_ref) = match acro.get(b"Fields") {
            Some(Object::Reference(r)) => match update.get(*r)? {
                Object::Array(a) => (a, Some(*r)),
                _ => (Vec::new(), None),
            },
            Some(Object::Array(a)) => (a.clone(), None),
            _ => (Vec::new(), None),
        };
        list.extend(refs.iter().map(|r| Object::Reference(*r)));
        match list_ref {
            Some(r) => update.set(r, Object::Array(list)),
            None => acro.insert("Fields", Object::Array(list)),
        }
        acro.insert("DR", Object::Dict(form.resources.clone()));
        if acro.get(b"DA").is_none() {
            acro.insert(
                "DA",
                Object::String(PdfString::from_text(&default_appearance(0.0))),
            );
        }
        if self
            .added
            .iter()
            .any(|a| a.field.kind == NewFieldKind::Signature)
        {
            // SignaturesExist: the form has signature fields.
            form.sig_flags |= 1;
            acro.insert("SigFlags", Object::Integer(i64::from(form.sig_flags)));
        }
        match acro_ref {
            Some(r) => update.set(r, Object::Dict(acro)),
            None => {
                catalog.insert("AcroForm", Object::Dict(acro));
                update.set(catalog_ref, Object::Dict(catalog));
            }
        }
        for (i, a) in self.added.iter().enumerate() {
            let index = self.own_fields + i;
            form.refresh_appearance(update, index)?;
            if a.field.kind == NewFieldKind::Checkbox {
                form.fields[index].widgets[0].on_state = Some(ON.into());
            }
        }
        Ok(())
    }

    /// The mark this document's added check boxes show, when it has a
    /// setting of its own.
    pub fn document_mark(&self) -> Option<CheckMark> {
        self.document_mark
    }

    /// Sets (or with `None` drops) the document's own mark for check
    /// boxes added from now on. It is saved in the file.
    pub fn set_document_mark(&mut self, mark: Option<CheckMark>) {
        if self.document_mark != mark {
            let name = |m: Option<CheckMark>| m.map_or("", CheckMark::label).to_string();
            self.touch(
                "Check box mark for new fields".into(),
                name(self.document_mark),
                name(mark),
                None,
            );
            self.document_mark = mark;
        }
    }

    pub(crate) fn saved_document_mark(doc: &Document) -> Option<CheckMark> {
        let catalog = doc.catalog().ok()?;
        let settings = doc.get_key(&catalog, SETTINGS).ok()?;
        let mark = doc.get_key(settings.as_dict()?, b"CheckMark").ok()?;
        CheckMark::parse(&mark.as_name()?.as_str_lossy())
    }

    pub(crate) fn document_mark_changed(&self) -> bool {
        self.document_mark != Self::saved_document_mark(&self.base)
    }

    /// Writes the document's setting into the catalog.
    pub(crate) fn write_document_mark(&self, update: &mut Update<'_>) -> Result<()> {
        if !self.document_mark_changed() {
            return Ok(());
        }
        let catalog_ref = match update.document().trailer().get(b"Root") {
            Some(Object::Reference(r)) => *r,
            _ => return Err(invalid("the catalog is not an object of its own")),
        };
        let Object::Dict(mut catalog) = update.get(catalog_ref)? else {
            return Err(invalid("the catalog is not a dictionary"));
        };
        match self.document_mark {
            Some(mark) => {
                let mut settings = Dict::new();
                settings.insert("CheckMark", Object::Name(Name::new(mark.key())));
                catalog.insert(Name::new(SETTINGS), Object::Dict(settings));
            }
            None => {
                catalog.remove(SETTINGS);
            }
        }
        update.set(catalog_ref, Object::Dict(catalog));
        Ok(())
    }

    /// The mark a check box shows when ticked, where it is one Atelier
    /// knows: as changed, as added, or as the file has it.
    pub fn check_mark(&self, field: usize) -> Option<CheckMark> {
        if let Some(mark) = self.marks.get(&field) {
            return Some(*mark);
        }
        if let Some(a) = field
            .checked_sub(self.own_fields)
            .and_then(|i| self.added.get(i))
        {
            return (a.field.kind == NewFieldKind::Checkbox)
                .then(|| a.field.mark.unwrap_or(CheckMark::Check));
        }
        let f = self.base_form.as_ref()?.fields.get(field)?;
        if f.kind != FieldKind::Checkbox {
            return None;
        }
        let Ok(Object::Dict(widget)) = self.base.get(f.widgets.first()?.obj) else {
            return None;
        };
        let mk = self.base.get_key(&widget, b"MK").ok()?;
        match self.base.get_key(mk.as_dict()?, b"CA").ok()? {
            Object::String(s) => CheckMark::from_code(*s.bytes.first()?),
            _ => None,
        }
    }

    /// Changes the mark a check box shows when ticked, and has its
    /// appearance drawn again with it.
    pub fn set_check_mark(&mut self, field: usize, mark: CheckMark) -> Result<()> {
        let f = self
            .base_form
            .as_ref()
            .and_then(|f| f.fields.get(field))
            .ok_or_else(|| invalid("Unknown field"))?;
        if f.kind != FieldKind::Checkbox || f.is_read_only() {
            return Err(invalid(
                "Only a check box that can be filled in has a mark to change",
            ));
        }
        if self.check_mark(field) == Some(mark) {
            return Ok(());
        }
        let before = self.check_mark(field).map_or("", CheckMark::label);
        let target = format!("Field: {} ({}) / mark", f.name, f.obj);
        let page = self.pages_of(field).first().copied();
        self.touch(target, before.into(), mark.label().into(), page);
        match field
            .checked_sub(self.own_fields)
            .and_then(|i| self.added.get_mut(i))
        {
            Some(a) => a.field.mark = Some(mark),
            None => {
                self.marks.insert(field, mark);
            }
        }
        Ok(())
    }

    /// Gives every check box that can be filled in the same mark.
    /// Returns how many changed.
    pub fn set_all_check_marks(&mut self, mark: CheckMark) -> usize {
        let boxes: Vec<usize> = self
            .base_form
            .iter()
            .flat_map(|f| f.fields.iter().enumerate())
            .filter(|(_, f)| f.kind == FieldKind::Checkbox && !f.is_read_only())
            .map(|(i, _)| i)
            .collect();
        boxes
            .into_iter()
            .filter(|&i| self.check_mark(i) != Some(mark) && self.set_check_mark(i, mark).is_ok())
            .count()
    }

    /// Draws again the check boxes whose mark was changed.
    pub(crate) fn apply_marks(&self, form: &mut Form, update: &mut Update<'_>) -> Result<()> {
        for (&field, mark) in &self.marks {
            let Some(f) = form.fields.get(field) else {
                continue;
            };
            for w in &f.widgets {
                let Object::Dict(mut d) = update.get(w.obj)? else {
                    continue;
                };
                let mut mk = match d.get(b"MK") {
                    Some(mk) => match update.resolve(mk)? {
                        Object::Dict(mk) => mk,
                        _ => Dict::new(),
                    },
                    None => Dict::new(),
                };
                mk.insert("CA", Object::String(PdfString::literal(vec![mark.code()])));
                d.insert("MK", Object::Dict(mk));
                update.set(w.obj, Object::Dict(d));
            }
            form.redraw_check(update, field)?;
        }
        Ok(())
    }

    /// Puts each added field on its page, once the pages are placed.
    pub(crate) fn place_added(
        &self,
        form: &mut Form,
        update: &mut Update<'_>,
        pages: &std::collections::HashMap<PageSlot, ObjRef>,
    ) -> Result<()> {
        for (i, a) in self.added.iter().enumerate() {
            let Some(&page_ref) = pages.get(&a.slot) else {
                continue;
            };
            let Some(f) = form.fields.get_mut(self.own_fields + i) else {
                continue;
            };
            let Object::Dict(mut d) = update.get(f.obj)? else {
                continue;
            };
            d.insert("P", Object::Reference(page_ref));
            update.set(f.obj, Object::Dict(d));
            let Object::Dict(mut page) = update.get(page_ref)? else {
                continue;
            };
            let mut list = match page.get(b"Annots") {
                Some(a) => match update.resolve(a)? {
                    Object::Array(items) => items,
                    _ => Vec::new(),
                },
                None => Vec::new(),
            };
            f.widgets[0].annot_index = Some(list.len());
            list.push(Object::Reference(f.obj));
            page.insert("Annots", Object::Array(list));
            update.set(page_ref, Object::Dict(page));
        }
        Ok(())
    }
}
