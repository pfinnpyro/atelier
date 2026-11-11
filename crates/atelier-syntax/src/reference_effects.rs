//! Bounded reference contexts. Reachability is not a rendering or MDP verdict.
use crate::{Dict, Document, Error, ObjRef, Object, Result, XrefEntry};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceImpact {
    /// Immediate indirect-object referrers, including structural backlinks.
    pub direct_referrers: Vec<ObjRef>,
    /// One-based pages whose forward dependency graph reaches this object.
    pub page_contexts: Vec<usize>,
    /// Qualified leaf field names from AcroForm forward/inherited contexts.
    /// These can overestimate effects when inherited values are overridden.
    pub field_contexts: Vec<String>,
    pub trailer_reference: bool,
}

#[derive(Default)]
pub(crate) struct ReferenceIndex {
    incoming: BTreeMap<ObjRef, BTreeSet<ObjRef>>,
    dependencies: BTreeMap<ObjRef, BTreeSet<ObjRef>>,
    pages: BTreeMap<ObjRef, BTreeSet<usize>>,
    fields: BTreeMap<ObjRef, BTreeSet<String>>,
    trailer: BTreeSet<ObjRef>,
    work: usize,
}

fn invalid(message: &str) -> Error {
    Error::Unsupported(message.into())
}
fn spend(work: &mut usize) -> Result<()> {
    *work += 1;
    if *work > 1_000_000 {
        return Err(invalid("Reference analysis work limit reached"));
    }
    Ok(())
}

// Keep raw referrers separate from ownership traversal: /Parent and /P are
// structural backlinks. Following them would assign every sibling page/field
// to each dependency. They are still shown in direct_referrers.
fn references(
    value: &Object,
    out: &mut BTreeSet<ObjRef>,
    forward: bool,
    depth: usize,
    work: &mut usize,
) -> Result<()> {
    spend(work)?;
    if depth > 32 {
        return Err(invalid("Reference analysis nesting limit reached"));
    }
    match value {
        Object::Reference(r) => {
            out.insert(*r);
        }
        Object::Dict(d) => {
            for (key, value) in d.iter() {
                if !forward || !backlink(d, key.as_bytes()) {
                    references(value, out, forward, depth + 1, work)?;
                }
            }
        }
        Object::Stream(s) => {
            references(&Object::Dict(s.dict.clone()), out, forward, depth + 1, work)?
        }
        Object::Array(values) => {
            for value in values {
                references(value, out, forward, depth + 1, work)?;
            }
        }
        _ => {}
    }
    Ok(())
}

// Resource names are arbitrary: a resource named /P or /Parent must not be
// mistaken for an annotation/page/field backlink.
fn backlink(dict: &Dict, key: &[u8]) -> bool {
    let widget = dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_some_and(|n| *n == b"Widget");
    match key {
        b"Parent" => {
            dict.is_type(b"Page")
                || dict.is_type(b"Pages")
                || widget
                || dict.get(b"FT").is_some()
                || dict.get(b"T").is_some_and(|t| t.as_string().is_some())
        }
        b"P" => {
            dict.is_type(b"Annot")
                || widget
                || (dict.get(b"Subtype").is_some() && dict.get(b"Rect").is_some())
        }
        _ => false,
    }
}

impl ReferenceIndex {
    pub(crate) fn build(doc: &Document) -> Result<Self> {
        let mut index = Self::default();
        let numbers = doc.object_numbers();
        if numbers.len() > 100_000 {
            return Err(invalid("Reference analysis object limit reached"));
        }
        for number in numbers {
            let generation = match doc.xref_entry(number) {
                Some(XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            let r = ObjRef::new(number, generation);
            let object = doc.get(r)?;
            let mut raw = BTreeSet::new();
            references(&object, &mut raw, false, 0, &mut index.work)?;
            for target in raw {
                index.incoming.entry(target).or_default().insert(r);
            }
            let mut dependencies = BTreeSet::new();
            references(&object, &mut dependencies, true, 0, &mut index.work)?;
            index.dependencies.insert(r, dependencies);
        }
        references(
            &Object::Dict(doc.trailer().clone()),
            &mut index.trailer,
            false,
            0,
            &mut index.work,
        )?;
        for (page, value) in doc.pages()?.iter().enumerate() {
            let mut roots = BTreeSet::new();
            if let Some(r) = value.obj_ref {
                roots.insert(r);
            }
            references(
                &Object::Dict(value.resources.clone()),
                &mut roots,
                true,
                0,
                &mut index.work,
            )?;
            for r in index.reachable(roots)? {
                index.pages.entry(r).or_default().insert(page + 1);
            }
        }
        let catalog = doc.catalog()?;
        if let Some(form) = catalog.get(b"AcroForm") {
            let mut ancestors = Vec::new();
            if let Some(r) = form.as_reference() {
                ancestors.push(r);
            }
            let form = doc.resolve_dict(form)?;
            let resources = form.get(b"DR").cloned().unwrap_or(Object::Null);
            if let Some(fields) = form.get(b"Fields") {
                let fields = doc.resolve(fields)?;
                let fields = fields
                    .as_array()
                    .ok_or_else(|| invalid("Invalid form Fields during reference analysis"))?;
                let mut seen = BTreeSet::new();
                let mut names = BTreeSet::new();
                for field in fields {
                    index.field(
                        doc,
                        field,
                        "",
                        &ancestors,
                        &[],
                        &resources,
                        &mut seen,
                        &mut names,
                        0,
                    )?;
                }
            }
        }
        if !doc.warnings().is_empty() {
            return Err(invalid("Reference analysis required object recovery"));
        }
        Ok(index)
    }

    fn reachable(&mut self, roots: BTreeSet<ObjRef>) -> Result<BTreeSet<ObjRef>> {
        let mut pending: Vec<_> = roots.into_iter().collect();
        let mut visited = BTreeSet::new();
        while let Some(r) = pending.pop() {
            spend(&mut self.work)?;
            if !visited.insert(r) {
                continue;
            }
            let children = self
                .dependencies
                .get(&r)
                .ok_or_else(|| invalid("Reference analysis encountered an unresolved object"))?;
            pending.extend(children.iter().copied());
        }
        Ok(visited)
    }

    #[allow(clippy::too_many_arguments)]
    fn field(
        &mut self,
        doc: &Document,
        field: &Object,
        prefix: &str,
        ancestors: &[ObjRef],
        inherited: &[Object],
        resources: &Object,
        seen: &mut BTreeSet<ObjRef>,
        names: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<()> {
        spend(&mut self.work)?;
        if depth > 32 {
            return Err(invalid("Form ownership depth limit reached"));
        }
        let reference = field.as_reference();
        if let Some(r) = reference {
            if !seen.insert(r) {
                return Err(invalid("Cyclic or shared form field ownership"));
            }
        }
        let dict = doc.resolve_dict(field)?;
        let partial = match dict.get(b"T") {
            Some(t) => doc
                .resolve(t)?
                .as_string()
                .map(|s| s.to_text())
                .ok_or_else(|| invalid("Invalid partial field name"))?,
            None => String::new(),
        };
        if partial.contains('.') {
            return Err(invalid("Ambiguous partial field name"));
        }
        let name = if prefix.is_empty() {
            partial
        } else if partial.is_empty() {
            prefix.into()
        } else {
            format!("{prefix}.{partial}")
        };
        if name.len() > 1024 {
            return Err(invalid("Form ownership name length limit reached"));
        }
        let kids = dict.get(b"Kids").map(|k| doc.resolve(k)).transpose()?;
        let kids = match &kids {
            Some(kids) => kids
                .as_array()
                .ok_or_else(|| invalid("Invalid form Kids during reference analysis"))?,
            None => &[],
        };
        let child_fields = kids
            .iter()
            .map(|k| doc.resolve_dict(k))
            .collect::<Result<Vec<_>>>()?;
        let named_children = child_fields
            .iter()
            .filter(|d| d.get(b"T").is_some() || d.get(b"FT").is_some())
            .count();
        let mut lineage = ancestors.to_vec();
        if let Some(r) = reference {
            lineage.push(r);
        }
        let mut inherited = inherited.to_vec();
        for key in [b"V".as_slice(), b"DV", b"DA", b"DR", b"Opt"] {
            if let Some(value) = dict.get(key) {
                inherited.push(value.clone());
            }
        }
        if named_children > 0 {
            if named_children != kids.len() {
                return Err(invalid("Mixed form-field and widget ownership"));
            }
            for child in kids {
                self.field(
                    doc,
                    child,
                    &name,
                    &lineage,
                    &inherited,
                    resources,
                    seen,
                    names,
                    depth + 1,
                )?;
            }
            return Ok(());
        }
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(invalid("Unnamed or duplicate form field ownership"));
        }
        let mut roots = BTreeSet::new();
        references(&Object::Dict(dict), &mut roots, true, 0, &mut self.work)?;
        references(resources, &mut roots, true, 0, &mut self.work)?;
        for value in inherited {
            references(&value, &mut roots, true, 0, &mut self.work)?;
        }
        if let Some(r) = reference {
            roots.insert(r);
        }
        let mut owned = self.reachable(roots)?;
        owned.extend(lineage);
        for r in owned {
            self.fields.entry(r).or_default().insert(name.clone());
        }
        Ok(())
    }

    pub(crate) fn work(&self) -> usize {
        self.work
    }

    pub(crate) fn impact(&self, r: ObjRef) -> ReferenceImpact {
        ReferenceImpact {
            direct_referrers: self
                .incoming
                .get(&r)
                .into_iter()
                .flatten()
                .copied()
                .collect(),
            page_contexts: self.pages.get(&r).into_iter().flatten().copied().collect(),
            field_contexts: self.fields.get(&r).into_iter().flatten().cloned().collect(),
            trailer_reference: self.trailer.contains(&r),
        }
    }
}
