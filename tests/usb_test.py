#!/usr/bin/env python3
"""Phase 3's permanent regression test for the ACPI/APIC/xHCI stack: the
interrupt topology this kernel now reads from firmware instead of
assuming, and the USB HID keyboard it can now read on a machine with no
PS/2 controller.

Follows `tests/fault_test.py`/`tests/ahci_test.py`'s isolated
build/ESP/target-dir pattern (never touches the project's real `./esp` or
`./target`) and reuses `tests/acceptance_test.py`'s `Monitor`/OCR
helpers.

Four independent scenarios, each its own fresh boot:

  1. `apic` -- the ordinary configuration, no USB devices. Proves the
     whole ACPI -> LAPIC -> I/O APIC path from the firmware's own
     tables: the MADT is parsed, the ISA IRQ0 -> GSI 2 Interrupt Source
     Override is *honored* (the single most load-bearing translation in
     the phase -- a kernel that assumed `gsi == irq` would route the
     timer to the wrong I/O APIC pin and never tick), the LAPIC timer
     calibrates and takes over, the 8259 is masked, and the shell is
     still fully interactive afterward with a live, advancing clock.
  2. `no_xhci` -- no xHCI controller present. Proves the "no USB
     controller" soft-fail path reports cleanly and leaves a working
     PS/2 keyboard, which is the configuration every other test in this
     project already runs in.
  3. `xhci_no_device` -- an xHCI controller with nothing plugged into
     it. Proves the "controller up, no keyboard" path completes within
     its bounded timeouts rather than hanging -- the same class of
     failure as the AHCI sti-ordering bug this project already found
     once (see CHANGES.md), which is why every wait loop in `xhci.rs`
     is deadline-bounded and why this scenario is permanent coverage
     rather than a one-off check.
  4. `usb_keyboard` -- an xHCI controller with a USB HID keyboard.
     Proves the full enumeration chain (Enable Slot -> Address Device ->
     descriptors -> Configure Endpoint -> SET_PROTOCOL) and, critically,
     that a keystroke actually arrives *through the USB path*: QEMU's
     `pc` machine always has an i8042 too, so a character appearing on
     screen alone would prove only that *some* keyboard worked. The
     kernel's own `[usb_hid] first keystroke received over USB` marker
     is what makes the claim unambiguous.

Usage: tests/usb_test.py [output_dir]
"""
import os
import re
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "out_usb")

BUILD_TARGET_DIR = os.path.join(OUT_DIR, "target")
ESP_DIR = os.path.join(OUT_DIR, "esp")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acceptance_test  # noqa: E402 -- screendump() writes under this module's OUT_DIR global
from acceptance_test import Monitor, wait_for_boot  # noqa: E402

BOOT_MARKERS = ["[pmm] self-test OK", "[boot] interrupts enabled"]


def build_test_binary():
    print("==> Building BOOTX64.efi (release, isolated target dir)")
    subprocess.run(
        [
            "cargo", "+nightly", "build", "--release",
            "--target", "x86_64-unknown-uefi",
            "--target-dir", BUILD_TARGET_DIR,
        ],
        cwd=ROOT,
        check=True,
    )
    efi_bin = os.path.join(BUILD_TARGET_DIR, "x86_64-unknown-uefi", "release", "BOOTX64.efi")
    if not os.path.isfile(efi_bin):
        raise RuntimeError(f"build succeeded but {efi_bin} was not produced")
    os.makedirs(os.path.join(ESP_DIR, "EFI", "BOOT"), exist_ok=True)
    shutil.copy2(efi_bin, os.path.join(ESP_DIR, "EFI", "BOOT", "BOOTX64.EFI"))
    # Only the one app this suite runs (primes.wisp, for the Esc-over-USB
    # check in scenario_usb_keyboard) -- nothing else is needed here, and a
    # minimal APPS keeps fs::snapshot_apps's boot-time walk cheap.
    apps_dir = os.path.join(ESP_DIR, "APPS")
    os.makedirs(apps_dir, exist_ok=True)
    shutil.copy2(os.path.join(ROOT, "esp", "APPS", "primes.wisp"), os.path.join(apps_dir, "primes.wisp"))
    print(f"==> Done: {os.path.join(ESP_DIR, 'EFI', 'BOOT', 'BOOTX64.EFI')}")


def ovmf_code_path() -> str:
    path = "/usr/share/OVMF/OVMF_CODE.fd"
    if not os.path.exists(path):
        path = "/usr/share/ovmf/OVMF.fd"
    return path


def build_qemu_cmd(debug_log: str, monitor_sock: str, xhci: bool, usb_keyboard: bool) -> list:
    cmd = [
        "qemu-system-x86_64",
        "-bios", ovmf_code_path(),
        "-drive", f"format=raw,file=fat:{ESP_DIR},if=none,id=stick,readonly=on",
        "-usb", "-device", "usb-storage,drive=stick",
    ]
    if xhci:
        # A second, separate USB controller from the UHCI bus the boot
        # stick hangs off, so the ESP's boot path stays byte-for-byte
        # what every other test in this project uses and only the new
        # surface is added.
        cmd += ["-device", "qemu-xhci,id=xhci"]
        if usb_keyboard:
            cmd += ["-device", "usb-kbd,bus=xhci.0"]
    cmd += [
        "-net", "none",
        "-m", "256M",
        "-vga", "std",
        "-display", "none",
        "-debugcon", f"file:{debug_log}",
        "-monitor", f"unix:{monitor_sock},server,nowait",
        "-serial", "none",
    ]
    return cmd


def assert_boot_markers_once(log: str, scenario: str):
    for marker in BOOT_MARKERS:
        count = log.count(marker)
        if count != 1:
            raise RuntimeError(
                f"[{scenario}] expected boot marker {marker!r} exactly once "
                f"in the debug log (found {count}) -- 0 means the kernel "
                f"never got this far (a hang or an early crash); 2+ means "
                f"the machine silently reset and rebooted.\n"
                f"Full debug log:\n{log}"
            )


def assert_log_contains(log: str, must_contain, scenario: str):
    if isinstance(must_contain, str):
        must_contain = [must_contain]
    missing = [s for s in must_contain if s not in log]
    if missing:
        raise RuntimeError(
            f"[{scenario}] debug-log verification failed (missing "
            f"{missing!r}).\nFull debug log:\n{log}"
        )


def assert_log_lacks(log: str, must_not_contain, scenario: str):
    if isinstance(must_not_contain, str):
        must_not_contain = [must_not_contain]
    present = [s for s in must_not_contain if s in log]
    if present:
        raise RuntimeError(
            f"[{scenario}] debug log unexpectedly contains {present!r}.\n"
            f"Full debug log:\n{log}"
        )


def run_scenario(scenario: str, xhci: bool, usb_keyboard: bool, interact=None) -> str:
    scenario_dir = os.path.join(OUT_DIR, scenario)
    os.makedirs(scenario_dir, exist_ok=True)
    debug_log = os.path.join(scenario_dir, "debug.log")
    monitor_sock = os.path.join(scenario_dir, "monitor.sock")

    qemu_cmd = build_qemu_cmd(debug_log, monitor_sock, xhci, usb_keyboard)
    print(f"==> [{scenario}] Launching:", " ".join(qemu_cmd))
    proc = subprocess.Popen(qemu_cmd, cwd=ROOT)
    try:
        time.sleep(1.0)
        # Monitor.screendump()/wait_for_boot() resolve OUT_DIR in
        # acceptance_test's own namespace, not this module's -- see
        # fault_test.py's identical monkey-patch and its explanation.
        acceptance_test.OUT_DIR = scenario_dir
        mon = Monitor(monitor_sock)
        wait_for_boot(mon, timeout=40.0)
        if interact is not None:
            interact(mon)
        mon.quit()
    finally:
        time.sleep(0.5)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    if not os.path.isfile(debug_log):
        raise RuntimeError(f"[{scenario}] no debug log produced at {debug_log}")
    with open(debug_log, "r", errors="replace") as f:
        return f.read()


def scenario_apic():
    """The interrupt-controller half of the phase, on its own."""
    scenario = "apic"

    def interact(mon):
        # A live, advancing tick proves the timer interrupt is really
        # being delivered -- through the I/O APIC and the LAPIC timer,
        # since by this point the 8259 is fully masked and the PIT's own
        # line has been masked at the I/O APIC too. And a responsive
        # `echo` proves the keyboard's IRQ1 is really being delivered
        # through its own redirection entry: both of the only two
        # interrupts this kernel routes, verified from the outside.
        mon.type_text("uptime")
        mon.enter()
        time.sleep(0.3)
        _shot, first = acceptance_test.read_uptime_ticks(
            mon, "01_uptime_first", f"{scenario} (uptime, first read)"
        )
        time.sleep(1.5)
        mon.type_text("uptime")
        mon.enter()
        time.sleep(0.3)
        _shot, second = acceptance_test.read_uptime_ticks(
            mon, "02_uptime_second", f"{scenario} (uptime, second read)"
        )
        if second <= first:
            raise RuntimeError(
                f"[{scenario}] tick count did not increase after 1.5s "
                f"({first} -> {second}) -- the timer interrupt is not "
                f"being delivered through the APIC."
            )
        # Deliberately *not* asserting a tick-rate bound here. The
        # obvious check -- "a 100 Hz clock should advance about 150
        # ticks across this 1.5 s sleep" -- was written first and
        # immediately produced a false failure at 518 ticks, because the
        # real elapsed guest time between two readings is the sleep
        # *plus* six monitor `sendkey` round trips plus a screendump plus
        # a tesseract OCR pass, and that total varies with host load by
        # more than a factor of two. Comparing a guest tick count against
        # an unmeasured host-side duration is simply not a sound test,
        # and loosening the bound until it stopped failing would have
        # left an assertion that no longer caught anything.
        #
        # The property that check was reaching for -- "the LAPIC timer
        # and the PIT's I/O APIC line are not both live, double-counting
        # every tick" -- has an exact, deterministic signal instead, and
        # it is asserted on the debug log below: the kernel logs the
        # masking of the now-redundant PIT line, and logs a specific
        # warning if it could not. That is checked rather than inferred
        # from timing. (Cross-checked independently while diagnosing the
        # false failure: the full acceptance suite's own two uptime
        # readings advance by 244 ticks before this phase and 245 after
        # it -- the same clock rate, through an entirely different
        # interrupt path.)
        print(f"    tick count advanced {second - first} ticks (clock is live)")

        # Every marker string this file types is deliberately free of
        # the letter "c": this font's lowercase "c" is a confirmed
        # tesseract misread as "e" (already on record in CHANGES.md, and
        # hit again here -- an "no_xhci_ok" marker OCR'd as "no_xhei_ok"
        # and failed a scenario whose kernel behavior was entirely
        # correct). Choosing strings the OCR pass cannot garble is a
        # better fix than loosening the assertion, because it keeps the
        # check exact.
        mon.type_text("echo irq_delivery_ok")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("03_echo"),
            "irq_delivery_ok",
            f"{scenario} (keyboard IRQ delivered through the I/O APIC)",
        )

    log = run_scenario(scenario, xhci=False, usb_keyboard=False, interact=interact)
    assert_boot_markers_once(log, scenario)
    assert_log_contains(
        log,
        [
            "[acpi] madt=",
            "[lapic] base=",
            "[lapic] timer calibration:",
            "[ioapic] id=",
            "[lapic] APIC mode active: 8259 fully masked",
            "[lapic] timer armed:",
        ],
        scenario,
    )

    # The MADT override this kernel must honor, and the routing that
    # proves it did. QEMU's firmware reports ISA IRQ0 on GSI 2; a kernel
    # that ignored the override would have programmed redirection entry
    # 0 instead of entry 2 and never seen a tick -- so asserting the
    # *routed* line, not just that an override was parsed, is what makes
    # this a real check.
    m = re.search(r"\[acpi\]\s+override isa_irq=0 -> gsi=(\d+)", log)
    if not m:
        raise RuntimeError(
            f"[{scenario}] expected the firmware to report an Interrupt "
            f"Source Override for ISA IRQ0 (it does on every PC this "
            f"project targets).\nFull debug log:\n{log}"
        )
    gsi = int(m.group(1))
    assert_log_contains(log, f"[ioapic] routed ISA IRQ0 -> GSI {gsi} ", scenario)
    assert_log_contains(log, "[ioapic] routed ISA IRQ1 -> GSI ", scenario)
    assert_log_contains(log, f"[ioapic] masked ISA IRQ0 (GSI {gsi}", scenario)

    # Nothing should have arrived on a vector this kernel never routed.
    assert_log_lacks(log, ["[idt] unexpected interrupt vector", "[idt] unexpected IRQ line"], scenario)

    # Exactly one timer source. `bring_up_apic` masks the PIT's line at
    # the I/O APIC once the LAPIC timer is armed, and logs a specific
    # warning if that masking fails -- so the absence of the warning,
    # together with the masking line already asserted above, is exact
    # proof that the tick counter has one incrementer rather than two.
    # This is the deterministic replacement for a tick-rate bound; see
    # the note in `interact` above for why the timing-based version was
    # not sound.
    assert_log_lacks(log, "[lapic] WARNING: PIT line still live", scenario)

    print(
        f"==> PASS [{scenario}]: ACPI -> LAPIC -> I/O APIC verified end to end "
        f"(ISA IRQ0 correctly followed its override onto GSI {gsi}; LAPIC timer "
        f"took over the tick; 8259 masked; keyboard still live)"
    )


def scenario_no_xhci():
    scenario = "no_xhci"

    def interact(mon):
        mon.type_text("echo usb_absent_ok")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("01_echo"),
            "usb_absent_ok",
            f"{scenario} (PS/2 keyboard still works with no USB controller)",
        )

    log = run_scenario(scenario, xhci=False, usb_keyboard=False, interact=interact)
    assert_boot_markers_once(log, scenario)
    assert_log_contains(log, "[xhci] no xHCI controller found on PCI bus 0", scenario)
    assert_log_lacks(log, "[usb_hid] USB keyboard installed", scenario)
    print(f"==> PASS [{scenario}]: no xHCI controller -- reported cleanly, PS/2 keyboard unaffected")


def scenario_xhci_no_device():
    scenario = "xhci_no_device"

    def interact(mon):
        mon.type_text("echo usb_empty_ok")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("01_echo"),
            "usb_empty_ok",
            f"{scenario} (shell still responsive with an empty xHCI controller)",
        )

    log = run_scenario(scenario, xhci=True, usb_keyboard=False, interact=interact)
    assert_boot_markers_once(log, scenario)
    assert_log_contains(
        log,
        ["[xhci] controller found at", "[xhci] controller reset and running"],
        scenario,
    )
    assert_log_lacks(log, "[usb_hid] USB keyboard installed", scenario)
    print(
        f"==> PASS [{scenario}]: xHCI controller reset and started with nothing "
        f"attached -- every port probe completed within its bounded timeout "
        f"and boot finished normally (hang regression guard)"
    )


def scenario_usb_keyboard():
    scenario = "usb_keyboard"

    def interact(mon):
        mon.type_text("echo usb_keyboard_ok")
        mon.enter()
        time.sleep(0.5)
        acceptance_test.assert_ocr(
            mon.screendump("01_echo"),
            "usb_keyboard_ok",
            f"{scenario} (typed text reached the shell)",
        )
        # v1.0: Esc over USB stops a running compute loop. The USB
        # keyboard has no interrupt -- this only works because the
        # interpreter's stop check polls the controller once per timer
        # tick (`ShellHost::stop_requested`), so it is the USB half of the
        # Esc feature specifically that this exercises.
        mon.type_text("run primes")
        mon.enter()
        time.sleep(1.0)
        mon.cmd("sendkey esc")
        time.sleep(0.5)
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("echo usb_esc_ok")
        mon.enter()
        time.sleep(0.5)
        acceptance_test.assert_ocr(
            mon.screendump("02_esc_stopped_primes"),
            "usb_esc_ok",
            f"{scenario} (prompt responsive after Esc over USB stopped a program)",
        )
        # The editing keys over USB (HID usages 0x4A-0x52, decoded to the
        # line editor's key codes): Left moves into the line, End returns,
        # Up recalls it. Repeats are spaced past QEMU's 100 ms key hold --
        # see Monitor.key.
        mon.type_text("echo usbedit")
        mon.key("left", 4, gap=0.25)
        mon.type_text("_")
        mon.key("end", gap=0.25)
        mon.type_text("_ok")
        mon.enter()
        time.sleep(0.5)
        mon.key("up", gap=0.25)
        mon.key("backspace", 2, gap=0.25)
        # Different letters, not a case change: tesseract reads "OK" as
        # "ok" in this font.
        mon.type_text("go")
        mon.enter()
        time.sleep(0.5)
        acceptance_test.assert_ocr(
            mon.screendump("03_usb_editing_keys"),
            ["usb_edit_ok", "usb_edit_go"],
            f"{scenario} (arrow/End editing and Up history over USB)",
        )
        # Give the 100 Hz poll a few more windows before the VM is torn
        # down, so the first-keystroke marker is definitely flushed.
        time.sleep(0.5)

    log = run_scenario(scenario, xhci=True, usb_keyboard=True, interact=interact)
    assert_boot_markers_once(log, scenario)
    assert_log_contains(
        log,
        [
            "[xhci] controller reset and running",
            "[xhci] slot",
            "HID boot keyboard on interface",
            "[xhci] USB HID boot keyboard ready",
            "[usb_hid] USB keyboard installed as an input source",
            # The unambiguous part: a keystroke really came in over USB,
            # not over the i8042 QEMU's pc machine also provides.
            "[usb_hid] first keystroke received over USB",
            "[wisp] primes.wisp stopped by Esc at line",
        ],
        scenario,
    )

    m = re.search(r"\[xhci\] port (\d+): slot (\d+) addressed", log)
    if not m:
        raise RuntimeError(
            f"[{scenario}] expected an '[xhci] port N: slot M addressed' line.\n"
            f"Full debug log:\n{log}"
        )
    port, slot = m.group(1), m.group(2)
    print(
        f"==> PASS [{scenario}]: USB HID boot keyboard enumerated on port {port} "
        f"as slot {slot}, a real keystroke was delivered through the USB path, "
        f"Esc over USB stopped a running program, and the editing keys (Left, "
        f"End, Up) edited and recalled lines"
    )


def main():
    shutil.rmtree(OUT_DIR, ignore_errors=True)
    os.makedirs(OUT_DIR, exist_ok=True)

    build_test_binary()

    scenario_apic()
    scenario_no_xhci()
    scenario_xhci_no_device()
    scenario_usb_keyboard()

    print("==> ALL USB/APIC SCENARIOS PASSED")
    print(f"    artifacts under: {OUT_DIR}")


if __name__ == "__main__":
    main()
