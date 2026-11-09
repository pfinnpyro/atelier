# Atelier 1.1.0-beta.2

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease adds certificate signing, signature verification and signature-aware
editing guards, editing sessions and field formatting, and Linux packages.

- **Sign with certificate…** (Save dropdown) signs a copy of the current revision with a
  PEM key/certificate or a PKCS#12 identity (`.p12`/`.pfx`, with or without a password).
  On Windows, **Windows certificate store** signs with an identity from the current user's
  Personal store; Windows keeps the private key and may ask for authorization.
- The sidebar's **Signatures** tab checks each detached CMS signature with OpenSSL. It shows
  signers, certificate details, signed-byte coverage, later-byte warnings, and separate
  integrity, chain-trust and date results. Choose a per-document PEM CA bundle or use
  OpenSSL's CA store. **Verify again** refreshes the results.
- Signatures are now found across the file's revision history, including ones later
  replaced or removed. Each signed revision reports what changed after it.
- Edits, rewrites, page extraction and encryption export that could break a signature
  restricted by DocMDP/FieldMDP are blocked, and the original signed bytes stay available.
  This is conservative: edits a signature would permit (P=2/P=3, unlocked fields) are
  blocked too for now.
- Tracked edits are grouped into explicit editing sessions with undo, redo, Accept and Reject.
- Field properties: font, size, colour, alignment, character/word/line spacing, position and
  size, preserved across save and reopen.
- Middle-click drag pans the document.
- Ink can be backed with a certificate signature; Settings manages named signature copies.
- Setting Atelier as the default PDF app opens the right system page on Windows and macOS,
  and registers and verifies the default on Linux.

## Signing and verification scope

Revocation and trusted timestamps are not checked. Certificate checks use purpose `any`,
not a document-signing policy. Embedded certificates do not establish trust, and no overall
trusted-document or PAdES conformance is claimed. Encrypted PDFs cannot be signed or
inspected. Passwords are not saved in settings or passed in command arguments.
File identities and verification require OpenSSL; Mac Keychain is not implemented.

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
