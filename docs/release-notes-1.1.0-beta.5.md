# Atelier 1.1.0-beta.5

> Release snapshot. Later working-tree changes and remaining work are tracked in the
> [roadmap](roadmap.md); these notes are not current platform-validation results.

This prerelease adds FIPS packages for Windows. The ordinary packages behave as in
1.1.0-beta.4.

- **FIPS packages.** Downloads with `-fips` in their names are a separate build for
  places that must use validated cryptography. In it, encrypted form profiles are
  sealed and opened entirely by Windows CNG's Cryptographic Primitives Library, the
  module Microsoft has validated under FIPS 140: key derivation (PBKDF2-HMAC-SHA256),
  encryption (AES-256-CBC), authentication (HMAC-SHA256) and random numbers. None of
  Atelier's own cryptography is used for profiles.
- **The same files.** Both builds use the same algorithms and file format, so a profile
  encrypted by one opens in the other.
- **Updates stay in kind.** A FIPS build updates only to FIPS packages and ignores
  releases that have none. About Atelier says when a copy is the FIPS build, and the
  profile editor names the module and shows whether the Windows FIPS policy is on.
- New passphrases need at least eight characters in the FIPS build.

The FIPS build covers form profiles only. Password-protecting a PDF, reading encrypted
PDFs and checking an update's checksum still use Atelier's own code, and signatures go
through OpenSSL as before. Using a validated module does not make Atelier itself
validated; whether the FIPS build satisfies a given policy is for whoever accredits the
system. macOS and Linux have no FIPS package yet. The FIPS build has been exercised by
automated tests, including byte-for-byte agreement with the ordinary build, not yet by
hand.

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
