# Atelier 1.1.0-beta.3

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease makes Atelier behave like a Mac app. Windows and Linux are unchanged
from 1.1.0-beta.2.

- **PDFs open from Finder.** Double-clicking a PDF, Open With > Atelier and `open -a Atelier`
  now open the file in a tab, whether or not Atelier is already running. Before, macOS
  reported that Atelier could not open PDFs.
- **The toolbar is the title bar.** The traffic lights sit at the start of the toolbar
  row, and the empty toolbar space moves the window and zooms it on double-click.
  `ATELIER_NATIVE_TITLEBAR` brings back the separate title bar.
- **A native menu bar** with Atelier, File, Edit, View, Window and Help menus replaces the
  toolbar's menu button on macOS. Open Recent lists recent files, items grey out without
  a document, and ticks follow the open panels and zoom mode. ⌘C, ⌘V, ⌘A and ⌘Z keep
  working in text boxes.
- Quit, from the menu or the Dock, asks about unsaved changes first.

## Downloads

- Windows x64 installer: `atelier-setup-windows-x64.exe`
- Windows x64 portable viewer and CLI: `atelier-windows-x64.zip`; standalone EXEs are also included.
- macOS ARM64: `atelier-macos-arm64.zip` containing Atelier.app and CLI.
- Linux x64 and ARM64 (GNU glibc 2.35+): `atelier-linux-x64.tar.gz`, `atelier-linux-arm64.tar.gz`.
  Run `./atelier-viewer` directly, or `python3 install.py` for a per-user install with
  verified updates and rollback (requires a current GitHub CLI).
- Verify downloads with `SHA256SUMS.txt`.

Windows binaries are unsigned. The macOS bundle is ad-hoc signed, not notarized. This is an
opt-in prerelease and is not offered to the Stable update channel.
