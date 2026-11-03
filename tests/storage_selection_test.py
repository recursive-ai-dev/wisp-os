#!/usr/bin/env python3
"""Choose between AHCI and USB stores without relying on enumeration order."""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

from usb_storage_test import VM, digest, image

ROOT = Path(__file__).resolve().parents[1]


def format_and_save(vm, number, value):
    vm.line(f"disk {number}")
    vm.expect(f"Selected disk {number}", f"select-{number}")
    vm.line("format")
    vm.expect("Type ERASE", f"confirm-{number}")
    vm.line("ERASE")
    vm.expect("save disk formatted", f"formatted-{number}")
    vm.line(f"save location {value}")
    vm.expect("saved 'location'", f"saved-{number}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if not args.skip_build:
        subprocess.run(["./build.sh"], cwd=ROOT, check=True)
    base = ROOT / "tests/out_usb"
    base.mkdir(exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="selection-", dir=base))
    esp = out / "esp"
    shutil.copytree(ROOT / "esp", esp)
    sata, usb = out / "sata.img", out / "usb.img"
    image(sata)
    image(usb)
    print(f"Artifacts: {out}", flush=True)
    with VM(out, "author", esp, usb_disk=usb, ahci_disk=sata) as vm:
        vm.boot()
        vm.line("save location SHOULD_NOT_WRITE")
        vm.expect("no save disk selected", "ambiguous-blank")
        vm.line("disks")
        vm.expect(["0: AHCI", "1: USB", "not formatted for WISP"], "inventory")
        format_and_save(vm, 0, "SATA_STORE")
        format_and_save(vm, 1, "USB_STORE")
        vm.line("disk 0")
        vm.line("load location")
        vm.expect("\nSATA_STORE\n", "switched-back")
    before = digest(sata), digest(usb)
    with VM(out, "reboot", esp, usb_disk=usb, ahci_disk=sata) as vm:
        vm.boot()
        vm.line("load location")
        vm.expect("no save disk selected", "ambiguous-mounted")
        for number, value in [(1, "USB_STORE"), (0, "SATA_STORE")]:
            vm.line(f"disk {number}")
            vm.line("load location")
            vm.expect(f"\n{value}\n", f"read-{number}")
    assert before == (digest(sata), digest(usb)), "boot/selection/read changed disk bytes"
    print("PASS ambiguous disks require selection; stores stay separate across reboot; selection and reads never write.", flush=True)


if __name__ == "__main__":
    main()
