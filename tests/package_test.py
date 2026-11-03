#!/usr/bin/env python3
"""Release packaging checks using an isolated fixture and no compiler build."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
NAME = "wisp-os-v9.8.7"


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in [".gitignore", ".cargo/config.toml", "Cargo.toml", "Cargo.lock",
                     "rust-toolchain.toml", "build.sh", "run.sh", "README.md",
                     "CHANGES.md", "USB_INSTALL.md", "TODO.md", "src/main.rs",
                     "docs/RELEASE_NOTES.md", "docs/RUNTIME_README.md", "docs/MANUAL.md",
                     "docs/WISP_LANGUAGE.md", "docs/WISP_GRAPHICS.md", "docs/WISP_AI.md",
                     "docs/SPRINT_VERIFICATION.md", "tools/tool.py", "tests/sample.py",
                     "tests/host/Cargo.toml", "tests/host/src/lib.rs",
                     "tests/host/target/junk", "tools/__pycache__/junk.pyc",
                     "esp/APPS/hello.wisp", "esp/EFI/BOOT/BOOTX64.EFI"]:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture\n")
        (self.root / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "9.8.7"\n')
        shutil.copy2(ROOT / "rust-toolchain.toml", self.root / "rust-toolchain.toml")
        (self.root / "scripts").mkdir()
        for name in ["make_release.sh", "install_usb_linux.sh", "install_usb_macos.sh", "install_usb_windows.ps1"]:
            shutil.copy2(ROOT / "scripts" / name, self.root / "scripts" / name)

    def package(self, **changes):
        env = dict(os.environ, SOURCE_DATE_EPOCH="1700000000")
        env.update(changes)
        return subprocess.run(["bash", "scripts/make_release.sh", "--skip-build"],
                              cwd=self.root, env=env, text=True, capture_output=True)

    def hashes(self):
        return {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in (self.root / "dist").iterdir() if p.is_file()}

    def test_runtime_content_checksums_and_reproducibility(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        first = self.hashes()
        with zipfile.ZipFile(self.root / "dist" / (NAME + "-runtime.zip")) as archive:
            prefix = NAME + "-runtime/"
            names = {name.removeprefix(prefix) for name in archive.namelist()}
            self.assertIn("esp/EFI/BOOT/BOOTX64.EFI", names)
            self.assertIn("esp/APPS/hello.wisp", names)
            self.assertIn("BUILD_INFO.json", names)
            self.assertIn("scripts/install_usb_linux.sh", names)
            self.assertFalse(any(name.startswith(("src/", "tests/", "tools/", ".git")) for name in names))
            self.assertNotIn("Cargo.toml", names)
            self.assertNotIn("docs/SPRINT_VERIFICATION.md", names)
            for line in archive.read(prefix + "SHA256SUMS").decode().splitlines():
                expected, path = line.split("  ", 1)
                self.assertEqual(hashlib.sha256(archive.read(prefix + path)).hexdigest(), expected)
        with tarfile.open(self.root / "dist" / (NAME + ".tar.gz")) as archive:
            names = archive.getnames()
            self.assertIn(NAME + "/tests/host/src/lib.rs", names)
            self.assertFalse(any("/target/" in name or "__pycache__" in name for name in names))
        for line in (self.root / "dist/SHA256SUMS").read_text().splitlines():
            expected, name = line.split("  ", 1)
            self.assertEqual(first[name], expected)
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.hashes(), first)

    def test_generation_failure_preserves_prior_release(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        first = self.hashes()
        fake = self.root / "fake-bin"
        fake.mkdir()
        gzip = fake / "gzip"
        gzip.write_text("#!/bin/sh\nexit 19\n")
        gzip.chmod(0o755)
        result = self.package(PATH=str(fake) + os.pathsep + os.environ["PATH"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.hashes(), first)
        self.assertFalse(list((self.root / "dist").glob(".release-*")))

    def test_artifacts_use_snapshot_when_live_binary_changes_during_packaging(self):
        fake = self.root / "fake-bin"
        fake.mkdir()
        rustc = fake / "rustc"
        rustc.write_text('#!/bin/sh\nprintf changed > "$WISP_TEST_EFI"\nprintf fixture-rustc\n')
        rustc.chmod(0o755)
        result = self.package(PATH=str(fake) + os.pathsep + os.environ["PATH"],
                              WISP_TEST_EFI=str(self.root / "esp/EFI/BOOT/BOOTX64.EFI"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.root / "esp/EFI/BOOT/BOOTX64.EFI").read_bytes(), b"changed")
        raw = (self.root / "dist" / (NAME + "-BOOTX64.EFI")).read_bytes()
        self.assertEqual(raw, b"fixture\n")
        with zipfile.ZipFile(self.root / "dist" / (NAME + ".zip")) as archive:
            self.assertEqual(archive.read(NAME + "/esp/EFI/BOOT/BOOTX64.EFI"), raw)

    def test_invalid_epoch_preserves_prior_release(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        first = self.hashes()
        for epoch in ["-1", "garbage", "999999999999999999999999", "4102444800"]:
            result = self.package(SOURCE_DATE_EPOCH=epoch)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SOURCE_DATE_EPOCH", result.stderr)
            self.assertEqual(self.hashes(), first)


if __name__ == "__main__":
    unittest.main()
