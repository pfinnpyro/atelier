# Review workspace and page tools

## Implemented

- Page context menu: Highlight, Underline, Strike out, Add comment here, and
  review tools. Text markup requires a selection; comments use the clicked PDF
  coordinate. Saved underline/strikeout annotations have standard PDF subtypes,
  QuadPoints and their own rendered appearances.
- Comments: click existing saved notes or inline comment highlights to open and
  edit author, subject and note text. Creation time is preserved, modification
  time updates on edit, and multi-line highlights use their individual quads for
  hit testing. Selected text becomes the anchor when adding a comment.
  Editable author identity, creation timestamp in UTC, standard `/T`,
  `/CreationDate` and `/M` metadata, and author/time shown with hover text. Author
  identity is saved in settings and can be changed there or in the comment editor.
  Authorship is descriptive metadata, not authenticated identity.
- Vertically centered toolbar with the main menu before the navigation toggle,
  no duplicate document title or unsaved indicator, and integrated close targets
  inside each tab. Tab dirty indicators remain visible.
- Open, Save, Fit width, Fit page, Side by side and Organize each have a saved
  Icon only / Icon and text / Text only preference in Settings → Toolbar buttons.
  Icons are drawn as vectors and accessible names remain available in every mode.
- Connected Open/Recent and Save/Options split controls with fixed hover geometry,
  shared rounding/spacing,
  theme-derived chrome, and persistent System/Light/Dark settings. Document pages
  and the ink pad remain white surfaces.
- Zoom percentage accepts typed values from 10% to 800%; Enter applies the
  value. Invalid/nonfinite values are rejected with a notice.
- Navigation-only sidebar with separate page/outline tabs and thumbnail spreads.
  Find (Ctrl/Cmd+F or the main menu) reveals and focuses the search field, including
  when the sidebar was closed. Search includes previous/next and close controls;
  Escape closes it while focused. Page operations live in Organize.
- Insert PDF appends or prepends another PDF, or inserts before the first or after
  the last selected page, through an explicit source/position dialog. Selection
  positions are disabled until pages are selected. Split writes
  two PDFs at a chosen page boundary. The toolbar's Organize toggle opens a
  virtualized thumbnail grid with top-left checkboxes, group drag/drop, range
  selection, move-to-start/end, rotate and export. Thumbnail dragging remains
  available in the left sidebar.
  Selected-page export is available in Organize. Splitting does not alter the source
  document and refuses to overwrite existing part files. Extraction retains page
  appearances but omits document-level forms/bookmarks, as stated in the dialog.
- Main-view Side by side toggle lays pages out in pairs, including mixed page
  sizes and rotations. This is separate from sidebar thumbnail spreads. A matching
  right-side toolbar toggle opens and closes the inspector.
- Clear “Ink signature” wording, avoiding a claim of certificate-backed signing.
- First follow-up compatibility fix: XFA presence is detected independently of
  AcroForm fields, with persistent compatibility messaging and an inspector
  explanation for XFA-only forms. Hybrid edits explicitly warn that XFA data is
  not synchronized. XFA scripts remain inactive.

## Platform window behavior

| Platform | Behavior |
| --- | --- |
| macOS | Content extends into a transparent native titlebar; native traffic lights and resize behavior stay available, with a reserved control area and draggable toolbar space |
| Windows | Custom titlebar with minimize, maximize/restore, close, double-click maximize, drag region, and eight edge/corner resize regions using native viewport commands |
| Linux | Native compositor decorations and window controls, with the same themed application toolbar; avoids assuming client decoration support across X11/Wayland |

Set `ATELIER_NATIVE_TITLEBAR=1` before launching to use standard native decorations
on Windows/macOS. Close requests still pass through the existing unsaved-tab
confirmation flow. Custom Windows controls do not currently reproduce the full
native Windows snap-layout hover UI or system menu. Windows and Linux runtime
behavior requires testing on those systems; these were not available locally.
CI now covers all three operating systems for build, lint, tests and the PDF
repair/render smoke corpus. CI results have not yet run for this working tree.

## Verified locally

- Workspace suite: 319 tests pass after the review/XFA changes; focused app/form
  tests pass again after theme persistence was added.
- Workspace clippy and formatting pass.
- Native macOS smoke: rendered pages/thumbnails; connected controls and sidebar
  layout; 125% custom zoom; rearrange `2,1`; split at page one; inspect both saved
  parts through CLI text extraction; context-menu comment creation with visible
  author/time; save; merge a part back to produce three pages.
- Regression tests cover PDF comment metadata, rendered Underline/StrikeOut
  appearances, zoom validation, order validation, XFA-only presence and the
  earlier PDF edge cases. Cross-platform CI is configured but not locally run.

## Next phase

1. Add a review list with comment navigation, replies, resolved status, filters
   and stable modification history. Creation, editing of saved and unsaved comments,
   inline highlight anchors and modification timestamps are now implemented.
2. Add document-wide undo for page operations and stronger form/bookmark
   preservation in split/merge workflows; test failure recovery and large files.
3. Validate Windows custom chrome with keyboard/system-menu/snap behavior and
   mixed DPI, and exercise macOS full screen plus Linux X11/Wayland. Add automated
   narrow-window, keyboard-only and screen-reader UI checks.
4. Introduce a certificate validation panel before real signing, followed by
   OS-held signing keys, ink appearances on signature widgets and independently
   verified PAdES output. Ink annotations currently carry no cryptographic proof.
5. Add bounded inspection reports and process-isolated PDF workers, then static
   XFA packet/data support. Do not enable document JavaScript or FormCalc as a
   shortcut to XFA compatibility.

The broader rationale and acceptance corpus remain in `QA_EVALUATION.md`.

## Follow-up verification

Saved comments open on click in the native macOS UI, and their edits mark the
document dirty. Paired pages render in the main view. The grid displays accessible
page checkboxes and selection controls. A headless egui pointer-event regression
exercises press, drag, drop and checked-group selection preservation, including
verification of the resulting PDF page order. Additional engine tests cover
inline comment appearances/metadata after reopening, saved review edits following
page reordering, unsaved note edits, mixed-height facing rows and right-page
coordinate transforms. The native automation drag gesture did not produce a
reorder, so drag/drop is validated by the egui pointer-event test, not a successful
native automation gesture.

## Toolbar and organization refinement

Automated coverage now checks split-button bounds and vertical centers before and
after hovering the primary and dropdown segments in all three display modes, plus
settings round trips with different per-button modes and insertion positions at
document/selection boundaries. Native macOS checks confirmed the menu's left
position, aligned toolbar, integrated tab close control, initially hidden search,
Find shortcut revealing/focusing search, search dismissal and page operations in
Organize. Desktop interaction was stopped when requested; further verification
uses source review, headless tests, build and lint only.
