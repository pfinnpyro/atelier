"""Packaging smoke tests; no Rust build or platform tools required.
Run: python -m unittest discover -s tools -p test_package_release.py
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

SPEC = importlib.util.spec_from_file_location("package_release", Path(__file__).with_name("package_release.py"))
packaging = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packaging)


class LinuxPackagingTests(unittest.TestCase):
    def test_archives_and_checksums_for_both_architectures(self):
        for target in ("linux-x64", "linux-arm64"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary_dir = root / "custom-target" / packaging.TARGETS[target] / "release"
                binary_dir.mkdir(parents=True)
                for name in ("atelier-viewer", "atelier"):
                    (binary_dir / name).write_bytes((target + name).encode())
                (root / "README.md").write_text("Linux packaging fixture", encoding="utf-8")
                icon = root / "crates/atelier-app/assets/atelier.png"
                icon.parent.mkdir(parents=True)
                icon.write_bytes(b"test icon")
                helper = root / "tools/linux_install.py"
                helper.parent.mkdir(parents=True)
                helper.write_text("# installer fixture", encoding="utf-8")
                metainfo = root / "installer/linux/org.atelier.Viewer.metainfo.xml"
                metainfo.parent.mkdir(parents=True)
                metainfo.write_text("<component/>", encoding="utf-8")
                metadata = json.dumps({"packages": [{"name": "atelier-app", "version": "1.2.3"}]}).encode()
                with patch.object(packaging, "validate_linux_binary"), patch.object(packaging, "ROOT", root), patch.object(packaging.platform, "system", return_value="Linux"), patch.object(packaging.subprocess, "check_output", return_value=metadata), patch.object(packaging, "run") as build, patch.dict(os.environ, {"CARGO_TARGET_DIR": "custom-target"}):
                    packaging.package(target)
                    build.assert_called_once_with("cargo", "build", "--locked", "--release", "--target", packaging.TARGETS[target], "-p", "atelier-app", "-p", "atelier-cli")
                    folder = root / "dist/1.2.3" / target
                    archive_path = folder / f"atelier-{target}.tar.gz"
                    with tarfile.open(archive_path) as archive:
                        expected = {"atelier-viewer", "atelier", "README.md", "atelier.png", "install.py", "package.json", "org.atelier.Viewer.metainfo.xml"}
                        self.assertEqual(set(archive.getnames()), {f"atelier-{target}/{n}" for n in expected})
                        for member in archive.getmembers():
                            name = Path(member.name).name
                            self.assertEqual(member.mode, 0o755 if name in ("atelier-viewer", "atelier") else 0o644)
                            if name in ("atelier-viewer", "atelier"):
                                self.assertEqual(archive.extractfile(member).read(), (target + name).encode())
                    checksum = (folder / "SHA256SUMS.txt").read_text().splitlines()
                    self.assertEqual(checksum, [hashlib.sha256(archive_path.read_bytes()).hexdigest() + "  " + archive_path.name])
                    # Repack existing binaries and verify stale output is removed.
                    (folder / "stale").write_text("old", encoding="utf-8")
                    build.reset_mock()
                    packaging.package(target, skip_build=True)
                    build.assert_not_called()
                    self.assertFalse((folder / "stale").exists())

    def test_linux_host_and_installer_checks_happen_before_build(self):
        with patch.object(packaging.platform, "system", return_value="Windows"), patch.object(packaging.subprocess, "check_output") as metadata:
            with self.assertRaisesRegex(SystemExit, "requires Linux"):
                packaging.package("linux-x64")
            metadata.assert_not_called()
        with self.assertRaisesRegex(SystemExit, "only supported for windows"):
            packaging.package("linux-arm64", installer=True)

    def test_elf_architecture_and_glibc_baseline_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "viewer"
            header = bytearray(20)
            header[:6] = b"\x7fELF\x02\x01"
            header[18:20] = (62).to_bytes(2, "little")
            binary.write_bytes(header)
            with patch.object(packaging.subprocess, "check_output", return_value="Name: GLIBC_2.34"):
                packaging.validate_linux_binary(binary, "linux-x64")
                with self.assertRaisesRegex(SystemExit, "architecture"):
                    packaging.validate_linux_binary(binary, "linux-arm64")
            with patch.object(packaging.subprocess, "check_output", return_value="Name: GLIBC_2.38"):
                with self.assertRaisesRegex(SystemExit, "baseline"):
                    packaging.validate_linux_binary(binary, "linux-x64")

    def test_rustrover_linux_commands(self):
        root = Path(__file__).resolve().parents[1]
        for name, targets in (
            ("Package Linux x64", ["linux-x64"]),
            ("Package Linux ARM64", ["linux-arm64"]),
            ("Package Linux x64 and ARM (Linux host)", ["linux-x64", "linux-arm64"]),
        ):
            config = ET.parse(root / ".run" / (name + ".run.xml")).getroot().find("configuration")
            self.assertEqual(config.attrib["name"], name)
            options = {item.attrib["name"]: item.attrib["value"] for item in config.findall("option")}
            self.assertEqual(options["SCRIPT_TEXT"], " && ".join("python3 tools/package_release.py " + target for target in targets))
            self.assertEqual(options["INTERPRETER_PATH"], "/bin/bash")
            self.assertEqual(options["SCRIPT_WORKING_DIRECTORY"], "$PROJECT_DIR$")
        combined = ET.parse(root / ".run/Package Linux x64 and ARM.run.xml").getroot().find("configuration")
        options = {item.attrib["name"]: item.attrib["value"] for item in combined.findall("option")}
        self.assertEqual(options["SCRIPT_TEXT"], "python tools/package_linux.py both")
        self.assertEqual(options["INTERPRETER_PATH"], "powershell.exe")


if __name__ == "__main__":
    unittest.main()
