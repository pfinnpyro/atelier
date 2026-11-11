//! Detached CMS signature container. A caller supplies the certificate provider.
//! This module prepares byte ranges; it does not verify CMS or certificate trust.
use crate::{Dict, Document, Error, Name, Object, PdfString, Result, serialize};

pub struct PreparedSignature {
    bytes: Vec<u8>,
    gap: std::ops::Range<usize>,
}

impl Document {
    /// Creates an invisible signature field and reserves room for detached DER CMS.
    /// Ink already present in the document is covered by the same byte ranges.
    pub fn prepare_signature(&self, signer: &str, capacity: usize) -> Result<PreparedSignature> {
        if self.is_encrypted() {
            return Err(Error::Unsupported("signing encrypted documents".into()));
        }
        if !(1024..=1_048_576).contains(&capacity) {
            return Err(Error::Unsupported(
                "CMS capacity must be 1024 to 1048576 bytes".into(),
            ));
        }
        let root = self
            .trailer()
            .get(b"Root")
            .and_then(Object::as_reference)
            .ok_or_else(|| Error::Structure("catalog must be indirect".into()))?;
        let mut update = self.update();
        let mut sig = Dict::new();
        sig.insert("Type", Object::Name(Name::new("Sig")));
        sig.insert("Filter", Object::Name(Name::new("Adobe.PPKLite")));
        sig.insert("SubFilter", Object::Name(Name::new("adbe.pkcs7.detached")));
        sig.insert("Name", Object::String(PdfString::from_text(signer)));
        const PLACEHOLDER: i64 = 1_000_000_000_000_000_000;
        sig.insert(
            "ByteRange",
            Object::Array(vec![Object::Integer(PLACEHOLDER); 4]),
        );
        sig.insert(
            "Contents",
            Object::String(PdfString {
                bytes: vec![0; capacity],
                hex: true,
            }),
        );
        let signature = Object::Dict(sig);
        let mut serialized = Vec::new();
        serialize(&mut serialized, &signature);
        let sig_ref = update.add(signature);
        let mut field = Dict::new();
        field.insert("FT", Object::Name(Name::new("Sig")));
        field.insert(
            "T",
            Object::String(PdfString::from_text(&format!(
                "AtelierSignature{}",
                sig_ref.num
            ))),
        );
        field.insert("V", Object::Reference(sig_ref));
        let field_ref = update.add(Object::Dict(field));
        let mut catalog = self.catalog()?;
        let mut form = match catalog.get(b"AcroForm") {
            Some(f) => self.resolve_dict(f)?,
            None => Dict::new(),
        };
        let mut fields = match form.get(b"Fields") {
            Some(f) => self
                .resolve(f)?
                .as_array()
                .map(<[_]>::to_vec)
                .ok_or_else(|| Error::Structure("invalid form Fields".into()))?,
            None => Vec::new(),
        };
        fields.push(Object::Reference(field_ref));
        form.insert("Fields", Object::Array(fields));
        form.insert("SigFlags", Object::Integer(3));
        catalog.insert(
            "AcroForm",
            Object::Reference(update.add(Object::Dict(form))),
        );
        update.set(root, Object::Dict(catalog));
        let mut bytes = update.to_bytes()?;
        // Locate the exact object we just wrote, never a pre-existing /Contents.
        let object_header = format!("{} {} obj\n", sig_ref.num, sig_ref.generation);
        let object_start = bytes
            .windows(object_header.len())
            .rposition(|w| w == object_header.as_bytes())
            .ok_or_else(|| Error::Structure("signature object missing".into()))?
            + object_header.len();
        if bytes.get(object_start..object_start + serialized.len()) != Some(serialized.as_slice()) {
            return Err(Error::Structure("signature serialization changed".into()));
        }
        let content = find(&serialized, b"/Contents <")? + object_start + b"/Contents ".len();
        let gap = content..content + capacity * 2 + 2;
        let marker = format!("[{PLACEHOLDER} {PLACEHOLDER} {PLACEHOLDER} {PLACEHOLDER}]");
        let range_at = object_start + find(&serialized, marker.as_bytes())?;
        let replacement = format!("[0 {} {} {}]", gap.start, gap.end, bytes.len() - gap.end);
        if replacement.len() > marker.len() {
            return Err(Error::Structure("byte range overflow".into()));
        }
        bytes[range_at..range_at + marker.len()].fill(b' ');
        bytes[range_at..range_at + replacement.len()].copy_from_slice(replacement.as_bytes());
        Ok(PreparedSignature { bytes, gap })
    }
}

fn find(data: &[u8], needle: &[u8]) -> Result<usize> {
    data.windows(needle.len())
        .position(|w| w == needle)
        .ok_or_else(|| Error::Structure("signature placeholder missing".into()))
}

impl PreparedSignature {
    /// The exact content to pass to a detached CMS signer, in byte order.
    pub fn signing_bytes(&self) -> Vec<u8> {
        [&self.bytes[..self.gap.start], &self.bytes[self.gap.end..]].concat()
    }

    /// Embeds the provider's DER CMS without changing the signed byte ranges.
    /// The provider is responsible for creating and validating the CMS signature.
    pub fn finish(mut self, cms: &[u8]) -> Result<Vec<u8>> {
        if cms.is_empty() || cms.len() > (self.gap.len() - 2) / 2 {
            return Err(Error::Unsupported(
                "CMS signature is empty or exceeds reserved space".into(),
            ));
        }
        const HEX: &[u8] = b"0123456789ABCDEF";
        for (i, b) in cms.iter().enumerate() {
            self.bytes[self.gap.start + 1 + i * 2] = HEX[(b >> 4) as usize];
            self.bytes[self.gap.start + 2 + i * 2] = HEX[(b & 15) as usize];
        }
        Ok(self.bytes)
    }
}

/// A detached signature's untrusted metadata and validated byte coverage.
/// This is NOT a certificate trust or cryptographic verification result.
#[derive(Debug, Clone)]
pub struct SignatureInspection {
    pub object: crate::ObjRef,
    pub signer: Option<String>,
    pub subfilter: Option<String>,
    pub signed_revision_len: usize,
    pub has_subsequent_bytes: bool,
    pub cms: Vec<u8>,
    pub signed_bytes: Vec<u8>,
}

impl Document {
    /// Inspect current and historical indirect signature dictionaries through /Prev.
    /// Unchanged object versions are listed once. Each malformed candidate or
    /// incomplete historical traversal is returned as a separate error.
    /// Byte coverage is measured against the complete, unchanged source file.
    pub fn inspect_signatures(&self) -> Result<Vec<Result<SignatureInspection>>> {
        if self.is_encrypted() {
            return Err(Error::Unsupported(
                "inspection of encrypted signatures".into(),
            ));
        }
        let mut signatures = Vec::new();
        let mut budget = SIGNED_BYTES_BUDGET;
        let errors = self.visit_signature_history(true, &mut |doc, reference, dict| {
            if is_signature(dict) {
                signatures.push(inspect(doc.data(), reference, dict, &mut budget));
            }
        });
        signatures.extend(errors.into_iter().map(Err));
        Ok(signatures)
    }

    pub(crate) fn visit_signature_history(
        &self,
        deduplicate: bool,
        visitor: &mut impl FnMut(&Document, crate::ObjRef, &Dict),
    ) -> Vec<Error> {
        let mut errors = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        if let Some(offset) = self.startxref() {
            visited.insert(offset);
        }
        self.inspect_graph(&mut seen, &mut errors, deduplicate, visitor);
        let mut previous = self.trailer().get(b"Prev").cloned();
        while let Some(value) = previous.take() {
            let offset = value.as_i64().and_then(|n| usize::try_from(n).ok());
            let Some(offset) = offset else {
                errors.push(Error::Structure("invalid signature history /Prev".into()));
                break;
            };
            if visited.len() >= 128 || !visited.insert(offset) {
                errors.push(Error::Structure(
                    "signature history cycle or revision limit reached".into(),
                ));
                break;
            }
            match self.at_xref(offset) {
                Ok(revision) => {
                    revision.inspect_graph(&mut seen, &mut errors, deduplicate, visitor);
                    previous = revision.trailer().get(b"Prev").cloned();
                }
                Err(error) => {
                    errors.push(error);
                    break;
                }
            }
        }
        errors
    }

    fn inspect_graph(
        &self,
        seen: &mut std::collections::HashSet<(
            crate::ObjRef,
            crate::XrefEntry,
            Option<crate::XrefEntry>,
        )>,
        errors: &mut Vec<Error>,
        deduplicate: bool,
        visitor: &mut impl FnMut(&Document, crate::ObjRef, &Dict),
    ) {
        let doc = self;
        for number in doc.object_numbers() {
            let generation = match doc.xref_entry(number) {
                Some(crate::XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            let reference = crate::ObjRef::new(number, generation);
            let Some(entry) = doc.xref_entry(number) else {
                continue;
            };
            // A compressed object's identity also depends on the version of its
            // containing stream, which a later revision can replace in place.
            let container = match entry {
                crate::XrefEntry::Compressed { stream, .. } => doc.xref_entry(stream),
                _ => None,
            };
            if entry == crate::XrefEntry::Free
                || (deduplicate && !seen.insert((reference, entry, container)))
            {
                continue;
            }
            let object = match doc.get(reference) {
                Ok(object) => object,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            let Some(dict) = object.as_dict() else {
                continue;
            };
            visitor(self, reference, dict);
        }
    }
}

/// Total signed-content copies held by one inspection. Each candidate copies its
/// covered bytes, so many dictionaries in a large file could otherwise exhaust memory.
const SIGNED_BYTES_BUDGET: usize = 512 * 1024 * 1024;

pub(crate) fn is_signature(dict: &Dict) -> bool {
    dict.is_type(b"Sig") || (dict.get(b"ByteRange").is_some() && dict.get(b"Contents").is_some())
}

fn inspect(
    data: &[u8],
    object: crate::ObjRef,
    dict: &Dict,
    budget: &mut usize,
) -> Result<SignatureInspection> {
    let invalid = || Error::Structure("invalid signature byte coverage".into());
    let range = dict
        .get(b"ByteRange")
        .and_then(Object::as_array)
        .ok_or_else(invalid)?;
    if range.len() != 4 {
        return Err(invalid());
    }
    let values = range
        .iter()
        .map(|o| {
            o.as_i64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(invalid)
        })
        .collect::<Result<Vec<_>>>()?;
    let (start, first_len, second, second_len) = (values[0], values[1], values[2], values[3]);
    let end = second.checked_add(second_len).ok_or_else(invalid)?;
    if start != 0 || first_len >= second || second > data.len() || end > data.len() {
        return Err(invalid());
    }
    // Require the entire excluded gap to be precisely the hex Contents string,
    // not an arbitrary unsigned region containing additional PDF objects.
    let gap = &data[first_len..second];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return Err(invalid());
    }
    let mut decoded = Vec::new();
    let mut high = None;
    for &byte in &gap[1..gap.len() - 1] {
        if matches!(byte, 0 | 9 | 10 | 12 | 13 | 32) {
            continue;
        }
        let digit = (byte as char).to_digit(16).ok_or_else(invalid)? as u8;
        if let Some(h) = high.take() {
            decoded.push(h * 16 + digit);
        } else {
            high = Some(digit);
        }
    }
    if let Some(h) = high {
        decoded.push(h * 16);
    }
    let contents = dict
        .get(b"Contents")
        .and_then(Object::as_string)
        .ok_or_else(invalid)?;
    if decoded != contents.bytes || decoded.is_empty() {
        return Err(invalid());
    }
    let signed_len = first_len + (end - second);
    *budget = budget
        .checked_sub(signed_len)
        .ok_or_else(|| Error::Unsupported("signature inspection memory limit reached".into()))?;
    Ok(SignatureInspection {
        object,
        signer: dict
            .get(b"Name")
            .and_then(Object::as_string)
            .map(|s| crate::text_from_bytes(&s.bytes)),
        subfilter: dict
            .get(b"SubFilter")
            .and_then(Object::as_name)
            .map(|n| n.as_str_lossy().into_owned()),
        signed_revision_len: end,
        has_subsequent_bytes: end < data.len(),
        cms: decoded,
        signed_bytes: [&data[..first_len], &data[second..end]].concat(),
    })
}

#[cfg(test)]
mod inspection_tests {
    use super::*;
    fn dictionary(range: [i64; 4]) -> Dict {
        let mut dict = Dict::new();
        dict.insert(
            "ByteRange",
            Object::Array(range.into_iter().map(Object::Integer).collect()),
        );
        dict.insert(
            "Contents",
            Object::String(PdfString {
                bytes: vec![0x30, 0],
                hex: true,
            }),
        );
        dict
    }
    #[test]
    fn rejects_invalid_coverage() {
        let data = b"abc<3000>tail";
        for range in [
            [1, 3, 9, 4],
            [0, -1, 9, 4],
            [0, 10, 9, 4],
            [0, 3, 99, 4],
            [0, 3, 9, i64::MAX],
            [0, 2, 9, 4],
            [0, 3, 10, 3],
        ] {
            assert!(
                inspect(data, crate::ObjRef::new(1, 0), &dictionary(range), &mut {
                    usize::MAX
                })
                .is_err(),
                "{range:?}"
            );
        }
        let mut dict = dictionary([0, 3, 9, 4]);
        dict.insert("Contents", Object::String(PdfString::literal(vec![1, 2])));
        assert!(inspect(data, crate::ObjRef::new(1, 0), &dict, &mut { usize::MAX }).is_err());
    }
    #[test]
    fn later_bytes_are_explicit_and_not_part_of_signed_content() {
        let dict = dictionary([0, 3, 9, 4]);
        let inspection = inspect(
            b"abc<3000>tailLATER",
            crate::ObjRef::new(1, 0),
            &dict,
            &mut { usize::MAX },
        )
        .unwrap();
        assert_eq!(inspection.signed_bytes, b"abctail");
        assert_eq!(inspection.signed_revision_len, 13);
        assert!(inspection.has_subsequent_bytes);
    }
    #[test]
    fn signed_copies_share_one_memory_budget() {
        let dict = dictionary([0, 3, 9, 4]);
        let data = b"abc<3000>tail";
        let mut budget = 10;
        assert!(inspect(data, crate::ObjRef::new(1, 0), &dict, &mut budget).is_ok());
        assert_eq!(budget, 3);
        let error = inspect(data, crate::ObjRef::new(2, 0), &dict, &mut budget).unwrap_err();
        assert!(error.to_string().contains("memory limit"), "{error}");
        assert_eq!(budget, 3);
    }
}
