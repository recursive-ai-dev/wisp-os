#!/usr/bin/env python3
"""Host checks for the ISO FAT generator; Python stdlib only, no VM or mount.

Run: python3 tests/iso_test.py
The live ISO's firmware boot and native installation are tested separately
by installer_test.py.
"""

import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("make_esp", ROOT / "tools" / "make_esp.py")
make_esp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(make_esp)


class FatReader:
    """Decode using BPB and on-disk directory records rather than generator constants."""

    def __init__(self, image):
        self.image = image
        self.sector = struct.unpack_from("<H", image, 11)[0]
        self.cluster_size = image[13] * self.sector
        self.fat_start = struct.unpack_from("<H", image, 14)[0] * self.sector
        self.fat_size = struct.unpack_from("<H", image, 22)[0] * self.sector
        self.root_start = self.fat_start + image[16] * self.fat_size
        self.root_size = struct.unpack_from("<H", image, 17)[0] * 32
        self.data_start = self.root_start + self.root_size

    def chain(self, cluster):
        data = bytearray()
        seen = set()
        while cluster >= 2 and cluster < 0xFFF8:
            if cluster in seen:
                raise AssertionError("loop in FAT chain")
            seen.add(cluster)
            start = self.data_start + (cluster - 2) * self.cluster_size
            data.extend(self.image[start:start + self.cluster_size])
            cluster = struct.unpack_from("<H", self.image, self.fat_start + cluster * 2)[0]
        return bytes(data)

    def directory(self, cluster=0):
        data = self.chain(cluster) if cluster else self.image[self.root_start:self.root_start + self.root_size]
        result = {}
        long_parts = {}
        for offset in range(0, len(data), 32):
            entry = data[offset:offset + 32]
            if not entry[0]:
                break
            if entry[11] == 0x0F:
                raw = entry[1:11] + entry[14:26] + entry[28:32]
                long_parts[entry[0] & 0x1F] = raw
                continue
            if entry[11] & 0x08:
                continue
            if long_parts:
                raw = b"".join(long_parts[index] for index in sorted(long_parts))
                units = struct.unpack("<" + "H" * (len(raw) // 2), raw)
                units = units[:units.index(0)] if 0 in units else units
                name = struct.pack("<" + "H" * len(units), *units).decode("utf-16le")
                long_parts = {}
            else:
                name = entry[:8].decode("ascii").strip()
                ext = entry[8:11].decode("ascii").strip()
                if ext:
                    name += "." + ext
            result[name] = (entry[11], struct.unpack_from("<H", entry, 26)[0],
                            struct.unpack_from("<I", entry, 28)[0])
        return result

    def file(self, *components):
        cluster = 0
        for component in components:
            attr, cluster, size = self.directory(cluster)[component]
        if attr & 0x10:
            raise AssertionError("expected file, got directory")
        return self.chain(cluster)[:size]


class BootImageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.source = Path(self.temporary.name) / "esp"
        (self.source / "EFI" / "BOOT").mkdir(parents=True)
        (self.source / "APPS").mkdir()
        self.binary = b"MZ" + bytes(range(256)) * 5
        (self.source / "EFI" / "BOOT" / "BOOTX64.EFI").write_bytes(self.binary)

    def test_boot_binary_and_long_names_round_trip(self):
        programs = {
            "hello.wisp": b'print("hello")\n',
            "adventure.wisp": bytes(range(256)) * 9,
            "stopwatch.wisp": b"",  # zero-size files must not allocate a chain
            "xor_brain.wisp": b"x" * 512,
        }
        # Sources are UTF-8; the long file spans multiple clusters.
        programs["adventure.wisp"] = b"# comment\n" * 260
        for name, payload in programs.items():
            (self.source / "APPS" / name).write_bytes(payload)
        image = make_esp.build_image(self.source)
        reader = FatReader(image)
        self.assertEqual(image[510:512], b"\x55\xaa")
        self.assertEqual(len(image), 16 * 1024 * 1024)
        self.assertEqual(reader.file("EFI", "BOOT", "BOOTX64.EFI"), self.binary)
        for name, payload in programs.items():
            self.assertEqual(reader.file("APPS", name), payload)
        self.assertEqual(image[reader.fat_start:reader.fat_start + reader.fat_size],
                         image[reader.fat_start + reader.fat_size:reader.root_start])
        self.assertEqual(image, make_esp.build_image(self.source))

    def test_directory_chains_and_alias_collisions(self):
        names = [f"long_name_{index:02d}.wisp" for index in range(40)]
        for name in names:
            (self.source / "APPS" / name).write_text(name)
        reader = FatReader(make_esp.build_image(self.source))
        apps_cluster = reader.directory()["APPS"][1]
        self.assertGreater(len(reader.chain(apps_cluster)), 512)
        for name in names:
            self.assertEqual(reader.file("APPS", name), name.encode())

    def test_bad_app_does_not_replace_existing_output(self):
        destination = Path(self.temporary.name) / "existing.img"
        destination.write_bytes(b"keep existing output")
        (self.source / "APPS" / "bad.wisp").write_bytes(b"\xff")
        with self.assertRaises(UnicodeError):
            make_esp.write_image(self.source, destination)
        self.assertEqual(destination.read_bytes(), b"keep existing output")

    def test_symlink_output_and_source_refused(self):
        destination = Path(self.temporary.name) / "output.img"
        destination.symlink_to(self.source / "APPS" / "missing")
        with self.assertRaises(ValueError):
            make_esp.write_image(self.source, destination)
        (self.source / "APPS" / "alias.wisp").symlink_to(self.source / "EFI" / "BOOT" / "BOOTX64.EFI")
        with self.assertRaises(ValueError):
            make_esp.build_image(self.source)

    def test_case_duplicate_and_snapshot_limit_refused(self):
        (self.source / "APPS" / "app.wisp").write_text("x")
        (self.source / "APPS" / "APP.WISP").write_text("y")
        with self.assertRaises(ValueError):
            make_esp.build_image(self.source)
        (self.source / "APPS" / "APP.WISP").unlink()
        (self.source / "APPS" / "app.wisp").write_bytes(b"x" * (256 * 1024 + 1))
        with self.assertRaises(ValueError):
            make_esp.build_image(self.source)


class RunnerTests(unittest.TestCase):
    """Inspect QEMU argv through a stub, without starting a VM or writing host disks."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.scratch = Path(self.temporary.name)
        self.environment = os.environ.copy()
        for variable in ("WISP_ISO", "WISP_BOOT_DISK", "WISP_INSTALL_DISK"):
            self.environment.pop(variable, None)
        self.environment["PATH"] = str(self.scratch) + os.pathsep + self.environment["PATH"]
        self.environment["WISP_TEST_ARGS"] = str(self.scratch / "args.json")
        qemu = self.scratch / "qemu-system-x86_64"
        qemu.write_text("#!/usr/bin/env python3\nimport json, os, pathlib, sys\n"
                        "pathlib.Path(os.environ['WISP_TEST_ARGS']).write_text(json.dumps(sys.argv[1:]))\n")
        qemu.chmod(0o755)

    def test_iso_uses_explicit_destination_without_recreating_it(self):
        iso = self.scratch / "live media.iso"
        disk = self.scratch / "destination,with spaces.img"
        iso.write_bytes(b"iso")
        disk.write_bytes(b"existing destination")
        self.environment.update(WISP_ISO=str(iso), WISP_INSTALL_DISK=str(disk))
        subprocess.run([str(ROOT / "run.sh")], env=self.environment, check=True, capture_output=True)
        args = json.loads((self.scratch / "args.json").read_text())
        self.assertEqual(args[args.index("-cdrom") + 1], str(iso))
        self.assertIn(f"id=savedisk,if=none,format=raw,file={str(disk).replace(',', ',,')}", args)
        self.assertEqual(disk.read_bytes(), b"existing destination")

    def test_installed_mode_has_no_live_medium_or_second_disk(self):
        disk = self.scratch / "installed.img"
        disk.write_bytes(b"installed")
        self.environment["WISP_BOOT_DISK"] = str(disk)
        subprocess.run([str(ROOT / "run.sh")], env=self.environment, check=True, capture_output=True)
        args = json.loads((self.scratch / "args.json").read_text())
        self.assertNotIn("-cdrom", args)
        self.assertFalse(any("fat:esp" in arg or "savedisk" in arg for arg in args))
        self.assertIn(f"id=system,if=none,format=raw,file={disk}", args)

    def test_symlink_disk_is_refused_before_qemu(self):
        target = self.scratch / "target.img"
        target.write_bytes(b"untouched")
        disk = self.scratch / "link.img"
        disk.symlink_to(target)
        self.environment["WISP_BOOT_DISK"] = str(disk)
        run = subprocess.run([str(ROOT / "run.sh")], env=self.environment, capture_output=True)
        self.assertNotEqual(run.returncode, 0)
        self.assertFalse((self.scratch / "args.json").exists())
        self.assertEqual(target.read_bytes(), b"untouched")


if __name__ == "__main__":
    unittest.main()
