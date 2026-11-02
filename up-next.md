# Up next

Work is paused after the initial encryption, certificate signing, markup and UI pass.
The following are proposed next passes, not commitments to implement yet.

## Recommended order

1. **Signature trust and identities** — Complete the verification side of certificate signing.
2. **Complete markup editing** — Extend the current drawing tools into a fuller markup workflow.
3. **Recovery and review workflow** — Protect editing work and improve collaboration.

Prioritize security policy controls instead if document access control is the immediate need.

## 1. Signature trust and identities

- Verify CMS signatures and document integrity when opening PDFs.
- Add a signatures sidebar with signer details, certificate chain, signed revisions and subsequent changes.
- Distinguish cryptographic validity from certificate trust, expiration and revocation status.
- Support PKCS#12 identities, macOS Keychain and Windows certificate stores.
- Add timestamping, revocation checks and eventually hardware tokens.
- Support signing encrypted PDFs.

Current signing uses OpenSSL with PEM credentials and covers the current document revision, including placed ink. Atelier does not yet verify signatures or certificate trust.

## 2. Complete markup editing

- Add selection handles for moving and resizing annotations.
- Add shapes, arrows and text boxes.
- Implement partial-stroke erasing and pressure-sensitive ink.
- Add redo and a consistent document-wide edit history.
- Add an annotation list with filtering and navigation.
- Preserve third-party annotation metadata, custom appearances and non-RGB colours when editing.

Current ink editing supports reopening, redrawing and whole-stroke erasing. Saved ink is replaced with a new standard Ink annotation.

## 3. Security policy controls

- Separate opening and owner passwords in the UI.
- Expose print, copy and edit permission controls.
- Support changing and removing existing encryption.
- Implement Unicode password normalization.
- Expand encryption interoperability fixtures and tests.

Current encrypted export uses AES-256 revision 6 with a single open/owner password, unrestricted permissions after opening and printable ASCII passwords. Documents with signature fields are rejected to avoid invalidating signatures. PDF permissions are enforced by viewers; they do not replace protection of the opening password.

## 4. Recovery and review workflow

- Add autosave and crash recovery drafts.
- Key recovery data to the source document revision and provide a restore flow.
- Add comment status and replies using standard review metadata.
- Export review summaries.

## 5. Compare documents

- Display two documents with synchronized navigation.
- Match pages and highlight text and geometry differences.
- Use raster comparison for scanned pages.

Current side-by-side mode pairs pages within one document; it does not compare separate files.

## 6. Secure redaction

- Mark sensitive regions and preview their removal.
- Remove intersecting text, image and vector content.
- Sanitize metadata and attachments.
- Rewrite output without historical document bytes.
- Verify removal through text extraction and rendering.

A black annotation alone is not secure redaction.

## UI follow-up

- Check narrow windows and multiple display scales on macOS and Windows.
- Review the taller default Settings window and toolbar wrapping.
- Consider a unified macOS titlebar/toolbar once supported native traffic-light positioning is available.

The current macOS implementation uses the native titlebar. An isolated visual smoke review covered toolbar alignment, settings separation and scrolling, and the toolbar's Set all control.

## Handoff reference

See [the feature-pass notes](docs/feature-passes.md) for the implemented scope, current limitations and validation details.
