# Releases and GitHub updates

## Current preview: 1.1.0-beta.1

The `prerelease` branch and `v1.1.0-beta.1` tag identify the review flight.
Notes are in `release-notes-1.1.0-beta.1.md`. The package workflow reads the
matching version-specific notes file and attaches Windows x64 installer,
portable binaries, macOS ARM64 app, and the combined SHA-256 manifest.
The release must be published with GitHub's prerelease flag.


## First official release: 1.0.0

The workspace and lockfile identify this release as **1.0.0**. Previous 0.x
versions are development releases. Release notes are in `release-notes-1.0.0.md`.

Shared RustRover configurations live in `.run/`:

- **Atelier Local** runs the viewer in debug mode; add a PDF after `--` in its arguments.
- **Build macOS ARM64** builds the viewer and CLI for Apple Silicon.
- **Build Windows x64** builds MSVC executables on Windows with Visual Studio C++ build tools.
- **Package macOS ARM64** builds and packages on a Mac (Shell Script plugin).
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

Both commands build with `--locked`; `--skip-build` packages existing binaries.
Output is in `dist/<version>/<platform>/`. The macOS ZIP contains `Atelier.app`
with the viewer, CLI, PDF association metadata and icon. Move it to Applications;
the CLI is `Atelier.app/Contents/MacOS/atelier`. The Windows ZIP contains the
viewer and CLI; the standalone viewer and setup EXEs retain the fixed names
required by the updater. Each platform produces `SHA256SUMS.txt`.

macOS packages are ad-hoc signed, **not Developer ID signed or notarized**.
Public downloads may be blocked by Gatekeeper. Developer ID signing and
notarization are a release-readiness task requiring the owner's Apple credentials.
Windows executables are also currently unsigned; SmartScreen may warn.

## Release procedure

1. Update the workspace version, run `cargo check --workspace` to refresh the
   lockfile, and commit the changes and release notes.
2. Run the **Package and release** workflow manually to test packaging without
   creating a release. Download both platform artifacts and smoke-test them.
3. Push a matching version tag on the reviewed commit, for example:
   `git tag v1.0.0` then `git push origin v1.0.0`.
4. The release workflow tests both native targets, builds packages, verifies
   hashes and required assets, and creates a **draft** GitHub release containing
   both platforms and a combined checksum manifest. Failed jobs publish nothing.
5. Test opening PDFs, form scripts, save/reopen, printing, and launch from the
   packaged app on both systems. Test a Windows update from an earlier installed
   and portable build. Verify the unsigned-distribution limitations above are
   acceptable before publishing. Publish the draft in GitHub when ready.

The workflow never commits or tags for you. A manual run only uploads Actions
artifacts. Re-running a failed draft creation may require deleting its incomplete
draft first; published releases and their assets must remain immutable.

## Channels

| Channel | Version/tag | GitHub status | Audience |
| --- | --- | --- | --- |
| Stable | `1.0.0` / `v1.0.0` | Published release | Default |
| Preview | `1.1.0-beta.1` / `v1.1.0-beta.1` | Published pre-release | Opt-in testers |

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

## Current updater and implementation plan

The existing app checks the latest 30 GitHub releases at startup when enabled,
compares versions, and presents release notes. On Windows it downloads the setup
EXE for installed copies or the viewer EXE for portable copies, verifies it with
`SHA256SUMS.txt`, and installs only after user action and saved document changes.
macOS offers the release download page; it does not replace its own app bundle.
Checksums detect corruption but are not an independent authenticity guarantee.

Next development work, in order:

1. **Harden release discovery.** Respect GitHub's prerelease flag in addition to
   semantic tags, validate version syntax, paginate results, and select only
   releases containing assets for this platform and a checksum manifest. Handle
   rate limits, offline startup and timeouts without interrupting document work.
   Test stable/preview transitions, malformed releases and missing assets.
2. **Authenticate updates.** Sign executables with platform credentials and add
   a signed manifest containing version, channel, target, size and SHA-256. Embed
   the verification public key in the app. Restrict downloads to expected HTTPS
   GitHub asset hosts and validate before executing anything. Document key rotation.
3. **Install safely.** On Windows, stage in unique temporary paths, synchronize
   concurrent requests, retain rollback state, and test installer/portable restart
   and failure recovery. Define whether CLI updates ship with the viewer (the
   current portable updater replaces only the viewer). On macOS, use a signed,
   notarized bundle and a maintained updater such as Sparkle after evaluating its
   integration; stage and verify the full app, then replace it after exit with
   rollback and permission handling. Keep manual download until this is ready.
4. **Operate channels.** Add an explicit signed channel manifest if release
   volume outgrows the GitHub API feed. Keep immutable versioned assets behind
   channel pointers. Test updates end-to-end from the previous stable and preview
   builds before each publication, including interrupted download, checksum/signature
   failure, unwritable install location, unsaved files and rollback. Document
   emergency withdrawal and key compromise handling before unattended installs.

GitHub runner reference:
https://docs.github.com/en/actions/reference/runners/github-hosted-runners
The workflow uses native `macos-15` ARM64 and `windows-2022` x64 runners.

Updater framework reference: https://sparkle-project.org/documentation/
