# Feature passes

## Implemented in this pass

- AES-256 revision 6 encrypted-copy export, printable ASCII passwords, system
  random keys/salts/IVs, and background export work.
- Detached CMS certificate signing through OpenSSL with PEM credentials;
  signing covers placed ink as well as all other content in the current revision.
- A reusable signature preparation API with fixed byte ranges and reserved CMS space.
- Signature settings: preview, create/replace/delete, named library copies,
  select a saved copy, and optional certificate-signing prompt after ink placement.
- Context menus work while drawing. Saved and new ink can be reopened, redrawn,
  erased by stroke or removed. Saved annotation removal and comment edits are undoable.
- Padding in Settings and the save prompt, section separators, bounded settings
  scrolling, toolbar row height, and a Set all toolbar display control.
- Native macOS titlebar placement. Default-app instructions by platform, with
  Finder reveal and Apple's instructions on macOS and the existing registration
  and Settings handoff on Windows.
- Pending ink counts as unsaved work and is committed when saving.

## Current boundaries

Security export is an initial usable workflow, not a complete signature trust
product. Signing requires an `openssl` executable and PEM files; private keys
are never imported into Atelier's persistent settings. No certificate validation,
revocation checking, timestamping, PKCS#12 import, OS key stores or hardware tokens
are implemented. Encrypted documents cannot yet be signed, and existing encryption
cannot yet be replaced. Encryption rejects signature fields rather than rewriting
signed content. The UI uses one open/owner password and does not expose permission
restrictions. Unicode password normalization remains to be implemented.

Ink editing currently replaces a saved Ink annotation with a new standard Ink
annotation. Geometry, RGB colour and line width are retained; arbitrary third-party
annotation metadata, custom appearances and non-RGB colour spaces are not fully
preserved. Erasing removes whole strokes, not portions of a stroke. Move/resize
handles, pressure sensitivity, shapes and annotation redo are not implemented.
Deleting the active signature does not delete separate named library copies.

The native macOS titlebar fixes control placement by putting window controls in
the system titlebar; a unified custom toolbar would require supported native
traffic-light positioning. A macOS smoke review checked toolbar alignment,
settings separation/scrolling and Set all behavior in an isolated app build.
Additional visual QA is still needed at multiple display scales and narrow window sizes.

## Options for the next passes

| Option | Concrete result | Work involved |
| --- | --- | --- |
| **1. Signature trust and identities** | A signatures sidebar with cryptographic integrity, signer details, certificate chain and revision coverage; PKCS#12 import and native key stores | CMS/X.509 verification, trust policy, credential storage, then timestamps and revocation. Never display a trusted badge from appearance metadata alone. |
| **2. Complete markup editing** | Selection handles to move/resize marks, shapes/arrows/text boxes, partial-stroke erasing, redo and annotation list/filtering | General annotation selection/model, geometry transforms and a document-wide edit history. Preserve existing annotation metadata and appearances when possible. |
| **3. Security policy controls** | Separate open/owner passwords, print/copy/edit controls, password changes/removal and Unicode passwords | SASLprep, permission UI, encryption rewrite validation and interoperability fixtures. PDF permissions are viewer-enforced, not a substitute for keeping the opening password secret. |
| **4. Recovery and review workflow** | Crash recovery drafts, autosave, comment status/replies and exportable review summaries | Recovery journal keyed to source revision, restore UI and standards-compatible review metadata. |
| **5. Compare documents** | Two files with synchronized navigation and highlighted content differences | Page matching, text/geometry diff, raster fallback for scans, and a comparison UI. Current side-by-side mode pairs pages of one document. |
| **6. Secure redaction** | Mark sensitive areas, preview removals, then export a scrubbed document | Remove intersecting text/images/vector content, sanitize metadata/attachments and rewrite without historical bytes; verify by extraction and rendering. A black annotation alone is insufficient. |

Suggested order: **1**, then **2**, then **4**. This completes the trust story for
certificate signing and gives the daily editing workflow a consistent interaction
and recovery model. Choose **3** first if document access control is the immediate priority.

## Validation

The security writer has round-trip and negative tests, an Apple PDFKit opening
check on macOS, and an OpenSSL CMS verification test that also rejects modified
content. Annotation tests cover saved removal, stable ordinals and undo. Existing
viewer, form and syntax tests continue to cover incremental saving, encrypted
forms, annotation rendering, page reordering and toolbar geometry.

Specification references: [PDF signature dictionaries and byte ranges](https://developer.adobe.com/document-services/docs/assets/35e4369068f86065372c18787171a17e/PDF_ISO_32000-1.pdf),
[revision 6 standard security handler corrections](https://pdfa.org/iso-32000-22020-clause-7-syntax/),
and [Apple's default-app guidance](https://support.apple.com/guide/mac-help/mh35597/mac).
