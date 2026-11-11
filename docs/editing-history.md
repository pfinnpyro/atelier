# Editing and history

The Changes pane supports explicit editing sessions. Enable tracking, choose
**Start editing session**, make edits, then choose **Finish editing session**.
Changes to the same target retain their initial value and latest value. A session
is one revision for accept, reject, undo and redo. Reopening starts a fresh group;
reviewing a revision also prevents subsequent edits from joining that revision.

History currently uses full document snapshots in the private PDF catalog entry
`AtelierRevisions`. This preserves the exact content and appearance captured by
supported edits. Other readers can display the PDF but do not understand Atelier
history, and may discard it when saving. Snapshots do not provide a semantic diff
of arbitrary page content. Current editing tools cover forms, annotations and
page organization; general page text/style editing remains future work.

## Implemented property tracking

Open **Inspector > Fields**, then expand **Format [field name]**. Text and choice
fields support font resource, font size, text color, alignment, character spacing,
word spacing and line spacing. Each widget also exposes X, Y, width and height.
Coordinates and spacing are PDF points; X/Y use the unrotated bottom-left page
origin. Font size zero fits text automatically; line spacing zero uses 1.15 times
the font size. Font choices come from resources already present in the form.
Read-only fields are excluded from these controls.

The typed `FieldStyle` API validates changes before mutating the session.
Each changed property receives its own history target: for example `font-size`,
`character-spacing`, or `widget-0 / width`. Repeated edits inside an explicit
session retain the initial and latest property values. PDF `/DA`, `/Q`, `/Rect`
and generated `/AP` streams carry the visible result. Wrapping, alignment and
automatic sizing account for spacing. Word spacing follows PDF character-code
32 semantics; it does not expand multibyte CID spaces. Explicit line spacing is
the baseline distance, not a multiplier. Fixed sizes can clip within widget bounds.

History still serializes property values as strings in the existing version-1
journal and uses snapshots to restore whole sessions. Property-level restoration
and automatic combination of independent revisions are not implemented. Other
editors may regenerate appearances differently or discard private history.

## Planned extensions

The semantic history model, native `.atelier` container, three-way merge and history-free
PDF export are planned, not implemented. Their dependency order and acceptance criteria
live only in the [roadmap](roadmap.md#2-annotation-model-history-and-semantic-merge).
The existing snapshot dependency guard remains in force.
