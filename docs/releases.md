# Releases and GitHub updates

## Current preview: 1.1.0-beta.4

The `prerelease` branch and `v1.1.0-beta.4` tag identify the review flight.
Notes are in `release-notes-1.1.0-beta.4.md`; the earlier previews are `v1.1.0-beta.3` (`release-notes-1.1.0-beta.3.md`), `v1.1.0-beta.2` (`release-notes-1.1.0-beta.2.md`) and
`v1.1.0-beta.1` (`release-notes-1.1.0-beta.1.md`). The package workflow reads the
matching version-specific notes file and attaches Windows x64 installer,
portable binaries, macOS ARM64 app, Linux x64/ARM64 archives, and the combined
SHA-256 manifest for newly built tags.
The release must be published with GitHub's prerelease flag.


## FIPS packages

`python tools/package_release.py windows-x64 --installer --fips` builds the viewer with
the `fips` feature and writes `atelier-windows-x64-fips.zip`, `atelier-windows-x64-fips.exe`,
`atelier-cli-windows-x64-fips.exe` and `atelier-setup-windows-x64-fips.exe` to
`dist/<version>/windows-x64-fips/`. In that build form profiles are sealed by Windows
CNG instead of Atelier's own code (see the README). The release workflow builds and
tests both sets and attaches both to the release; each build updates to its own kind,
and a FIPS build ignores releases without FIPS packages. `--fips` is refused for macOS
and Linux until a validated module is wired up there. Both variants build into the same
`target/` folder, so package one, then the other.

## First official release: 1.0.0

The first stable release is **1.0.0**; the current workspace version is
**1.1.0-beta.4**. Previous 0.x versions are development releases. Release notes are in `release-notes-1.0.0.md`.

Shared RustRover configurations live in `.run/`:

- **Atelier Local** runs the viewer in debug mode; add a PDF after `--` in its arguments.
- **Build macOS ARM64** builds the viewer and CLI for Apple Silicon.
- **Build Windows x64** builds MSVC executables on Windows with Visual Studio C++ build tools.
- **Package macOS ARM64** builds and packages on a Mac (Shell Script plugin).
- **Package Linux x64 and ARM** builds both Linux architectures through Docker on
  Windows (PowerShell/Python); no Windows-to-Linux linker setup is required.
- **Package Linux x64**, **Package Linux ARM64**, and **Package Linux x64 and ARM
  (Linux host)** run native Linux shell commands (cross linker needed for the other architecture).
- **Package Windows x64** builds portable files and the installer on Windows
  (Shell Script plugin, PowerShell, Python 3.9+, Inno Setup 6).

From the repository root, the equivalent packaging commands are:

```sh
rustup target add aarch64-apple-darwin
python3 tools/package_release.py macos-arm64
```

```powershell
rustup target add x86_64-pc-windows-msvc
python tools/package_release.py windows-x64 --installer
```

All packaging commands build with `--locked`; `--skip-build` packages existing binaries.
Output is in `dist/<version>/<platform>/`. The macOS ZIP contains `Atelier.app`
with the viewer, CLI, PDF association metadata and icon. Move it to Applications;
the CLI is `Atelier.app/Contents/MacOS/atelier`. The Windows ZIP contains the
viewer and CLI; the standalone viewer and setup EXEs retain the fixed names
required by the updater. Each platform produces `SHA256SUMS.txt`.

macOS packages are ad-hoc signed, **not Developer ID signed or notarized**.
Public downloads may be blocked by Gatekeeper. Developer ID signing and
notarization are a release-readiness task requiring the owner's Apple credentials.
Windows executables are also currently unsigned; SmartScreen may warn.

## Linux packaging (x64 and ARM64)

Local packaging targets are `linux-x64` (`x86_64-unknown-linux-gnu`) and
`linux-arm64` (`aarch64-unknown-linux-gnu`). ARM here means 64-bit AArch64,
not 32-bit ARM. These are GNU/glibc builds, not musl/static executables.
See the [Rust target reference](https://doc.rust-lang.org/stable/rustc/platform-support.html).

Run on Linux (or inside a Linux/WSL environment with the repository and toolchain):

```sh
rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
python3 tools/package_release.py linux-x64
python3 tools/package_release.py linux-arm64
```

RustRover's **Package Linux x64 and ARM** uses PowerShell and
`python tools/package_linux.py both`, so it works on the current Windows host
with Docker Desktop running in Linux-container mode. On other hosts run:

```sh
python3 tools/package_linux.py both
```

The helper builds Ubuntu 22.04 images, uses dedicated Docker build/cache volumes,
and writes packages into the project's `dist/` directory. ARM64 is cross-compiled
on x64; it does not require privileged containers or binfmt registration. Use
`linux-x64`/`linux-arm64` instead of `both` to select one architecture, `--jobs 2`
to reduce build concurrency, or `--skip-build` to repack cached binaries.
Docker must be running and have network access for the first image/build.

The individual native shell configurations and **Package Linux x64 and ARM
(Linux host)** use `/bin/bash` and `python3`. Run these on Linux or a configured
WSL IDE execution environment.

Native builds on matching architectures are the simplest route. Building both on
one Linux host requires the other architecture's C compiler/linker, target sysroot
and development libraries. For example, an x64 host can configure
`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc` after
installing that toolchain. Any native library discovery must use target libraries
and target pkg-config/sysroot settings. `rustup target add` alone does not supply
these dependencies; the packaging script does not install or configure them.

`tools/setup_linux.sh` installs build/runtime/QA dependencies on Debian-family
Linux systems. The builder images and native release runners use Ubuntu 22.04.
The supported release baseline is **GNU glibc 2.35 or newer**, on x64 or ARM64;
this does not include musl/Alpine or 32-bit ARM. Packaging checks the ELF machine
and rejects glibc requirements newer than 2.35. Tests on later Debian WSL do not
replace baseline testing. Review [eframe's Linux prerequisites](https://docs.rs/crate/eframe/0.37.0)
when changing the desktop stack.

Output: `dist/<version>/linux-{x64,arm64}/atelier-linux-{x64,arm64}.tar.gz`
and `SHA256SUMS.txt`. Each archive contains executable viewer and CLI binaries,
README, icon, AppStream metadata, `package.json`, and `install.py` under
`atelier-linux-<arch>/`. `--installer` remains Windows-only.

### Install, update, roll back and uninstall

Extract the matching archive. For a portable copy, run `./atelier-viewer` from a
stable location. For a per-user managed installation (no sudo):

```sh
python3 install.py
# Default location, respecting XDG_DATA_HOME:
~/.local/share/atelier/app/current/atelier-viewer
```

Managed installation creates a versioned app directory, a stable `current` link,
a desktop entry, a hicolor icon and AppStream metadata in the per-user XDG data
folders. Add `~/.local/share/atelier/app/current` to PATH for CLI use. It does not
change the default PDF application; use Atelier's Settings to do that explicitly.
The original PDF default is recorded when available for restoration on uninstall.
Settings, PDFs and OCR models are retained. Existing unrelated integration files
and unmanaged directories are not overwritten.

The app offers updates for managed installations only, selects the matching Linux
architecture, and ignores releases missing that package or its checksum manifest.
Updates require Python 3 and a **current GitHub CLI** with artifact-attestation
verification support ([install GitHub CLI](https://cli.github.com/)). Older distro
`gh` packages may lack the required verification flags.

Before switching versions, the updater checks SHA-256 and requires a GitHub
provenance attestation for `pfinnpyro/atelier/.github/workflows/release.yml` and the
exact selected `refs/tags/v<version>`, rejecting self-hosted signing runners.
Missing/offline/failed verification leaves the current version untouched. The
whole viewer/CLI package is staged and the `current` link is replaced atomically;
the prior version is kept for rollback. Attestations are produced for new version
tags, not old published assets or local builds. Manual downloads/source builds
continue to work without being promoted to authenticated update packages.
See [GitHub's attestation verification](https://cli.github.com/manual/gh_attestation_verify).

```sh
python3 ~/.local/share/atelier/app/current/install.py --update 1.2.3
python3 ~/.local/share/atelier/app/current/install.py --rollback
python3 ~/.local/share/atelier/app/current/install.py --uninstall
```

For custom locations, pass the same `--root /absolute/path` to each operation.
Uninstall removes only recorded application files and unmodified integration
files; unexpected files stop removal rather than being deleted. Operations use
an exclusive installation lock. A running old viewer can finish before restart;
restart selects the new `current` version.

### Runtime dependencies and validation

The runtime image in `tools/Dockerfile.linux-runtime` contains the supported
runtime dependencies without compilers or development headers. This includes
Wayland/X11 libraries (**including libxkbcommon-x11-0**), fontconfig, Mesa/OpenGL/
Vulkan software fallback, desktop portals, Liberation/DejaVu/CJK fonts, Python 3,
CA certificates, curl, OpenSSL, CUPS clients and XDG desktop tools. The archive
bundles none of these system packages, OCR models or certificate stores. The
optional `gh` dependency is for authenticated managed updates.

CI tests Ubuntu x64 and ARM64 natively. The release workflow builds both on
Ubuntu 22.04, validates package checksums and dependencies, installs the package
in an isolated XDG environment, validates its desktop entry, runs the CLI PDF
repair/render corpus and creates an X11 viewer window with a PDF under Xvfb.
The clean runtime container repeats this smoke check on each native runner.
Archive provenance, artifact uploads and the combined release checksum collector
include both architectures.

Local clean-runtime validation after packaging:

```sh
docker build -f tools/Dockerfile.linux-runtime -t atelier-linux-runtime:qa tools
docker run --rm --mount type=bind,source="$(pwd)",target=/workspace,readonly \
  atelier-linux-runtime:qa python3 tools/qa_linux_package.py dist --target linux-x64
```

### Acceptance evidence still requiring native sessions

See the [local Linux acceptance record](linux-acceptance.md) for completed checks
and the remaining external gates.

Automated smoke tests do not establish GNOME/KDE/Wayland interaction, hardware GPU
behavior, physical printing, real clipboard/drag-drop, multi-DPI accessibility,
or signing interoperability with independent viewers and real identities.
Run these on x64 and ARM64 desktops before marking a public release accepted.
Cross-compilation does not establish ARM64 runtime success; a configured CI job
is not a passing job. No GitHub workflow was run/published by this local edit.
Linux file identities use OpenSSL; native certificate-store identities are a
separate provider feature, not implied by Linux packaging.

The repository owner must supply the project license before distribution through
stores that require it. AppStream metadata deliberately leaves project licensing
unset rather than inventing a license. This does not affect local packaging.


## Release procedure

1. Update the workspace version, run `cargo check --workspace` to refresh the
   lockfile, and commit the changes and release notes.
2. Run the **Package and release** workflow manually to test packaging without
   creating a release. Download all four platform artifacts and smoke-test them.
3. Push a matching version tag on the reviewed commit, for example:
   `git tag v1.0.0` then `git push origin v1.0.0`.
4. The release workflow tests all four native targets, builds packages, verifies
   hashes and required assets, and creates a **draft** GitHub release containing
   all four platforms and a combined checksum manifest. Failed jobs publish nothing.
5. Test opening PDFs, form scripts, save/reopen, printing, and launch from the
   packaged app on each supported system. Test a Windows update from an earlier installed
   and portable build. Verify the unsigned-distribution limitations above are
   acceptable before publishing. Publish the draft in GitHub when ready.

The workflow never commits or tags for you. A manual run only uploads Actions
artifacts. Re-running a failed draft creation may require deleting its incomplete
draft first; published releases and their assets must remain immutable.

## Channels

| Channel | Version/tag | GitHub status | Audience |
| --- | --- | --- | --- |
| Stable | `1.0.0` / `v1.0.0` | Published release | Default |
| Preview | `1.1.0-beta.4` / `v1.1.0-beta.4` | Pre-release | Opt-in testers |

The in-app setting already persists Stable or Preview. Stable selects newer
stable versions; Preview selects the newest version across both channels using
semantic version ordering. Drafts are ignored. Switching back to Stable never
downgrades a newer preview; the user waits for the next stable version or manually
installs an earlier build. Promote by building a new stable tag from the approved
code with its stable version; do not relabel a beta binary as stable.

Keep prerelease flags consistent with tags. Do not publish an experimental build
with a stable-looking version. Ship an urgent repair as a higher patch version;
withdraw a bad release from discovery and publish a higher fixed version rather
than overwriting assets or moving tags.

## Current updater

The existing app checks the latest 30 GitHub releases at startup when enabled,
compares versions, and presents release notes. On Windows it downloads the setup
EXE for installed copies or the viewer EXE for portable copies, verifies it with
`SHA256SUMS.txt`, and installs only after user action and saved document changes.
On macOS it downloads `atelier-macos-arm64.zip`, verifies it with `SHA256SUMS.txt`,
unpacks it beside the running app, checks its code signature and swaps the two
`Atelier.app` bundles, keeping the old one as `Atelier.app.old` until the next start.
That needs a writable folder, so a copy still running from Downloads (translocated)
or from an `/Applications` the user cannot write to gets the release page instead.
Linux managed installs support verified versioned updates and rollback, while
portable/source copies use manual downloads.

A preview build on the Stable channel is still offered later previews of its own
version (1.1.0-beta.2 is offered 1.1.0-beta.3 and 1.1.0, never 1.2.0-beta.1), and a
fresh preview install starts on the Preview channel.
Checksums detect corruption but are not an independent authenticity guarantee.

Implementation priorities for release discovery, authenticated manifests, staging,
rollback and channel operations live in the [roadmap](roadmap.md#release-and-updater-work).
This guide documents operations, not a second development plan.

The current host cannot run macOS. Mac package/signing/notarization/updater checks must
run on a Mac or native CI; Windows results do not satisfy those release gates.

GitHub runner reference:
https://docs.github.com/en/actions/reference/runners/github-hosted-runners
The workflow uses native `macos-15` ARM64, `windows-2022` x64,
`ubuntu-22.04` x64 and `ubuntu-22.04-arm` ARM64 runners.

Updater framework reference: https://sparkle-project.org/documentation/
