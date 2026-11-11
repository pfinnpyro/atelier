# Testing protocol: changes since 1.1.0-beta.1

Scope: compare tag `v1.1.0-beta.1` with the current working tree, including
uncommitted changes. Written 2026-11-04. Basic change tracking was already in
beta 1; its session/property extensions and affected regression paths are below.
This is a protocol, not a claim that these manual checks have passed.

## 1. Prepare and record the build

Use copies of PDFs and disposable signing identities. Keep originals and beta 1
outputs in separate folders. Never overwrite the published beta 1 assets: the
workspace still reports the beta 1 version, so record the commit **and** working
diff with each result.

From the repository root, save these outputs in your test record:

```sh
git rev-parse HEAD
git status --short
git diff --stat v1.1.0-beta.1
```

Record OS/version, architecture, desktop/session (X11 or Wayland), GPU/driver,
screen scaling, OpenSSL version, package source, and test date. For each case
record **PASS / FAIL / BLOCKED / NOT RUN**, actual result, screenshots, output PDF,
and independent-reader result. A missing fixture/environment is BLOCKED, not PASS.

Build the current Windows viewer with `cargo build --release -p atelier-app -p
atelier-cli`, then launch `target/release/atelier-viewer.exe`. Existing WSL x64
binaries are under `target/wsl-linux/x86_64-unknown-linux-gnu/release/`; launch
`atelier-viewer` there from WSL. For distribution tests use the Ubuntu-built
archives in `dist/1.1.0-beta.1/linux-x64/` and `linux-arm64/`. The Debian 13 WSL
build is for local testing; it does not establish glibc 2.35 compatibility.

Use an independent reader such as Acrobat Reader for appearance/signature checks;
on Mac also use Preview. Record its version. Keep a beta 1 build available to
produce an old tracked PDF and compare unchanged rendering. Do not run old and
new builds concurrently against the same settings or PDF.

### Fixture set

| ID | File or resource needed |
| --- | --- |
| F1 | Ordinary text PDF, at least three pages, with searchable text and images |
| F2 | AcroForm: text, multiline text, combo/list, checkbox, read-only field, and one field with multiple widgets; existing font resources |
| F3 | Rotated/cropped pages, CJK text and unembedded fonts |
| F4 | PDF with third-party comments and ink, including a custom appearance or non-RGB color |
| F5 | PDF saved with beta 1 tracking/history, plus an untracked copy |
| F6 | Scripted calculation/validation form; hybrid/static/dynamic XFA examples if available |
| F7 | Unsigned PDF and separately an already-encrypted PDF |
| F8 | Approval-signed PDF, multiply signed PDF, and certified PDFs declaring DocMDP P=1, P=2, P=3 and FieldMDP locks |
| F9 | Signed files with later form, annotation, page and metadata revisions; a changed-then-reverted object; shared/inherited resources and qualified/nested fields |
| F10 | Malformed ByteRange, unsigned trailing bytes, broken/cyclic revision chain, and replaced/removed historical signature dictionaries |
| F11 | Test certificate/key, encrypted PEM key, passworded and passwordless P12/PFX, unrelated key, CA chain, unrelated CA, expired/not-yet-valid signer |
| F12 | Disposable Windows CurrentUser/Personal identity, physical printer, native Linux ARM64 desktop, and native Mac as applicable |

F8–F10 often need a PDF signing tool or a developer-created fixture. Ordinary
page edits do not reliably create the required historical-object cases. Use the
automated syntax tests in section 3 for exact synthetic edge cases; those tests
do not replace UI/interoperability checks. Do not edit binary signature bytes in
a text editor to create a supposedly valid fixture.

### Make a basic disposable OpenSSL identity

In a scratch folder, with OpenSSL on PATH (commands work in PowerShell or Linux):

```sh
openssl req -x509 -newkey rsa:2048 -keyout qa-key.pem -out qa-cert.pem -sha256 -days 7 -noenc -subj "/CN=Atelier QA disposable"
openssl pkcs12 -export -out qa-identity.p12 -inkey qa-key.pem -in qa-cert.pem
openssl pkey -in qa-key.pem -aes-256-cbc -out qa-key-encrypted.pem
```

The last two commands prompt for test passwords. Repeat P12 export with a second
filename and empty export password to make the passwordless case. This
self-signed identity is suitable for integrity and untrusted-chain tests; it
does not provide a real CA chain, expired certificate, or Windows store identity.
Do not install it into your normal trusted-root store. Select `qa-cert.pem` as a
per-document CA bundle only when testing explicit trust of this disposable cert.

## 2. First manual pass (about 45–60 minutes, excluding builds)

Run UI-01, HIST-01/02, FORM-01, ANN-01/02, INK-01/02, ENC-01, SIGN-01,
VERIFY-01/02 and POLICY-01 first. On Linux add LINUX-01/02. Save and reopen every
output. Then execute the complete matrix below before release acceptance.

## 3. Automated regression gate

Run on each native platform being released, with build prerequisites installed:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python -m unittest discover -s tools -p "test_*.py"
cargo build --release -p atelier-app -p atelier-cli
```

On Windows:

```powershell
python tools/qa_pdf_edges.py target/qa-current --binary target/release/atelier.exe
```

On Linux (use the actual target directory for a cross/WSL build):

```sh
python3 tools/qa_pdf_edges.py target/qa-current --binary target/release/atelier
```

Expected: zero command failures and **22/22** PDF checks. Read test output for
skipped/environment-dependent cases; an unavailable OpenSSL/native provider is
not evidence that its integration passed. Linux installer tests skipped on
Windows must pass on Linux. Targeted diagnosis commands:

```sh
cargo test -p atelier-form
cargo test -p atelier-syntax --test write
cargo test -p atelier-app
```

The syntax suite covers historical signatures, malformed policy, signed-write
restrictions, per-revision changes, object streams and reference contexts. Form
tests cover session grouping, property history and appearance restoration.

## 4. UI, navigation and settings

| Case | Steps | Expected result |
| --- | --- | --- |
| UI-01 | Open F1; middle-drag on a page; release; select text normally; scroll and zoom afterward. | Panning follows drag and stops on release; no accidental selection/edit; ordinary navigation still works. |
| UI-02 | Exercise each split button: main action, arrow/menu, outside click, Escape. Include Save and Mark. | Correct action/menu; dismissing does not execute an action; no dead or overlapping hit areas. |
| UI-03 | Settings: change individual toolbar buttons to icon/text/both; use Set all; restart. | All selected modes persist; menus/tooltips remain usable. |
| UI-04 | Light/dark themes; narrow and short windows; 100%, 150%, 200% scaling; open Settings, comments and security dialogs. | Controls and scroll areas reachable; no clipped confirmation buttons or overlapping toolbar rows. |
| UI-05 | Windows: drag/maximize/restore/resize; double-click title area; test keyboard navigation and mixed-DPI monitors. Repeat with `ATELIER_NATIVE_TITLEBAR=1`. Mac/Linux: native controls/full-screen. | Window actions remain usable; record snap/system-menu/accessibility differences rather than assuming native parity. |
| UI-06 | Open two PDFs; switch rapidly during edits, verification and background exports; close each and the whole window. | Correct document keeps its state/results; unsaved prompts refer to the right tab; no background result appears in another PDF. |

## 5. Editing sessions and field formatting

| Case | Steps | Expected result |
| --- | --- | --- |
| HIST-01 | F2: Changes > enable tracking > Start editing session. Change one text field A→B→C, another field, and one appearance property. Finish editing session. | One grouped revision with initial/latest values, author/time and distinct targets; no intermediate B presented as the initial value. |
| HIST-02 | Undo the session, redo, Reject; separately make and Accept another session. Save/reopen after decisions. | Whole session restores/reapplies consistently; Accept retains content, Reject restores before-state; decisions/history survive reopen. |
| HIST-03 | Create two dependent sessions; attempt Reject/restore of the older one first. Resolve newest first. Also introduce an untracked edit before restoration. | Unsafe older restoration is blocked; no unrelated newer/untracked work silently disappears. |
| HIST-04 | Undo then make a different edit; finish/save/reopen; inspect prior revision. | New branch preserves historical evidence; reopening/reviewing does not merge subsequent edits into the old session. |
| HIST-05 | Open F5 from beta 1, inspect/review history, add a new session, save as a new file and reopen. | Old current content/history readable; new session separate; no migration data loss. |
| FORM-01 | F2: F12 > Fields > Format [name]. Select an existing font; size 12; color; left/center/right alignment. Save/reopen in Atelier and independent reader. | Matching text appearance/value and alignment; properties persist. Only available form font resources are offered. |
| FORM-02 | Multiline field: character spacing 1, word spacing 3, line spacing 18; then size 0 and line spacing 0. Use long text. | Spacing affects layout/wrapping; size 0 fits, leading 0 uses automatic spacing. Fixed sizes may clip within the field. Multibyte CID spaces need not behave like code-32 word spaces. |
| FORM-03 | Change widget X/Y/W/H; test multiple widgets, rotated/cropped F3; undo/redo and save/reopen. | Correct widget moves/resizes; coordinates use unrotated PDF bottom-left origin; appearance and hit target agree. |
| FORM-04 | Try negative/zero width/height, invalid or extreme input, and formatting a read-only field. | Invalid geometry rejected/clamped without corrupting state; read-only fields excluded; no crash or non-finite saved values. |
| FORM-05 | Track repeated property changes in one session; inspect targets, then undo/reject. | Initial/latest property values retained; restoration includes appearance and geometry, not only field value. |

## 6. Comments, ink and saved signatures

| Case | Steps | Expected result |
| --- | --- | --- |
| ANN-01 | Add pinned and selected-text comments. Edit author, subject, text; switch cards; cancel an edit; save/reopen and view independently. | Correct comment/card targeted; cancel keeps prior content; metadata and visible content persist; creation/modification timestamps sensible. |
| ANN-02 | F4: edit/remove saved comment or ink; undo, save/reopen. Also try new highlights, underlines and strikeouts. | Correct annotation changed; supported standard appearances visible independently; nearby annotations untouched. Record custom appearance/non-RGB loss when ink is replaced as a current preservation limitation. |
| INK-01 | Draw several strokes; use whole-stroke eraser, erase-last and clear; change color/width; Enter to commit. Undo; save/reopen. | Intended strokes removed/committed; color/width applied; saved ink renders independently. Eraser is whole-stroke, not partial-stroke. |
| INK-02 | Leave ink pending without Enter; Save, Save As, switch tabs, close tab/window. Repeat Cancel/Don't save/Save choices. | Saving includes pending ink; close warns; Cancel preserves work; no silent loss or ink placed into the other document. |
| INK-03 | Right-click saved ink > Edit drawing and Redraw; modify, cancel and commit in separate runs. | Correct drawing loaded/replaced; cancellation preserves original; unrelated marks survive. |
| INK-04 | Settings > Ink signatures: draw, preview, save named copies, select/use, replace and delete; restart. | Named copies persist independently; deleting active drawing leaves named library copies available; chosen drawing is placed correctly. |
| INK-05 | Enable certificate-signing offer after placement; place ink; cancel offer, then repeat and sign. | Declining retains ink; accepting opens certificate flow and exported CMS covers the revision containing ink. Ink alone never appears as authenticated identity. |

## 7. Encryption export

| Case | Steps | Expected result |
| --- | --- | --- |
| ENC-01 | F7 unsigned: Save dropdown > Encrypt a copy; matching ASCII password; new output path. Open output in Atelier and independent reader. | Password required, correct password opens, wrong password fails; source/path unchanged. Fields, marks and pages preserved. |
| ENC-02 | Try mismatch, empty input, non-ASCII, 127 printable ASCII bytes and 128 bytes. | Empty/mismatched/unsupported passwords cannot export; nonempty valid boundary accepted; no unusable successful export. |
| ENC-03 | Export same content twice with same password; compare hashes. | Different encrypted bytes due to fresh randomness; both copies open and render equivalently. |
| ENC-04 | Try existing encrypted input, signature-field/signed input, source output path, cancel dialog, unwritable destination. | Unsupported operations/path conflicts fail clearly; no partial successful file; original remains intact; UI recovers. |

Current scope is one open/owner password with unrestricted permissions. Separate
permissions, changing/removing existing encryption and Unicode password support
are not acceptance requirements for this implementation.

## 8. Certificate signing and verification

| Case | Steps | Expected result |
| --- | --- | --- |
| SIGN-01 | F1 with edited field/comment/placed ink: Save > Sign with certificate > PEM; select test cert/key; export to new file. | Source remains open/unchanged on disk; exported current content included; Atelier integrity valid; independent validator recognizes signature (self-signed trust warning is expected). |
| SIGN-02 | Repeat with encrypted PEM, passworded P12/PFX, passwordless P12, then a P12 containing signer and intermediates. | Correct password succeeds; passwordless works; included P12 CA chain is embedded; identity choice unambiguous. |
| SIGN-03 | Wrong password; unrelated key/cert; missing file; malformed/cert-only/multiple-identity P12; missing OpenSSL; encrypted PDF; output equal to source or credential path; unwritable folder; cancel. | Clear failure, no false signing success, no overwritten source/credential; retry with valid inputs works. UI stays responsive. |
| SIGN-04 | Windows store mode: refresh CurrentUser/Personal; inspect subject/issuer/expiry/thumbprint; select disposable identity and sign. Cancel provider authorization if offered. | Correct identity used; failure/cancellation recoverable. Native signing can work without OpenSSL; verification still requires it. No private key extraction. |
| SIGN-05 | Repeat with real non-exportable/provider-backed RSA/ECDSA keys if supported by available hardware. | Record actual provider/algorithm and prompts; no token compatibility claim from an ephemeral-key test. Missing hardware is BLOCKED. |
| SIGN-06 | Enter password, change mode/credentials, close/reopen dialog; inspect settings and process arguments after export. | Password not persisted/repopulated or present in command arguments. This does not prove memory zeroization; file signing uses child-process environment passwords. |
| VERIFY-01 | Open signed output > sidebar Signatures; inspect certificates/fingerprints/coverage; Verify again. | Integrity, chain trust excluding dates, chain with dates and current signer dates reported separately; correct signer identity; no overall trusted-document badge. |
| VERIFY-02 | Self-signed file: default store, selected correct CA bundle, unrelated CA, Use default CA store. Repeat in two tabs with different bundles. | Integrity unchanged; chain result follows selected trust; per-document choice/results do not leak across tabs. Embedded cert alone does not establish trust. |
| VERIFY-03 | Expired/not-yet-valid signer and missing intermediate; verify against appropriate and unrelated CA bundles. | Integrity may remain valid while dates/chain fail separately; revocation and timestamps remain explicitly unchecked. |
| VERIFY-04 | Modify a byte of signed content in a disposable copy while preserving parseability; compare original. | Tampered signature fails integrity or reports a parsing error, never valid. Original still verifies. |
| VERIFY-05 | Verify a large/multiply signed file; switch/close/reopen tabs mid-job; retry; test unavailable OpenSSL. | UI responsive; stale results discarded; useful error/retry; reports belong to the correct file generation. |

## 9. Historical signatures, later revisions and write policy

| Case | Steps | Expected result |
| --- | --- | --- |
| REV-01 | F8/F10: inspect multiple signatures, including dictionaries replaced/removed in later incremental updates. | Historical indirect signatures remain listed; unchanged object versions not duplicated; errors visible per candidate/history. Direct dictionaries are not fully supported for enumeration. |
| REV-02 | F9: inspect form/appearance/page/metadata additions, modifications, removals and reverted edits across multiple revisions. | Each later revision remains visible, including reverted steps; changed keys/object categories and coverage endpoints plausible. Earlier integrity does not certify later edits. |
| REV-03 | Shared resources, inherited page resources, nested qualified fields; move/detach object between revisions. | Before/after reference contexts show relevant pages/fields/referrers; ambiguous/unresolved ownership reported conservatively. Reachability is not proof of exclusive ownership or visual effect. |
| REV-04 | F10 malformed/overlapping ranges, trailing bytes, broken/cyclic history, object streams and analysis limits. | Error/unknown/incomplete state visible; no crash, hang or false complete analysis. Automated tests provide precise limit/object-stream fixtures. |
| POLICY-01 | Certified P=1/2/3 and FieldMDP files: attempt field edit, annotation/ink, rotation/delete, page extraction, encryption and save. | Restricted writes blocked with explanation, including P=2/3 and nominally unlocked fields; selective permitted-edit classification is not implemented. Source unchanged; pending work not silently consumed by failed Save. |
| POLICY-02 | Approval signature without MDP: append a supported field/comment edit; separately try deletion/full rewrite, extraction and encryption. | Supported incremental append may succeed, preserving earlier signature integrity with later-byte warning; signature-destroying rewrite/extraction/encryption blocked. |
| POLICY-03 | Unchanged signed file: Save a copy; compare source/output SHA-256. Test removed historical restriction and unknown/malformed transform separately. | Unchanged byte copy allowed and identical; historical/malformed restrictions cannot be bypassed by removing current declarations. |

These are conservative policy checks. Structural change/reference reports do not
establish DocMDP compliance, document trust, revocation status or a PAdES level.

## 10. Default application integration

Before changing defaults, record the previous PDF app and restore it afterward.

| Case | Steps | Expected result |
| --- | --- | --- |
| DEFAULT-01 | Windows Settings > Make Atelier the default PDF app; complete selection in Windows Settings; double-click PDF. Cancel in another run. | OS default-app controls open for PDF; app does not silently overwrite user choice; completed choice opens correct file in Atelier. |
| DEFAULT-02 | Mac: open PDF default-app controls, choose Atelier in Get Info > Open with > Change All; repeat with no current document and cancel file picker. | Correct document/control guidance; cancellation harmless; Finder opens PDFs after completed choice. Requires a Mac. |
| DEFAULT-03 | Linux: install managed package; use Settings default action; query `xdg-mime query default application/pdf`; double-click PDF with spaces in path. | Correct desktop id and stable current path/icon; file passed intact; verified success or explicit desktop-tool failure. |

## 11. Linux packages, desktop operation and lifecycle

Run on native x64 **and native ARM64**, Ubuntu 22.04 baseline and at least one
newer desktop. Repeat desktop interaction on GNOME/KDE with X11/Wayland where
available. WSLg and Xvfb are useful smoke environments; neither replaces this matrix.

Build both release packages from Windows/any Docker host:

```sh
python tools/package_linux.py both
```

RustRover: run **Package Linux x64 and ARM** on this Windows host. Individual
Linux shell configurations require Linux/WSL and the appropriate toolchain.

On the matching Linux architecture, copy/extract the package into a scratch folder:

```sh
sha256sum -c SHA256SUMS.txt
tar -xzf atelier-linux-x64.tar.gz
cd atelier-linux-x64
python3 install.py
~/.local/share/atelier/app/current/atelier-viewer
```

Substitute `arm64` on ARM64. Paths below assume default XDG locations; use your
actual `$XDG_DATA_HOME` if set. No sudo is needed for per-user installation.

| Case | Steps | Expected result |
| --- | --- | --- |
| LINUX-01 | Verify manifest; inspect archive; run portable viewer and CLI; install. | Matching architecture ELF binaries; seven expected package files; glibc 2.35+; no missing library/startup errors; portable PDF opens. |
| LINUX-02 | Launch from application menu; open via dialog, recent list, drag/drop and desktop double-click; test Unicode/spaces in paths. | Icon/app identity correct; file opens without shell quoting problems; installing alone does not change PDF default. |
| LINUX-03 | Clipboard both directions, text selection, CJK/unembedded fonts, scrolling/zoom, multi-monitor DPI, file chooser portals, GPU and software rendering. | Correct text/layout and interactions; no blank window, dialog hang or severe corruption. Record renderer/session/driver. |
| LINUX-04 | Print F1 and rotated/cropped pages to physical printer; cancel; unavailable printer; save-to-PDF if offered. | Correct pages/orientation/scale; cancellation/failure recoverable. Check paper output, not just job submission. |
| LINUX-05 | Run file-based signing/verification, encryption, edits/save/reopen and OCR on native ARM64 as well as x64. | Functional parity; missing optional tools/models reported usefully; no architecture-only crashes. Native store signing is Windows-specific. |
| LINUX-06 | Install again; custom root with spaces; attempt wrong-arch package and unmanaged/nonempty root; concurrent installer operations. | Safe repeat behavior; valid custom path works; wrong arch/unmanaged files protected; installation lock serializes operations or reports failure. |
| LINUX-07 | After two distinct installed versions, run rollback command below and relaunch menu app/CLI. | Previous complete viewer/CLI/version selected through current link; integration follows selected version; PDFs/settings/models preserved. One-version installation cannot prove rollback. |
| LINUX-08 | Uninstall; inspect desktop entry/icon/metadata/default and user data; reinstall. Separately modify an integration file or put unexpected data in managed release folder before uninstall. | Only managed/unmodified files removed; original default restored when known; user data retained; unexpected files protected and reported; reinstall works. |

```sh
python3 ~/.local/share/atelier/app/current/install.py --rollback
python3 ~/.local/share/atelier/app/current/install.py --uninstall
```

Custom root: pass the same `--root /absolute/path` to install and lifecycle calls.
For automated clean-runtime smoke use the exact Docker commands in
[release operations](releases.md#runtime-dependencies-and-validation). ARM64
cross-compilation alone is not a runtime pass.

## 12. Updates and release acceptance

Authenticated Linux update requires a **new reviewed, published attested release**,
current GitHub CLI, Python 3 and an older managed installation. Local archives and
the existing beta 1 release cannot prove that end-to-end path. Use a separate
test account/installation; do not republish/overwrite beta 1 or weaken verification.

| Case | Steps | Expected result |
| --- | --- | --- |
| UPDATE-01 | Check Stable/Preview settings and newer-release discovery; restart; test offline/rate-limited/malformed response and release missing matching Linux asset/manifest. | Channel persists; no unwanted downgrade or editing interruption; incomplete Linux releases ignored; failures recoverable. |
| UPDATE-02 | Managed Linux: update to the new attested version from UI, or command below; relaunch; inspect viewer and CLI; rollback. | Matching architecture, SHA-256 and exact release-workflow/tag attestation verified; whole package activated atomically; previous version available. |
| UPDATE-03 | Simulate failed download/checksum/attestation, missing/old gh, offline verifier, unwritable destination, concurrent operations in disposable test environment. | No activation on failed verification; current working version preserved; useful error and successful retry. Installer regression tests cover synthetic failures. |
| UPDATE-04 | Attempt install/restart with dirty active tab, dirty inactive tab, live uncommitted field editor, pending ink and pending OCR layers, one at a time. | Unsafe install/restart blocked or requires work to be saved; no silent loss in any tab. Retry after saving succeeds. |
| UPDATE-05 | Portable Linux/source and macOS: inspect update action. Windows: test installed and portable paths from older build using a real newer test release. | Linux portable/Mac use manual download; Windows chooses setup vs viewer appropriately; checksum failure prevents install; documents preserved. Portable Windows update currently replaces viewer only. |
| RELEASE-01 | Run native CI/package jobs on all four targets; download artifacts; verify combined manifest; repeat launch/save/sign/print smoke from packaged copy. | Actual jobs pass; complete expected assets/checksums; archive provenance present for new Linux tags. Configured workflow alone is not evidence. |

```sh
python3 ~/.local/share/atelier/app/current/install.py --update VERSION
```

Replace VERSION with the real newer version without `v`. Tag/publish operations
are release-owner steps after review, not part of running this local protocol.
macOS ad-hoc signing and unsigned Windows distribution remain existing limitations.

## 13. Regression pass and decision

For each platform: open ordinary/damaged/large PDFs; search/copy; rotate/reorder/
insert/split/export unsigned pages; fill forms; approve a known form script and
verify calculation; reopen to confirm script approval resets; inspect XFA fallback
warnings; OCR a scan; print; save/reopen annotations and tracking. Compare original
rendering with beta 1 and an independent reader. Test cancellation and a read-only
destination. General page text editing, semantic merge, secure redaction, dynamic
XFA execution, trusted timestamps/revocation and Mac Keychain are not implemented.

Block release acceptance for data loss, corrupted/unreadable output, misleading
signature validity/trust, bypassed signed-write restrictions, update activation
after failed authentication, or a supported package failing to launch. Record
all other defects with reproduction files and severity. Missing native ARM64/Mac,
desktop, printer or published-update evidence remains an explicit open gate.

Suggested result row (one per case/platform):

```text
Case | Build+diff | OS/arch/session | Fixture | Result | Actual vs expected | Evidence | Issue
```

Existing local evidence is in [Linux acceptance](linux-acceptance.md); it covers
builds, selected automated suites and x64 clean-runtime smoke, not this full
manual protocol. Store/AppStream distribution also awaits the owner's project
license choice.
