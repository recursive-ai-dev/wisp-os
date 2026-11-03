#!/usr/bin/env python3
"""Native NVMe persistence on disposable QEMU media, including 4-KiB LBAs.

No host disks are opened. The readonly boot ESP is separate from every
temporary NVMe image. Uses the embedded console font, without OCR packages.
Usage: python3 tests/nvme_test.py [--skip-build]
"""

import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile

from acceptance_test import Monitor
from ide_test import VM

ROOT = Path(__file__).resolve().parents[1]
REGION_BYTES = 513 * 512


class NvmeVM(VM):
    def __init__(self, out, name, disk, block_size=512, bridge=False, nsid=1):
        self.out = out / name
        self.out.mkdir()
        self.log = self.out / "debug.log"
        self.counts = {}
        firmware = next((p for p in (
            Path("/usr/share/OVMF/OVMF_CODE.fd"), Path("/usr/share/ovmf/OVMF.fd")
        ) if p.is_file()), None)
        if firmware is None:
            raise RuntimeError("OVMF firmware not found")
        cmd = [
            "qemu-system-x86_64", "-bios", str(firmware),
            "-drive", f"format=raw,file=fat:{out / 'esp'},if=none,id=stick,readonly=on",
            "-usb", "-device", "usb-storage,drive=stick,bootindex=1",
            "-drive", f"id=nvmdata,if=none,format=raw,file={disk}",
        ]
        controller = "nvme,id=nvmctrl,serial=WISP_NVME_TEST"
        if bridge:
            cmd += ["-device", "pci-bridge,id=nvmebridge,chassis_nr=1"]
            controller += ",bus=nvmebridge"
        cmd += [
            "-device", controller,
            "-device", f"nvme-ns,drive=nvmdata,nsid={nsid},logical_block_size={block_size},physical_block_size={block_size}",
            "-net", "none", "-m", "256M", "-vga", "std", "-display", "none",
            "-debugcon", f"file:{self.log}",
            "-monitor", f"unix:{self.out / 'monitor.sock'},server,nowait",
            "-serial", "none", "-no-reboot",
        ]
        self.stderr = (self.out / "qemu.log").open("w")
        self.process = subprocess.Popen(cmd, cwd=ROOT, stdout=self.stderr, stderr=subprocess.STDOUT)
        try:
            self.mon = Monitor(str(self.out / "monitor.sock"))
        except BaseException:
            self.process.terminate()
            self.process.wait(timeout=10)
            self.stderr.close()
            raise


def image(path):
    with path.open("xb") as stream:
        stream.truncate(64 * 1024 * 1024)
        # The last save sector occupies only the first512 bytes of a
        # native4-KiB block. RMW must preserve every neighboring sector.
        stream.seek(REGION_BYTES)
        stream.write(b"ADJACENT-NVME-DATA".ljust(7 * 512, b"!"))
        stream.seek(-512, 2)
        stream.write(b"END-OF-DISK".ljust(512, b"?"))


def persistence(out, block_size, bridge=False, nsid=1):
    prefix = f"nvme-{block_size}" + ("-bridge" if bridge else "")
    disk = out / f"{prefix}.img"
    image(disk)
    with disk.open("rb") as stream:
        stream.seek(REGION_BYTES)
        outside_before = hashlib.file_digest(stream, "sha256").hexdigest()
    with NvmeVM(out, prefix + "-write", disk, block_size, bridge, nsid) as vm:
        log = vm.wait("[shell] ready")
        assert f"namespace={nsid}" in log and f"block_bytes={block_size}" in log, log
        if bridge:
            assert "[nvme] controller 01:" in log, log
        vm.expect(["NVMe disk identified", "NVMe disk is blank"], "boot")
        vm.line("format")
        vm.expect(["NVMe namespace", "Type ERASE"], "format")
        vm.line("ERASE")
        vm.wait("[save_format] disk provisioned by explicit 'format' command")
        vm.line("save nvme_data survives-reboot")
        vm.expect("saved 'nvme_data'", "save")
        vm.open("nvme_probe")
        vm.type('print("NATIVE_NVME_OK");')
        vm.key("f2")
        vm.expect("Saved prog.nvme_probe", "ide-save")
        vm.close_editor()
    with NvmeVM(out, prefix + "-reboot", disk, block_size, bridge, nsid) as vm:
        vm.wait("[shell] ready")
        vm.expect("NVMe save disk mounted", "reboot")
        vm.line("load nvme_data")
        vm.expect("\nsurvives-reboot\n", "load")
        vm.line("run nvme_probe")
        vm.expect("\nNATIVE_NVME_OK\n", "run")
    with disk.open("rb") as stream:
        assert stream.read(9) == b"WISPSAVE1"
        stream.seek(REGION_BYTES)
        assert hashlib.file_digest(stream, "sha256").hexdigest() == outside_before
    print(f"PASS {prefix}: namespace{nsid}, format/save/IDE/reboot, neighboring sectors unchanged", flush=True)


def foreign(out, block_size):
    disk = out / f"foreign-{block_size}.img"
    image(disk)
    with disk.open("r+b") as stream:
        # A malformed GPT is sufficient to prove refusal: neither a
        # protective MBR nor an unrecognized nativeGPT may be formatted.
        if block_size == 512:
            stream.seek(510)
            stream.write(b"\x55\xaa")
        stream.seek(block_size)
        stream.write(b"EFI PART" + b"FOREIGN PARTITION TABLE".ljust(504, b"@"))
    before = hashlib.sha256(disk.read_bytes()).digest()
    with NvmeVM(out, f"foreign-{block_size}", disk, block_size) as vm:
        vm.wait("[shell] ready")
        vm.line("disk 0")
        vm.line("format")
        vm.expect("format: installer:" if block_size == 512 else "4Kn disks are read-only", "format-refused")
        vm.line("save probe must-not-write")
        vm.expect("not formatted", "save-refused")
    assert hashlib.sha256(disk.read_bytes()).digest() == before
    print(f"PASS native{block_size} foreign GPT remains byte-identical", flush=True)


def read_only_4k(out):
    disk = out / "nvme-4096-read-only.img"
    image(disk)
    before = hashlib.sha256(disk.read_bytes()).digest()
    with NvmeVM(out, "nvme-4096-read-only", disk, 4096) as vm:
        log = vm.wait("[shell] ready")
        assert "block_bytes=4096" in log, log
        vm.expect("NVMe disk identified", "identified")
        vm.line("disk 0")
        vm.line("format")
        vm.expect("4Kn disks are read-only", "format-refused")
        vm.line("save probe must-not-write")
        vm.expect("not formatted", "save-refused")
    assert hashlib.sha256(disk.read_bytes()).digest() == before
    print("PASS native4096: discovery/read probe, format/save refused, image unchanged", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if not args.skip_build:
        subprocess.run(["./build.sh"], cwd=ROOT, check=True)
    base = ROOT / "tests/out_ahci"
    base.mkdir(exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="nvme-", dir=base))
    # Keep firmware-visible files stable if another driver is being rebuilt.
    shutil.copytree(ROOT / "esp", out / "esp")
    print(f"NVMe test artifacts: {out}", flush=True)
    persistence(out, 512)
    read_only_4k(out)
    persistence(out, 512, bridge=True, nsid=7)
    foreign(out, 512)
    foreign(out, 4096)
    print("All NVMe acceptance checks passed.", flush=True)


if __name__ == "__main__":
    main()
