# Historical QA evidence

This file preserves past observations; it is not an implementation plan or a current
capability assessment. All remaining work and current status live in the
[roadmap](docs/roadmap.md). The active host is Windows and has no macOS environment.
Earlier macOS results below must not be presented as verification of the current tree.

Evaluated 2026-11-01 on macOS, Rust 1.99.1, using the working tree at that time.
Existing application, renderer, README and IDE edits were preserved. This change
adds tests, a dependency-free smoke runner and this assessment; it does not add
certificate signing, a JavaScript runtime, an OS sandbox or XFA rendering.

## Result

Atelier has a substantial viewer/editor foundation. The tested paths are stable,
including repair, form persistence and inactive action inspection. It is not yet
validated as a high-fidelity, accessible, security-hardened document workstation.
That assessment motivated the compatibility/security work now consolidated in the
[roadmap](docs/roadmap.md).

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

## Status of the original findings

| Original finding | Current disposition |
| --- | --- |
| XFA-only warning missing | Document-level detection, classification and fallback messaging were subsequently implemented |
| Hybrid XFA edits can diverge | Explicit save warnings exist; dataset synchronization remains unsupported |
| Ink called signing without cryptographic assurance | UI now distinguishes ink from CMS; file and Windows-store signing plus integrity/chain/date checks exist |
| Sidebar controls cramped | Separate/wrapped navigation controls were implemented; narrow-window/DPI/accessibility validation remains open |
| Action inspection limits incomplete | Bounded inspection exists; cumulative budgets, visited tracking and visible incomplete reports remain roadmap work |

The older capability table and duplicate signature/security/XFA delivery sequences have
been merged into the roadmap. The evidence above retains its original scope; it does
not prove current OCR/printing, hostile-input resistance, release/update, accessibility
or certificate interoperability.

## Later historical UI and review reports

Prior review records reported 319 passing workspace tests and workspace lint/format
checks after the review/XFA changes. These are historical counts, not a new run.
Reported Mac smoke coverage included rendered pages/thumbnails, connected controls,
sidebar/Find behavior, typed 125% zoom, rearrange `2,1`, split/CLI extraction, saved
comments with author/time, merge, toolbar alignment, integrated tab close, Settings
separators/scrolling and Set all. A later native titlebar pass superseded the earlier
transparent-titlebar/control-reservation implementation; macOS currently uses native
decorations. Windows and Linux were unavailable for that earlier UI review, not for
all subsequent development.

Native automation did not successfully demonstrate grid drag/drop. A headless egui
pointer-event regression covered press/drag/drop, checked-group preservation and the
resulting PDF order. Other regression coverage included comment metadata/appearances,
saved/unsaved edits, reordered-page review, mixed-height facing rows, right-page
transforms, split-control hover geometry across display modes, settings round trips,
insertion boundaries, safety/script-index refresh and dynamic-XFA organization gates.
These checks do not replace native platform interaction testing.

Branch integration retained XFA fallback, attachment/action inspection and the
approved-script helper. New inserted script source is not automatically approved.
Review edits and script-driven visibility changes retain clean saved state. Personal
IDE workspace changes were excluded, with the IDE version backed up locally.

The completed 1.1.0-beta.1 review pass added the persistent revision journal, Changes
tab, tracked form/script/visibility/page/annotation/comment changes, snapshots,
authorship/time, navigation, dependency-safe decisions and persisted undo/redo.
Standard comment review states are distinct from document revision decisions. Current
storage behavior is documented in [editing history](docs/editing-history.md) and the
[release snapshot](docs/release-notes-1.1.0-beta.1.md).

## Historical limitations and reproduction

At the time, `pdftoppm` was not installed; no independent Poppler visual comparison was possible.
`tools/make_samples.py` failed because Python `fontTools` was missing. The new CLI
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
