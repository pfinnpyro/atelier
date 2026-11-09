//! Conservative write policy, independent of cryptographic trust.
//! Transform declarations are not evidence that a certificate or signature is valid.
//! Declaration meanings follow ISO 32000-1 sections 12.8.2.2 and 12.8.2.4:
//! https://opensource.adobe.com/dc-acrobat-sdk-docs/standards/pdfstandards/pdf/PDF32000_2008.pdf
use crate::{Dict, Document, Error, Object, Result};

#[derive(Debug, Clone, Default)]
pub struct SignatureWritePolicy {
    pub has_signatures: bool,
    /// Declared transform restrictions; permitted subcategories are not yet classified.
    pub restrictions: Vec<String>,
    pub incomplete: Vec<String>,
}

impl SignatureWritePolicy {
    /// Check an edit, or a full rewrite/extraction when `rewrite` is true.
    /// Callers may always copy the original bytes without invoking this check.
    pub fn check(&self, rewrite: bool) -> Result<()> {
        let reason = if !self.incomplete.is_empty() {
            Some(format!(
                "Signature policy could not be completely inspected: {}",
                self.incomplete.join("; ")
            ))
        } else if rewrite && self.has_signatures {
            Some("Rewriting or extracting this signed PDF would discard signed bytes and invalidate its signatures. The original must be preserved.".into())
        } else if !self.restrictions.is_empty() {
            Some(format!(
                "This PDF declares {}. Atelier cannot yet classify the complete edit against these restrictions, so changes are blocked. The signed original is preserved.",
                self.restrictions.join("; ")
            ))
        } else {
            None
        };
        match reason {
            Some(reason) => Err(Error::Unsupported(reason)),
            None => Ok(()),
        }
    }
}

impl Document {
    /// Cached declarations from current and historical object graphs. No trust verdict.
    pub fn signature_write_policy(&self) -> std::sync::Arc<SignatureWritePolicy> {
        self.memo("signature-write-policy", 0, || {
            let mut policy = SignatureWritePolicy::default();
            let errors = self.visit_signature_history(false, &mut |doc, _, dict| {
                scan_dictionary(doc, dict, &mut policy, 0);
            });
            policy
                .incomplete
                .extend(errors.into_iter().map(|e| e.to_string()));
            policy.restrictions.sort();
            policy.restrictions.dedup();
            policy.incomplete.sort();
            policy.incomplete.dedup();
            policy
        })
    }
}

// Only descend into direct containers. Indirect objects are visited by the
// revision walker so references cannot introduce recursion cycles here.
fn scan_dictionary(doc: &Document, dict: &Dict, policy: &mut SignatureWritePolicy, depth: usize) {
    if depth > 32 {
        policy
            .incomplete
            .push("Signature policy nesting limit reached".into());
        return;
    }
    let result = (|| -> Result<()> {
        if let Some(perms) = dict.get(b"Perms").filter(|_| dict.is_type(b"Catalog")) {
            let perms = doc.resolve_dict(perms)?;
            if let Some(signature) = perms.get(b"DocMDP") {
                policy.has_signatures = true;
                policy
                    .restrictions
                    .push("catalog DocMDP certification".into());
                collect(doc, &doc.resolve_dict(signature)?, policy)?;
            }
        }
        if dict
            .get(b"FT")
            .and_then(Object::as_name)
            .is_some_and(|n| *n == b"Sig")
        {
            if let Some(value) = dict.get(b"V").filter(|v| !matches!(v, Object::Null)) {
                collect(doc, &doc.resolve_dict(value)?, policy)?;
            }
        }
        if crate::signature::is_signature(dict) {
            collect(doc, dict, policy)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        policy.incomplete.push(error.to_string());
    }
    for (_, value) in dict.iter() {
        scan_value(doc, value, policy, depth + 1);
    }
}

fn scan_value(doc: &Document, value: &Object, policy: &mut SignatureWritePolicy, depth: usize) {
    if depth > 32 {
        policy
            .incomplete
            .push("Signature policy nesting limit reached".into());
        return;
    }
    match value {
        Object::Dict(dict) => scan_dictionary(doc, dict, policy, depth),
        Object::Array(values) => {
            for value in values {
                scan_value(doc, value, policy, depth + 1);
            }
        }
        _ => {}
    }
}

fn collect(doc: &Document, dict: &Dict, policy: &mut SignatureWritePolicy) -> Result<()> {
    policy.has_signatures = true;
    if doc.is_encrypted() {
        policy
            .incomplete
            .push("Encrypted signature policy is unsupported".into());
    }
    let Some(references) = dict.get(b"Reference") else {
        return Ok(());
    };
    let references = doc.resolve(references)?;
    let references = references
        .as_array()
        .ok_or_else(|| Error::Structure("invalid signature Reference array".into()))?;
    for reference in references {
        let reference = doc.resolve_dict(reference)?;
        let method = doc.get_key(&reference, b"TransformMethod")?;
        let method = method
            .as_name()
            .ok_or_else(|| Error::Structure("missing signature TransformMethod".into()))?;
        let params = reference
            .get(b"TransformParams")
            .map(|p| doc.resolve_dict(p))
            .transpose()?;
        match method.as_bytes() {
            b"DocMDP" => {
                let p = params.as_ref().and_then(|d| d.get(b"P"));
                let level = match p {
                    None => 2,
                    Some(p) => doc.resolve(p)?.as_i64().unwrap_or(0),
                };
                let description = match level {
                    1 => "no changes",
                    2 => "form filling and signing only",
                    3 => "form filling, signing and annotations only",
                    _ => return Err(Error::Structure("invalid DocMDP permission level".into())),
                };
                policy
                    .restrictions
                    .push(format!("DocMDP P={level} ({description})"));
            }
            b"FieldMDP" => {
                let params =
                    params.ok_or_else(|| Error::Structure("missing FieldMDP parameters".into()))?;
                let action = doc.get_key(&params, b"Action")?;
                let action = action
                    .as_name()
                    .ok_or_else(|| Error::Structure("missing FieldMDP Action".into()))?;
                let description = match action.as_bytes() {
                    b"All" => "all fields".to_string(),
                    b"Include" | b"Exclude" => {
                        let fields = doc.get_key(&params, b"Fields")?;
                        let fields = fields
                            .as_array()
                            .ok_or_else(|| Error::Structure("invalid FieldMDP Fields".into()))?;
                        let names = fields
                            .iter()
                            .map(|f| {
                                let f = doc.resolve(f)?;
                                f.as_string().map(|s| s.to_text()).ok_or_else(|| {
                                    Error::Structure("invalid FieldMDP field name".into())
                                })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        format!("{} [{}]", action.as_str_lossy(), names.join(", "))
                    }
                    _ => return Err(Error::Structure("invalid FieldMDP Action".into())),
                };
                policy
                    .restrictions
                    .push(format!("FieldMDP ({description})"));
            }
            _ => policy.incomplete.push(format!(
                "Unsupported signature transform {}",
                method.as_str_lossy()
            )),
        }
    }
    Ok(())
}
