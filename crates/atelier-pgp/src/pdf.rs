//! PDF files protected with OpenPGP.
//!
//! The whole PDF is encrypted as one OpenPGP message, and that message
//! travels as the encrypted payload of a small wrapper PDF (ISO 32000-2,
//! 7.6.7) whose only page says what it is. Unlike PDF's own encryption,
//! nothing of the document shows from outside (not its structure, its
//! size in pages, nor its metadata) and any change to it is detected.
//! The cost is that the file is decrypted whole when opened and sealed
//! whole when saved.
//!
//! The payload is plain RFC 9580: taken out of the wrapper, any OpenPGP
//! program that knows AES-GCM and the NIST curves decrypts it to the
//! original PDF.

use atelier_syntax::{
    Cover, Document, OPENPGP_PAYLOAD, OPENPGP_PAYLOAD_VERSION, wrap_encrypted_payload,
};

use crate::{
    Certificate, Error, Provider, Readers, Result, SecretKey, Summary, decrypt_keeping_readers,
    encrypt, summary,
};

/// What the embedded message is called.
const PAYLOAD_NAME: &str = "document.pdf.pgp";

/// Encrypts a PDF file to `recipients` and, if given, a `passphrase`,
/// and returns the wrapper PDF that carries it.
pub fn protect_pdf(
    provider: &dyn Provider,
    pdf: &[u8],
    recipients: &[&Certificate],
    passphrase: Option<&[u8]>,
    now: u64,
) -> Result<Vec<u8>> {
    Ok(wrap(&encrypt(provider, pdf, recipients, passphrase, now)?))
}

/// Encrypts a changed PDF for exactly the readers of the protected file
/// it was opened from (see [`Readers`]), and returns the wrapper PDF.
pub fn protect_pdf_for(provider: &dyn Provider, pdf: &[u8], readers: &Readers) -> Result<Vec<u8>> {
    Ok(wrap(&readers.encrypt(provider, pdf)?))
}

fn wrap(message: &[u8]) -> Vec<u8> {
    let cover = Cover {
        title: "This document is encrypted",
        lines: &[
            "It is protected with OpenPGP and can be read only by the people it was",
            "encrypted for, or with its passphrase.",
            "",
            "To read it, open this file in Atelier or another PDF viewer that",
            "understands OpenPGP-protected documents.",
            "",
            "The encrypted document is attached to this page as document.pdf.pgp. It is",
            "a standard OpenPGP message (RFC 9580, AES-256 in GCM); any OpenPGP",
            "program that supports those decrypts it to the original PDF.",
        ],
    };
    wrap_encrypted_payload(
        message,
        OPENPGP_PAYLOAD,
        OPENPGP_PAYLOAD_VERSION,
        PAYLOAD_NAME,
        &cover,
    )
}

/// The OpenPGP message inside `doc`, if it is such a wrapper. An error
/// for a wrapper around something else, which this cannot open.
pub fn protected_payload(doc: &Document) -> Result<Option<Vec<u8>>> {
    let Ok(Some(payload)) = doc.encrypted_payload() else {
        return Ok(None);
    };
    if payload.filter != OPENPGP_PAYLOAD {
        return Err(Error::Unsupported(format!(
            "documents protected by {}",
            payload.filter
        )));
    }
    Ok(Some(payload.data))
}

/// Who the payload is encrypted for, to know what to ask for.
pub fn payload_summary(payload: &[u8]) -> Result<Summary> {
    summary(payload)
}

/// Decrypts a payload back to the PDF file it holds.
pub fn unprotect_pdf(
    provider: &dyn Provider,
    payload: &[u8],
    keys: &[&SecretKey],
    passphrase: Option<&[u8]>,
) -> Result<Vec<u8>> {
    Ok(unprotect_pdf_keeping_readers(provider, payload, keys, passphrase)?.0)
}

/// Decrypts a payload, and also returns what [`protect_pdf_for`] needs
/// to save a changed copy for the same readers.
pub fn unprotect_pdf_keeping_readers(
    provider: &dyn Provider,
    payload: &[u8],
    keys: &[&SecretKey],
    passphrase: Option<&[u8]>,
) -> Result<(Vec<u8>, Readers)> {
    let (pdf, readers) = decrypt_keeping_readers(provider, payload, keys, passphrase)?;
    if !pdf[..pdf.len().min(1024)].windows(5).any(|w| w == b"%PDF-") {
        return Err(Error::Malformed("what was encrypted is not a PDF".into()));
    }
    Ok((pdf, readers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Curve, Software};

    const NOW: u64 = 1_800_000_000;

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        5 0 obj << /Title (A secret title) >> endobj\n\
        trailer << /Root 1 0 R /Info 5 0 R >>\n%%EOF";

    #[test]
    fn a_protected_pdf_shows_only_its_cover_until_opened() {
        let ada = SecretKey::generate(&Software, Curve::P256, "Ada", NOW).unwrap();
        let wrapped = protect_pdf(
            &Software,
            PDF,
            &[ada.certificate()],
            Some(b"or this passphrase"),
            NOW,
        )
        .unwrap();
        assert!(!wrapped.windows(14).any(|w| w == b"A secret title"));
        let cover = Document::open(wrapped).unwrap();
        assert_eq!(cover.pages().unwrap().len(), 1);
        let payload = protected_payload(&cover).unwrap().unwrap();
        let said = payload_summary(&payload).unwrap();
        assert!(said.passphrase);
        assert_eq!(said.recipients.len(), 1);

        for (keys, passphrase) in [
            (vec![&ada], None),
            (vec![], Some(&b"or this passphrase"[..])),
        ] {
            let pdf = unprotect_pdf(&Software, &payload, &keys, passphrase).unwrap();
            assert_eq!(pdf, PDF);
            assert_eq!(Document::open(pdf).unwrap().pages().unwrap().len(), 2);
        }
        assert_eq!(
            unprotect_pdf(&Software, &payload, &[], Some(b"not this one")),
            Err(Error::NoKey)
        );

        // Opened by passphrase, changed and saved: Ada's key still opens
        // it, though her certificate was not at hand.
        let (pdf, readers) =
            unprotect_pdf_keeping_readers(&Software, &payload, &[], Some(b"or this passphrase"))
                .unwrap();
        let mut changed = pdf.clone();
        changed.extend_from_slice(b"\n% changed\n");
        let saved = protect_pdf_for(&Software, &changed, &readers).unwrap();
        let payload = protected_payload(&Document::open(saved).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            unprotect_pdf(&Software, &payload, &[&ada], None).unwrap(),
            changed
        );
    }

    #[test]
    fn other_documents_are_told_apart() {
        let plain = Document::open(PDF.to_vec()).unwrap();
        assert_eq!(protected_payload(&plain), Ok(None));
        let other = wrap_encrypted_payload(b"x", "SomeoneElses", "1", "x.bin", &Cover::default());
        let other = Document::open(other).unwrap();
        assert!(matches!(
            protected_payload(&other),
            Err(Error::Unsupported(_))
        ));
        // A message that decrypts to something other than a PDF.
        let sealed = encrypt(&Software, b"not a PDF", &[], Some(b"pw"), NOW).unwrap();
        assert!(matches!(
            unprotect_pdf(&Software, &sealed, &[], Some(b"pw")),
            Err(Error::Malformed(_))
        ));
    }
}
