#!/usr/bin/env python3
"""Install, update, roll back or uninstall a per-user Atelier Linux package."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

REPO = "pfinnpyro/atelier"
DESKTOP_ID = "org.atelier.Viewer"
FILES = {"atelier-viewer", "atelier", "atelier.png", "README.md", "install.py", "package.json", "org.atelier.Viewer.metainfo.xml"}
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")


def command(*args):
    subprocess.run(args, check=True, timeout=180)


def desktop(executable):
    text = str(executable)
    if any(c in text for c in "\n\r\0"):
        raise ValueError("Unsupported characters in installation path")
    quoted = '\"'
    for c in text:
        if c == "%": quoted += "%%"
        elif c in '\\"`$': quoted += "\\" + c
        else: quoted += c
    quoted += '\"'
    return "[Desktop Entry]\nType=Application\nName=Atelier\nComment=PDF viewer and editor\nExec=" + quoted.replace("\\", "\\\\") + " %f\nIcon=org.atelier.Viewer\nTerminal=false\nMimeType=application/pdf;\nCategories=Office;Viewer;\nStartupWMClass=org.atelier.Viewer\n"


def data_home():
    value = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
    if not value.is_absolute():
        raise ValueError("XDG_DATA_HOME must be absolute")
    return value


def layout(root):
    return {"metainfo": data_home() / "metainfo/org.atelier.Viewer.metainfo.xml",
            "desktop": data_home() / "applications/org.atelier.Viewer.desktop",
            "icon": data_home() / "icons/hicolor/512x512/apps/org.atelier.Viewer.png"}


def atomic_text(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".atelier-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output: output.write(text)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def state(root):
    marker = root / "managed.json"
    if not marker.is_file():
        raise ValueError("This is not an Atelier managed installation")
    result = json.loads(marker.read_text(encoding="utf-8"))
    if result.get("format") != 1: raise ValueError("Unknown installation format")
    if (root / "releases").is_symlink(): raise ValueError("Linked release directory is not supported")
    for key in ("current", "previous"):
        if result.get(key) is not None and result[key] not in result.get("versions", []):
            raise ValueError("Invalid installation version pointer")
    for entry in result.get("versions", []):
        if Path(entry).name != entry or entry in (".", ".."):
            raise ValueError("Invalid installation state")
    return result


def switch(root, version):
    if not (root / "releases" / version / "atelier-viewer").is_file():
        raise ValueError("Missing version executable")
    temporary = root / ".current-new"
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(Path("releases") / version, target_is_directory=True)
    os.replace(temporary, root / "current")


def refresh():
    if shutil.which("update-desktop-database"):
        command("update-desktop-database", str(data_home() / "applications"))


def install(source, root):
    family, libc = platform.libc_ver()
    if family != "glibc" or tuple(int(n) for n in libc.split(".")) < (2, 35):
        raise ValueError("This package requires GNU glibc 2.35 or newer")
    metadata = json.loads((source / "package.json").read_text(encoding="utf-8"))
    version, target = metadata["version"], metadata["target"]
    if not VERSION.fullmatch(version) or target not in ("linux-x64", "linux-arm64"):
        raise ValueError("Invalid package identity")
    expected = {"x86_64": "linux-x64", "amd64": "linux-x64", "aarch64": "linux-arm64", "arm64": "linux-arm64"}.get(platform.machine().lower())
    if target != expected: raise ValueError("Package architecture does not match this machine")
    for name in FILES:
        if not (source / name).is_file() or (source / name).is_symlink():
            raise ValueError("Missing or linked package file: " + name)
    managed = (root / "managed.json").is_file()
    previous = state(root) if managed else {"format": 1, "versions": [], "current": None}
    def reserved_empty(path):
        return path.name == "releases" and path.is_dir() and not path.is_symlink() and not any(path.iterdir())
    if not managed and any(p.name != ".lock" and not reserved_empty(p) for p in root.iterdir()):
        raise ValueError("Refusing to replace an unmanaged installation directory")
    paths = layout(root)
    if not managed and shutil.which("xdg-mime"):
        query = subprocess.run(["xdg-mime", "query", "default", "application/pdf"], capture_output=True, text=True, timeout=15)
        original = query.stdout.strip()
        if query.returncode == 0 and original and original != DESKTOP_ID + ".desktop":
            previous["previous_default"] = original
    entry = version + "-" + target
    destination = root / "releases" / entry
    if destination.exists():
        # A release kept after rollback can be reactivated, but only when its
        # files are identical to the package being installed.
        retained = managed and entry in previous["versions"] and entry != previous.get("current")
        if not retained or destination.is_symlink() or any(
                not (destination / name).is_file() or (destination / name).is_symlink()
                or hashlib.sha256((destination / name).read_bytes()).digest()
                != hashlib.sha256((source / name).read_bytes()).digest() for name in FILES):
            raise ValueError("Version already installed; use --rollback or another version")
        activate(root, previous, entry)
        with contextlib.suppress(OSError, subprocess.SubprocessError): refresh()
        print("Activated retained", entry, "at", root / "current")
        return
    # Do not overwrite another app's metadata, even when its file name collides.
    for path in paths.values():
        if (path.exists() or path.is_symlink()) and str(path) not in previous.get("integration", {}):
            raise ValueError("Refusing to replace existing desktop integration: " + str(path))
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".stage-", dir=destination.parent) as temporary:
        stage = Path(temporary)
        for name in FILES:
            shutil.copyfile(source / name, stage / name)
            (stage / name).chmod(0o755 if name in ("atelier", "atelier-viewer") else 0o644)
        os.replace(stage, destination)
    # Save the recoverable version before activating the new one.
    updated = dict(previous, versions=previous["versions"] + [entry],
                   previous=previous.get("current"), current=entry)
    integration = {str(paths["desktop"]): desktop(root / "current/atelier-viewer"),
                   str(paths["icon"]): hashlib.sha256((destination / "atelier.png").read_bytes()).hexdigest()}
    integration[str(paths["metainfo"])] = hashlib.sha256((destination / "org.atelier.Viewer.metainfo.xml").read_bytes()).hexdigest()
    updated["integration"] = integration
    try:
        atomic_text(paths["desktop"], integration[str(paths["desktop"])])
        paths["icon"].parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(destination / "atelier.png", paths["icon"])
        paths["metainfo"].parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(destination / "org.atelier.Viewer.metainfo.xml", paths["metainfo"])
        atomic_text(root / "managed.json", json.dumps(updated, indent=2))
        switch(root, entry)
    except Exception:
        # The old executable is retained. Recover the state pointer on failure.
        atomic_text(root / "managed.json", json.dumps(previous, indent=2))
        if previous.get("current"):
            switch(root, previous["current"])
            atomic_text(paths["desktop"], previous["integration"][str(paths["desktop"])])
            shutil.copyfile(root / "releases" / previous["current"] / "atelier.png", paths["icon"])
            shutil.copyfile(root / "releases" / previous["current"] / "org.atelier.Viewer.metainfo.xml", paths["metainfo"])
        else:
            for path in paths.values(): path.unlink(missing_ok=True)
            (root / "managed.json").unlink(missing_ok=True)
        for name in FILES: (destination / name).unlink(missing_ok=True)
        destination.rmdir()
        raise
    with contextlib.suppress(OSError, subprocess.SubprocessError): refresh()
    print("Installed", entry, "at", root / "current")


def activate(root, info, entry):
    """Make a retained release current, keeping the current one for rollback."""
    switch(root, entry)
    icon = layout(root)["icon"]
    shutil.copyfile(root / "releases" / entry / "atelier.png", icon)
    info["integration"][str(icon)] = hashlib.sha256(icon.read_bytes()).hexdigest()
    metainfo = layout(root)["metainfo"]
    shutil.copyfile(root / "releases" / entry / "org.atelier.Viewer.metainfo.xml", metainfo)
    info["integration"][str(metainfo)] = hashlib.sha256(metainfo.read_bytes()).hexdigest()
    info["previous"], info["current"] = info["current"], entry
    atomic_text(root / "managed.json", json.dumps(info, indent=2))


def rollback(root):
    info = state(root)
    old = info.get("previous")
    if old not in info["versions"]: raise ValueError("No previous version available")
    activate(root, info, old)


def remove_pdf_association():
    config = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    if not config.is_absolute(): raise ValueError("XDG_CONFIG_HOME must be absolute")
    for folder in (config, data_home() / "applications"):
        candidates = {folder / "mimeapps.list", *folder.glob("*-mimeapps.list")}
        for path in candidates:
            if path.is_symlink() or not path.is_file(): continue
            original = path.read_text(encoding="utf-8")
            lines, section = [], ""
            for line in original.splitlines(keepends=True):
                stripped = line.strip()
                if stripped.startswith("["): section = stripped
                key, separator, value = line.partition("=")
                if section in ("[Default Applications]", "[Added Associations]", "[Removed Associations]") and separator and key.strip() == "application/pdf":
                    values = [v for v in value.strip().split(";") if v]
                    if DESKTOP_ID + ".desktop" in values:
                        values = [v for v in values if v != DESKTOP_ID + ".desktop"]
                        line = key + "=" + ";".join(values) + ";\n" if values else ""
                lines.append(line)
            updated = "".join(lines)
            if updated != original: atomic_text(path, updated)


def uninstall(root):
    info = state(root)
    releases = root / "releases"
    if releases.exists() and any(p.name not in info["versions"] for p in releases.iterdir()):
        raise ValueError("Refusing to uninstall unexpected release directories")
    for version in info["versions"]:
        folder = root / "releases" / version
        if folder.is_symlink() or (folder.exists() and {p.name for p in folder.iterdir()} - FILES):
            raise ValueError("Refusing to uninstall a modified version directory")
    paths = layout(root)
    desktop_path = paths["desktop"]
    own_desktop = desktop_path.is_file() and not desktop_path.is_symlink() and desktop_path.read_text(encoding="utf-8") == info.get("integration", {}).get(str(desktop_path))
    preserve_integration = (desktop_path.exists() or desktop_path.is_symlink()) and not own_desktop
    if own_desktop and shutil.which("xdg-mime"):
        query = subprocess.run(["xdg-mime", "query", "default", "application/pdf"], capture_output=True, text=True, timeout=15)
        if query.stdout.strip() == DESKTOP_ID + ".desktop":
            if info.get("previous_default"):
                command("xdg-mime", "default", info["previous_default"], "application/pdf")
            else:
                remove_pdf_association()
    for text, recorded in info.get("integration", {}).items():
        if preserve_integration: continue
        path = Path(text)
        if path not in layout(root).values(): raise ValueError("Unexpected integration path")
        if path.is_symlink(): raise ValueError("Refusing to remove linked integration files")
        if path.is_file():
            matches = path.read_text(encoding="utf-8") == recorded if path.suffix == ".desktop" else hashlib.sha256(path.read_bytes()).hexdigest() == recorded
            if matches: path.unlink()
    for version in info["versions"]:
        folder = root / "releases" / version
        if folder.is_symlink(): raise ValueError("Refusing to remove linked version")
        if folder.exists():
            for name in FILES:
                (folder / name).unlink(missing_ok=True)
            folder.rmdir()  # Preserve unexpected files instead of deleting them.
    releases.rmdir()
    (root / "current").unlink(missing_ok=True)
    (root / "managed.json").unlink()
    refresh()
    print("Uninstalled Atelier; PDFs, settings and user data were retained.")


def download(url, destination, limit):
    request = urllib.request.Request(url, headers={"User-Agent": "Atelier-Linux-Updater"})
    with urllib.request.urlopen(request, timeout=60) as response, destination.open("wb") as out:
        if not response.url.startswith("https://"): raise ValueError("Non-HTTPS update redirect")
        total = 0
        while True:
            block = response.read(1024 * 1024)
            if not block: break
            total += len(block)
            if total > limit: raise ValueError("Update exceeds size limit")
            out.write(block)


def update(root, version):
    if not VERSION.fullmatch(version): raise ValueError("Invalid update version")
    state(root)
    metadata = json.loads((root / "current/package.json").read_text(encoding="utf-8"))
    target = metadata["target"]
    if target not in ("linux-x64", "linux-arm64"): raise ValueError("Invalid update architecture")
    if not shutil.which("gh"): raise ValueError("Install GitHub CLI (gh) for authenticated updates")
    name = "atelier-" + target + ".tar.gz"
    with tempfile.TemporaryDirectory(prefix="atelier-update-") as temporary:
        directory = Path(temporary)
        archive = directory / name
        url = "https://github.com/" + REPO + "/releases/download/v" + version + "/"
        download(url + name, archive, 512 * 1024 * 1024)
        sums = directory / "SHA256SUMS.txt"
        download(url + "SHA256SUMS.txt", sums, 1024 * 1024)
        expected = [line.split()[0] for line in sums.read_text().splitlines() if len(line.split()) == 2 and line.split()[1] == name]
        if expected != [hashlib.sha256(archive.read_bytes()).hexdigest()]: raise ValueError("Update checksum mismatch")
        # Require both workflow identity and the selected release tag, not just a
        # checksum obtained from the same server as the package.
        command("gh", "attestation", "verify", str(archive), "--repo", REPO,
                "--signer-workflow", REPO + "/.github/workflows/release.yml",
                "--source-ref", "refs/tags/v" + version, "--deny-self-hosted-runners")
        source = directory / "package"
        source.mkdir()
        with tarfile.open(archive) as tar:
            members = tar.getmembers()
            expected_names = {"atelier-" + target + "/" + name for name in FILES}
            if len(members) != len(FILES) or {m.name for m in members} != expected_names:
                raise ValueError("Unexpected update archive members")
            if sum(m.size for m in members) > 1024 * 1024 * 1024: raise ValueError("Expanded archive exceeds size limit")
            for member in members:
                if not member.isfile() or member.size > 512 * 1024 * 1024:
                    raise ValueError("Unsafe archive entry")
                with tar.extractfile(member) as content, (source / Path(member.name).name).open("wb") as out:
                    shutil.copyfileobj(content, out)
        package = json.loads((source / "package.json").read_text())
        if package.get("version") != version or package.get("target") != target:
            raise ValueError("Update package identity mismatch")
        install(source, root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=data_home() / "atelier/app")
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--update", metavar="VERSION")
    actions.add_argument("--rollback", action="store_true")
    actions.add_argument("--uninstall", action="store_true")
    args = parser.parse_args()
    if platform.system() != "Linux": parser.error("Linux only")
    import fcntl
    root = args.root.expanduser().absolute()
    if root.is_symlink(): parser.error("Installation root must not be a symlink")
    root.mkdir(parents=True, exist_ok=True)
    with (root / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.update: update(root, args.update)
        elif args.rollback: rollback(root)
        elif args.uninstall: uninstall(root)
        else: install(Path(__file__).resolve().parent, root)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        raise SystemExit("Atelier installation failed: " + str(error))
