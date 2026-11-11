# Atelier 1.1.0-beta.1

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease adds document change tracking and a dedicated review pane.

- Toggle the right-hand review pane from the SVG comment button. It sits beside the document with a small margin.
- Click a page comment to open and focus its card; other comment cards dim while the document stays unchanged. Click elsewhere to dismiss a pane opened this way. Manually opened panes remain visible, and draft edits survive switching comments.
- Use the Changes tab to enable Track changes. Form text/values and scripted field updates, visibility, rotations, page reordering/insertion/deletion, annotations, and comment edits become revisions with before/after details, stable revision IDs, author, and timestamps.
- Accept keeps a proposed edit; Reject restores the earlier document content. Resolve dependent edits newest first. Unsafe restoration across newer or untracked edits is blocked.
- Navigate revisions, undo/redo tracked edits (Ctrl+Z / Ctrl+Shift+Z), and inspect decision history after saving and reopening the PDF. New edits after undo preserve earlier history as a superseded branch.
- Added consistent theme-aware SVG controls for navigation, settings, zoom, menu, comments, sidebars, and window actions, plus a Find button beside sidebar page navigation.

## Tracking scope and storage

Tracking is opt-in and applies to the operations Atelier can edit today. PDF page text is reviewed through highlights, underlines, strikeouts, and comments; arbitrary replacement of existing page text is not an editor feature. Text fields are fully tracked as content edits.

The PDF stores a private Atelier revision journal with compressed original/new snapshots, including prior values and deleted pages. This supports exact restoration and increases file size. Tracked deletion is reversible, not secure erasure. Other PDF viewers show the current content and standard comment review metadata; revision decisions require Atelier. Tracking supports up to 512 revisions, 128 MiB per snapshot, and 256 MiB of uncompressed history. Extracted-page documents start a new history rather than carrying the source document journal.

## Downloads

- Windows x64 installer: `atelier-setup-windows-x64.exe`
- Windows x64 portable viewer and CLI: `atelier-windows-x64.zip`; standalone EXEs are also included.
- macOS ARM64: `atelier-macos-arm64.zip` containing Atelier.app and CLI.
- Verify downloads with `SHA256SUMS.txt`.

Windows binaries are unsigned. The macOS bundle is ad-hoc signed, not notarized. This is an opt-in prerelease and is not offered to the Stable update channel.
