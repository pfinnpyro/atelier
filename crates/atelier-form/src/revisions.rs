//! Persistent, reversible document revisions. Snapshots omit the journal itself.
//! Decisions are dependency checked: restoring an older snapshot must never erase
//! unrelated newer edits. The journal is private PDF catalog data, not page content.
use atelier_syntax::{Dict, Document, Error, Object, PdfString, Result, Stream};
use std::collections::HashSet;
use std::sync::Arc;

const KEY: &[u8] = b"AtelierRevisions";
const MAX_REVISIONS: usize = 512;
const MAX_SNAPSHOT: usize = 128 * 1024 * 1024;
const MAX_HISTORY: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    Pending,
    Accepted,
    Rejected,
    Undone,
    Superseded,
}
impl RevisionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Accepted => "Accepted",
            Self::Rejected => "Rejected",
            Self::Undone => "Undone",
            Self::Superseded => "Superseded",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "Pending" => Ok(Self::Pending),
            "Accepted" => Ok(Self::Accepted),
            "Rejected" => Ok(Self::Rejected),
            "Undone" => Ok(Self::Undone),
            "Superseded" => Ok(Self::Superseded),
            _ => Err(Error::Structure("Unknown revision state".into())),
        }
    }
}
#[derive(Debug, Clone)]
pub struct RevisionChange {
    /// Stable field/page/annotation identity within this revision's snapshots.
    pub target: String,
    pub before: String,
    pub after: String,
    pub page: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct RevisionEvent {
    pub state: RevisionState,
    pub author: String,
    pub date: String,
}
#[derive(Debug, Clone)]
pub struct Revision {
    pub id: u64,
    pub author: String,
    pub created: String,
    pub changes: Vec<RevisionChange>,
    pub state: RevisionState,
    pub events: Vec<RevisionEvent>,
    before: Arc<[u8]>,
    after: Arc<[u8]>,
}
#[derive(Clone, Default)]
pub(crate) struct History {
    pub enabled: bool,
    pub author: String,
    pub revisions: Vec<Revision>,
    pub pending: Vec<RevisionChange>,
    pub dirty: bool,
    pub problem: Option<String>,
}
fn text(value: &str) -> Object {
    Object::String(PdfString::from_text(value))
}
fn read_text(d: &Dict, key: &[u8]) -> String {
    match d.get(key) {
        Some(Object::String(s)) => s.to_text(),
        _ => String::new(),
    }
}
fn root(doc: &Document) -> Result<atelier_syntax::ObjRef> {
    doc.trailer()
        .get(b"Root")
        .and_then(Object::as_reference)
        .ok_or_else(|| Error::Structure("Missing catalog reference".into()))
}
pub(crate) fn snapshot(doc: &Document) -> Result<Arc<[u8]>> {
    let mut catalog = doc.catalog()?;
    catalog.remove(KEY);
    let mut update = doc.update();
    update.set(root(doc)?, Object::Dict(catalog));
    let bytes = update.to_compact_bytes(&HashSet::new())?;
    if bytes.len() > MAX_SNAPSHOT {
        return Err(Error::Unsupported(
            "Tracked snapshots are limited to 128 MiB. Disable tracking for larger documents."
                .into(),
        ));
    }
    Ok(bytes.into())
}
fn equivalent(base: &Document, a: &[u8], b: &[u8]) -> Result<bool> {
    // Ignore the trailer's changing second file ID and randomized encryption IVs.
    let a = base.reopen(a.to_vec())?;
    let b = base.reopen(b.to_vec())?;
    let mut an = a.object_numbers();
    an.sort_unstable();
    let mut bn = b.object_numbers();
    bn.sort_unstable();
    if an != bn {
        return Ok(false);
    }
    for n in an {
        // Compacted snapshots use original generation numbers; resolve through
        // their parsed xref entries instead of assuming generation zero.
        let generation = |d: &Document| match d.xref_entry(n) {
            Some(atelier_syntax::XrefEntry::Offset { generation, .. }) => generation,
            _ => 0,
        };
        if a.get(atelier_syntax::ObjRef::new(n, generation(&a)))?
            != b.get(atelier_syntax::ObjRef::new(n, generation(&b)))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
impl History {
    pub fn load(doc: &Document) -> Self {
        match Self::read(doc) {
            Ok(h) => h,
            Err(e) => Self {
                problem: Some(format!("Could not read revision history: {e}")),
                ..Self::default()
            },
        }
    }
    fn read(doc: &Document) -> Result<Self> {
        let catalog = doc.catalog()?;
        let Some(value) = catalog.get(KEY) else {
            return Ok(Self::default());
        };
        let d = doc.resolve_dict(value)?;
        if d.get(b"Version").and_then(Object::as_i64) != Some(1) {
            return Err(Error::Unsupported("Revision journal version".into()));
        }
        let mut history = Self {
            enabled: matches!(d.get(b"Enabled"), Some(Object::Bool(true))),
            author: read_text(&d, b"Author"),
            ..Self::default()
        };
        let Object::Array(entries) = d
            .get(b"Entries")
            .cloned()
            .unwrap_or(Object::Array(Vec::new()))
        else {
            return Err(Error::Structure("Invalid revision entries".into()));
        };
        if entries.len() > MAX_REVISIONS {
            return Err(Error::Structure("Too many revisions".into()));
        }
        let mut ids = HashSet::new();
        let mut total_bytes = 0usize;
        for value in entries {
            let d = doc.resolve_dict(&value)?;
            let id = d
                .get(b"Id")
                .and_then(Object::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| Error::Structure("Invalid revision ID".into()))?
                as u64;
            if !ids.insert(id) {
                return Err(Error::Structure("Duplicate revision ID".into()));
            }
            let bytes = |key: &[u8]| -> Result<Arc<[u8]>> {
                let Object::Stream(stream) = doc.get_key(&d, key)? else {
                    return Err(Error::Structure("Missing revision snapshot".into()));
                };
                let decoded = doc.decode_stream(&stream)?;
                if decoded.data.len() > MAX_SNAPSHOT {
                    return Err(Error::Structure("Revision snapshot too large".into()));
                }
                Ok(decoded.data.into())
            };
            let mut changes = Vec::new();
            if let Some(Object::Array(items)) = d.get(b"Changes") {
                for item in items {
                    let c = doc.resolve_dict(item)?;
                    changes.push(RevisionChange {
                        target: read_text(&c, b"Target"),
                        before: read_text(&c, b"Before"),
                        after: read_text(&c, b"After"),
                        page: c
                            .get(b"Page")
                            .and_then(Object::as_i64)
                            .filter(|p| *p >= 0)
                            .map(|p| p as usize),
                    });
                }
            }
            let mut events = Vec::new();
            if let Some(Object::Array(items)) = d.get(b"Events") {
                for item in items {
                    let e = doc.resolve_dict(item)?;
                    events.push(RevisionEvent {
                        state: RevisionState::parse(&read_text(&e, b"State"))?,
                        author: read_text(&e, b"Author"),
                        date: read_text(&e, b"Date"),
                    });
                }
            }
            let before = bytes(b"Before")?;
            let after = bytes(b"After")?;
            total_bytes = total_bytes
                .saturating_add(before.len())
                .saturating_add(after.len());
            if total_bytes > MAX_HISTORY {
                return Err(Error::Unsupported(
                    "Revision history exceeds 256 MiB".into(),
                ));
            }
            history.revisions.push(Revision {
                id,
                author: read_text(&d, b"Author"),
                created: read_text(&d, b"Created"),
                state: RevisionState::parse(&read_text(&d, b"State"))?,
                changes,
                events,
                before,
                after,
            });
        }
        Ok(history)
    }
    pub fn touch(&mut self, target: String, before: String, after: String, page: Option<usize>) {
        if self.enabled && before != after {
            self.pending.push(RevisionChange {
                target,
                before,
                after,
                page,
            });
        }
    }
    pub fn record(&mut self, before: &Document, after: &Document) -> Result<bool> {
        if !self.enabled || self.pending.is_empty() {
            return Ok(false);
        }
        if self.revisions.len() >= MAX_REVISIONS {
            return Err(Error::Unsupported(
                "Revision history has 512 entries. Disable tracking to continue editing.".into(),
            ));
        }
        let before_bytes = snapshot(before)?;
        let after_bytes = snapshot(after)?;
        if equivalent(before, &before_bytes, &after_bytes)? {
            self.pending.clear();
            return Ok(false);
        }
        let total: usize = self
            .revisions
            .iter()
            .map(|r| r.before.len() + r.after.len())
            .sum();
        if total
            .saturating_add(before_bytes.len())
            .saturating_add(after_bytes.len())
            > MAX_HISTORY
        {
            return Err(Error::Unsupported(
                "Revision history exceeds 256 MiB. Disable tracking to continue editing.".into(),
            ));
        }
        let date = crate::annotation_date();
        for revision in &mut self.revisions {
            if revision.state == RevisionState::Undone {
                revision.state = RevisionState::Superseded;
                revision.events.push(RevisionEvent {
                    state: RevisionState::Superseded,
                    author: self.author.clone(),
                    date: date.clone(),
                });
            }
        }
        let id = self.revisions.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        self.revisions.push(Revision {
            id,
            author: self.author.clone(),
            created: date.clone(),
            state: RevisionState::Pending,
            changes: std::mem::take(&mut self.pending),
            events: vec![RevisionEvent {
                state: RevisionState::Pending,
                author: self.author.clone(),
                date,
            }],
            before: before_bytes,
            after: after_bytes,
        });
        self.dirty = true;
        Ok(true)
    }
    pub fn write(&self, doc: Arc<Document>) -> Result<Arc<Document>> {
        if !self.dirty && self.revisions.is_empty() && !self.enabled {
            return Ok(doc);
        }
        if let Some(problem) = &self.problem {
            return Err(Error::Structure(problem.clone()));
        }
        let mut update = doc.update();
        let mut catalog = doc.catalog()?;
        let mut entries = Vec::new();
        for revision in &self.revisions {
            let mut d = Dict::new();
            d.insert("Id", Object::Integer(revision.id as i64));
            d.insert("Author", text(&revision.author));
            d.insert("Created", text(&revision.created));
            d.insert("State", text(revision.state.label()));
            for (key, bytes) in [("Before", &revision.before), ("After", &revision.after)] {
                let mut sd = Dict::new();
                sd.insert(
                    "Filter",
                    Object::Name(atelier_syntax::Name::from("FlateDecode")),
                );
                let data = atelier_filters::encode_flate(bytes);
                let reference = update.add(Object::Stream(Stream {
                    dict: sd,
                    data: data.into(),
                }));
                d.insert(key, Object::Reference(reference));
            }
            d.insert(
                "Changes",
                Object::Array(
                    revision
                        .changes
                        .iter()
                        .map(|c| {
                            let mut d = Dict::new();
                            d.insert("Target", text(&c.target));
                            d.insert("Before", text(&c.before));
                            d.insert("After", text(&c.after));
                            if let Some(page) = c.page {
                                d.insert("Page", Object::Integer(page as i64));
                            }
                            Object::Dict(d)
                        })
                        .collect(),
                ),
            );
            d.insert(
                "Events",
                Object::Array(
                    revision
                        .events
                        .iter()
                        .map(|e| {
                            let mut d = Dict::new();
                            d.insert("State", text(e.state.label()));
                            d.insert("Author", text(&e.author));
                            d.insert("Date", text(&e.date));
                            Object::Dict(d)
                        })
                        .collect(),
                ),
            );
            entries.push(Object::Reference(update.add(Object::Dict(d))));
        }
        let mut journal = Dict::new();
        journal.insert("Version", Object::Integer(1));
        journal.insert("Enabled", Object::Bool(self.enabled));
        journal.insert("Author", text(&self.author));
        journal.insert("Entries", Object::Array(entries));
        let reference = update.add(Object::Dict(journal));
        catalog.insert("AtelierRevisions", Object::Reference(reference));
        update.set(root(&doc)?, Object::Dict(catalog));
        // Compact so superseded copies of the journal do not accumulate.
        Ok(Arc::new(
            doc.reopen(update.to_compact_bytes(&HashSet::new())?)?,
        ))
    }
    pub fn decide(
        &mut self,
        current: &Document,
        id: u64,
        state: RevisionState,
        author: &str,
    ) -> Result<Option<Arc<Document>>> {
        if state == RevisionState::Superseded {
            return Err(Error::Structure(
                "Superseded is an internal history state.".into(),
            ));
        }
        if !self.pending.is_empty() {
            return Err(Error::Structure(
                "Finish the current edit before reviewing revisions.".into(),
            ));
        }
        let index = self
            .revisions
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| Error::Structure("Revision not found".into()))?;
        let revision = &self.revisions[index];
        if revision.state == RevisionState::Superseded {
            return Err(Error::Structure(
                "This revision belongs to an earlier undo branch.".into(),
            ));
        }
        if revision.state == state {
            return Ok(None);
        }
        let applied = matches!(
            revision.state,
            RevisionState::Pending | RevisionState::Accepted
        );
        let will_apply = matches!(state, RevisionState::Pending | RevisionState::Accepted);
        let restore = if applied != will_apply {
            let expected = if applied {
                &revision.after
            } else {
                &revision.before
            };
            if !equivalent(current, &snapshot(current)?, expected)? {
                return Err(Error::Structure(
                    "Resolve or undo newer edits first; this revision has dependent changes."
                        .into(),
                ));
            }
            let bytes = if will_apply {
                &revision.after
            } else {
                &revision.before
            };
            Some(Arc::new(current.reopen(bytes.to_vec())?))
        } else {
            None
        };
        let revision = &mut self.revisions[index];
        revision.state = state;
        revision.events.push(RevisionEvent {
            state,
            author: author.to_owned(),
            date: crate::annotation_date(),
        });
        self.dirty = true;
        Ok(restore)
    }
    pub fn undo_id(&self) -> Option<u64> {
        self.revisions
            .iter()
            .rev()
            .find(|r| matches!(r.state, RevisionState::Pending | RevisionState::Accepted))
            .map(|r| r.id)
    }
    pub fn redo_id(&self) -> Option<(u64, RevisionState)> {
        self.revisions
            .iter()
            .find(|r| r.state == RevisionState::Undone)
            .map(|r| {
                (
                    r.id,
                    r.events
                        .iter()
                        .rev()
                        .skip(1)
                        .find(|e| e.state != RevisionState::Undone)
                        .map_or(RevisionState::Pending, |e| e.state),
                )
            })
    }
}
