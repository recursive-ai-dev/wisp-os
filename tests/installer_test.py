#!/usr/bin/env python3
"""Live ISO -> confirmed install -> standalone boot -> persistent saves.

All disks are temporary regular image files under tests/out_installer.
Requires QEMU/OVMF and xorriso. Screens are decoded from Wisp's bitmap
font directly, with no OCR or Python imaging dependencies.
Usage: python3 tests/installer_test.py [--skip-build]
"""
import argparse
import hashlib
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import zlib

from acceptance_test import Monitor, wait_for_debug_marker
import console_capture

ROOT = Path(__file__).resolve().parents[1]
SECTOR = 512
SAVE_START = 133120


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class VM:
    def __init__(self, out, name, disk, iso=None):
        self.out = out / name
        self.out.mkdir()
        self.log = self.out / "debug.log"
        firmware = next((p for p in [Path("/usr/share/OVMF/OVMF_CODE.fd"), Path("/usr/share/ovmf/OVMF.fd")] if p.exists()), None)
        if firmware is None:
            raise RuntimeError("OVMF firmware not found")
        cmd = ["qemu-system-x86_64", "-bios", str(firmware),
               "-drive", f"id=target,if=none,format=raw,file={disk}",
               "-device", "ahci,id=ahci0", "-device", "ide-hd,drive=target,bus=ahci0.0",
               "-net", "none", "-m", "256M", "-vga", "std", "-display", "none",
               "-debugcon", f"file:{self.log}", "-monitor", f"unix:{self.out / 'monitor.sock'},server,nowait",
               "-serial", "none"]
        if iso:
            cmd += ["-cdrom", str(iso), "-boot", "order=d"]
        self.stderr = (self.out / "qemu.log").open("w")
        self.process = subprocess.Popen(cmd, cwd=ROOT, stdout=self.stderr, stderr=subprocess.STDOUT)
        try:
            self.mon = Monitor(str(self.out / "monitor.sock"))
        except BaseException:
            self.process.terminate()
            self.process.wait(timeout=10)
            self.stderr.close()
            raise

    def wait(self, marker, timeout=45):
        markers = [marker] if isinstance(marker, str) else marker
        failures = ["[installer] failed:", "[installer] source snapshot unavailable;"]
        log = wait_for_debug_marker(str(self.log), markers + failures, timeout)
        if not any(m in log for m in markers):
            raise AssertionError(f"installer failed before {markers}:\n{log}")
        return log

    def line(self, value):
        self.mon.type_text(value)
        self.mon.enter()

    def expect_screen(self, needle, name):
        deadline = time.monotonic() + 12
        last = ""
        while time.monotonic() < deadline:
            shot = self.capture(name)
            last = console_capture.text(shot)
            if needle in last:
                return
            time.sleep(0.3)
        raise AssertionError(f"screen does not contain {needle!r}: {last!r}")

    def capture(self, name):
        return console_capture.capture(self.mon, self.out / (name + ".ppm"))

    def close(self):
        try:
            self.mon.cmd("quit")
        except (OSError, TimeoutError):
            pass
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=10)
        self.mon.sock.close()
        self.stderr.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


def new_disk(path, mib):
    with path.open("xb") as stream:
        stream.truncate(mib * 1024 * 1024)
        # Nonzero sentinels expose unintended writes on cancelled installs.
        stream.seek(0)
        stream.write(b"DO NOT CHANGE THIS DISK".ljust(512, b"!"))
        stream.seek(-512, 2)
        stream.write(b"END SENTINEL".ljust(512, b"?"))


def validate_payload(disk, out):
    """Decode GPT and FAT independently, comparing every installed file."""
    with disk.open("rb") as stream:
        stream.seek(512)
        header = bytearray(stream.read(512))
        assert header[:8] == b"EFI PART"
        recorded = struct.unpack_from("<I", header, 16)[0]
        struct.pack_into("<I", header, 16, 0)
        assert zlib.crc32(header[:92]) == recorded
        table_lba, count, size, checksum = struct.unpack_from("<QIII", header, 72)
        stream.seek(table_lba * SECTOR)
        table = stream.read(count * size)
        assert zlib.crc32(table) == checksum
        esp_first, esp_last = struct.unpack_from("<QQ", table, 32)
        assert esp_first == 2048 and esp_last + 1 == SAVE_START
        assert struct.unpack_from("<Q", table, 128 + 32)[0] == SAVE_START
        stream.seek(esp_first * SECTOR)
        volume = stream.read((esp_last - esp_first + 1) * SECTOR)
        stream.seek(-512, 2)
        backup = bytearray(stream.read(512))
        backup_crc = struct.unpack_from("<I", backup, 16)[0]
        struct.pack_into("<I", backup, 16, 0)
        assert zlib.crc32(backup[:92]) == backup_crc
    reserved = struct.unpack_from("<H", volume, 14)[0]
    fats = volume[16]
    fat_size = struct.unpack_from("<I", volume, 36)[0]
    data_sector = reserved + fats * fat_size

    def chain(cluster):
        seen = set()
        result = bytearray()
        while 2 <= cluster < 0x0FFFFFF8:
            assert cluster not in seen, "FAT cycle"
            seen.add(cluster)
            start = (data_sector + cluster - 2) * SECTOR
            result += volume[start:start + SECTOR]
            cluster = struct.unpack_from("<I", volume, reserved * SECTOR + cluster * 4)[0] & 0x0FFFFFFF
        return bytes(result)

    def directory(cluster):
        entries, long_parts = {}, {}
        raw = chain(cluster)
        for offset in range(0, len(raw), 32):
            entry = raw[offset:offset + 32]
            if entry[0] == 0:
                break
            if entry[11] == 0x0F:
                units = [struct.unpack_from("<H", entry, i)[0] for i in (1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30)]
                long_parts[entry[0] & 0x1F] = "".join(chr(n) for n in units if n not in (0, 0xFFFF))
                continue
            if long_parts:
                name = "".join(long_parts[n] for n in sorted(long_parts))
                long_parts = {}
            else:
                base = entry[:8].decode("ascii").rstrip()
                ext = entry[8:11].decode("ascii").rstrip()
                name = base + ("." + ext if ext else "")
            first = struct.unpack_from("<H", entry, 26)[0] | (struct.unpack_from("<H", entry, 20)[0] << 16)
            length = struct.unpack_from("<I", entry, 28)[0]
            entries[name] = (first, length)
        return entries

    root = directory(2)
    efi = directory(root["EFI"][0])
    boot = directory(efi["BOOT"][0])
    cluster, length = boot["BOOTX64.EFI"]
    assert chain(cluster)[:length] == (ROOT / "esp/EFI/BOOT/BOOTX64.EFI").read_bytes()
    apps = directory(root["APPS"][0])
    sources = list((ROOT / "esp/APPS").glob("*.wisp"))
    assert set(apps) - {".", ".."} == {p.name for p in sources}
    for source in sources:
        cluster, length = apps[source.name]
        assert chain(cluster)[:length] == source.read_bytes(), source.name
    esp_image = out / "installed-esp.img"
    esp_image.write_bytes(volume)
    if shutil.which("fsck.fat"):
        result = subprocess.run(["fsck.fat", "-n", str(esp_image)], text=True, capture_output=True)
        (out / "fsck.log").write_text(result.stdout + result.stderr)
        assert result.returncode == 0, result.stdout + result.stderr


def boot_installer(vm):
    vm.wait("[installer] boot menu ready")
    vm.mon.cmd("sendkey i")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if not args.skip_build:
        subprocess.run(["./build.sh"], cwd=ROOT, check=True)
        subprocess.run(["./scripts/make_iso.sh", "--skip-build"], cwd=ROOT, check=True)
    isos = sorted((ROOT / "dist").glob("wisp-os-v*-live.iso"), key=lambda p: p.stat().st_mtime)
    if not isos:
        raise RuntimeError("No live ISO; run scripts/make_iso.sh")
    iso = isos[-1]
    base = ROOT / "tests/out_installer"
    base.mkdir(exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="run-", dir=base))
    print(f"==> Installer test artifacts: {out}", flush=True)
    disk = out / "system.img"
    new_disk(disk, 128)
    before = digest(disk)
    with VM(out, "cancel", disk, iso) as vm:
        boot_installer(vm)
        vm.wait("[installer] target prompt")
        vm.line("1")
        vm.wait("[installer] erase confirmation prompt")
        vm.line("ERASE 2")
        vm.wait("[installer] cancelled; no writes")
        vm.wait("[shell] ready")
    assert digest(disk) == before, "cancelled installer changed the target"
    print("PASS cancellation leaves target byte-identical", flush=True)

    small = out / "small.img"
    new_disk(small, 64)
    before_small = digest(small)
    with VM(out, "small", small, iso) as vm:
        boot_installer(vm)
        vm.wait("[installer] no eligible targets; no writes")
        vm.wait("[shell] ready")
    assert digest(small) == before_small
    print("PASS unsupported small target is untouched", flush=True)

    with VM(out, "install", disk, iso) as vm:
        boot_installer(vm)
        vm.wait("[installer] target prompt")
        vm.line("1")
        vm.wait("[installer] erase confirmation prompt")
        vm.line("ERASE 1")
        vm.wait("[installer] installation verified", timeout=90)
        vm.capture("installation-complete")
        vm.line("t")
        vm.wait("[shell] ready")
    validate_payload(disk, out)
    print("PASS live payload copied byte-for-byte; GPT/FAT validated", flush=True)

    # No CD/USB source is attached: prove standalone installed boot.
    with VM(out, "installed", disk) as vm:
        vm.wait("[installer] boot menu ready")
        vm.mon.cmd("sendkey t")
        vm.wait("[shell] ready")
        vm.wait("[save_format] existing save store mounted")
        vm.line("save installproof wisp-installed-ok")
        vm.expect_screen("saved", "saved")
        vm.line("run xor_brain")
        vm.expect_screen("XOR", "installed-app")
    with VM(out, "reboot", disk) as vm:
        # A writable boot disk must never appear among install targets.
        boot_installer(vm)
        vm.wait("[installer] no eligible targets; no writes")
        vm.wait("[shell] ready")
        vm.wait("[save_format] existing save store mounted")
        vm.line("load installproof")
        vm.expect_screen("wisp-installed-ok", "persistent-value")
    print("PASS standalone boot, app execution, source protection, save across reboot", flush=True)
    print(f"All installer scenarios passed. Artifacts: {out}")


if __name__ == "__main__":
    main()
