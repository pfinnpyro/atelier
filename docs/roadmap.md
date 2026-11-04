# Atelier roadmap and current status

This is the single source for implementation plans and outstanding validation.
It replaces up-next.md, UI_REVIEW_CHANGES.md, REVIEW-NEXT-PASS.md,
docs/feature-passes.md and docs/execution-plan.md. Product usage belongs in the
[README](../README.md), current history semantics in [editing history](editing-history.md),
release procedures in [releases](releases.md), and past test evidence in the
[historical QA record](../QA_EVALUATION.md).

## Platform and evidence policy

The current development host is Windows. **macOS is not available on this host.**
Defer Keychain implementation, native macOS visual/interaction QA, PDFKit checks,
packaging, signing/notarization and updater validation until a Mac or an appropriate
native CI runner is available. These tasks do not block work that can be implemented
and validated on Windows. macOS retains its native titlebar; a unified toolbar
requires supported native window-control positioning and subsequent Mac testing.
Linux native UI checks also require an appropriate environment.

Earlier macOS smoke reports are historical evidence, not validation of this working
tree. Configured cross-platform CI is not a passing CI result. Do not mark a feature
complete from a build alone when its acceptance requires interaction or an independent
PDF reader. The repository contains working-tree changes beyond the published releases.

## Implemented baseline

| Area | Current behavior | Boundaries |
| --- | --- | --- |
| Viewer and navigation | GPU tiles, tabs, search, outline, thumbnail spreads, paired-page view, configurable toolbar, typed 10-800% zoom, themes | Paired-page view uses one document; it is not document comparison |
| Review and markup | Highlight, Underline, StrikeOut, pinned and inline comments, author/subject/text/timestamps, comment pane, saved/new ink editing and whole-stroke erasing | Comment authorship is descriptive metadata; arbitrary annotation metadata, appearances and non-RGB colors are not fully preserved when ink is replaced |
| History | Opt-in persistent revision journal, Changes pane, explicit editing sessions, before/after snapshots, dependency-safe Accept/Reject, undo/redo and decision history | Snapshot restoration is not semantic three-way merge; annotation fallback undo and tracked revisions are not yet a unified history |
| Field properties | Font resource/size/color/alignment, character/word/line spacing and widget geometry, with typed validation and property targets | Journal values are still serialized as strings; arbitrary page text/style editing is unsupported |
| Page organization | Grid selection, range selection, checked-group drag/drop, move/rotate/export, insert and split | Extraction omits source-level forms/bookmarks and starts a fresh history; preservation and large-file coverage need expansion |
| Ink signatures | Saved drawing, preview, named library copies, replace/delete/select, optional certificate-signing prompt after placement | Ink alone proves no identity; deleting the active drawing does not delete named copies; pending ink counts as unsaved work and is committed on save |
| Encryption | Background AES-256 revision 6 copy export, system randomness, one open/owner password, unrestricted permissions | New passwords are printable ASCII up to 127 bytes; existing encryption cannot be changed/removed; signature fields are rejected |
| File signing | Detached CMS over the current revision, including placed ink; PEM and password-protected/passwordless PKCS#12 (.p12/.pfx), with PKCS#12 CA-chain inclusion | Requires OpenSSL; exactly one signing identity; no legacy-provider fallback or encrypted-PDF signing |
| Windows signing | Background discovery in CurrentUser/Personal; subject/issuer/expiry/thumbprint selection; native .NET detached SHA-256 CMS | Includes signer certificate only; no LocalMachine selection, native trust verdict or claimed hardware-token compatibility |
| Signature inspection | Validated ByteRange/gap extraction, CMS and exact signed bytes, byte endpoint and later-byte detection, per-candidate errors | Current indirect dictionaries only; replaced/removed historical signatures may be absent; byte coverage does not classify edits |
| Signature verification | Background OpenSSL CMS checks, per-document results, stale-result replacement, retry, all verifying signer certificates and SHA-256 fingerprints | Supported detached formats only; encrypted inspection is unsupported; no overall trusted-document or PAdES badge |
| Certificate checks | Separate integrity, chain trust without dates, full chain with dates, and signer validity at verification time | OpenSSL CA store or selected per-tab PEM bundle, not native OS trust; purpose `any` does not evaluate document-signing policy; revocation and timestamps remain unchecked |
| Compatibility and scripts | Document-level XFA detection/classification/fallback gate, hybrid-save warnings, embedded-file/action inspection, exact-source script approvals | XFA datasets are not synchronized; XFA/FormCalc execution is unsupported; approved AcroForm/click scripts use a constrained helper, not an OS-enforced sandbox |
| Settings and integration | Saved per-button icon/text modes, Set all, bounded Settings scrolling, signature library controls and platform default-app guidance | Current narrow-window and native platform interaction checks remain open |
| Window chrome | Custom Windows controls with native viewport drag/resize commands; native decorations on macOS/Linux; Windows opt-out through ATELIER_NATIVE_TITLEBAR | Snap hover/system-menu parity, mixed DPI and keyboard accessibility need manual checks |

Signing passwords are not saved in settings or command arguments. File identities
use child-process environment passwords; UI values are dropped on job start,
close and credential/mode changes. This is not a memory-zeroization guarantee.
PKCS#12 private keys are read directly by OpenSSL, never extracted by Atelier.
Windows keys remain with the OS provider, which may request authorization.
Native selection is session-only; store discovery opens read-only. Temporary
content/public-certificate/CMS workspaces are cleaned on normal success/error exits.
Export rejects source/credential paths and writes only after signing succeeds.
Native Windows signing can generate CMS without OpenSSL; verification still needs it.

## Execution order and shared foundations

Continue in the order below, with validation alongside each affected feature.
macOS-specific tasks stay deferred rather than becoming the next Windows-host task.
Security policy can move ahead if document access control becomes the immediate need.
Renderer/security correctness regressions take precedence over feature expansion.
The next actionable Windows-host feature is historical signature/revision analysis;
Mac-only work is deferred, not the next blocking step.

| Priority | Deliverable | Reuse and dependency |
| --- | --- | --- |
| 1 | Finish portable signature policy and revision analysis | Existing byte inspection, provider interface and sidebar; feeds signed-edit policy, comparison and recovery identity |
| 2 | Annotation preservation, selection and unified history | Builds on existing snapshots/property tracking; common base for geometry tools, review navigation and recovery |
| 3 | Autosave, crash recovery and review workflow | Reuses stable edit identities, source revision fingerprints and annotation index |
| 4 | Encryption policy controls | Shared security export/provider UI and independent-reader fixtures |
| 5 | Compare separate documents | Shared document sessions, page/text/geometry index and revision inspection |
| 6 | Secure redaction and sanitization | Requires actual content removal, history-free full rewrite and independent extraction/render checks |
| Parallel | Renderer corpus, bounded processing, accessibility and distribution quality | Cross-cutting acceptance gates; platform-specific work only where it can be validated |
| Deferred platform lane | macOS Keychain, UI and release validation; Linux native UI validation | Resume on matching hardware or native CI; do not infer runtime success from Windows tests |

### 1. Signature completion

Completed: byte-range foundation, background integrity verification, separate chain/date
results, sidebar, PKCS#12 identities and Windows CurrentUser/Personal signing.

- [ ] Enumerate signed historical revisions, including signatures replaced/removed later.
- [ ] Classify subsequent changes and enforce DocMDP/FieldMDP before editing, rewriting,
  extraction or sanitization. Preserve original signed files and explain signature loss.
- [ ] Define document-signing certificate usage/policy and explicit unknown/offline results.
  Do not promote appearance metadata, self-reported signing time or CMS integrity into trust.
- [ ] Add timestamp and revocation providers with bounded/offline/failure behavior, then
  independently validate any advertised PAdES level (B-B before B-T/B-LT/B-LTA).
- [ ] Support signing encrypted PDFs after encrypted signature parsing/interoperability.
- [ ] Add a positioned signature widget/appearance transaction with signer/reason preview
  on rotated/cropped pages. Current invisible CMS covers ink already placed in the document.
- [ ] Independently verify exported signatures before reporting signing success; handle
  CMS reservation overflow by rebuilding/re-signing, never truncating.
- [ ] Expand Windows providers, local intermediate inclusion and native trust separately;
  test real non-exportable keys, RSA/ECDSA, authorization cancellation and PIN failures.
- [ ] Defer macOS Keychain and hardware-token claims until their providers can be tested.

Acceptance corpus: visible/invisible and multiple signatures, malformed/overlapping
ranges and unsigned gaps, tampered content/appearance, expired/not-yet-valid/untrusted/
revoked certificates, offline status, timestamp failure, permitted/forbidden later
edits, encrypted input, oversized CMS, cancelled prompts, disk/write failures and
cross-viewer/independent-validator checks. No PAdES conformance is claimed today.

### 2. Annotation model, history and semantic merge

- [ ] Preserve third-party dictionaries, custom appearances and non-RGB colors where
  edits permit; make stable annotation/element identity independent of PDF object numbers.
- [ ] Unify document-wide undo/redo across annotation, form and page operations while
  retaining the existing dependency guard until semantic restoration is reliable.
- [ ] Build selection handles and move/resize transforms, then shapes/arrows/text boxes,
  partial-stroke erasing and pressure-sensitive ink on the shared geometry/history model.
- [ ] Reuse one annotation index for filtering, navigation and review workflows.
- [ ] Broaden split/merge form, bookmark, resource and annotation preservation tests.

The planned semantic model extends current history rather than assuming snapshots are
mergeable. Use persistent element UUIDs and separate UUID-to-PDF bindings; fields and
widgets become distinct elements, with future page text represented as blocks/runs.
Version-2 sessions are immutable nodes with ID, parent IDs, author, start/finish times,
label and typed property changes (before/after/units). Track point geometry, enum
alignment, font asset IDs and tagged gray/RGB/CMYK values. Preserve version-1 revisions
as opaque snapshots and record review decisions as events.

Proposed native artifact: `.atelier` ZIP with versioned manifest/head, rendered
`document.pdf`, `model.json`, revision history and deduplicated hashed assets. Validate
paths, hashes and resource limits; save atomically. This container is not implemented.

Three-way merge compares left/right descendants with a common ancestor. Unchanged-side
and identical changes resolve automatically; independent properties combine only when
element validation permits. Same-property, deletion/edit, overlapping text and
incompatible geometry need explicit Left/Right/custom resolution and rendered preview.
Commit a new node with both parents and retain alternatives and decisions.

Native Save retains the tree. Export PDF writes only the resolved appearance, compacts
unreachable history bytes and leaves the native artifact's saved state/path unchanged.
Test independent changes, conflicts, fonts, damaged history, reopened merges, encrypted
input, rendering equivalence and absence of recoverable snapshot bytes. Existing
tracked deletion is reversible and must never be represented as secure erasure.

### 3. Recovery and review

- [ ] Atomically journal autosave/crash-recovery drafts against source revision fingerprints.
- [ ] Detect changed/missing source documents; provide explicit restore/discard and stale-draft flows.
- [ ] Test interruption, disk full, permissions, concurrent sessions and large documents.
- [ ] Extend existing comment states with a navigable/filterable list, replies, resolved
  workflows, stable modification history and exportable review summaries using standard metadata.

### 4. Encryption policy

- [ ] Separate open and owner passwords and expose print/copy/edit permissions.
- [ ] Change/remove existing encryption with signature-impact checks.
- [ ] Implement Unicode password normalization and interoperable password boundaries.
- [ ] Expand independent-reader fixtures and negative/round-trip coverage.

Viewer-enforced permission flags do not substitute for protecting the opening password.

### 5. Document comparison

- [ ] Open two independent sessions with synchronized navigation.
- [ ] Match pages and highlight text and geometry changes; use raster comparison for scans.
- [ ] Handle different counts/sizes/rotations and reuse revision/page identities where useful.

### 6. Secure redaction

- [ ] Mark regions and preview removal, then remove intersecting text, image and vector content.
- [ ] Sanitize metadata, attachments and active content with explicit signature impact.
- [ ] Rewrite without original historical bytes or private history; incremental hiding is insufficient.
- [ ] Verify by text extraction AND independent rendering; a black annotation is not redaction.

### Cross-cutting quality and security

- [ ] Build a small reproducible reference-renderer corpus: transparency, isolation/knockout,
  blend modes, soft masks, overprint, ICC, shadings/patterns and font fallback. Add RTL/CJK,
  orphan widgets, duplicate names, multiline/Unicode and rotated/cropped geometry fixtures.
- [ ] Bound cumulative object traversal, decoded streams/images/fonts, script extraction,
  allocations, IPC and execution time. Add visited-object tracking, visible incomplete
  inspection results, cancellation/restart, hostile-input fixtures and continuous fuzzing.
- [ ] Isolate PDF workers and strengthen the script helper with OS-enforced restrictions.
  Evaluate Windows restricted tokens/AppContainer/Job Objects and platform equivalents.
  Broker authorized file/network/clipboard/printing/key operations in the UI; scripts
  never inherit signing privileges or gain trust from a document signature.
- [ ] Centralize action policy for JavaScript, Launch, URI, SubmitForm, ImportData, remote
  GoTo, embedded files and XFA. Restrict URI schemes and external effects. Keep blocking
  execution separate from removal of active content, which needs a full graph rewrite.
- [ ] Extend XFA safely: bounded XML with external entities/DTD/XInclude/network disabled;
  read-only packet inspection, static template/dataset binding with conflict reporting,
  then static layout. Preserve unknown packets; defer dynamic pagination/repeating forms
  and FormCalc until resource budgets and isolated execution policy exist.
- [ ] Validate narrow windows, settings height/scrolling, toolbar wrapping and signature
  controls at multiple DPI scales. Exercise Windows snap/system menu, keyboard-only
  navigation, screen readers and contrast. Add tagged PDF structure/reading order.
- [ ] Exercise large documents, rapid tab changes, cancellation, GUI create/save/reopen,
  multi-viewer interoperability, physical printing and OCR engines/language packs.
- [ ] Defer macOS full-screen/titlebar/default-app/PDFKit smoke and Linux X11/Wayland
  interaction tests until matching runtime environments are available.

### Release and updater work

Keep operational steps in [releases](releases.md). Remaining development work:

1. Harden discovery: honor prerelease flags and semantic versions, paginate, require
   matching platform assets/checksums, and handle missing/malformed releases, rate limits,
   offline startup and timeouts without interrupting editing.
2. Authenticate updates: platform signing credentials and signed version/channel/target/
   size/SHA-256 manifest, embedded verification key, expected HTTPS asset hosts and key rotation.
3. Stage/install safely: unique temporary paths, concurrency control, restart/rollback,
   failure recovery and installed/portable Windows coverage. Decide CLI update behavior;
   the current portable updater replaces only the viewer.
4. Defer notarized macOS bundle replacement/updater integration until a Mac and credentials
   are available. Retain manual downloads; evaluate a maintained updater such as Sparkle.
5. Operate channels with immutable assets; add a signed channel manifest if needed. Test
   previous stable/preview transitions, interrupted downloads, failed signatures/checksums,
   unwritable locations, unsaved files and rollback; document withdrawal/key compromise.

## Validation record and next gates

Latest recorded Windows checks: **39 app tests passed**, including native .NET CMS
verified by OpenSSL and tamper rejection; app build check, Clippy with warnings denied,
formatting and diff checks passed. Earlier signature-foundation validation recorded
46 syntax tests. These counts describe the implementation session, not a new
workspace-wide run or new macOS/Linux validation during documentation consolidation.

Coverage includes byte ranges and later revisions; untrusted/missing-CA/expired/
not-yet-valid certificates; root/intermediate/signer chains and unrelated roots;
PKCS#12 passwords, malformed/missing/certificate-only identities and CA-chain inclusion;
encrypted PEM keys, mismatched keys and 2 MB signing input; read-only Windows discovery,
missing native selections, Unicode metadata and ephemeral-key native CMS. No test
certificate was installed into the user's Windows store.

Manual gates remain: real store-key authorization, non-exportable/provider/ECDSA/token
behavior, native UI/DPI/accessibility, independent encryption readers, physical printing,
OCR and clean-machine install/update/rollback. Existing macOS reports and historical
workspace counts are preserved in [QA_EVALUATION.md](../QA_EVALUATION.md), explicitly
separate from validation possible on this Windows host.

## Technical references

- [PDF signature dictionaries and byte ranges](https://developer.adobe.com/document-services/docs/assets/35e4369068f86065372c18787171a17e/PDF_ISO_32000-1.pdf)
- [OpenSSL CMS](https://docs.openssl.org/3.6/man1/openssl-cms/), [PKCS#12](https://docs.openssl.org/3.5/man1/openssl-pkcs12/), [chain verification](https://github.com/openssl/openssl/blob/master/doc/man1/openssl-verification-options.pod)
- [Windows SignedCms](https://learn.microsoft.com/en-us/dotnet/api/system.security.cryptography.pkcs.signedcms.computesignature?view=netframework-4.8.1), [Apple CMS identities](https://developer.apple.com/documentation/security/cmsencoderaddsigners(_:_:))
- [PAdES EN 319 142-1](https://www.etsi.org/deliver/etsi_en/319100_319199/31914201/01.02.00_20/en_31914201v010200a.pdf), [Adobe signature API](https://developer.adobe.com/acrobat-pdfl-sdk/guides/jsapiref/security-signature-apis)
- [Revision 6 corrections](https://pdfa.org/iso-32000-22020-clause-7-syntax/), [XFA](https://pdfa.org/norm-refs/XFA-3_3.pdf), [FormCalc](https://www.w3.org/1999/05/XFA/xfa-formcalc.html)
- [Acrobat JavaScript controls](https://www.adobe.com/devnet-docs/acrobatetk/tools/AppSec/javascript.html), [sandbox concepts](https://www.adobe.com/devnet-docs/acrobatetk/tools/AppSec/sandboxprotections.html)
- [Sparkle](https://sparkle-project.org/documentation/), [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
