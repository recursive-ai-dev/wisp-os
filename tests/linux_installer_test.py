#!/usr/bin/env python3
"""Test Linux installer rejection paths without accessing any host disk."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "install_usb_linux.sh"


class InstallerSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.marker = self.directory / "destructive-command"

    def run_script(self, body, kind="disk", status="0"):
        # Functions replace every command that could mutate a device.
        # All target names are nonexistent; no privileged test operations.
        program = '''source "$1"
require_root() { :; }
require_cmds() { :; }
lsblk() { printf '%s\\n' "$WISP_TEST_KIND"; return "$WISP_TEST_STATUS"; }
parted() { touch "$WISP_TEST_MARKER"; return 99; }
mkfs.vfat() { touch "$WISP_TEST_MARKER"; return 99; }
umount() { touch "$WISP_TEST_MARKER"; return 99; }
''' + body
        return subprocess.run(["bash", "-c", program, "test", str(SCRIPT)],
            env=dict(os.environ, WISP_TEST_KIND=kind, WISP_TEST_STATUS=status,
                     WISP_TEST_MARKER=str(self.marker)),
            text=True, capture_output=True, input="/dev/wisp-nonexistent\nERASE\n")

    def test_partition_mapper_loop_and_unknown_are_refused_even_with_force(self):
        for kind in ["part", "crypt", "lvm", "mpath", "loop", "", "disk\npart"]:
            with self.subTest(kind=kind):
                result = self.run_script("cmd_install /dev/wisp-nonexistent --force", kind)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("must be one whole disk", result.stderr)
                self.assertFalse(self.marker.exists())

    def test_failed_discovery_fails_closed(self):
        result = self.run_script("cmd_install /dev/wisp-nonexistent --force", "disk", "1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be one whole disk", result.stderr)
        self.assertFalse(self.marker.exists())

    def test_whole_disk_check_accepts_only_one_disk(self):
        result = self.run_script("require_whole_disk /dev/wisp-nonexistent; printf accepted")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "accepted")
        self.assertFalse(self.marker.exists())

    def test_valid_type_does_not_bypass_real_block_device_check(self):
        result = self.run_script("cmd_install /dev/wisp-nonexistent --force")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not a block device", result.stderr)
        self.assertFalse(self.marker.exists())

    def test_ambiguous_targets_and_missing_source_are_rejected(self):
        for args, message in [
            ("/dev/first /dev/second --force", "exactly one target"),
            ("/dev/first --source", "--source needs a directory"),
        ]:
            result = self.run_script("cmd_install " + args)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(message, result.stderr)
            self.assertFalse(self.marker.exists())

    def test_partition_names(self):
        result = self.run_script("partition_path /dev/sdz 1; partition_path /dev/nvme0n1 1; partition_path /dev/mmcblk0 2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["/dev/sdz1", "/dev/nvme0n1p1", "/dev/mmcblk0p2"])


if __name__ == "__main__":
    unittest.main()
