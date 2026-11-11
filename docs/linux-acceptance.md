# Linux release acceptance evidence

Current working-tree validation, 2026-11-04. These are local results, not a
published release or a passing GitHub Actions run.

| Check | Evidence |
| --- | --- |
| Linux x64 release | Built viewer and CLI on Ubuntu 22.04 in Docker; ELF/glibc checks passed |
| Linux ARM64 release | Cross-built viewer and CLI with Ubuntu 22.04 ARM64 libraries; ELF/glibc checks passed |
| Combined RustRover command | Docker helper built and packaged both targets on the Windows host |
| x64 clean runtime | No compiler/dev libraries; archive checksum, ldd, per-user install, desktop-file validation, PDF open/X11 window creation and uninstall passed |
| PDF engine smoke | All 22 CLI PDF repair/render and render-equivalence checks passed in the clean runtime |
| Rust regressions | Native Ubuntu x64 app, syntax and form test suites passed |
| Installer/update regressions | Native Linux lifecycle, rollback, failure recovery, MIME cleanup, architecture checks and attestation-required/failure tests passed |
| Workflow syntax | actionlint passed for the repository workflows |

The clean runtime test found a missing `libxkbcommon-x11-0` dependency; the build,
runtime and CI dependency definitions now include it. X11 startup was re-tested.

## Gates requiring external evidence

- Run the new native `ubuntu-22.04-arm` CI/release jobs on the reviewed commit.
  Docker ARM emulation was unavailable locally; cross-compilation is not runtime
  validation. No kernel/binfmt settings were changed to enable emulation.
- Exercise GNOME/KDE on X11 and Wayland, portals/file dialogs, clipboard,
  drag/drop, real GPU drivers, DPI/accessibility and physical printing on both
  architectures. Xvfb/Mesa smoke coverage does not replace these checks.
- Publish a new, reviewed version tag through the release workflow, then test
  authenticated update and rollback from that actual attested release. Local
  fixtures test mandatory verification and failure behavior; they do not prove
  that a not-yet-published attestation exists. Existing published tags/assets
  must remain immutable.
- Supply the project license for AppStream/store metadata. The field remains
  unset pending the repository owner's choice.

The [release guide](releases.md#linux-packaging-x64-and-arm64) documents build,
install, managed update, rollback and uninstall commands. Local archives are in
`dist/1.1.0-beta.1/linux-x64/` and `dist/1.1.0-beta.1/linux-arm64/`.
