# Atelier 1.1.0-beta.7

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease is 1.1.0-beta.6 with its dependencies cleaned up. 1.1.0-beta.6 was
built but not published, so if you are coming from 1.1.0-beta.5, everything in
[its notes](release-notes-1.1.0-beta.6.md) is new to you as well: OpenPGP protection
for whole documents, faster drawing, the rebuilt menus and Settings, and fuller form
filling.

- **Fewer dependencies.** The viewer asked for an image library with every format it
  knows, and used only PNG. It now takes PNG alone, which removes a set of AVIF and
  OpenEXR encoders from the build along with two crates that had advisories against
  them.
- **Known advisories are checked on every change.** The build now fails if a locked
  dependency has a known vulnerability. Two advisories against `quick-xml` are passed
  over, with the reason recorded: it is present only through the Linux accessibility
  layer of the interface toolkit, where it reads descriptions that ship with that
  layer, and Atelier never gives it a file. It cannot be updated until the toolkit
  is. Four other crates from the same layer are reported as unmaintained, with no
  replacement available yet.
- **Developer tools.** The Python libraries used to build the form-label model's
  corpus are updated to versions without known advisories, and those tools now run on
  Windows. Neither library is part of Atelier itself.

Nothing in how the viewer or the command-line tool behaves has changed since
1.1.0-beta.6. The limits in its notes still apply, among them that OpenPGP protection
and the FIPS build have been exercised by automated tests and not yet by hand on each
platform.

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
