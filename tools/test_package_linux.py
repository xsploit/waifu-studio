import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("package_linux", Path(__file__).with_name("package_linux.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "package with spaces"
        self.root.mkdir()
        (self.root / "asset.txt").write_text("owned fixture")
        (self.root / "start.sh").write_text("#!/bin/sh\nexit 0\n")
        (self.root / "start.sh").chmod(0o755)
        data = {"format_version": 1, "private": True, "files": package.inventory(self.root), "binary_sha256": "fixture"}
        (self.root / package.MANIFEST).write_text(json.dumps(data))

    def test_roundtrip_archive_and_readonly_package(self):
        package.verify(self.root)
        archive = Path(self.temp.name) / "candidate.tar.gz"
        package.archive(self.root, archive)
        with tarfile.open(archive) as tar:
            tar.extractall(Path(self.temp.name) / "extracted", filter="data")
        moved = Path(self.temp.name) / "extracted" / self.root.name
        for path in moved.iterdir():
            path.chmod(0o555 if path.stat().st_mode & 0o111 else 0o444)
        package.verify(moved)

    def test_corrupt_missing_and_unexpected_files_are_detected(self):
        path = self.root / "asset.txt"
        original = path.read_bytes()
        path.write_text("corrupt")
        with self.assertRaises(ValueError):
            package.verify(self.root)
        path.unlink()
        with self.assertRaises(ValueError):
            package.verify(self.root)
        path.write_bytes(original)
        (self.root / "unexpected.txt").write_text("extra")
        with self.assertRaises(ValueError):
            package.verify(self.root)

    def test_links_and_hidden_files_cannot_enter_inventory(self):
        link = self.root / "outside"
        link.symlink_to(Path(self.temp.name))
        with self.assertRaises(ValueError):
            package.inventory(self.root)
        link.unlink()
        (self.root / ".env").write_text("fixture, no credential")
        with self.assertRaises(ValueError):
            package.inventory(self.root)

    def test_profile_files_cannot_enter_inventory(self):
        (self.root / "credentials.json").write_text("{}")
        with self.assertRaises(ValueError):
            package.inventory(self.root)

    def test_existing_packages_and_archives_are_preserved(self):
        with self.assertRaises(ValueError):
            package.build(Path(self.temp.name), Path(self.temp.name) / "missing", self.root)
        archive = Path(self.temp.name) / "keep.tar.gz"
        archive.write_bytes(b"existing")
        with self.assertRaises(ValueError):
            package.archive(self.root, archive)
        self.assertEqual(archive.read_bytes(), b"existing")

    def test_platform_speech_bundle_keeps_notices_and_excludes_other_runtime(self):
        repo = Path(self.temp.name) / "source"
        for directory in ["assets/silero/windows-x86_64", "assets/silero/linux-x86_64", "docs", "vendor"]:
            (repo / directory).mkdir(parents=True)
        for relative in ["LICENSE", "README.md", "REFERENCES.md", "FEATURE-STATUS.md", "VALIDATION.md", "binary",
                         "assets/silero/silero_vad.onnx", "assets/silero/SILERO-LICENSE.txt"]:
            (repo / relative).write_text("fixture")
        for platform, library in [("windows-x86_64", "onnxruntime.dll"), ("linux-x86_64", "libonnxruntime.so")]:
            for name in [library, "LICENSE", "provenance.json"]:
                (repo / "assets/silero" / platform / name).write_text("fixture")
        for platform, other in [("windows-x86_64", "linux-x86_64"), ("linux-x86_64", "windows-x86_64")]:
            output = Path(self.temp.name) / platform
            package.copy_runtime(repo, repo / "binary", output, platform)
            self.assertTrue((output / "assets/silero" / platform / "LICENSE").is_file())
            self.assertTrue((output / "assets/silero/silero_vad.onnx").is_file())
            self.assertFalse((output / "assets/silero" / other).exists())
        (repo / "assets/silero/silero_vad.onnx").unlink()
        with self.assertRaisesRegex(ValueError, "prepare_silero"):
            package.copy_runtime(repo, repo / "binary", Path(self.temp.name) / "incomplete")

    def test_preview_requires_version_and_source_commit(self):
        path = self.root / package.MANIFEST
        data = json.loads(path.read_text())
        data.update(private=False, release="v0.1.0-preview.1", source_commit="a" * 40)
        path.write_text(json.dumps(data))
        package.verify(self.root)
        data.pop("source_commit")
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            package.verify(self.root)
        data.update(source_commit="a" * 40, release="../../invalid")
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            package.verify(self.root)


if __name__ == "__main__":
    unittest.main()
