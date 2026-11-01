# Atelier 1.0.0

The first official Atelier release, with packaged builds for Apple Silicon Macs
and Windows x64.

- Native PDF viewing and editing, tabs, search, forms, page organization, markup,
  printing and OCR.
- macOS ARM64 app bundle in a ZIP; Windows x64 setup EXE, portable viewer EXE,
  command-line EXE and ZIP.
- SHA-256 checksums for all downloads.
- Stable and opt-in Preview update channels. Windows supports in-app installation;
  macOS links to the release downloads.

## Installation

On macOS, extract `atelier-macos-arm64.zip` and move `Atelier.app` to Applications.
On Windows, run `atelier-setup-windows-x64.exe`, or extract the portable ZIP.

## Known limitations

Builds are currently unsigned for public distribution (the Mac bundle has an
ad-hoc signature only). Gatekeeper or SmartScreen may block or warn about downloads.
macOS automatic update installation is planned. PDF rendering fidelity needs
broader real-world validation; keep originals when editing important documents.
