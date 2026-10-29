//! A form being filled in: the document as opened, the values changed since,
//! and the document those changes make. Each change rebuilds the edited
//! document from the original as one incremental update, so what is saved
//! is always the original bytes plus a single appended section.

use std::sync::Arc;

use atelier_syntax::{Document, Object, Result};

use crate::{FieldKind, FillWarning, Form, Value};

/// The document with the changes applied.
pub struct Built {
    pub doc: Arc<Document>,
    pub form: Form,
    pub warnings: Vec<FillWarning>,
}

pub struct Session {
    base: Arc<Document>,
    base_form: Form,
    /// Changed fields and their new values, oldest change first, one entry
    /// per field.
    edits: Vec<(usize, Value)>,
}

impl Session {
    /// A session for a document's form, or None when it has none.
    pub fn new(base: Arc<Document>) -> Result<Option<Self>> {
        let Some(base_form) = Form::load(&base)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            base,
            base_form,
            edits: Vec::new(),
        }))
    }

    /// The form as the file has it.
    pub fn original(&self) -> &Form {
        &self.base_form
    }

    /// Records a new value for a field. Setting a field back to the value
    /// it was opened with drops the change.
    pub fn set(&mut self, field: usize, value: Value) {
        self.edits.retain(|(f, _)| *f != field);
        if self
            .base_form
            .fields
            .get(field)
            .is_some_and(|f| !f.value.same_as(&value))
        {
            self.edits.push((field, value));
        }
    }

    /// Whether there are changes to save.
    pub fn is_dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    /// Drops every change.
    pub fn revert(&mut self) {
        self.edits.clear();
    }

    /// The document with the changes, and with every field redrawn when
    /// the file asks for that (/NeedAppearances).
    pub fn build(&self) -> Result<Built> {
        let mut form = self.base_form.clone();
        if self.edits.is_empty() && !form.need_appearances {
            return Ok(Built {
                doc: self.base.clone(),
                form,
                warnings: Vec::new(),
            });
        }
        let mut update = self.base.update();
        let mut warnings = Vec::new();
        if form.need_appearances {
            for i in 0..form.fields.len() {
                // Redrawing fields the viewer cannot fill would only lose
                // their authored look.
                if matches!(
                    form.fields[i].kind,
                    FieldKind::Text
                        | FieldKind::ComboBox
                        | FieldKind::ListBox
                        | FieldKind::Checkbox
                        | FieldKind::Radio
                ) {
                    warnings.extend(form.refresh_appearance(&mut update, i)?);
                }
            }
        }
        for (field, value) in &self.edits {
            warnings.extend(form.set_value(&mut update, *field, value.clone())?);
        }
        let bytes = update.to_bytes()?;
        let doc = self.base.reopen(bytes)?;
        Ok(Built {
            doc: Arc::new(doc),
            form,
            warnings,
        })
    }

    /// After saving: the saved document becomes the new original.
    pub fn saved(&mut self, doc: Arc<Document>) -> Result<()> {
        if let Some(form) = Form::load(&doc)? {
            self.base_form = form;
        }
        self.base = doc;
        self.edits.clear();
        Ok(())
    }

    /// Pages whose widgets show `field`.
    pub fn pages_of(&self, field: usize) -> Vec<usize> {
        let mut pages: Vec<usize> = self
            .base_form
            .fields
            .get(field)
            .map(|f| f.widgets.iter().filter_map(|w| w.page).collect())
            .unwrap_or_default();
        pages.sort_unstable();
        pages.dedup();
        pages
    }
}

impl Form {
    /// Widgets a person can fill, in Tab order: page by page, and within a
    /// page by the page's /Tabs: rows, columns, or the /Annots order.
    pub fn tab_order(&self, doc: &Document) -> Vec<(usize, usize)> {
        let tabs: Vec<Option<u8>> = doc
            .pages()
            .map(|pages| {
                pages
                    .iter()
                    .map(|p| match doc.get_key(&p.dict, b"Tabs") {
                        Ok(Object::Name(n)) => n.0.first().copied(),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            if f.is_read_only() || !f.is_fillable() {
                continue;
            }
            for (j, w) in f.widgets.iter().enumerate() {
                if w.page.is_some() && !w.is_hidden() {
                    out.push((i, j));
                }
            }
        }
        let key = |&(i, j): &(usize, usize)| {
            let w = &self.fields[i].widgets[j];
            let page = w.page.unwrap_or(usize::MAX);
            let top = -(w.rect.y1.max(w.rect.y0) as i64);
            let left = w.rect.x0.min(w.rect.x1) as i64;
            let annot = w.annot_index.unwrap_or(usize::MAX) as i64;
            match tabs.get(page).copied().flatten() {
                Some(b'R') => (page, top, left, annot),
                Some(b'C') => (page, left, top, annot),
                _ => (page, annot, 0, 0),
            }
        };
        out.sort_by_key(key);
        out
    }

    /// The fillable widget at a point in a page's user space, topmost
    /// (last in /Annots) first.
    pub fn widget_at(&self, page: usize, x: f64, y: f64) -> Option<(usize, usize)> {
        let mut best: Option<((usize, usize), usize)> = None;
        for (i, f) in self.fields.iter().enumerate() {
            for (j, w) in f.widgets.iter().enumerate() {
                let r = &w.rect;
                let inside = x >= r.x0.min(r.x1)
                    && x <= r.x0.max(r.x1)
                    && y >= r.y0.min(r.y1)
                    && y <= r.y0.max(r.y1);
                if w.page == Some(page) && inside && !w.is_hidden() {
                    let order = w.annot_index.unwrap_or(0);
                    if best.is_none_or(|(_, o)| order >= o) {
                        best = Some(((i, j), order));
                    }
                }
            }
        }
        best.map(|b| b.0)
    }
}

impl crate::Field {
    /// A kind of field the viewer can fill in.
    pub fn is_fillable(&self) -> bool {
        matches!(
            self.kind,
            FieldKind::Text
                | FieldKind::ComboBox
                | FieldKind::ListBox
                | FieldKind::Checkbox
                | FieldKind::Radio
        ) && !self.has_flag(crate::flags::FILE_SELECT)
    }
}
