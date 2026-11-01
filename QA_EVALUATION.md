# Atelier feature and quality evaluation

Evaluated 2026-11-01 on macOS, Rust 1.99.1, using the current working tree.
Existing application, renderer, README and IDE edits were preserved. This change
adds tests, a dependency-free smoke runner and this assessment; it does not add
certificate signing, a JavaScript runtime, an OS sandbox or XFA rendering.

## Result

Atelier has a substantial viewer/editor foundation. The tested paths are stable,
including repair, form persistence and inactive action inspection. It is not yet
validated as a high-fidelity, accessible, security-hardened document workstation.
The most valuable next investment is reproducible compatibility and security
coverage, followed by signing and carefully scoped form compatibility.

## Executed validation

| Check | Outcome | Scope |
| --- | --- | --- |
| `cargo test --workspace` before changes | 311 passed | Existing unit, integration and documentation suites |
| `cargo test --workspace` after changes | 315 passed | Four additional form/action/signature regression tests |
| `cargo build -p atelier-app -p atelier-cli` | Passed | Native binaries built locally |
| `python3 tools/qa_pdf_edges.py /tmp/atelier-qa-repeat` | 22/22 passed | Seven inputs, info/render commands, eight rendering equivalence checks |
| Native UI smoke | Passed for exercised paths | Open dialog, two page document, rendering/thumbnails, search across pages, zoom, inspector/action source |

The native app was launched through a disposable `/tmp/AtelierQA.app` wrapper.
The fixture contains Helvetica text, a vector rectangle, negative page origin,
90-degree rotation and a document-open `app.alert(1)` action. Auto-orient reported
turning one page. Text and graphics appeared legible. Searching `page` found two
matches; Enter moved from page one to page two; zoom changed from 174% to 217%.
The inspector displayed the script and the message that scripts never run; no
script alert appeared. Accessibility clicks did not focus the search box on one
attempt; a coordinate click did. This is an automation observation, not proof of
a user-facing focus defect.

The CLI corpus checks a valid input, incorrect startxref, missing xref/trailer,
incorrect stream lengths, truncated EOF, empty input and non-PDF bytes. All four
repairable variants render both pages identically to the valid fixture. The
wrong-length mutation preserves offsets to isolate stream repair. Invalid inputs
exit with a structured error instead of crashing. Each subprocess has a 15-second
timeout. JSON and PDF/PNG fixtures are written to the supplied temporary folder.
PNG equality establishes repair consistency, not correctness against an
independent renderer.

The four added regression tests cover bounded cyclic action chains, `/Next`
arrays with SubmitForm and Launch targets, preservation of stream and packet-array
XFA references while filling AcroForm fields, and rejection of text writes into
signature fields. Existing tests also cover encrypted form round trips, widget
hit testing/tab order, inherited fields, field cycles, annotations, page edits,
malformed xrefs, encryption variants, mutation robustness, layout and tile caches.

## Findings

1. **High: XFA-only compatibility can be hidden.** `Form::load` computes
   `has_xfa`, then returns `None` if there are no AcroForm fields. The inspector's
   Fields tab returns early for `None`. An XFA-only form can therefore look like
   a document with no fields instead of an unsupported form. Detect XFA at the
   document level, independently of editable fields; show a persistent warning
   with supported fallback behavior. This finding is from code inspection.
2. **High: hybrid XFA edits can diverge.** The new tests confirm AcroForm values
   change while XFA references survive. There is no XFA dataset synchronization.
   Warn before saving such edits; do not promise consistent values in XFA-aware
   viewers. Preservation of references is not validation of XFA semantics.
3. **High: ink signing has no cryptographic assurance.** The existing Sign tool
   writes ink annotations and stores reusable strokes in `signature.txt`. The
   README explains this, but the placement notice says “Signed.” Use explicit
   “Ink signature” wording in the UI and reserve certificate-valid status for a
   verified digital signature. Offer deletion of saved strokes and consider
   opt-in persistence and OS-protected storage.
4. **Medium: sidebar controls are cramped.** At the tested 1100x800 window and
   default sidebar width, Pages/Outline/Search/Spreads controls collide or become
   hard to distinguish. Use wrapping or a separate view control row, then test
   minimum window size, high DPI and keyboard navigation.
5. **Medium: action inspection is bounded but incomplete as a security report.**
   Cycles in `/Next` repeat until the depth limit. Name-tree traversal has a depth
   bound and an entry check at function entry, but can collect a large leaf before
   checking the cap again. Truncated traversal is not reported. Add a shared work
   budget, visited-object tracking, decoded-source byte limits and a visible
   “inspection incomplete” result. An empty action list is not a safety verdict.

## Feature assessment

| Area | Current capability and evidence | Next quality gate |
| --- | --- | --- |
| Viewing/navigation | Native tiles, thumbnails, fit modes, zoom, search, selection; UI smoke and view tests | Long documents, rapid tab switches, cancellation, mixed sizes, RTL/CJK search, high DPI |
| Rendering | Broad paths/fonts/images/colour/transparency implementation; engine suite passes | Independent visual corpus for blending, masks, knockout/isolation, overprint, ICC, patterns, font fallback |
| Forms | AcroForm text/choice/button fields, appearances, encrypted saves; integration coverage | Real-world orphan widgets, duplicate names, multiline/Unicode, focus order and screen readers |
| Markup | Highlight/comment/ink, undo and saved appearances; annotation tests | GUI create/save/reopen in multiple viewers; rotated/cropped-page placement |
| Page organization | Rotate/reorder/merge/extract/delete; page integration tests | GUI drag/drop and multi-selection; resource/form/bookmark preservation audits |
| Saving | Incremental edits and full rewrite for deletion; persistence tests | Interrupted writes, disk full, permissions, signed revisions, atomic replacement and recovery |
| Security | Password encryption, inert actions, recursion/field limits | Decode/allocation/time budgets and OS process isolation; continuous fuzzing |
| Signatures | Ink appearance and recognition of signature fields | Certificate-backed signing and independent validation; no current trust verdict |
| XFA | Presence detection for hybrid forms; AcroForm fallback | XFA-only warning first, then static data binding and explicitly scoped rendering |
| OCR/printing | Implemented with supporting tests | End-to-end printer/OCR engine availability, cancellation, language packs, output comparisons |
| Accessibility | Native control accessibility is observable | Tagged PDF reading order, keyboard-only workflows, screen-reader and contrast audits |
| Distribution | Windows installer/updater infrastructure exists | Windows clean-machine install/update/rollback, signed packages and authenticated release metadata |

These are capability assessments from implementation and existing tests, not a
claim that every feature was manually exercised. Physical printing, live OCR,
update/install, GUI save/discard flows, Windows behavior, screen readers and
certificate interoperability were not tested in this pass.

## Certificate-backed signatures with an ink appearance

Keep the pen drawing as the visible appearance of a real signature widget.
The cryptographic signature must cover document bytes, including the final
appearance, rather than merely an exported image of the strokes.

Recommended sequence:

1. Build a read-only signature panel first: enumerate signed revisions and
   signature fields; validate ByteRange structure, CMS digest/signature,
   certificate chain, usage, timestamps, and subsequent modifications. Separate
   integrity, identity trust, revocation status and allowed-edit status. Unknown
   or offline validation must never display as trusted.
2. Add a signing transaction: select a certificate/key through the OS provider
   (Windows first), draw or reuse ink, preview position on rotated/cropped pages,
   show signer/reason, then explicitly sign. Private keys stay with the provider;
   password/PIN values must not enter logs or settings. A self-signed test
   certificate must remain visibly untrusted unless deliberately trusted.
3. Freeze edits, generate `/FT /Sig`, appearance and signature dictionary, reserve
   `/Contents`, finalize `/ByteRange`, hash exact byte ranges, obtain detached
   CMS and fill the reservation without shifting offsets. Save atomically and
   independently verify before reporting success. Handle oversized CMS responses
   by rebuilding and re-signing, never by truncating.
4. Support multiple signatures through incremental revisions. Honor DocMDP and
   FieldMDP restrictions; warn before edits, rewrites, page extraction or
   sanitization that would invalidate or discard signatures. Preserve original
   signed files and explain later changes per revision.
5. Target PAdES B-B first, then trusted timestamps (B-T), validation material
   (B-LT) and archival timestamps (B-LTA). Verify these claims with an independent
   validator and a cross-viewer corpus before advertising conformance.

Acceptance corpus: visible/invisible signatures, detached CMS, RSA/ECDSA,
expired/untrusted/revoked certificates, offline revocation, timestamp failure,
tampered bytes/appearance, malformed or overlapping byte ranges, unexpected
unsigned gaps, post-signing permitted/forbidden edits, two signers, encrypted
documents, cancelled key prompts, wrong PIN, disk failure and reservation overflow.

PAdES is defined by [ETSI EN 319 142-1](https://www.etsi.org/deliver/etsi_en/319100_319199/31914201/01.02.00_20/en_31914201v010200a.pdf).
Adobe's [signature API reference](https://developer.adobe.com/acrobat-pdfl-sdk/guides/jsapiref/security-signature-apis)
also distinguishes certificate trust, signature data and appearance. This roadmap
is a proposed implementation, not a current conformance or legal assurance claim.

## JavaScript isolation and blocking

Preserve today's default: document scripts do not execute. Action inspection is
useful transparency, but isolation must also cover parsing, fonts, image codecs
and rendering, which process untrusted bytes even with scripts disabled.

- Move document processing into a restricted worker process with a narrow,
  validated IPC protocol. On Windows evaluate restricted tokens/AppContainer
  and Job Objects; use OS-specific restrictions for other platforms. Background
  threads provide responsiveness but are not a security boundary.
- Give workers no general network, file-write, shell, clipboard, printing or
  certificate-key access. The UI broker owns user-authorized operations. Restrict
  handles and file access to the document and explicit resources.
- Enforce cumulative limits on decompression, images, fonts, object traversal,
  script-source extraction, memory, IPC message size and execution time. Support
  cancellation and worker restart; a deep graph bound alone does not cap total
  work or decoded bytes.
- If calculations later need scripting, use an optional runtime in a separate
  restricted process with per-document state, a minimal capability-based API,
  instruction/time/memory quotas and forced termination. Reject dynamic module
  loading, host FFI, network, process creation and unrestricted filesystem APIs.
  Do not pass signing privileges to document scripts or infer script trust from
  a valid document signature. Disable unsupported privileged APIs explicitly.
- Centralize policy for JavaScript, Launch, URI, SubmitForm, ImportData, remote
  GoTo, embedded files and XFA actions. Require deliberate broker-mediated user
  action for external effects; enumerate allowed URI schemes and block dangerous
  schemes and local/remote filesystem launches by default.
- Treat “block execution” and “remove active content” as separate operations.
  Incremental saves retain original bytes. Sanitization needs a full reachable
  graph rewrite, explicit signature impact, and inspection after reopening.

Adobe documents separate [JavaScript controls](https://www.adobe.com/devnet-docs/acrobatetk/tools/AppSec/javascript.html)
and [OS sandbox protections](https://www.adobe.com/devnet-docs/acrobatetk/tools/AppSec/sandboxprotections.html).
The architecture above is a recommendation for Atelier, not a claim that these
protections currently exist.

## XFA extensions

Start with accurate detection and safe fallback, then expand in stages:

1. Detect XFA stream and packet-array encodings even with absent/empty Fields.
   Distinguish hybrid fallback from XFA-only documents and clearly state that
   scripts, calculations and dynamic layout are unsupported.
2. Parse XML with external entities, DTD fetching, XInclude and network resource
   resolution disabled. Bound XML depth/nodes/text, packet bytes, expansions and
   layout work. Preserve unknown packets without interpreting them as authority.
3. Add read-only packet inspection and static template/dataset binding, with
   explicit conflict reporting when AcroForm and XFA values disagree. Decide and
   document how edits synchronize datasets before allowing hybrid save claims.
4. Add static XFA layout/appearance fidelity. Defer dynamic pagination/repeating
   subforms until deterministic resource and layout budgets are established.
5. Treat both JavaScript and FormCalc as active content. Route any future
   supported calculations through the same isolated capability policy. Block
   connection sets, submissions, external resources and privileged operations
   by default; do not silently flatten interactive or signed forms.

Adobe's [XFA specification](https://pdfa.org/norm-refs/XFA-3_3.pdf) describes
template/data/layout and scripting behavior; the W3C-hosted
[FormCalc specification](https://www.w3.org/1999/05/XFA/xfa-formcalc.html)
documents its separate language. Supporting XFA is more than enabling JavaScript.

Acceptance corpus should include hybrid and XFA-only forms, single stream and
packet arrays, malformed/missing packets, stale datasets, duplicate bindings,
repeating subforms, namespace variants, Unicode, external-entity attempts,
recursive XML, JavaScript/FormCalc loops and submission attempts.

## Delivery priorities

1. **Immediate quality:** resolve the XFA-only warning and hybrid-save risks;
   label ink signing accurately; repair sidebar layout; run this smoke corpus in
   CI and establish independent renderer comparisons.
2. **Reliability/security:** cumulative decode budgets, fuzz targets, worker
   isolation, crash recovery, atomic-save failure tests, and Windows GUI smoke.
3. **Signing:** validation panel, OS-backed keys, ink appearance plus PAdES B-B,
   independent verification, then timestamps and long-term validation.
4. **Forms/accessibility:** safe static XFA support and tagged-PDF reading order,
   with real-world compatibility tests before dynamic XFA or scripting.

## Limitations and reproduction

`pdftoppm` is not installed; no independent Poppler visual comparison was possible.
`tools/make_samples.py` failed because Python `fontTools` is missing. The new CLI
runner needs only Python's standard library. macOS build/test output included
SDK/xcrun environment warnings but completed successfully. Tiny synthetic files
do not establish performance or resistance to hostile large files.

Reproduce with:

```sh
cargo build -p atelier-app -p atelier-cli
cargo test --workspace
cargo fmt --all -- --check
python3 tools/qa_pdf_edges.py /tmp/atelier-qa
```

Open `/tmp/atelier-qa/valid.pdf` for the interactive smoke. Test geometry with
auto-orient disabled as well; the observed auto-orient behavior means this pass
did not independently validate the rotated UI coordinate mapping.
