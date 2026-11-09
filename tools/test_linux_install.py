"""Native Linux lifecycle/security tests. Run with unittest discovery."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("linux_install", Path(__file__).with_name("linux_install.py"))
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)


@unittest.skipUnless(platform.system() == "Linux", "requires Linux symlinks and filesystem semantics")
class LinuxInstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / "data/atelier/app"
        self.root.mkdir(parents=True)
        self.target = "linux-arm64" if platform.machine().lower() in ("aarch64", "arm64") else "linux-x64"
        self.environment = patch.dict(os.environ, {"XDG_DATA_HOME": str(self.base / "data"), "XDG_CONFIG_HOME": str(self.base / "config")})
        self.environment.start(); self.addCleanup(self.environment.stop)
        self.which = patch.object(installer.shutil, "which", return_value=None)
        self.which.start(); self.addCleanup(self.which.stop)

    def source(self, version):
        source = self.base / version
        source.mkdir()
        for name in installer.FILES:
            (source / name).write_bytes((version + name).encode())
        (source / "package.json").write_text(json.dumps({"version": version, "target": self.target}))
        return source

    def test_install_update_rollback_and_uninstall_preserve_user_data(self):
        installer.install(self.source("1.2.3"), self.root)
        first = (self.root / "current/atelier-viewer").read_bytes()
        document = self.base / "document.pdf"
        document.write_bytes(b"user PDF")
        models = self.root.parent / "ocr-models"
        models.mkdir(); (models / "user-model").write_bytes(b"model")
        installer.install(self.source("1.2.4"), self.root)
        self.assertNotEqual((self.root / "current/atelier-viewer").read_bytes(), first)
        installer.rollback(self.root)
        self.assertEqual((self.root / "current/atelier-viewer").read_bytes(), first)
        installer.uninstall(self.root)
        self.assertFalse((self.root / "current").exists())
        self.assertFalse(installer.layout(self.root)["desktop"].exists())
        self.assertEqual(document.read_bytes(), b"user PDF")
        self.assertTrue((models / "user-model").exists())

    def test_failed_activation_restores_executable_metadata_and_state(self):
        installer.install(self.source("1.2.3"), self.root)
        previous = installer.state(self.root)
        original_switch = installer.switch
        def failure(root, version):
            if version.startswith("1.2.4"): raise OSError("simulated disk failure")
            original_switch(root, version)
        with patch.object(installer, "switch", side_effect=failure):
            with self.assertRaisesRegex(OSError, "disk failure"):
                installer.install(self.source("1.2.4"), self.root)
        self.assertEqual(installer.state(self.root), previous)
        self.assertFalse((self.root / "releases" / ("1.2.4-" + self.target)).exists())
        self.assertEqual(installer.layout(self.root)["icon"].read_bytes(), b"1.2.3atelier.png")

    def test_initial_activation_failure_can_be_retried(self):
        source = self.source("1.2.3")
        with patch.object(installer, "switch", side_effect=OSError("activation failed")):
            with self.assertRaisesRegex(OSError, "activation failed"):
                installer.install(source, self.root)
        installer.install(source, self.root)
        self.assertTrue((self.root / "current/atelier-viewer").is_file())

    def test_uninstall_refuses_unexpected_version_files(self):
        installer.install(self.source("1.2.3"), self.root)
        (self.root / "current/unexpected.pdf").write_bytes(b"retain")
        with self.assertRaisesRegex(ValueError, "modified version"):
            installer.uninstall(self.root)
        self.assertTrue(installer.layout(self.root)["desktop"].exists())
        self.assertTrue((self.root / "current/unexpected.pdf").exists())

    def test_unmanaged_root_and_external_metadata_are_not_overwritten(self):
        source = self.source("1.2.3")
        (self.root / "user-file").write_bytes(b"retain")
        with self.assertRaisesRegex(ValueError, "unmanaged"):
            installer.install(source, self.root)
        (self.root / "user-file").unlink()
        desktop = installer.layout(self.root)["desktop"]
        desktop.parent.mkdir(parents=True)
        desktop.write_text("someone else's entry")
        with self.assertRaisesRegex(ValueError, "existing desktop"):
            installer.install(source, self.root)
        self.assertEqual(desktop.read_text(), "someone else's entry")

    def archive_download(self, source, unsafe=False):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
            for name in installer.FILES:
                if unsafe and name == "atelier-viewer":
                    entry = tarfile.TarInfo("atelier-" + self.target + "/" + name)
                    entry.type = tarfile.SYMTYPE; entry.linkname = "/tmp/unsafe"
                    tar.addfile(entry)
                else: tar.add(source / name, arcname="atelier-" + self.target + "/" + name)
        data = buffer.getvalue()
        def download(url, destination, limit):
            if url.endswith(".tar.gz"): destination.write_bytes(data)
            else: destination.write_text(hashlib.sha256(data).hexdigest() + "  atelier-" + self.target + ".tar.gz\n")
        return download

    def test_update_requires_workflow_and_release_tag_attestation(self):
        installer.install(self.source("1.2.3"), self.root)
        source = self.source("1.2.4")
        with patch.object(installer, "download", side_effect=self.archive_download(source)), patch.object(installer, "command") as verify, patch.object(installer.shutil, "which", side_effect=lambda name: "/bin/gh" if name == "gh" else None):
            installer.update(self.root, "1.2.4")
            args = verify.call_args.args
            self.assertIn("--signer-workflow", args)
            self.assertIn(installer.REPO + "/.github/workflows/release.yml", args)
            self.assertIn("refs/tags/v1.2.4", args)
            self.assertIn("--deny-self-hosted-runners", args)
        self.assertTrue(installer.state(self.root)["current"].startswith("1.2.4"))

    def test_attestation_failure_and_linked_archive_leave_current_unchanged(self):
        installer.install(self.source("1.2.3"), self.root)
        source = self.source("1.2.4")
        previous = installer.state(self.root)
        with patch.object(installer, "download", side_effect=self.archive_download(source)), patch.object(installer.shutil, "which", return_value="gh"), patch.object(installer, "command", side_effect=subprocess.CalledProcessError(1, "gh")):
            with self.assertRaises(subprocess.CalledProcessError): installer.update(self.root, "1.2.4")
        with patch.object(installer, "download", side_effect=self.archive_download(source, unsafe=True)), patch.object(installer.shutil, "which", return_value="gh"), patch.object(installer, "command"):
            with self.assertRaisesRegex(ValueError, "Unsafe archive"):
                installer.update(self.root, "1.2.4")
        self.assertEqual(installer.state(self.root), previous)

    def test_reinstall_after_uninstall_and_preserve_reassigned_desktop(self):
        source = self.source("1.2.3")
        installer.install(source, self.root)
        installer.uninstall(self.root)
        installer.install(source, self.root)
        desktop = installer.layout(self.root)["desktop"]
        desktop.write_text("[Desktop Entry]\nExec=/other/atelier\n")
        installer.uninstall(self.root)
        self.assertIn("/other/atelier", desktop.read_text())
        self.assertTrue(installer.layout(self.root)["icon"].exists())

    def test_mime_cleanup_preserves_other_apps_and_comments(self):
        config = self.base / "config/mimeapps.list"
        config.parent.mkdir(parents=True)
        config.write_text("# retained comment\n[Default Applications]\napplication/pdf=org.atelier.Viewer.desktop;other.desktop;\ntext/plain=editor.desktop;\n")
        installer.remove_pdf_association()
        self.assertEqual(config.read_text(), "# retained comment\n[Default Applications]\napplication/pdf=other.desktop;\ntext/plain=editor.desktop;\n")

    def test_invalid_update_version_and_wrong_architecture_are_rejected(self):
        installer.install(self.source("1.2.3"), self.root)
        with self.assertRaises(ValueError): installer.update(self.root, "../../escape")
        source = self.source("1.2.4")
        metadata = json.loads((source / "package.json").read_text())
        metadata["target"] = "linux-arm64" if self.target == "linux-x64" else "linux-x64"
        (source / "package.json").write_text(json.dumps(metadata))
        with self.assertRaisesRegex(ValueError, "architecture"):
            installer.install(source, self.root)


class DesktopEscapingTests(unittest.TestCase):
    def test_exec_escaping_and_injection_rejection(self):
        self.assertIn('100%%', installer.desktop(Path('/opt/Atelier Viewer/100%/viewer')))
        self.assertIn('Icon=org.atelier.Viewer', installer.desktop(Path('/opt/viewer')))
        for value in ('/opt/evil\nHidden=true', '/opt/evil\0'):
            with self.assertRaises(ValueError): installer.desktop(Path(value))


if __name__ == "__main__": unittest.main()
