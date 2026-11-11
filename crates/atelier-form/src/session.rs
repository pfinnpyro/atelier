//! A document being edited: the document as opened, the changes made since
//! (field values, page turns and the order of the pages), and the document those changes make.
//! Each change rebuilds the edited document from the original as one
//! incremental update, so what is saved is always the original bytes plus
//! a single appended section.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use atelier_render::PageText;
use atelier_syntax::{Dict, Document, ObjRef, Object, Page, Result, Update};

use crate::pages::{self, Copier, PageSlot};
use crate::{FieldKind, FillWarning, Form, NewAnnot, Value, Widget};

/// The document with the changes applied.
pub struct Built {
    pub doc: Arc<Document>,
    /// The form with its new values, when the document has one.
    pub form: Option<Form>,
    pub warnings: Vec<FillWarning>,
}

pub struct Session {
    pub(crate) base: Arc<Document>,
    history: crate::revisions::History,
    savepoint: Arc<Document>,
    /// The file's form, followed by the fields added since.
    pub(crate) base_form: Option<Form>,
    /// How many of the form's fields are the file's own.
    pub(crate) own_fields: usize,
    /// Fields added, oldest first: the form's fields from `own_fields` on.
    pub(crate) added: Vec<crate::new_field::Added>,
    /// Changed fields and their new values, oldest change first, one entry
    /// per field.
    pub(crate) edits: Vec<(usize, Value)>,
    /// The pages, in order: where each comes from.
    pub(crate) order: Vec<PageSlot>,
    /// Files whose pages were added.
    files: Vec<Arc<Document>>,
    /// Pages turned to a new /Rotate.
    rotations: BTreeMap<PageSlot, u16>,
    /// Turns that do not count as unsaved changes (made by auto-orient
    /// when the file opened); they are still saved with the file.
    quiet: BTreeMap<PageSlot, u16>,
    /// Invisible text to add over scanned pages (what OCR read).
    /// Saved with the file but not counted as an unsaved change.
    layers: BTreeMap<PageSlot, Arc<PageText>>,
    /// Annotations added, oldest first, with their page.
    annots: Vec<(PageSlot, NewAnnot)>,
    review_edits: BTreeMap<(PageSlot, usize), crate::Review>,
    removed_annots: BTreeSet<(PageSlot, usize)>,
    annot_history: Vec<AnnotSnapshot>,
    /// Fields shown (false) or hidden (true) by form scripts, by index.
    pub(crate) hidden: BTreeMap<usize, bool>,
    pub(crate) styles: BTreeMap<usize, crate::FieldStyle>,
    pub(crate) bounds: BTreeMap<(usize, usize), atelier_syntax::Rect>,
    /// Check boxes given another mark, by field.
    pub(crate) marks: BTreeMap<usize, crate::CheckMark>,
    /// The document's own mark for check boxes added to it.
    pub(crate) document_mark: Option<crate::CheckMark>,
}

struct AnnotSnapshot {
    page: PageSlot,
    annots: Vec<(PageSlot, NewAnnot)>,
    reviews: BTreeMap<(PageSlot, usize), crate::Review>,
    removed: BTreeSet<(PageSlot, usize)>,
}

/// The pages of a document as opened, in their own order.
fn own_order(doc: &Document) -> Vec<PageSlot> {
    let count = doc.pages().map_or(0, <[_]>::len);
    (0..count).map(PageSlot::Own).collect()
}

impl Session {
    pub(crate) fn base_document(&self) -> &Document {
        &self.base
    }
    /// A session for a document. A form that cannot be read is left out
    /// rather than stopping the document from opening.
    pub fn new(base: Arc<Document>) -> Self {
        let base_form = Form::load(&base).ok().flatten();
        Self {
            order: own_order(&base),
            savepoint: base.clone(),
            history: crate::revisions::History::load(&base),
            own_fields: base_form.as_ref().map_or(0, |f| f.fields.len()),
            added: Vec::new(),
            base_form,
            edits: Vec::new(),
            files: Vec::new(),
            rotations: BTreeMap::new(),
            quiet: BTreeMap::new(),
            layers: BTreeMap::new(),
            annots: Vec::new(),
            review_edits: BTreeMap::new(),
            removed_annots: BTreeSet::new(),
            annot_history: Vec::new(),
            hidden: BTreeMap::new(),
            styles: BTreeMap::new(),
            bounds: BTreeMap::new(),
            marks: BTreeMap::new(),
            document_mark: Self::saved_document_mark(&base),
            base,
        }
    }

    /// Group subsequent edits into one reviewable revision. Reopening ends the group.
    pub fn begin_revision_session(&mut self) {
        self.history.grouping = true;
        self.history.group_id = None;
    }
    pub fn end_revision_session(&mut self) {
        self.history.grouping = false;
        self.history.group_id = None;
    }
    pub fn revision_session_active(&self) -> bool {
        self.history.grouping
    }
    pub fn tracking_enabled(&self) -> bool {
        self.history.enabled
    }
    pub fn tracking_problem(&self) -> Option<&str> {
        self.history.problem.as_deref()
    }
    pub fn revisions(&self) -> &[crate::Revision] {
        &self.history.revisions
    }
    pub fn set_tracking(&mut self, enabled: bool, author: &str) -> Result<()> {
        if let Some(problem) = &self.history.problem {
            return Err(atelier_syntax::Error::Structure(problem.clone()));
        }
        self.end_revision_session();
        self.history.enabled = enabled;
        self.history.author = if author.trim().is_empty() {
            "Reviewer".into()
        } else {
            author.trim().to_owned()
        };
        self.history.dirty = true;
        Ok(())
    }
    pub fn set_revision_author(&mut self, author: &str) {
        let author = if author.trim().is_empty() {
            "Reviewer"
        } else {
            author.trim()
        };
        if self.history.enabled && self.history.author != author {
            self.history.author = author.to_owned();
            self.history.dirty = true;
        }
    }
    fn saved_review_summary(&self, slot: PageSlot, ordinal: usize) -> String {
        let source = match slot {
            PageSlot::Own(_) => &self.base,
            PageSlot::Added { file, .. } => &self.files[file],
        };
        let read = || -> Option<String> {
            let page = self.page(slot)?;
            let Object::Array(list) = source.get_key(&page.dict, b"Annots").ok()? else {
                return None;
            };
            let d = source.resolve_dict(list.get(ordinal)?).ok()?;
            let text = |key: &[u8]| match source.get_key(&d, key).ok() {
                Some(Object::String(s)) => s.to_text(),
                _ => String::new(),
            };
            Some(Self::review_summary(&crate::Review {
                text: text(b"Contents"),
                author: text(b"T"),
                subject: text(b"Subj"),
                created: text(b"CreationDate"),
                modified: text(b"M"),
                state: Some(text(b"State")).filter(|s| !s.is_empty()),
            }))
        };
        read().unwrap_or_else(|| "Saved comment".into())
    }
    pub fn revision_undo_id(&self) -> Option<u64> {
        self.history.undo_id()
    }
    pub fn revision_redo(&self) -> Option<(u64, crate::RevisionState)> {
        self.history.redo_id()
    }
    /// Build one user transaction and record its original/new document state.
    pub fn build_tracked(&mut self, before: &Document) -> Result<Built> {
        let mut built = self.build_content()?;
        let mut history = self.history.clone();
        history.record(before, &built.doc)?;
        built.doc = history.write(built.doc)?;
        history.pending.clear();
        self.history = history;
        Ok(built)
    }
    pub fn decide_revision(
        &mut self,
        current: &Document,
        id: u64,
        state: crate::RevisionState,
        author: &str,
    ) -> Result<()> {
        let mut history = self.history.clone();
        let restore = history.decide(current, id, state, author)?;
        if let Some(doc) = restore {
            let savepoint = self.savepoint.clone();
            *self = Self::new(doc);
            self.savepoint = savepoint;
        }
        self.history = history;
        Ok(())
    }
    pub(crate) fn touch(
        &mut self,
        target: String,
        before: String,
        after: String,
        page: Option<usize>,
    ) {
        self.history.touch(target, before, after, page);
    }
    fn review_summary(review: &crate::Review) -> String {
        format!(
            "{}: {}\n{}\n{} [{}]",
            review.author,
            review.subject,
            review.text,
            review.modified,
            review.state.as_deref().unwrap_or("Pending")
        )
    }
    fn annot_summary(a: &NewAnnot) -> String {
        match &a.markup {
            crate::Markup::Note { review, .. } | crate::Markup::Comment { review, .. } => format!(
                "{}: {} [{}]",
                review.author,
                review.text,
                review.state.as_deref().unwrap_or("Pending")
            ),
            crate::Markup::Highlight { .. } => "Highlight".into(),
            crate::Markup::TextLine { strike, .. } => {
                if *strike { "Strikeout" } else { "Underline" }.into()
            }
            crate::Markup::Ink { .. } => "Ink / signature".into(),
        }
    }

    /// Whether a field's widgets are hidden, with the changes.
    pub fn is_hidden(&self, field: usize) -> bool {
        self.hidden
            .get(&field)
            .copied()
            .unwrap_or_else(|| self.hidden_at_first(field))
    }

    fn hidden_at_first(&self, field: usize) -> bool {
        self.base_form
            .as_ref()
            .and_then(|f| f.fields.get(field))
            .is_some_and(|f| !f.widgets.is_empty() && f.widgets.iter().all(Widget::is_hidden))
    }

    /// Shows or hides a field's widgets (as form scripts do).
    pub fn set_hidden(&mut self, field: usize, hidden: bool) {
        if let Some(f) = self
            .base_form
            .as_ref()
            .and_then(|form| form.fields.get(field))
        {
            self.touch(
                format!("Field visibility: {} ({})", f.name, f.obj),
                self.is_hidden(field).to_string(),
                hidden.to_string(),
                self.pages_of(field).first().copied(),
            );
        }
        if hidden == self.hidden_at_first(field) {
            self.hidden.remove(&field);
        } else {
            self.hidden.insert(field, hidden);
        }
    }

    /// Adds an annotation (a highlight, comment or ink) to a page.
    pub fn add_annot(&mut self, page: usize, annot: NewAnnot) {
        if let Some(&slot) = self.order.get(page) {
            self.remember_annots(slot);
            self.touch(
                format!(
                    "Annotation on page {} ({slot:?}, added {})",
                    page + 1,
                    self.annots.len()
                ),
                String::new(),
                Self::annot_summary(&annot),
                Some(page),
            );
            self.annots.push((slot, annot));
        }
    }

    /// Maps a visible saved-annotation ordinal back to the original array.
    pub fn original_annot_ordinal(&self, page: usize, visible: usize) -> Option<usize> {
        let slot = *self.order.get(page)?;
        (0..)
            .filter(|n| !self.removed_annots.contains(&(slot, *n)))
            .nth(visible)
    }

    /// Removes an original annotation, preserving ordinals for other edits.
    pub fn remove_saved_annot(&mut self, page: usize, ordinal: usize) {
        if let Some(&slot) = self.order.get(page) {
            self.remember_annots(slot);
            self.removed_annots.insert((slot, ordinal));
            self.review_edits.remove(&(slot, ordinal));
        }
    }

    /// Edit a saved review by its ordinal in the original page's Annots array.
    /// The page slot follows reordering and imported pages through copying.
    pub fn set_review(&mut self, page: usize, ordinal: usize, review: crate::Review) {
        if let Some(&slot) = self.order.get(page) {
            self.remember_annots(slot);
            let before = self
                .review_edits
                .get(&(slot, ordinal))
                .map(Self::review_summary)
                .unwrap_or_else(|| self.saved_review_summary(slot, ordinal));
            self.touch(
                format!(
                    "Comment on page {} ({slot:?}, annotation {ordinal})",
                    page + 1
                ),
                before,
                Self::review_summary(&review),
                Some(page),
            );
            self.review_edits.insert((slot, ordinal), review);
        }
    }

    pub fn set_added_review(&mut self, index: usize, review: crate::Review) -> bool {
        if let Some(&(slot, _)) = self.annots.get(index) {
            self.remember_annots(slot);
        }
        if let Some((slot, annot)) = self.annots.get(index) {
            if let Some(old) = annot.review() {
                let before = Self::review_summary(old);
                let page = self.order.iter().position(|s| *s == *slot);
                self.touch(
                    format!("Added comment {index} ({slot:?})"),
                    before,
                    Self::review_summary(&review),
                    page,
                );
            }
        }
        self.annots
            .get_mut(index)
            .is_some_and(|(_, annot)| annot.set_review(review))
    }

    fn remember_annots(&mut self, page: PageSlot) {
        self.annot_history.push(AnnotSnapshot {
            page,
            annots: self.annots.clone(),
            reviews: self.review_edits.clone(),
            removed: self.removed_annots.clone(),
        });
        // Bound history memory for long review sessions.
        if self.annot_history.len() > 100 {
            self.annot_history.remove(0);
        }
    }

    /// Undoes the most recent annotation addition, removal or review edit.
    pub fn undo_annot(&mut self) -> Option<usize> {
        let old = self.annot_history.pop()?;
        self.touch(
            format!("Undo annotation edit ({:?})", old.page),
            String::new(),
            String::new(),
            self.order.iter().position(|s| *s == old.page),
        );
        self.annots = old.annots;
        self.review_edits = old.reviews;
        self.removed_annots = old.removed;
        self.order.iter().position(|s| *s == old.page)
    }

    /// Removes an added annotation, by its place in [`Session::added_annots`].
    pub fn remove_annot(&mut self, index: usize) -> Option<usize> {
        if index >= self.annots.len() {
            return None;
        }
        self.remember_annots(self.annots[index].0);
        let (slot, annot) = self.annots.remove(index);
        let page = self.order.iter().position(|s| *s == slot);
        self.touch(
            format!("Remove annotation {index} ({slot:?})"),
            Self::annot_summary(&annot),
            String::new(),
            page,
        );
        self.order.iter().position(|s| *s == slot)
    }

    /// The annotations added so far, with the page each is on now.
    pub fn added_annots(&self) -> Vec<(usize, &NewAnnot)> {
        self.annots
            .iter()
            .filter_map(|(slot, a)| Some((self.order.iter().position(|s| s == slot)?, a)))
            .collect()
    }

    /// The number of pages with the changes.
    pub fn page_count(&self) -> usize {
        self.order.len()
    }

    /// Whether pages were moved, removed or added.
    pub fn pages_changed(&self) -> bool {
        !self.files.is_empty() || self.order != own_order(&self.base)
    }

    /// The page a slot names, from the file it comes from.
    pub(crate) fn page(&self, slot: PageSlot) -> Option<&Page> {
        match slot {
            PageSlot::Own(n) => self.base.pages().ok()?.get(n),
            PageSlot::Added { file, page } => self.files.get(file)?.pages().ok()?.get(page),
        }
    }

    /// Adds invisible text over a page, such as the words OCR read from a
    /// scan, so it is saved with the file and other apps can search it.
    pub fn set_text_layer(&mut self, page: usize, text: Arc<PageText>) {
        if let Some(&slot) = self.order.get(page) {
            self.layers.insert(slot, text);
        }
    }

    /// Whether a page has a text layer waiting to be saved.
    pub fn has_text_layer(&self, page: usize) -> bool {
        self.order
            .get(page)
            .is_some_and(|s| self.layers.contains_key(s))
    }

    /// The form as the file has it, followed by any fields added since.
    pub fn original(&self) -> Option<&Form> {
        self.base_form.as_ref()
    }

    /// Records a new value for a field. Setting a field back to the value
    /// it was opened with drops the change.
    pub fn set(&mut self, field: usize, value: Value) {
        if let Some(f) = self
            .base_form
            .as_ref()
            .and_then(|form| form.fields.get(field))
        {
            let before = self
                .edits
                .iter()
                .find(|(i, _)| *i == field)
                .map_or(&f.value, |(_, v)| v)
                .display();
            self.touch(
                format!("Field: {} ({})", f.name, f.obj),
                before,
                value.display(),
                self.pages_of(field).first().copied(),
            );
        }
        self.edits.retain(|(f, _)| *f != field);
        let changed = self
            .base_form
            .as_ref()
            .and_then(|f| f.fields.get(field))
            .is_some_and(|f| !f.value.same_as(&value));
        if changed {
            self.edits.push((field, value));
        }
    }

    /// A page's rotation (/Rotate, clockwise degrees) with the changes.
    pub fn rotation(&self, page: usize) -> u16 {
        let Some(&slot) = self.order.get(page) else {
            return 0;
        };
        self.rotations
            .get(&slot)
            .copied()
            .unwrap_or_else(|| self.original_rotation(slot))
    }

    fn original_rotation(&self, slot: PageSlot) -> u16 {
        self.page(slot).map_or(0, |p| p.rotate)
    }

    /// Turns a page to `degrees` clockwise (any multiple of 90). Turning it
    /// back to how it was drops the change.
    pub fn rotate(&mut self, page: usize, degrees: i32) {
        let Some(&slot) = self.order.get(page) else {
            return;
        };
        let r = degrees.rem_euclid(360) as u16 / 90 * 90;
        self.touch(
            format!("Rotate page {} ({slot:?})", page + 1),
            self.rotation(page).to_string(),
            r.to_string(),
            Some(page),
        );
        if r == self.original_rotation(slot) {
            self.rotations.remove(&slot);
        } else {
            self.rotations.insert(slot, r);
        }
    }

    /// Makes the page turns so far not count as unsaved changes. They
    /// are still part of the document and saved with it.
    pub fn quiet_rotations(&mut self) {
        self.quiet = self.rotations.clone();
        self.history.pending.clear();
    }

    /// Moves `pages` (current indices) so they sit together, in their
    /// current order, before the page now at `to` (or at the end when
    /// `to` is the page count). Returns, for each new page, the index it
    /// had before.
    pub fn move_pages(&mut self, pages: &[usize], to: usize) -> Vec<usize> {
        let moving: BTreeSet<usize> = pages
            .iter()
            .copied()
            .filter(|&p| p < self.order.len())
            .collect();
        let before = to.min(self.order.len());
        let mut old: Vec<usize> = Vec::with_capacity(self.order.len());
        for i in 0..=self.order.len() {
            if i == before {
                old.extend(moving.iter().copied());
            }
            if i < self.order.len() && !moving.contains(&i) {
                old.push(i);
            }
        }
        let before = format!("{:?}", self.order);
        self.order = old.iter().map(|&i| self.order[i]).collect();
        self.touch(
            "Page order".into(),
            before,
            format!("{:?}", self.order),
            pages.iter().min().copied(),
        );
        old
    }

    /// Removes pages. At least one page always stays: returns `None`, and
    /// changes nothing, when every page would go. Otherwise returns, for
    /// each page left, the index it had before.
    pub fn delete_pages(&mut self, pages: &[usize]) -> Option<Vec<usize>> {
        let gone: BTreeSet<usize> = pages.iter().copied().collect();
        let old: Vec<usize> = (0..self.order.len())
            .filter(|i| !gone.contains(i))
            .collect();
        if old.is_empty() {
            return None;
        }
        let before = format!("{:?}", self.order);
        self.order = old.iter().map(|&i| self.order[i]).collect();
        self.touch(
            "Delete pages".into(),
            before,
            format!("{:?}", self.order),
            old.first().copied(),
        );
        let kept: BTreeSet<PageSlot> = self.order.iter().copied().collect();
        self.rotations.retain(|s, _| kept.contains(s));
        self.quiet.retain(|s, _| kept.contains(s));
        self.layers.retain(|s, _| kept.contains(s));
        self.annots.retain(|(s, _)| kept.contains(s));
        self.review_edits.retain(|(slot, _), _| kept.contains(slot));
        self.removed_annots.retain(|(slot, _)| kept.contains(slot));
        // Fields added to the pages go with them.
        for i in (0..self.added.len()).rev() {
            if !kept.contains(&self.added[i].slot) {
                self.drop_added(i);
            }
        }
        Some(old)
    }

    /// Adds every page of another file before page `at` (or at the end).
    /// Returns, for each page, the index it had before, or `None` for the
    /// new ones.
    pub fn insert_file(&mut self, doc: Arc<Document>, at: usize) -> Result<Vec<Option<usize>>> {
        let count = doc.pages()?.len();
        let file = self.files.len();
        self.files.push(doc);
        let at = at.min(self.order.len());
        let added = (0..count).map(|page| PageSlot::Added { file, page });
        let before = format!("{:?}", self.order);
        self.order.splice(at..at, added);
        self.touch(
            "Insert PDF pages".into(),
            before,
            format!("{:?}", self.order),
            Some(at),
        );
        Ok((0..self.order.len())
            .map(|i| match i {
                i if i < at => Some(i),
                i if i < at + count => None,
                i => Some(i - count),
            })
            .collect())
    }

    /// Whether there are changes to save.
    pub fn is_dirty(&self) -> bool {
        !self.edits.is_empty()
            || self.rotations != self.quiet
            || self.pages_changed()
            || !self.annots.is_empty()
            || !self.review_edits.is_empty()
            || !self.removed_annots.is_empty()
            || !self.hidden.is_empty()
            || !self.styles.is_empty()
            || !self.bounds.is_empty()
            || !self.added.is_empty()
            || !self.marks.is_empty()
            || self.document_mark_changed()
            || self.history.dirty
    }

    /// Drops every change.
    pub fn revert(&mut self) {
        *self = Self::new(self.savepoint.clone());
    }

    /// The document with the changes, and with every field redrawn when
    /// the file asks for that (/NeedAppearances).
    pub fn build(&self) -> Result<Built> {
        let mut built = self.build_content()?;
        built.doc = self.history.write(built.doc)?;
        Ok(built)
    }

    fn build_content(&self) -> Result<Built> {
        let mut form = self.base_form.clone();
        let redraw = form.as_ref().is_some_and(|f| f.need_appearances);
        let reorder = self.pages_changed();
        if self.edits.is_empty()
            && self.rotations.is_empty()
            && self.layers.is_empty()
            && self.annots.is_empty()
            && self.review_edits.is_empty()
            && self.removed_annots.is_empty()
            && self.hidden.is_empty()
            && self.styles.is_empty()
            && self.bounds.is_empty()
            && self.added.is_empty()
            && self.marks.is_empty()
            && !self.document_mark_changed()
            && !redraw
            && !reorder
        {
            return Ok(Built {
                doc: self.base.clone(),
                form,
                warnings: Vec::new(),
            });
        }
        let mut update = self.base.update();
        let mut warnings = Vec::new();
        if let Some(form) = &mut form {
            // First of all, so the added fields get the objects foreseen.
            self.write_added(form, &mut update)?;
            self.apply_styles(form, &mut update, &mut warnings)?;
            self.apply_marks(form, &mut update)?;
            if form.need_appearances {
                for i in 0..form.fields.len() {
                    // Redrawing fields the viewer cannot fill would only
                    // lose their authored look.
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
            for (&field, &hidden) in &self.hidden {
                let Some(f) = form.fields.get_mut(field) else {
                    continue;
                };
                for w in &mut f.widgets {
                    // Shown means neither Hidden (2) nor NoView (32).
                    w.annot_flags = if hidden {
                        w.annot_flags | 2
                    } else {
                        w.annot_flags & !(2 | 32)
                    };
                    if let Object::Dict(mut d) = update.get(w.obj)? {
                        d.insert("F", Object::Integer(i64::from(w.annot_flags)));
                        update.set(w.obj, Object::Dict(d));
                    }
                }
            }
        }
        self.write_document_mark(&mut update)?;
        let refs = self.place_pages(&mut update, reorder)?;
        if let Some(form) = &mut form {
            self.place_added(form, &mut update, &refs)?;
        }
        for (slot, &r) in &self.rotations {
            let Some(&page_ref) = refs.get(slot) else {
                continue;
            };
            if let Object::Dict(mut d) = update.get(page_ref)? {
                d.insert("Rotate", Object::Integer(i64::from(r)));
                update.set(page_ref, Object::Dict(d));
            }
        }
        for ((slot, ordinal), review) in &self.review_edits {
            let Some(&page_ref) = refs.get(slot) else {
                continue;
            };
            let Object::Dict(mut page) = update.get(page_ref)? else {
                continue;
            };
            let Some(list) = page.get(b"Annots") else {
                continue;
            };
            let Object::Array(mut list) = update.resolve(list)? else {
                continue;
            };
            let Some(target) = list.get(*ordinal) else {
                continue;
            };
            let reference = target.as_reference();
            let Object::Dict(mut annot) = update.resolve(target)? else {
                continue;
            };
            crate::annots::write_review(&mut annot, review);
            if let Some(reference) = reference {
                update.set(reference, Object::Dict(annot));
            } else {
                list[*ordinal] = Object::Dict(annot);
                page.insert("Annots", Object::Array(list));
                update.set(page_ref, Object::Dict(page));
            }
        }
        for (slot, &page_ref) in &refs {
            let removed: BTreeSet<_> = self
                .removed_annots
                .iter()
                .filter(|(s, _)| s == slot)
                .map(|(_, n)| *n)
                .collect();
            if removed.is_empty() {
                continue;
            }
            if let Object::Dict(mut page) = update.get(page_ref)? {
                if let Some(list) = page.get(b"Annots") {
                    if let Object::Array(list) = update.resolve(list)? {
                        let list = list
                            .into_iter()
                            .enumerate()
                            .filter(|(i, _)| !removed.contains(i))
                            .map(|(_, a)| a)
                            .collect();
                        page.insert("Annots", Object::Array(list));
                        update.set(page_ref, Object::Dict(page));
                    }
                }
            }
        }
        for (n, (slot, a)) in self.annots.iter().enumerate() {
            if let Some(&page_ref) = refs.get(slot) {
                crate::annots::add(&mut update, page_ref, a, n)?;
            }
        }
        if !self.layers.is_empty() {
            let empty = Dict::new();
            let layers: Vec<(ObjRef, &Dict, &PageText)> = self
                .layers
                .iter()
                .filter_map(|(slot, t)| {
                    let r = *refs.get(slot)?;
                    let resources = self.page(*slot).map_or(&empty, |p| &p.resources);
                    Some((r, resources, &**t))
                })
                .collect();
            crate::text_layer::add_layers(&mut update, &layers)?;
        }
        // Removed pages must not linger in the file's bytes, so the file is
        // then written out whole without them.
        let removed = self.removed_pages()?;
        let bytes = if removed.is_empty() {
            update.to_bytes()?
        } else {
            update.to_compact_bytes(&removed)?
        };
        let doc = Arc::new(self.base.reopen(bytes)?);
        if reorder {
            // The widgets now sit on other pages.
            form = Form::load(&doc).ok().flatten();
        }
        Ok(Built {
            doc,
            form,
            warnings,
        })
    }

    /// The objects of the original pages that were removed.
    fn removed_pages(&self) -> Result<HashSet<ObjRef>> {
        let kept: HashSet<usize> = self
            .order
            .iter()
            .filter_map(|s| match s {
                PageSlot::Own(n) => Some(*n),
                PageSlot::Added { .. } => None,
            })
            .collect();
        Ok(self
            .base
            .pages()?
            .iter()
            .enumerate()
            .filter(|(i, _)| !kept.contains(i))
            .filter_map(|(_, p)| p.obj_ref)
            .collect())
    }

    /// Writes the page tree in the new order, copying in added pages, and
    /// returns each page's object. Without `reorder` the tree is left as
    /// it is.
    fn place_pages(&self, update: &mut Update, reorder: bool) -> Result<HashMap<PageSlot, ObjRef>> {
        let mut refs = HashMap::new();
        if !reorder {
            for (n, p) in self.base.pages()?.iter().enumerate() {
                if let Some(r) = p.obj_ref {
                    refs.insert(PageSlot::Own(n), r);
                }
            }
            return Ok(refs);
        }
        let root = pages::pages_root(update)?;
        let own = self.base.pages()?;
        let mut copiers: Vec<Option<Copier>> = (0..self.files.len()).map(|_| None).collect();
        let mut kids = Vec::with_capacity(self.order.len());
        for &slot in &self.order {
            let r = match slot {
                PageSlot::Own(n) => {
                    let Some(page) = own.get(n) else { continue };
                    let mut dict = match page.obj_ref {
                        Some(r) => match update.get(r)? {
                            Object::Dict(d) => d,
                            _ => page.dict.clone(),
                        },
                        None => page.dict.clone(),
                    };
                    dict.insert("Parent", Object::Reference(root));
                    pages::inherit(&mut dict, page);
                    if dict.get(b"Resources").is_none() {
                        dict.insert("Resources", Object::Dict(page.resources.clone()));
                    }
                    match page.obj_ref {
                        Some(r) => {
                            update.set(r, Object::Dict(dict));
                            r
                        }
                        None => update.add(Object::Dict(dict)),
                    }
                }
                PageSlot::Added { file, page } => {
                    let Some(src) = self.files.get(file) else {
                        continue;
                    };
                    let src_pages = src.pages()?;
                    let Some(p) = src_pages.get(page) else {
                        continue;
                    };
                    let copier = copiers[file].get_or_insert_with(|| {
                        let used: Vec<usize> = self
                            .order
                            .iter()
                            .filter_map(|s| match s {
                                PageSlot::Added { file: f, page } if *f == file => Some(*page),
                                _ => None,
                            })
                            .collect();
                        Copier::new(src, &used)
                    });
                    copier.page(update, p, root)?
                }
            };
            refs.insert(slot, r);
            kids.push(r);
        }
        pages::set_kids(update, root, &kids)?;
        Ok(refs)
    }

    /// A new file holding only `pages` (current indices, in that order) of
    /// the document with the changes. Everything the other pages used is
    /// left out.
    pub fn extract(&self, pages: &[usize]) -> Result<Vec<u8>> {
        let built = self.build()?;
        let doc = &built.doc;
        let all = doc.pages()?;
        let mut update = doc.update();
        let root = pages::pages_root(&update)?;
        let mut kids = Vec::new();
        let mut gone: HashSet<ObjRef> = all.iter().filter_map(|p| p.obj_ref).collect();
        for &i in pages {
            let Some(page) = all.get(i) else { continue };
            let Some(r) = page.obj_ref else { continue };
            let Object::Dict(mut dict) = update.get(r)? else {
                continue;
            };
            dict.insert("Parent", Object::Reference(root));
            pages::inherit(&mut dict, page);
            if dict.get(b"Resources").is_none() {
                dict.insert("Resources", Object::Dict(page.resources.clone()));
            }
            update.set(r, Object::Dict(dict));
            gone.remove(&r);
            kids.push(r);
        }
        pages::set_kids(&mut update, root, &kids)?;
        // Leave out the form and the outline: they point into pages that
        // are gone.
        let mut catalog = doc.catalog()?;
        let catalog_ref = match doc.trailer().get(b"Root") {
            Some(Object::Reference(r)) => *r,
            _ => {
                return Err(atelier_syntax::Error::Structure(
                    "the catalog is not an object of its own".into(),
                ));
            }
        };
        for key in [
            b"AcroForm".as_slice(),
            b"Outlines",
            b"Dests",
            b"StructTreeRoot",
            b"AtelierRevisions",
        ] {
            catalog.remove(key);
        }
        update.set(catalog_ref, Object::Dict(catalog));
        update.to_compact_bytes(&gone)
    }

    /// After saving: the saved document becomes the new original.
    pub fn saved(&mut self, doc: Arc<Document>) {
        self.savepoint = doc.clone();
        self.history = crate::revisions::History::load(&doc);
        self.base_form = Form::load(&doc).ok().flatten();
        self.own_fields = self.base_form.as_ref().map_or(0, |f| f.fields.len());
        self.added.clear();
        self.marks.clear();
        self.document_mark = Self::saved_document_mark(&doc);
        self.order = own_order(&doc);
        self.base = doc;
        self.edits.clear();
        self.files.clear();
        self.rotations.clear();
        self.quiet.clear();
        self.layers.clear();
        self.annots.clear();
        self.review_edits.clear();
        self.removed_annots.clear();
        self.annot_history.clear();
        self.hidden.clear();
        self.styles.clear();
        self.bounds.clear();
    }

    /// Pages whose widgets show `field`, as the pages are now ordered.
    pub fn pages_of(&self, field: usize) -> Vec<usize> {
        if let Some(slot) = self.added_slot(field) {
            return self
                .order
                .iter()
                .position(|s| *s == slot)
                .into_iter()
                .collect();
        }
        let own: BTreeSet<usize> = self
            .base_form
            .as_ref()
            .and_then(|f| f.fields.get(field))
            .map(|f| f.widgets.iter().filter_map(|w| w.page).collect())
            .unwrap_or_default();
        self.order
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                PageSlot::Own(n) if own.contains(n) => Some(i),
                _ => None,
            })
            .collect()
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
