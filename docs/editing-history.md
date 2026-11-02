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

## Next format and merge work

The intended native artifact should contain a rendered PDF plus a versioned
editing model and revision tree. Each session needs an immutable ID, parent IDs,
author, timestamps and changes keyed by stable element and property identity.
Text, font, size, spacing, bounds and other appearance properties must be tracked
as typed values, rather than inferred from snapshot byte differences.

A three-way merge should compare both descendants with their common ancestor.
Independent property changes can combine automatically. Conflicting properties
need a preview and explicit left, right or manually combined value. A merge must
record both parents and retain rejected alternatives in history. Current snapshot
restoration deliberately blocks changes with dependencies; it is not this merge.

Native Save should retain the complete tree. Export PDF should write the current
resolved appearance without the private history and explain that reopening the
export cannot recover it. Such export must compact unreachable objects so old
snapshots cannot remain in appended PDF sections. Saving an export should leave
the native working artifact and its history intact. The native container and this
export workflow are not implemented yet.

## Proposed implementation sequence

1. Introduce a version-2 semantic model with persistent element UUIDs. Import
   existing fields as elements; map their widgets to separate geometry elements.
   Map future editable page text into text blocks/runs rather than PDF operators.
   Keep UUID-to-PDF-object bindings separate so compaction does not change identity.
2. Store sessions as immutable nodes with `id`, `parents`, `author`, `started`,
   `finished`, `label`, and `changes`. Each change contains `element`, `property`,
   typed `before`/`after`, and units. Use enums for alignment, point values for
   geometry/spacing, font asset IDs and tagged gray/RGB/CMYK colors. Record review
   decisions as events; never rewrite a finished node. Version-1 histories remain
   readable as opaque snapshot revisions rather than guessing their semantics.
3. Use a native `.atelier` ZIP container with `manifest.json` (schema and head),
   `document.pdf` (current appearance), `model.json`, `history/` and deduplicated
   `assets/`. Hash referenced assets and validate limits and paths when loading.
   Save atomically; a failed write must leave the previous artifact intact.
4. Add a session tree and three-column resolution view: left, ancestor, right.
   For each property, choose the changed side when the other equals the ancestor;
   identical changes agree automatically. Concurrent changes to different
   properties combine, subject to element validation. Conflicts offer Left,
   Right, or a custom combined value with a rendered preview. Deletion-versus-edit,
   text-range overlap and incompatible geometry require explicit decisions.
5. Commit a resolution as a new node with both parents. Retain both alternatives
   and the decisions so reopening can explain the merge. Require all conflicts
   to be resolved and validate the resulting layout before committing.
6. Add **Save Atelier Artifact** and **Export PDF**. When saving an edited imported
   PDF, offer the native artifact to retain reliable history. A compatibility
   export explains that it omits history; replacing an existing PDF requires the
   normal overwrite confirmation. Export must never mark the native artifact as
   saved or change its path. Test absence of history bytes, reopening in the PDF
   parser, and rendering equivalence with the resolved native document.

Acceptance coverage should include branches touching independent properties,
same-property conflicts, deletion/edit conflicts, text edits with simultaneous
formatting, reopened merge nodes, font assets, damaged history, encrypted input,
and export without recoverable snapshots. The current snapshot dependency guard
remains in force until semantic merge can preserve unrelated edits reliably.
