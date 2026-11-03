#!/usr/bin/env python3
"""Exercise Wisp Studio's edit/run/save loop on real QEMU input and storage.

Uses disposable disk images and a read-only boot ESP. Screens are decoded
with Wisp's embedded bitmap font; no OCR or Python imaging dependency.
Use --usb to exercise the USB HID keyboard instead of the default PS/2 path.
Usage: python3 tests/ide_test.py [--skip-build] [--usb]
"""

import argparse
from pathlib import Path
import subprocess
import tempfile
import time

from acceptance_test import Monitor, key_name_for
import console_capture


ROOT = Path(__file__).resolve().parents[1]


class VM:
    def __init__(self, out, name, disk, usb=False):
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
            "-drive", f"format=raw,file=fat:{ROOT / 'esp'},if=none,id=stick,readonly=on",
            "-usb", "-device", "usb-storage,drive=stick",
            "-drive", f"id=savedisk,if=none,format=raw,file={disk}",
            "-device", "ahci,id=ahci0", "-device", "ide-hd,drive=savedisk,bus=ahci0.0",
            "-net", "none", "-m", "256M", "-vga", "std", "-display", "none",
            "-debugcon", f"file:{self.log}",
            "-monitor", f"unix:{self.out / 'monitor.sock'},server,nowait",
            "-serial", "none", "-no-reboot",
        ]
        if usb:
            cmd += ["-device", "qemu-xhci,id=xhci", "-device", "usb-kbd,bus=xhci.0"]
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
        """Wait for the next occurrence, never accepting an earlier editor run."""
        target = self.counts.get(marker, 0) + 1
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            if self.log.exists():
                last = self.log.read_text(errors="replace")
                if last.count(marker) >= target:
                    self.counts[marker] = target
                    return last
            if self.process.poll() is not None:
                raise AssertionError(f"QEMU exited before {marker!r}:\n{last}")
            time.sleep(0.1)
        raise AssertionError(f"waiting for occurrence {target} of {marker!r}:\n{last}")

    def key(self, name, count=1):
        for _ in range(count):
            # Explicit release intervals matter for repeated USB key usages.
            self.mon.cmd(f"sendkey {name} 30")
            time.sleep(0.08)

    def type(self, text):
        for char in text:
            self.key("ret" if char == "\n" else key_name_for(char))

    def line(self, text):
        self.type(text)
        self.key("ret")

    def expect(self, needles, name, timeout=12):
        needles = [needles] if isinstance(needles, str) else needles
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            shot = console_capture.capture(self.mon, self.out / f"{name}.ppm")
            last = console_capture.text(shot)
            if all(needle in last for needle in needles):
                (self.out / f"{name}.txt").write_text(last)
                return last
            time.sleep(0.2)
        (self.out / f"{name}.txt").write_text(last)
        raise AssertionError(f"screen missing {needles!r}:\n{last}")

    def open(self, name, command="ide"):
        self.line(f"{command} {name}")
        self.wait("[ide] ready")

    def close_editor(self, discard=False):
        self.key("f10")
        if discard:
            self.expect("[D]iscard", "discard-prompt")
            self.key("d")
        self.wait("[ide] closed")

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


def boot(vm, usb):
    vm.wait("[shell] ready")
    if usb:
        vm.wait("[usb_hid] USB keyboard installed as an input source")
    vm.expect("Custom Kernel", "boot")


def edit_run_and_save(out, disk, usb):
    with VM(out, "author", disk, usb) as vm:
        boot(vm, usb)
        vm.open("demo")
        vm.type('print("IDE_REDY");')
        vm.key("home")
        vm.key("right", 13)
        vm.type("A")
        vm.key("end")
        vm.type('\nprint(2 + 3);')
        vm.expect(['print("IDE_READY");', "print(2 + 3);"], "edited-source")
        vm.key("f6")
        vm.expect("Syntax OK", "syntax-ok")
        vm.key("f5")
        vm.wait("[ide] run finished")
        vm.expect(["IDE_READY", "5", "Press any key to return to Wisp Studio."], "run-buffer")
        vm.key("ret")
        vm.key("f2")
        vm.expect("not formatted", "save-refused")
        vm.key("f10")
        vm.expect(["[S]ave", "[D]iscard", "Cancel"], "dirty-quit")
        vm.key("esc")
        vm.expect('print("IDE_READY");', "quit-cancelled")
        vm.close_editor(discard=True)
        print("PASS multiline editing, run, failed-save retention and dirty-quit cancellation", flush=True)

        vm.line("format")
        vm.expect("Type ERASE", "format-prompt")
        vm.line("ERASE")
        vm.wait("[save_format] disk provisioned by explicit 'format' command")
        vm.open("demo")
        vm.type('print("IDE_PERSISTED");\nprint(40 + 2);')
        vm.key("f2")
        vm.expect("Saved prog.demo", "saved")
        vm.close_editor()
        vm.line("run demo")
        vm.expect(["IDE_PERSISTED", "42"], "run-saved")
        vm.open("demo", command="edit")
        vm.expect(['print("IDE_PERSISTED");', "print(40 + 2);"], "edit-alias")
        vm.close_editor()
        if usb:
            vm.wait("[usb_hid] first keystroke received over USB")
        print("PASS formatted save, shell run and edit alias", flush=True)


def reboot_and_recover(out, disk, usb):
    with VM(out, "reboot", disk, usb) as vm:
        boot(vm, usb)
        vm.wait("[save_format] existing save store mounted")
        vm.line("run demo")
        vm.expect(["IDE_PERSISTED", "42"], "persistent-run")
        vm.open("demo")
        vm.expect(['print("IDE_PERSISTED");', "print(40 + 2);"], "persistent-edit")
        vm.key("f1")
        vm.expect(["WISP STUDIO / HELP", "Press any key"], "help")
        vm.key("ret")
        vm.key("f3")
        vm.expect(["OPEN OR CREATE", "hello.wisp  [bundled]", "demo.wisp  [saved]"], "browser")
        vm.line("hello")
        vm.expect(["WISP STUDIO / hello.wisp", "let name = input"], "open-example")
        vm.key("f6")
        vm.expect("Syntax OK", "example-syntax")
        vm.key("f3")
        vm.expect("OPEN OR CREATE", "browser-long-example")
        vm.line("undo")
        screen = vm.expect(["WISP STUDIO / undo.wisp", "Ln 1  Col 1"], "long-example")
        page_height = len(screen.splitlines()) - 6
        assert page_height > 0
        vm.key("pgdn")
        vm.expect(f"Ln {page_height + 1}  Col 1", "page-down")
        vm.key("pgup")
        vm.expect("Ln 1  Col 1", "page-up")
        vm.close_editor()
        print("PASS reboot persistence, help, browser examples and page navigation", flush=True)

        vm.open("broken")
        vm.type('print("before");\n\nprint(missing);')
        vm.key("home")
        vm.key("up", 2)
        vm.key("f5")
        vm.wait("[ide] run finished")
        vm.expect(["line 3", "missing", "Press any key"], "runtime-error")
        vm.key("ret")
        vm.expect(["print(missing);", "Ln 3  Col 1"], "error-return")
        vm.close_editor(discard=True)
        vm.open("syntax")
        vm.type('print("before");\n\nlet = 1;')
        vm.key("home")
        vm.key("up", 2)
        vm.key("f6")
        vm.expect(["wisp: line 3:", "Ln 3  Col 1", "let = 1;"], "syntax-error")
        vm.close_editor(discard=True)
        vm.open("invalidtarget")
        vm.type("items[0]() = 1;")
        vm.key("f6")
        vm.expect(["wisp: line 1:", "invalid assignment target", "Ln 1  Col 1"], "invalid-target-check")
        vm.key("f5")
        vm.wait("[ide] run finished")
        vm.expect(["invalid assignment target", "Press any key"], "invalid-target-run")
        vm.key("ret")
        vm.expect("items[0]() = 1;", "invalid-target-preserved")
        vm.close_editor(discard=True)
        vm.open("loop")
        vm.type("while true { }")
        vm.key("f5")
        time.sleep(0.3)
        vm.key("esc")
        vm.wait("[ide] run finished")
        vm.expect(["stopped (Esc pressed)", "Press any key"], "stopped")
        vm.key("ret")
        vm.expect("while true { }", "stopped-return")
        vm.close_editor(discard=True)
        print("PASS runtime-error recovery and Esc cancellation preserve source", flush=True)

        vm.line("wisp")
        vm.wait("[wisp] interactive prompt started")
        vm.line("let items = [4, 3]")
        vm.line("clear()")
        vm.line("items[0] + items[1] # trailing comment")
        screen = vm.expect(["trailing comment", "\n7\n", "wisp>"], "indexed-prompt-expression")
        assert any(row.strip() == "7" for row in screen.splitlines()), screen
        vm.line("items[0]() = 1;")
        vm.expect("invalid assignment target", "prompt-error")
        vm.line("items[0] + 6")
        screen = vm.expect(["items[0] + 6", "\n10\n"], "prompt-recovered")
        assert any(row.strip() == "10" for row in screen.splitlines()), screen
        vm.line("exit")
        vm.wait("[wisp] interactive prompt ended")
        print("PASS indexed expressions, trailing comments and parser-error recovery at the prompt", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--usb", action="store_true")
    args = parser.parse_args()
    if not args.skip_build:
        subprocess.run(["./build.sh"], cwd=ROOT, check=True)
    if not (ROOT / "esp/EFI/BOOT/BOOTX64.EFI").is_file():
        raise RuntimeError("Missing BOOTX64.EFI; run ./build.sh first")
    base = ROOT / "tests/out_ide"
    base.mkdir(exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="usb-" if args.usb else "ps2-", dir=base))
    disk = out / "save.img"
    with disk.open("xb") as stream:
        stream.truncate(64 * 1024 * 1024)
    print(f"==> IDE test artifacts: {out}", flush=True)
    edit_run_and_save(out, disk, args.usb)
    reboot_and_recover(out, disk, args.usb)
    print("All IDE acceptance checks passed.", flush=True)


if __name__ == "__main__":
    main()
