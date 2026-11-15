# Atelier 1.1.0-beta.6

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease makes drawing faster and lighter on memory, rebuilds the menus and
Settings, and fills more kinds of form field from a profile.

- **Faster drawing.** Each open document parses its fonts and decodes its images once
  and shares them across every tile, thread and zoom level, instead of once per tile.
  Content outside the part of the page being drawn is skipped without being decoded.
  While you zoom or resize the window, sharp tiles wait until the zoom settles.
- **Memory you can set.** The memory used for drawn pages and decoded images is now
  sized from the computer's memory, or set yourself in Settings. A file in a
  background tab keeps only enough to show at once when you switch back to it.
- **Files open and save without stalling.** Reading and saving happen off the main
  thread, and search reads several pages at a time while still listing hits in page
  order.
- **One set of menus.** The macOS menu bar and the menu button on Windows and Linux
  are built from one list of commands, so they offer the same items in the same order
  under the same names. On macOS, shortcuts are shown as key symbols (⌘⇧S).
- **Settings in its own window.** Kinds of setting are listed down the side. Form
  profiles are managed there: a table with each profile's name and creation date, and
  the profile editor shows details in headed groups.
- **Toolbar.** The icons are sharp at any display scale. When an update is available,
  a symbol in the toolbar opens what it is and what to do, and nothing covers the page.
- **Better form filling.** Names and addresses are stored in parts, so a field gets
  the part it asks for, and a field that asks for several gets them combined:
  "Last, First MI", or city, state and ZIP on one line. A date of birth is understood
  however it was typed. It is written in the order the field shows ("MM/DD/YYYY"), or
  split into separate month, day and year fields. Profiles now record when they were
  created.
- **`atelier bench`** times drawing pages in tiles, the way the viewer draws them.

The source tree also gains `atelier-fieldml`: a small model, about 1 MB, that reads
a form's labels and layout. For each field it says what detail it asks for, which
fields belong together, and whose they are. The app does not use it yet; filling
still follows the rules above.

## Downloads

- Windows x64 installer: `atelier-setup-windows-x64.exe`
- Windows x64 portable viewer and CLI: `atelier-windows-x64.zip`; standalone EXEs are also included.
- Windows x64 FIPS build: `atelier-setup-windows-x64-fips.exe`, `atelier-windows-x64-fips.zip`
  and the standalone `-fips` EXEs.
- macOS ARM64: `atelier-macos-arm64.zip` containing Atelier.app and CLI.
- Linux x64 and ARM64 (GNU glibc 2.35+): `atelier-linux-x64.tar.gz`, `atelier-linux-arm64.tar.gz`.
  Run `./atelier-viewer` directly, or `python3 install.py` for a per-user install with
  verified updates and rollback (requires a current GitHub CLI).
- Verify downloads with `SHA256SUMS.txt`.

Windows binaries are unsigned. The macOS bundle is ad-hoc signed, not notarized. This is an
opt-in prerelease and is not offered to the Stable update channel.
