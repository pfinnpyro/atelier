# Atelier 1.1.0-beta.4

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease adds form profiles and printing from the page organizer.

- **Fill from a profile.** Keep named sets of details, such as "Home" and "Work", and
  fill a page or a whole document with one. A profile has places for a name, a point of
  contact, mailing and street addresses, city, state, ZIP code, email and phone, and
  takes any other details you add under your own labels. Atelier lists the fields it
  would fill before filling them, so a wrong match can be unticked.
- **Forgiving matching.** Fields are matched by what they are called. Word order, filler
  words, abbreviations and small misspellings are allowed for; "Company name", "Spouse
  name" and "Address line 2" are left empty. Labels are read in English only.
- **Encrypted profiles.** A profile can be sealed with a passphrase. It is then decrypted
  only into memory, and only while the Form profiles window is open: closing the window,
  **Lock**, or five minutes without use overwrites the details and the keys. Nothing
  decrypted is written to disk, and without the passphrase the profile cannot be
  recovered.
- **Print selected…** in the page organizer opens the Print dialog on the ticked pages.

The encryption (PBKDF2-HMAC-SHA256, AES-256-CBC, HMAC-SHA256) is Atelier's own and has
not been independently reviewed. Overwriting is best effort: text boxes keep copies that
are dropped rather than overwritten, the system may page memory to disk, and a
solid-state drive may keep the old plain file when encryption is turned on. The README
says more. The Form profiles window has been exercised by automated tests, not yet by
hand on each platform.

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
