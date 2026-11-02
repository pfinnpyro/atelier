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
