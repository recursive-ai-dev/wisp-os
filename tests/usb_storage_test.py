#!/usr/bin/env python3
"""Native xHCI BOT storage, keyboard coexistence, boundaries and persistence.

Every writable device is a disposable regular image under tests/out_usb.
The host's real disks are never attached. Uses QEMU/OVMF and the embedded
font decoder. Usage: python3 tests/usb_storage_test.py [--skip-build]
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from acceptance_test import Monitor
from ide_test import VM as EditorVM

ROOT = Path(__file__).resolve().parents[1]
SAVE_START = 133120
SAVE_END = SAVE_START + 513


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def image(path, size=64):
    with path.open("xb") as stream:
        stream.truncate(size * 1024 * 1024)


class VM(EditorVM):
    def __init__(self, out, name, esp, usb_disk=None, ahci_disk=None,
                 boot_usb=False, readonly_source=False, block_size=512, readonly_disk=False):
        self.out = out / name
        self.out.mkdir()
        self.log = self.out / "debug.log"
        self.counts = {}
        firmware = next((p for p in (
            Path("/usr/share/OVMF/OVMF_CODE.fd"),
            Path("/usr/share/ovmf/OVMF.fd"),
        ) if p.exists()), None)
        if firmware is None:
            raise RuntimeError("OVMF firmware not found")
        cmd = [
            "qemu-system-x86_64", "-bios", str(firmware),
            "-device", "qemu-xhci,id=xhci", "-device", "usb-kbd,bus=xhci.0",
            "-net", "none", "-m", "256M", "-vga", "std", "-display", "none",
            "-debugcon", f"file:{self.log}",
            "-monitor", f"unix:{self.out / 'monitor.sock'},server,nowait",
            "-serial", "none", "-no-reboot",
        ]
        if not boot_usb:
            cmd += ["-drive", f"format=raw,file=fat:{esp},if=none,id=source,readonly=on"]
            if readonly_source:
                cmd += ["-device", "usb-storage,drive=source,bus=xhci.0,bootindex=1"]
            else:
                cmd += ["-usb", "-device", "usb-storage,drive=source,bus=usb-bus.0"]
        if usb_disk:
            cmd += [
                "-drive", f"id=usbdisk,if=none,format=raw,file={usb_disk}" + (",readonly=on" if readonly_disk else ""),
                "-device", f"usb-storage,drive=usbdisk,bus=xhci.0,logical_block_size={block_size},physical_block_size={block_size}",
            ]
        if ahci_disk:
            cmd += [
                "-drive", f"id=satadisk,if=none,format=raw,file={ahci_disk}",
                "-device", "ahci,id=ahci0",
                "-device", "ide-hd,drive=satadisk,bus=ahci0.0",
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

    def boot(self):
        self.wait("[shell] ready")
        self.wait("[usb_hid] USB keyboard installed as an input source")
        self.expect("USB disk identified:", "boot")


def installed_image(out, esp):
    """Generate the real installer's GPT/FAT/save layout into a regular file."""
    source = out / "layout-fixture.rs"
    source.write_text(r'''
#![allow(dead_code)]
extern crate alloc;
#[path = DISK_LAYOUT]
mod disk_layout;
use std::io::{Seek, SeekFrom, Write};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = std::path::Path::new(&args[1]);
    let mut payloads = vec![("BOOTX64.EFI".to_string(),
        std::fs::read(root.join("EFI/BOOT/BOOTX64.EFI")).unwrap())];
    for entry in std::fs::read_dir(root.join("APPS")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "wisp") {
            payloads.push((path.file_name().unwrap().to_str().unwrap().to_string(),
                std::fs::read(path).unwrap()));
        }
    }
    let files: Vec<disk_layout::InstallFile> = payloads.iter()
        .map(|(name, data)| disk_layout::InstallFile { name, data }).collect();
    let mut disk = std::fs::OpenOptions::new().create_new(true).write(true)
        .open(&args[2]).unwrap();
    disk.set_len(128 * 1024 * 1024).unwrap();
    disk_layout::install(128 * 1024 * 1024 / 512, &files, [0x27; 16],
        |lba, sector| {
            disk.seek(SeekFrom::Start(lba * 512)).map_err(|_| "fixture seek")?;
            disk.write_all(sector).map_err(|_| "fixture write")
        }).unwrap();
    disk.sync_all().unwrap();
}
'''.replace("DISK_LAYOUT", json.dumps(str(ROOT / "src/disk_layout.rs"))))
    binary = out / "layout-fixture"
    subprocess.run(["rustc", "--edition", "2021", "-O", str(source), "-o", str(binary)],
                   cwd=ROOT, check=True)
    disk = out / "installed-usb.img"
    subprocess.run([str(binary), str(esp), str(disk)], check=True)
    return disk


def blank_and_reboot(out, esp):
    disk = out / "save.img"
    image(disk)
    with VM(out, "blank", esp, disk) as vm:
        vm.boot()
        vm.wait("[usb-storage] disk 0")
        vm.expect("USB disk is blank", "blank")
        vm.line("format")
        vm.expect(["USB mass storage 0", "Type ERASE"], "format-confirm")
        vm.line("ERASE")
        vm.wait("[save_format] disk provisioned by explicit 'format' command")
        vm.line("save usbproof USB_PERSISTED")
        vm.expect("saved 'usbproof'", "saved")
        vm.open("usb_app")
        vm.type('print("USB_IDE_OK");')
        vm.key("f2")
        vm.expect("Saved prog.usb_app", "ide-saved")
        vm.close_editor()
        vm.wait("[usb_hid] first keystroke received over USB")
    with VM(out, "reboot", esp, disk) as vm:
        vm.boot()
        vm.expect("USB save disk mounted", "mounted")
        vm.line("load usbproof")
        vm.expect("\nUSB_PERSISTED\n", "persistent-data")
        vm.line("run usb_app")
        vm.expect("\nUSB_IDE_OK\n", "persistent-source")
    print("PASS USB BOT format/save/IDE/reboot with a USB keyboard", flush=True)


def readonly_io_failure(out, esp):
    disk = out / "readonly-save.img"
    shutil.copyfile(out / "save.img", disk)
    before = digest(disk)
    with VM(out, "readonly-disk", esp, disk, readonly_disk=True) as vm:
        vm.boot()
        vm.expect("USB save disk mounted", "readonly-mounted")
        vm.line("save rejected MUST_NOT_PERSIST")
        vm.expect("save: usb-storage:", "readonly-write-refused")
        vm.line("echo KEYBOARD_STILL_OK")
        vm.expect("\nKEYBOARD_STILL_OK\n", "keyboard-after-error")
    assert digest(disk) == before, "write-protected USB image changed"
    print("PASS USB write errors preserve media and keep keyboard input usable", flush=True)


def foreign_and_4kn(out, esp):
    disk = out / "foreign.img"
    image(disk)
    with disk.open("r+b") as stream:
        mbr = bytearray(b"!" * 512)
        mbr[446:510] = bytes(64)
        mbr[450] = 0x83
        mbr[454:458] = (2048).to_bytes(4, "little")
        mbr[458:462] = (4096).to_bytes(4, "little")
        mbr[510:512] = b"\x55\xaa"
        stream.write(mbr)
    before = digest(disk)
    with VM(out, "foreign", esp, disk) as vm:
        vm.boot()
        vm.line("disk 0")
        vm.line("format")
        vm.expect("format refused", "foreign-refused")
    assert digest(disk) == before, "foreign USB media changed"
    disk = out / "4kn.img"
    image(disk)
    before = digest(disk)
    with VM(out, "4kn", esp, disk, block_size=4096) as vm:
        vm.boot()
        vm.wait("logical_block_size=4096")
        vm.line("disk 0")
        vm.line("format")
        vm.expect("4Kn disks are read-only", "4kn-refused")
    assert digest(disk) == before, "4Kn USB media changed"
    print("PASS foreign media and native 4Kn media remain byte-identical", flush=True)


def readonly_source(out, esp):
    disk = out / "sata-save.img"
    image(disk)
    with VM(out, "readonly-source", esp, ahci_disk=disk, readonly_source=True) as vm:
        vm.boot()
        vm.line("disks")
        vm.expect(["AHCI port 0", "USB mass storage 0"], "disks")
        vm.line("format")
        vm.expect(["AHCI port 0", "Type ERASE"], "sata-selected")
        vm.line("ERASE")
        vm.wait("[save_format] disk provisioned by explicit 'format' command")
        vm.line("save separate SATA_OK")
        vm.expect("saved 'separate'", "sata-saved")
    print("PASS readonly USB boot source coexists with a separate AHCI save disk", flush=True)


def boot_and_save_same_usb(out, esp):
    disk = installed_image(out, esp)
    with VM(out, "installed-usb", esp, disk, boot_usb=True) as vm:
        vm.boot()
        vm.expect("USB save disk mounted", "installed-mounted")
        before = disk.read_bytes()
        vm.line("save sameusb SAME_USB_OK")
        vm.expect("saved 'sameusb'", "same-usb-save")
    after = disk.read_bytes()
    assert before[:SAVE_START * 512] == after[:SAVE_START * 512], "write escaped save partition"
    assert before[SAVE_END * 512:] == after[SAVE_END * 512:], "write escaped save region"
    with VM(out, "installed-usb-reboot", esp, disk, boot_usb=True) as vm:
        vm.boot()
        vm.line("load sameusb")
        vm.expect("\nSAME_USB_OK\n", "same-usb-reboot")
    print("PASS USB-only GPT boot and partition-bounded saves survive reboot", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if not args.skip_build:
        subprocess.run(["./build.sh"], cwd=ROOT, check=True)
    base = ROOT / "tests/out_usb"
    base.mkdir(exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="storage-", dir=base))
    esp = out / "esp"
    shutil.copytree(ROOT / "esp", esp)
    print(f"==> USB storage artifacts: {out}", flush=True)
    blank_and_reboot(out, esp)
    readonly_io_failure(out, esp)
    foreign_and_4kn(out, esp)
    readonly_source(out, esp)
    boot_and_save_same_usb(out, esp)
    print("All USB storage scenarios passed.", flush=True)


if __name__ == "__main__":
    main()
