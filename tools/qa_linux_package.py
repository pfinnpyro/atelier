#!/usr/bin/env python3
"""Validate a locally built Linux archive and launch its viewer under Xvfb.
This is a native automated smoke check, not interactive desktop acceptance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile


def run(*args, **kwargs):
    return subprocess.run(args, check=True, timeout=120, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path)
    parser.add_argument("--target", required=True, choices=("linux-x64", "linux-arm64"))
    args = parser.parse_args()
    expected = "linux-arm64" if platform.machine() in ("aarch64", "arm64") else "linux-x64"
    if args.target != expected: parser.error("Run package smoke tests on matching hardware/emulation")
    archives = list(args.dist.glob("**/atelier-" + args.target + ".tar.gz"))
    if len(archives) != 1: parser.error("Expected exactly one archive for the selected target")
    archive = archives[0].resolve()
    lines = (archive.parent / "SHA256SUMS.txt").read_text().splitlines()
    expected_hash = hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name
    if expected_hash not in lines: raise RuntimeError("Package checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="atelier-linux-qa-") as temporary:
        root = Path(temporary)
        with tarfile.open(archive) as tar:
            for member in tar:
                if not member.isfile() or Path(member.name).parent.as_posix() != "atelier-" + args.target:
                    raise RuntimeError("Unexpected archive entry")
                target = root / Path(member.name).name
                with tar.extractfile(member) as source, target.open("wb") as out: shutil.copyfileobj(source, out)
                target.chmod(member.mode)
        package = json.loads((root / "package.json").read_text())
        if package["target"] != args.target: raise RuntimeError("Package identity mismatch")
        for binary in ("atelier", "atelier-viewer"):
            result = run("ldd", str(root / binary), capture_output=True, text=True)
            if "not found" in result.stdout: raise RuntimeError(result.stdout)
            print(result.stdout)
        env = dict(os.environ, XDG_DATA_HOME=str(root / "data"), XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"), LIBGL_ALWAYS_SOFTWARE="1", WINIT_UNIX_BACKEND="x11")
        settings = root / "config/atelier/settings.txt"
        settings.parent.mkdir(parents=True)
        settings.write_text("ocr = no\norient-on-open = no\ncheck-updates = no\n")
        install_root = root / "managed"
        run("python3", str(root / "install.py"), "--root", str(install_root), env=env)
        desktop = root / "data/applications/org.atelier.Viewer.desktop"
        run("desktop-file-validate", str(desktop))
        # The CLI corpus verifies native parsing/rendering before viewer startup.
        run("python3", "tools/qa_pdf_edges.py", str(root / "pdf-corpus"), "--binary", str(root / "atelier"), env=env)
        display_test = root / "viewer-smoke.py"
        display_test.write_text("""import subprocess,time,sys
from pathlib import Path
log = Path(sys.argv[3])
with log.open('wb') as output:
    p = subprocess.Popen([sys.argv[1], sys.argv[2]], stdout=output, stderr=output)
    try:
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if p.poll() is not None:
                raise RuntimeError(log.read_text(errors='replace'))
            tree = subprocess.run(['xwininfo','-root','-tree'], capture_output=True, text=True, timeout=5)
            if 'Atelier' in tree.stdout:
                time.sleep(2)
                if p.poll() is not None: raise RuntimeError(log.read_text(errors='replace'))
                break
            time.sleep(0.5)
        else:
            raise RuntimeError('Viewer did not create its X11 window: ' + log.read_text(errors='replace'))
    finally:
        p.terminate()
        try: p.wait(timeout=15)
        except subprocess.TimeoutExpired: p.kill(); p.wait()
""")
        run("xvfb-run", "-a", "dbus-run-session", "--", "python3", str(display_test), str(install_root / "current/atelier-viewer"), str(root / "pdf-corpus/valid.pdf"), str(root / "viewer.log"), env=env)
        run("python3", str(root / "install.py"), "--root", str(install_root), "--uninstall", env=env)
        if desktop.exists(): raise RuntimeError("Uninstall left its desktop entry")
    print("Linux package checksum, dependencies, CLI, desktop installation and viewer startup passed")


if __name__ == "__main__": main()
