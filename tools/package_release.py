#!/usr/bin/env python3
"""Build and package release binaries on their native OS (Python 3.9+)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
}


def run(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def checksums(folder):
    files = sorted(p for p in folder.iterdir() if p.is_file() and p.name != "SHA256SUMS.txt")
    (folder / "SHA256SUMS.txt").write_text("".join(
        f"{sha256(p)}  {p.name}\n" for p in files
    ), encoding="utf-8")


def validate_linux_binary(path, target):
    with path.open("rb") as binary:
        header = binary.read(20)
    expected = 62 if target == "linux-x64" else 183
    if len(header) < 20 or header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != expected:
        raise SystemExit(f"Wrong ELF architecture: {path} for {target}")
    versions = subprocess.check_output(["readelf", "--version-info", str(path)], text=True)
    required = {tuple(map(int, value.split("."))) for value in re.findall(r"GLIBC_([0-9]+(?:\.[0-9]+)+)", versions)}
    if any(version > (2, 35) for version in required):
        raise SystemExit(f"{path} requires glibc newer than the Ubuntu 22.04 baseline (2.35). Build on the baseline image.")
    if "GLIBC_PRIVATE" in versions or "GLIBC_ABI_" in versions:
        raise SystemExit(f"Unsupported private/new glibc ABI in {path}")


def package(target, skip_build=False, installer=False, fips=False):
    triple = TARGETS[target]
    if fips and target != "windows-x64":
        raise SystemExit("--fips is only supported for windows-x64: the validated module wired up so far is Windows CNG.")
    if installer and target != "windows-x64":
        raise SystemExit("--installer is only supported for windows-x64.")
    if target.startswith("linux-") and platform.system() != "Linux":
        raise SystemExit("Linux packaging requires Linux. Use a Linux host or run the command inside WSL; cross-architecture builds also need a target linker and sysroot.")
    if target == "macos-arm64" and platform.system() != "Darwin":
        raise SystemExit("macOS packaging requires macOS (codesign, sips, iconutil and ditto).")
    if target == "windows-x64" and platform.system() != "Windows":
        raise SystemExit("Windows packaging requires Windows with the MSVC build tools and Inno Setup 6.")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version=1", "--locked", "--offline"], cwd=ROOT))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "atelier-app")
    if not skip_build:
        # The FIPS build seals form profiles with the system's validated module.
        features = ("--features", "atelier-app/fips") if fips else ()
        run("cargo", "build", "--locked", "--release", "--target", triple,
            "-p", "atelier-app", "-p", "atelier-cli", *features)
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target_dir.is_absolute():
        target_dir = ROOT / target_dir
    binaries = target_dir / triple / "release"
    suffix = ".exe" if target == "windows-x64" else ""
    for name in ("atelier-viewer", "atelier"):
        if not (binaries / (name + suffix)).is_file():
            raise SystemExit(f"Missing binary: {binaries / (name + suffix)}")
    if target.startswith("linux-"):
        for name in ("atelier-viewer", "atelier"):
            validate_linux_binary(binaries / name, target)
    # FIPS packages carry their own names, so the two sets can sit in one
    # release and each build updates to its own kind.
    variant = "-fips" if fips else ""
    out = ROOT / "dist" / version / (target + variant)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    if target == "macos-arm64":
        app = out / "Atelier.app"
        contents = app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "Resources").mkdir()
        for name in ("atelier-viewer", "atelier"):
            shutil.copy2(binaries / name, contents / "MacOS" / name)
            (contents / "MacOS" / name).chmod(0o755)
        with (contents / "Info.plist").open("wb") as f:
            plistlib.dump({
                "CFBundleName": "Atelier", "CFBundleDisplayName": "Atelier",
                "CFBundleIdentifier": "com.pfinnpyro.atelier",
                "CFBundleExecutable": "atelier-viewer", "CFBundlePackageType": "APPL",
                "CFBundleShortVersionString": version, "CFBundleVersion": version.split('-')[0],
                "CFBundleIconFile": "Atelier.icns", "LSMinimumSystemVersion": "11.0",
                "NSHighResolutionCapable": True,
                "CFBundleDocumentTypes": [{"CFBundleTypeName": "PDF document",
                    "CFBundleTypeRole": "Editor", "LSHandlerRank": "Alternate",
                    "LSItemContentTypes": ["com.adobe.pdf"]}],
            }, f)
        icons = out / "Atelier.iconset"
        icons.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                name = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
                run("sips", "-z", str(size * scale), str(size * scale),
                    str(ROOT / "crates/atelier-app/assets/atelier.png"), "--out", str(icons / name),
                    stdout=subprocess.DEVNULL)
        run("iconutil", "-c", "icns", str(icons), "-o", str(contents / "Resources/Atelier.icns"))
        shutil.rmtree(icons)
        # Ad-hoc signing supports local Apple Silicon execution; Developer ID and
        # notarization are separate distribution credentials (see docs/releases.md).
        run("codesign", "--force", "--sign", "-", str(contents / "MacOS/atelier"))
        run("codesign", "--force", "--sign", "-", str(app))
        run("codesign", "--verify", "--deep", "--strict", str(app))
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app),
            str(out / "atelier-macos-arm64.zip"))
    elif target.startswith("linux-"):
        # Tar records Unix modes explicitly, independent of the source build's umask.
        with tarfile.open(out / f"atelier-{target}.tar.gz", "w:gz") as archive:
            for source, name, mode in (
                (binaries / "atelier-viewer", "atelier-viewer", 0o755),
                (binaries / "atelier", "atelier", 0o755),
                (ROOT / "README.md", "README.md", 0o644),
                (ROOT / "crates/atelier-app/assets/atelier.png", "atelier.png", 0o644),
                (ROOT / "tools/linux_install.py", "install.py", 0o644),
                (ROOT / "installer/linux/org.atelier.Viewer.metainfo.xml", "org.atelier.Viewer.metainfo.xml", 0o644),
            ):
                info = archive.gettarinfo(str(source), arcname=f"atelier-{target}/{name}")
                info.mode = mode
                info.uid = info.gid = 0
                info.uname = info.gname = "root"
                with source.open("rb") as content:
                    archive.addfile(info, content)
            import io
            data = json.dumps({"version": version, "target": target,
                               "glibc_baseline": "2.35", "format": 1}).encode("utf-8")
            info = tarfile.TarInfo(f"atelier-{target}/package.json")
            info.mode = 0o644
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    else:
        shutil.copy2(binaries / "atelier-viewer.exe", out / f"atelier-windows-x64{variant}.exe")
        shutil.copy2(binaries / "atelier.exe", out / f"atelier-cli-windows-x64{variant}.exe")
        with zipfile.ZipFile(out / f"atelier-windows-x64{variant}.zip", "w", zipfile.ZIP_DEFLATED) as archive:
            archive.write(binaries / "atelier-viewer.exe", "atelier-viewer.exe")
            archive.write(binaries / "atelier.exe", "atelier.exe")
            archive.write(ROOT / "README.md", "README.md")
        if installer:
            iscc = shutil.which("ISCC.exe") or str(Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Inno Setup 6/ISCC.exe")
            run(iscc, f"/DAppVersion={version}", f"/DBinaryDir={binaries}",
                f"/O{out}", f"/Fatelier-setup-windows-x64{variant}", str(ROOT / "installer/atelier.iss"))
    checksums(out)
    print(f"Packaged Atelier {version}: {out}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("--skip-build", action="store_true", help="Package existing release binaries")
    parser.add_argument("--installer", action="store_true", help="Also build the Windows installer with Inno Setup 6")
    parser.add_argument("--fips", action="store_true",
                        help="Build the FIPS variant, which uses the system's validated cryptography (windows-x64 only)")
    args = parser.parse_args()
    package(args.target, args.skip_build, args.installer, args.fips)
