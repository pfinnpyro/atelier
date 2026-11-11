//! Structural revision differences, independent of CMS integrity and MDP permission.
//! Categories and reference contexts describe structure, not all visual effects
//! or exclusive ownership. They must not be used alone to authorize edits.
use crate::{Dict, Document, Error, ObjRef, Object, Result, XrefEntry};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

const MAX_REVISIONS: usize = 128;
const MAX_OBJECTS: usize = 100_000;
const MAX_CHANGES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}
impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeCategory {
    FormValue,
    FormAppearance,
    Annotation,
    PageContent,
    PageStructure,
    DocumentStructure,
    Signature,
    Metadata,
    Stream,
    CrossReference,
    Other,
}
impl ChangeCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::FormValue => "form value dictionary",
            Self::FormAppearance => "widget appearance dictionary",
            Self::Annotation => "annotation dictionary",
            Self::PageContent => "page content reference",
            Self::PageStructure => "page structure",
            Self::DocumentStructure => "document structure",
            Self::Signature => "signature dictionary",
            Self::Metadata => "metadata",
            Self::Stream => "stream data or dictionary",
            Self::CrossReference => "cross-reference data",
            Self::Other => "other or unclassified object",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjectDelta {
    pub object: ObjRef,
    pub kind: ChangeKind,
    pub category: ChangeCategory,
    /// Top-level dictionary keys only. Values and stream bytes are not retained.
    pub keys: Vec<String>,
    pub references_before: crate::ReferenceImpact,
    pub references_after: crate::ReferenceImpact,
}

#[derive(Debug, Clone)]
pub struct RevisionDelta {
    pub xref_offset: usize,
    pub objects: Vec<ObjectDelta>,
    pub trailer_keys: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SignedRevisionChanges {
    pub signed_revision_len: usize,
    /// Oldest subsequent revision first, so reverted changes remain visible.
    pub revisions: Vec<RevisionDelta>,
    /// Unsigned tail with no subsequent xref, or bytes after the final recognized
    /// startxref/EOF pair. This does not account for all bytes between revisions.
    pub unclassified_trailing_bytes: bool,
}

impl Document {
    /// Compare every subsequent revision with its predecessor. `signed_revision_len`
    /// should come from validated signature byte coverage. This is not an MDP verdict.
    /// Broken/foreign revision chains and encrypted files yield an explicit error.
    pub fn changes_since_signed_revision(
        &self,
        signed_revision_len: usize,
    ) -> Arc<Result<SignedRevisionChanges>> {
        self.memo(
            "signed-revision-changes",
            signed_revision_len as u64,
            || compare_history(self, signed_revision_len),
        )
    }
}

fn unsupported(message: &str) -> Error {
    Error::Unsupported(message.into())
}

fn compare_history(doc: &Document, end: usize) -> Result<SignedRevisionChanges> {
    if doc.data().len() > 256 * 1024 * 1024 {
        return Err(unsupported(
            "Revision analysis is limited to 256 MiB source files",
        ));
    }
    if doc.is_encrypted() || doc.was_repaired() {
        return Err(unsupported(
            "Revision analysis requires an unencrypted PDF with intact cross-reference data",
        ));
    }
    let prefix = doc
        .data()
        .get(..end)
        .filter(|data| !data.is_empty())
        .ok_or_else(|| unsupported("Invalid signed revision endpoint"))?;
    if !ends_at_eof(prefix) {
        return Err(unsupported(
            "Signed coverage does not end at a complete PDF revision",
        ));
    }
    let mut before = Document::open(Arc::<[u8]>::from(prefix))?;
    if before.was_repaired() || !before.warnings().is_empty() {
        return Err(unsupported(
            "Signed revision requires repair; changes cannot be classified",
        ));
    }
    let signed_xref = before
        .startxref()
        .ok_or_else(|| unsupported("Signed revision has no startxref"))?;
    if !terminal_revision(prefix, signed_xref) {
        return Err(unsupported(
            "Signed revision has unexplained bytes around its end marker",
        ));
    }
    let mut offset = doc
        .startxref()
        .ok_or_else(|| unsupported("Current revision has no startxref"))?;
    let mut visited = HashSet::new();
    let mut offsets = Vec::new();
    while offset != signed_xref {
        if visited.len() >= MAX_REVISIONS || !visited.insert(offset) {
            return Err(unsupported(
                "Revision analysis cycle or revision limit reached",
            ));
        }
        let revision = doc.at_xref(offset)?;
        offsets.push(offset);
        let previous = revision
            .trailer()
            .get(b"Prev")
            .and_then(Object::as_i64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| unsupported("Signed revision is not in the current /Prev chain"))?;
        if previous >= offset {
            return Err(unsupported(
                "Revision /Prev does not point to an earlier cross-reference section",
            ));
        }
        offset = previous;
    }
    let mut revisions = Vec::new();
    let mut count = 0;
    let mut comparisons = 0;
    for offset in offsets.into_iter().rev() {
        let after = doc.at_xref(offset)?;
        let delta = compare_objects(&before, &after, offset, &mut comparisons)?;
        count += delta.objects.len();
        if count > MAX_CHANGES {
            return Err(unsupported("Revision analysis change limit reached"));
        }
        revisions.push(delta);
        before = after;
    }
    // Whitespace appended to a PDF is still outside signed coverage; it is not
    // silently promoted into a permission or integrity result.
    let unclassified_trailing_bytes = end < doc.data().len()
        && (revisions.is_empty() || !terminal_revision(doc.data(), doc.startxref().unwrap_or(0)));
    Ok(SignedRevisionChanges {
        signed_revision_len: end,
        revisions,
        unclassified_trailing_bytes,
    })
}

fn ends_at_eof(data: &[u8]) -> bool {
    data.trim_ascii_end().ends_with(b"%%EOF")
}

// Attribute the final bytes to the actual startxref/EOF pair, rather than any
// arbitrary trailing text that happens to end with the string "%%EOF".
fn terminal_revision(data: &[u8], expected: usize) -> bool {
    let tail_start = data.len().saturating_sub(4096);
    let Some(pos) = data[tail_start..]
        .windows(9)
        .rposition(|w| w == b"startxref")
    else {
        return false;
    };
    let suffix = data[tail_start + pos + 9..].trim_ascii_start();
    let digits = suffix.iter().take_while(|b| b.is_ascii_digit()).count();
    let offset = std::str::from_utf8(&suffix[..digits])
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    offset == Some(expected) && suffix[digits..].trim_ascii() == b"%%EOF"
}

fn live_objects(doc: &Document) -> Result<BTreeSet<ObjRef>> {
    let numbers = doc.object_numbers();
    if numbers.len() > MAX_OBJECTS {
        return Err(unsupported("Revision analysis object limit reached"));
    }
    Ok(numbers
        .into_iter()
        .map(|n| {
            let generation = match doc.xref_entry(n) {
                Some(XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            ObjRef::new(n, generation)
        })
        .collect())
}

fn compare_objects(
    before: &Document,
    after: &Document,
    offset: usize,
    comparisons: &mut usize,
) -> Result<RevisionDelta> {
    let old = live_objects(before)?;
    let new = live_objects(after)?;
    *comparisons += old.len() + new.len();
    if *comparisons > 1_000_000 {
        return Err(unsupported("Revision analysis comparison limit reached"));
    }
    let before_index = crate::reference_effects::ReferenceIndex::build(before)?;
    let after_index = crate::reference_effects::ReferenceIndex::build(after)?;
    *comparisons += before_index.work() + after_index.work();
    if *comparisons > 1_000_000 {
        return Err(unsupported(
            "Revision analysis reference work limit reached",
        ));
    }
    let mut objects = Vec::new();
    for reference in old.union(&new).copied() {
        let a = old
            .contains(&reference)
            .then(|| before.get(reference))
            .transpose()?;
        let b = new
            .contains(&reference)
            .then(|| after.get(reference))
            .transpose()?;
        if a == b {
            continue;
        }
        let kind = match (&a, &b) {
            (None, _) => ChangeKind::Added,
            (_, None) => ChangeKind::Removed,
            _ => ChangeKind::Modified,
        };
        let keys = changed_keys(
            a.as_ref().and_then(dictionary),
            b.as_ref().and_then(dictionary),
        );
        let category = category(reference, a.as_ref(), b.as_ref(), &keys, before, after);
        objects.push(ObjectDelta {
            object: reference,
            kind,
            category,
            keys,
            references_before: before_index.impact(reference),
            references_after: after_index.impact(reference),
        });
        if objects.len() > MAX_CHANGES {
            return Err(unsupported("Revision analysis change limit reached"));
        }
    }
    if !before.warnings().is_empty() || !after.warnings().is_empty() {
        return Err(unsupported(
            "Object resolution required recovery; revision analysis is incomplete",
        ));
    }
    Ok(RevisionDelta {
        xref_offset: offset,
        objects,
        trailer_keys: changed_keys(Some(before.trailer()), Some(after.trailer())),
    })
}

fn dictionary(object: &Object) -> Option<&Dict> {
    match object {
        Object::Dict(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

fn changed_keys(before: Option<&Dict>, after: Option<&Dict>) -> Vec<String> {
    let a: BTreeMap<_, _> = before.into_iter().flat_map(Dict::iter).collect();
    let b: BTreeMap<_, _> = after.into_iter().flat_map(Dict::iter).collect();
    a.keys()
        .chain(b.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| a.get(key) != b.get(key))
        .map(ToString::to_string)
        .collect()
}

fn category(
    reference: ObjRef,
    a: Option<&Object>,
    b: Option<&Object>,
    keys: &[String],
    before: &Document,
    after: &Document,
) -> ChangeCategory {
    use ChangeCategory::*;
    let roles = [a, b]
        .into_iter()
        .flatten()
        .map(|o| {
            let Some(d) = dictionary(o) else { return Other };
            if d.is_type(b"XRef") {
                return CrossReference;
            }
            if crate::signature::is_signature(d)
                || d.get(b"FT")
                    .and_then(Object::as_name)
                    .is_some_and(|n| *n == b"Sig")
            {
                return Signature;
            }
            if d.is_type(b"Page") {
                return if !keys.is_empty() && keys.iter().all(|k| k == "/Contents") {
                    PageContent
                } else {
                    PageStructure
                };
            }
            if d.is_type(b"Catalog") || d.is_type(b"Pages") {
                return DocumentStructure;
            }
            if d.get(b"FT").is_some()
                && !keys.is_empty()
                && keys.iter().all(|k| matches!(k.as_str(), "/V" | "/I"))
            {
                return FormValue;
            }
            if d.get(b"Subtype")
                .and_then(Object::as_name)
                .is_some_and(|n| *n == b"Widget")
            {
                return if !keys.is_empty()
                    && keys.iter().all(|k| matches!(k.as_str(), "/AP" | "/AS"))
                {
                    FormAppearance
                } else {
                    Other
                };
            }
            if d.is_type(b"Annot") {
                return Annotation;
            }
            if d.is_type(b"Metadata")
                || [before, after].iter().any(|doc| {
                    doc.trailer().get(b"Info").and_then(Object::as_reference) == Some(reference)
                })
            {
                return Metadata;
            }
            if matches!(o, Object::Stream(_)) {
                Stream
            } else {
                Other
            }
        })
        .collect::<Vec<_>>();
    roles
        .first()
        .copied()
        .filter(|role| roles.iter().all(|r| r == role))
        .unwrap_or(Other)
}
