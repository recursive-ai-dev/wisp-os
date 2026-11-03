"""Reproducible ISO tests with disposable source/output; no VM or host disks."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("xorriso"), "xorriso is required for ISO mastering")
class IsoReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in ("scripts/make_iso.sh", "tools/make_esp.py"):
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, destination)
        (self.root / "esp/EFI/BOOT").mkdir(parents=True)
        (self.root / "esp/EFI/BOOT/BOOTX64.EFI").write_bytes(b"MZ" + b"test EFI payload" * 512)
        (self.root / "Cargo.toml").write_text('[package]\nversion = "0.0.0"\n')
        for name in ("README.md", "USB_INSTALL.md"):
            (self.root / name).write_text("Reproducibility test source\n")

    def build(self, output, epoch="1700000000"):
        environment = os.environ.copy()
        environment["SOURCE_DATE_EPOCH"] = epoch
        return subprocess.run([str(self.root / "scripts/make_iso.sh"), "--skip-build", "--output", str(output)],
                              env=environment, capture_output=True, text=True)

    def test_fixed_epoch_repeats_exactly_and_checksum_matches(self):
        first, second = self.root / "first.iso", self.root / "second.iso"
        result = self.build(first)
        self.assertEqual(result.returncode, 0, result.stderr)
        # Mastering time and input mtimes must not enter the image.
        time.sleep(1.1)
        os.utime(self.root / "README.md", (1800000000, 1800000000))
        (self.root / "README.md").chmod(0o600)
        result = self.build(second)
        self.assertEqual(result.returncode, 0, result.stderr)
        digest = hashlib.sha256(first.read_bytes()).hexdigest()
        self.assertEqual(hashlib.sha256(second.read_bytes()).hexdigest(), digest)
        self.assertEqual(Path(str(first) + ".sha256").read_text(), f"{digest}  first.iso\n")
        inspect = subprocess.run(["xorriso", "-indev", str(first), "-report_el_torito", "plain"],
                                 capture_output=True, text=True)
        self.assertEqual(inspect.returncode, 0, inspect.stderr)
        self.assertIn("UEFI", inspect.stdout + inspect.stderr)

    def test_bad_epoch_preserves_existing_iso_and_checksum(self):
        output = self.root / "existing.iso"
        checksum = Path(str(output) + ".sha256")
        for epoch in ("-1", "garbage", "1.5", "9" * 100, "4102444800"):
            with self.subTest(epoch=epoch):
                output.write_bytes(b"existing ISO")
                checksum.write_bytes(b"existing checksum")
                result = self.build(output, epoch)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("SOURCE_DATE_EPOCH", result.stderr)
                self.assertEqual(output.read_bytes(), b"existing ISO")
                self.assertEqual(checksum.read_bytes(), b"existing checksum")


if __name__ == "__main__":
    unittest.main()
