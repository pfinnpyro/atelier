#!/usr/bin/env python3
"""Build and package release binaries on their native OS (Python 3.9+)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
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


def package(target, skip_build=False, installer=False):
    triple = TARGETS[target]
    if target == "macos-arm64" and platform.system() != "Darwin":
        raise SystemExit("macOS packaging requires macOS (codesign, sips, iconutil and ditto).")
    if target == "windows-x64" and platform.system() != "Windows":
        raise SystemExit("Windows packaging requires Windows with the MSVC build tools and Inno Setup 6.")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version=1", "--locked", "--offline"], cwd=ROOT))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "atelier-app")
    if not skip_build:
        run("cargo", "build", "--locked", "--release", "--target", triple,
            "-p", "atelier-app", "-p", "atelier-cli")
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target_dir.is_absolute():
        target_dir = ROOT / target_dir
    binaries = target_dir / triple / "release"
    suffix = ".exe" if target == "windows-x64" else ""
    for name in ("atelier-viewer", "atelier"):
        if not (binaries / (name + suffix)).is_file():
            raise SystemExit(f"Missing binary: {binaries / (name + suffix)}")
    out = ROOT / "dist" / version / target
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
    else:
        shutil.copy2(binaries / "atelier-viewer.exe", out / "atelier-windows-x64.exe")
        shutil.copy2(binaries / "atelier.exe", out / "atelier-cli-windows-x64.exe")
        with zipfile.ZipFile(out / "atelier-windows-x64.zip", "w", zipfile.ZIP_DEFLATED) as archive:
            archive.write(binaries / "atelier-viewer.exe", "atelier-viewer.exe")
            archive.write(binaries / "atelier.exe", "atelier.exe")
            archive.write(ROOT / "README.md", "README.md")
        if installer:
            iscc = shutil.which("ISCC.exe") or str(Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Inno Setup 6/ISCC.exe")
            run(iscc, f"/DAppVersion={version}", f"/DBinaryDir={binaries}",
                f"/O{out}", "/Fatelier-setup-windows-x64", str(ROOT / "installer/atelier.iss"))
    checksums(out)
    print(f"Packaged Atelier {version}: {out}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("--skip-build", action="store_true", help="Package existing release binaries")
    parser.add_argument("--installer", action="store_true", help="Also build the Windows installer with Inno Setup 6")
    args = parser.parse_args()
    package(args.target, args.skip_build, args.installer)
