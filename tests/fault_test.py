#!/usr/bin/env python3
"""Proves the GDT/IDT/fault-screen path built for the "leave Boot Services
behind" phase, and the own-page-tables/guard-page path built for Phase 2,
actually catch and report real CPU exceptions -- rather than silently
resetting the VM (a triple fault under this project's own hand-written
descriptor tables produces exactly that failure mode with zero diagnostic
output, which is why this needed its own targeted test rather than trusting
the absence of a crash during the ordinary acceptance test).

Builds a single binary with the `fault-test-hooks` Cargo feature enabled (a
handful of hidden shell commands, each issuing one specific, deliberate
fault) under an isolated ESP directory and target dir (never touching the
project's real `./esp` or `./target`, so this can't leave a feature-enabled
binary sitting where `run.sh` or the ordinary acceptance test would pick it
up), then boots it once per scenario -- the fault handler halts forever once
it reports, so each scenario needs its own fresh boot -- and asserts on the
`-debugcon` log that:

  1. The exception was actually caught and reported with the *expected*
     vector (not just "some" exception), and
  2. The machine did not silently reset afterward (this project's own boot
     markers -- "[pmm] self-test OK" / "[boot] interrupts enabled" -- each
     appear exactly once in the log, not twice; a silent reset would boot
     firmware and this kernel all over again, producing a second copy of
     both).

Six scenarios:
  - `__fault_test_int3`: a plain `int3` breakpoint (#3) -- proves the basic
    fault-catching path from Phase 1 still works.
  - `__fault_test_pagefault`: a write to a definitely-unmapped, far-outside-
    the-physical-memory-map canonical address -- proves `paging.rs`'s own
    page tables actually enforce "unmapped means unmapped" (#14, with `cr2`
    matching the exact target address).
  - `__fault_test_guard_overflow`: a raw-asm downward-walking loop that
    probes (via a scratch register, not RSP itself) off the bottom of the
    mainline stack into its guard page -- proves Build Order Step 4's guard
    page is genuinely unmapped, in isolation (#14, with `cr2` falling
    inside the guard range `paging.rs` itself logged at boot).
  - `__fault_test_stack_overflow`: the same downward walk, but through RSP
    itself -- simulates a *real* stack overflow (unbounded recursion), where
    the CPU's own attempt to deliver the resulting #PF also faults (RSP is
    already invalid), escalating to #8 Double Fault. Proves the
    DOUBLE_FAULT_STACK/IST1 mechanism (`gdt.rs`) actually survives that,
    catching and reporting it instead of silently triple-faulting the whole
    machine -- the exact scenario that mechanism exists for, now verified
    end to end instead of only assumed correct from the architecture.
  - `__fault_test_wp`: a ring-0 write to a constant in the image's
    read-only `.rdata` -- proves v1.0's W^X write protection (read-only
    leaf entries plus `CR0.WP`): #14 with P and W set, `cr2` equal to the
    probe address the hook logged.
  - `__fault_test_nx`: a call into a `ret`-filled buffer in the writable
    `.data` section -- proves v1.0's execute-disable (EFER.NXE plus NX on
    every non-code page): #14 with P and I/D set. Were NX missing, the
    `ret` would just return and the hook logs "NOT enforced", which fails
    the test explicitly instead of hanging.

Usage: tests/fault_test.py [output_dir]
"""
import os
import re
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "out_fault")

# Fully isolated from the project's real ./esp and ./target -- this test
# builds and boots a binary with hidden crash-hook commands that must never
# ship in the normal release artifact (see Cargo.toml's `fault-test-hooks`
# feature doc comment), so it never touches the directories those normally
# live in.
BUILD_TARGET_DIR = os.path.join(OUT_DIR, "target")
ESP_DIR = os.path.join(OUT_DIR, "esp")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acceptance_test  # noqa: E402 -- screendump() writes under this module's OUT_DIR global
from acceptance_test import Monitor, key_name_for, wait_for_debug_marker  # noqa: E402

BOOT_MARKERS = ["[pmm] self-test OK", "[boot] interrupts enabled"]

EXCEPTION_NAMES = {
    3: "#3 Breakpoint",
    8: "#8 Double Fault",
    14: "#14 Page Fault",
}

# Page-fault error code bits (Intel SDM Vol 3, 4.7).
PF_PRESENT = 1 << 0
PF_WRITE = 1 << 1
PF_FETCH = 1 << 4


def build_fault_test_binary():
    print("==> Building BOOTX64.efi (release, fault-test-hooks feature, isolated target dir)")
    subprocess.run(
        [
            "cargo", "+nightly", "build", "--release",
            "--target", "x86_64-unknown-uefi",
            "--features", "fault-test-hooks",
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
    # APPS/ isn't exercised by this test, but fs::snapshot_apps still walks
    # it at boot; an empty directory keeps that walk well-defined rather
    # than testing a code path (missing APPS entirely) nothing else covers.
    os.makedirs(os.path.join(ESP_DIR, "APPS"), exist_ok=True)
    print(f"==> Done: {os.path.join(ESP_DIR, 'EFI', 'BOOT', 'BOOTX64.EFI')}")


def boot_and_trigger(scenario_dir: str, command: str) -> str:
    """Boot the fault-test binary, type `command` at the shell prompt, give
    the fault handler time to report and halt, then tear the VM down and
    return the full `-debugcon` log text. One fresh boot per call -- the
    fault handler this triggers never returns, so a second scenario cannot
    be run against the same still-live VM."""
    os.makedirs(scenario_dir, exist_ok=True)
    debug_log = os.path.join(scenario_dir, "debug.log")
    monitor_sock = os.path.join(scenario_dir, "monitor.sock")

    ovmf_code = "/usr/share/OVMF/OVMF_CODE.fd"
    if not os.path.exists(ovmf_code):
        ovmf_code = "/usr/share/ovmf/OVMF.fd"

    qemu_cmd = [
        "qemu-system-x86_64",
        "-bios", ovmf_code,
        "-drive", f"format=raw,file=fat:{ESP_DIR},if=none,id=stick,readonly=on",
        "-usb", "-device", "usb-storage,drive=stick",
        "-net", "none",
        "-m", "256M",
        "-vga", "std",
        "-display", "none",
        "-debugcon", f"file:{debug_log}",
        "-monitor", f"unix:{monitor_sock},server,nowait",
        "-serial", "none",
    ]

    print(f"==> [{command}] Launching:", " ".join(qemu_cmd))
    proc = subprocess.Popen(qemu_cmd, cwd=ROOT)
    try:
        time.sleep(1.0)
        # `Monitor.screendump()` (defined in acceptance_test.py) resolves
        # `OUT_DIR` as a free variable in *that* module's own namespace, not
        # wherever it's called from -- so without this, every screendump
        # this test takes silently lands under acceptance_test.py's default
        # `tests/out/` instead of this scenario's own directory under
        # `OUT_DIR` (`tests/out_fault/...`, set at the top of this file).
        # Harmless-looking on a machine where `tests/out/` already exists
        # (the screenshot just ends up in the "wrong" place, no error), but
        # fatal on a clean checkout that has never run acceptance_test.py
        # first: QEMU's own `screendump` monitor command fails silently
        # against a nonexistent directory, and the very next line --
        # `Image.open(ppm_path)` -- then throws `FileNotFoundError` instead.
        # `tests/ahci_test.py` already carries this same monkey-patch (with
        # this same explanation); this file needs it for exactly the same
        # reason, and previously lacked it only because every prior run in
        # this project's history happened to follow an `acceptance_test.py`
        # run that had already created `tests/out/` -- see CHANGES.md.
        acceptance_test.OUT_DIR = scenario_dir
        mon = Monitor(monitor_sock)

        # Wait for the kernel's own "[shell] ready" debugcon marker rather
        # than OCR or a fixed sleep: this test's only real dependency stays
        # the debugcon log it asserts against, and it can no longer type
        # into a VM that is still booting (a fixed 4 s sleep did exactly
        # that once when the host was loaded -- see CHANGES.md, v1.0.0).
        wait_for_debug_marker(debug_log, "[shell] ready", timeout=60.0)

        for ch in command:
            mon.cmd(f"sendkey {key_name_for(ch)}")
            time.sleep(0.03)
        mon.cmd("sendkey ret")
        time.sleep(0.05)

        # The fault handler halts forever once it's reported -- wait for
        # the report itself (or a hook's own "NOT enforced" line, which is
        # how the W^X hooks fail when protection is missing), then give the
        # framebuffer a moment to finish drawing for the screenshot, and
        # stop the VM ourselves (a clean `quit` isn't expected to be
        # answered, since the guest is deliberately wedged by design). A
        # missing report is not raised here: the assertions below produce
        # the precise, scenario-specific error message for it.
        try:
            wait_for_debug_marker(debug_log, ["[CPU EXCEPTION]", "NOT enforced"], timeout=20.0)
        except RuntimeError:
            pass
        time.sleep(1.0)
        mon.screendump(f"{os.path.basename(scenario_dir)}_screen")
    finally:
        time.sleep(0.5)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    if not os.path.isfile(debug_log):
        raise RuntimeError(f"[{command}] no debug log produced at {debug_log}")
    with open(debug_log, "r", errors="replace") as f:
        return f.read()


def assert_boot_markers_once(log: str, command: str):
    for marker in BOOT_MARKERS:
        count = log.count(marker)
        if count != 1:
            raise RuntimeError(
                f"[{command}] expected boot marker {marker!r} to appear "
                f"exactly once in the debug log (found {count}) -- "
                f"{count} == 0 means the kernel never got as far as usual; "
                f"{count} >= 2 means the machine silently reset and "
                f"rebooted at some point, which is exactly the failure "
                f"mode this test exists to catch.\nFull debug log:\n{log}"
            )


def assert_exception(log: str, command: str, vector: int):
    if "[CPU EXCEPTION]" not in log:
        raise RuntimeError(
            f"[{command}] expected a '[CPU EXCEPTION]' line from "
            f"panic::fault_screen in the debug log -- none found (the "
            f"trigger either never executed, or the fault path itself is "
            f"broken).\nFull debug log:\n{log}"
        )
    expected_name = EXCEPTION_NAMES[vector]
    if expected_name not in log:
        raise RuntimeError(
            f"[{command}] found a '[CPU EXCEPTION]' line but not the "
            f"expected {expected_name!r} -- some other, unexpected "
            f"exception fired instead.\nFull debug log:\n{log}"
        )


def extract_cr2(log: str, command: str) -> int:
    m = re.search(r"cr2=0x([0-9a-fA-F]+)", log)
    if not m:
        raise RuntimeError(
            f"[{command}] expected a 'cr2=0x...' field in the fault "
            f"report but found none.\nFull debug log:\n{log}"
        )
    return int(m.group(1), 16)


def extract_error_code(log: str, command: str) -> int:
    m = re.search(r"error_code=0x([0-9a-fA-F]+)", log)
    if not m:
        raise RuntimeError(
            f"[{command}] expected an 'error_code=0x...' field in the fault "
            f"report but found none.\nFull debug log:\n{log}"
        )
    return int(m.group(1), 16)


def extract_probe_target(log: str, command: str, kind: str) -> int:
    m = re.search(rf"\[fault_test\] {kind} target=0x([0-9a-fA-F]+)", log)
    if not m:
        raise RuntimeError(
            f"[{command}] expected a '[fault_test] {kind} target=0x...' line "
            f"(logged by the hook just before it triggers) but found none.\n"
            f"Full debug log:\n{log}"
        )
    return int(m.group(1), 16)


def assert_wx_fault(log: str, command: str, kind: str, must_have_bits: int, description: str):
    """Shared assertions for the two W^X hooks: the hook must not have
    reported its own "NOT enforced" fallback, the fault must be a #PF at
    exactly the probe address the hook logged, and the error code must
    carry `must_have_bits` (protection violation plus the access type)."""
    if f"[fault_test] {kind} NOT enforced" in log:
        raise RuntimeError(
            f"[{command}] the hook ran to completion without faulting: "
            f"{description} is not being enforced.\nFull debug log:\n{log}"
        )
    assert_exception(log, command, 14)
    target = extract_probe_target(log, command, kind)
    cr2 = extract_cr2(log, command)
    if cr2 != target:
        raise RuntimeError(
            f"[{command}] expected cr2=0x{target:x} (the probe address the "
            f"hook logged) but the fault report says cr2=0x{cr2:x}.\n"
            f"Full debug log:\n{log}"
        )
    error_code = extract_error_code(log, command)
    if error_code & must_have_bits != must_have_bits:
        raise RuntimeError(
            f"[{command}] expected page-fault error code bits "
            f"0x{must_have_bits:x} to be set, got error_code=0x{error_code:x}.\n"
            f"Full debug log:\n{log}"
        )
    return cr2, error_code


def extract_guard_ranges(log: str) -> list:
    """Parse every '[paging] guard page: 0xSTART..0xEND' line the kernel
    itself logged at boot (see `paging::init`), in the order they were
    passed to `paging::init` -- ground truth from this exact run, not a
    guessed/hardcoded address. `main.rs` passes
    `[double_fault_stack_guard_range(), mainline_stack_guard_range()]`, so
    index 0 is the double-fault stack's guard page and index 1 is the
    mainline stack's."""
    ranges = []
    for m in re.finditer(r"\[paging\] guard page: 0x([0-9a-fA-F]+)\.\.0x([0-9a-fA-F]+)", log):
        ranges.append((int(m.group(1), 16), int(m.group(2), 16)))
    return ranges


def main():
    shutil.rmtree(OUT_DIR, ignore_errors=True)
    os.makedirs(OUT_DIR, exist_ok=True)

    build_fault_test_binary()

    # --- Scenario 1: int3 breakpoint (#3) -- the Phase 1 fault path -----
    log = boot_and_trigger(os.path.join(OUT_DIR, "int3"), "__fault_test_int3")
    assert_boot_markers_once(log, "__fault_test_int3")
    assert_exception(log, "__fault_test_int3", 3)
    print("==> PASS [__fault_test_int3]: breakpoint exception caught and reported")

    # --- Scenario 2: write to a definitely-unmapped address (#14) -------
    log = boot_and_trigger(os.path.join(OUT_DIR, "pagefault"), "__fault_test_pagefault")
    assert_boot_markers_once(log, "__fault_test_pagefault")
    assert_exception(log, "__fault_test_pagefault", 14)
    cr2 = extract_cr2(log, "__fault_test_pagefault")
    expected = 0x0000_1000_0000_0000  # must match shell.rs's FAULT_TEST_UNMAPPED_ADDR
    if cr2 != expected:
        raise RuntimeError(
            f"[__fault_test_pagefault] expected cr2=0x{expected:x} (the "
            f"deliberate target address) but the fault report says "
            f"cr2=0x{cr2:x} -- some other address faulted instead.\n"
            f"Full debug log:\n{log}"
        )
    print(f"==> PASS [__fault_test_pagefault]: page fault caught, cr2=0x{cr2:x} matches the deliberate target")

    # --- Scenario 3: mainline-stack guard-page overflow (#14) -----------
    log = boot_and_trigger(os.path.join(OUT_DIR, "guard_overflow"), "__fault_test_guard_overflow")
    assert_boot_markers_once(log, "__fault_test_guard_overflow")
    assert_exception(log, "__fault_test_guard_overflow", 14)
    cr2 = extract_cr2(log, "__fault_test_guard_overflow")
    guard_ranges = extract_guard_ranges(log)
    if len(guard_ranges) < 2:
        raise RuntimeError(
            f"[__fault_test_guard_overflow] expected at least 2 "
            f"'[paging] guard page: ...' lines in the debug log (double-"
            f"fault stack, then mainline stack) but found "
            f"{len(guard_ranges)} -- paging::init's guard-range logging "
            f"may have changed shape.\nFull debug log:\n{log}"
        )
    mainline_guard_start, mainline_guard_end = guard_ranges[1]
    if not (mainline_guard_start <= cr2 < mainline_guard_end):
        raise RuntimeError(
            f"[__fault_test_guard_overflow] expected cr2 to fall inside "
            f"the mainline stack's guard range "
            f"0x{mainline_guard_start:x}..0x{mainline_guard_end:x} "
            f"(as logged by the kernel itself at boot) but the fault "
            f"report says cr2=0x{cr2:x} -- the overflow landed somewhere "
            f"unexpected instead of the guard page.\nFull debug log:\n{log}"
        )
    print(
        f"==> PASS [__fault_test_guard_overflow]: page fault caught, "
        f"cr2=0x{cr2:x} falls inside the mainline guard page "
        f"0x{mainline_guard_start:x}..0x{mainline_guard_end:x}"
    )

    # --- Scenario 4: a *real* stack overflow -- RSP itself walks into the
    # guard page, cascading #PF -> #DF -- proves DOUBLE_FAULT_STACK/IST1
    # actually survives it instead of silently triple-faulting the machine.
    log = boot_and_trigger(os.path.join(OUT_DIR, "stack_overflow"), "__fault_test_stack_overflow")
    assert_boot_markers_once(log, "__fault_test_stack_overflow")
    assert_exception(log, "__fault_test_stack_overflow", 8)
    print("==> PASS [__fault_test_stack_overflow]: real stack overflow correctly escalated to #8 Double Fault, caught and reported via IST1 (not a silent triple fault)")

    # --- Scenario 5: W^X write-protect -- ring-0 write to .rdata (#14) ---
    log = boot_and_trigger(os.path.join(OUT_DIR, "wp"), "__fault_test_wp")
    assert_boot_markers_once(log, "__fault_test_wp")
    cr2, error_code = assert_wx_fault(
        log, "__fault_test_wp", "wp", PF_PRESENT | PF_WRITE, "kernel write protection (read-only .rdata + CR0.WP)"
    )
    if "page fault: write, protection violation" not in log:
        raise RuntimeError(
            "[__fault_test_wp] expected the fault screen's decoded line "
            "'page fault: write, protection violation'.\nFull debug log:\n" + log
        )
    print(f"==> PASS [__fault_test_wp]: write to read-only kernel data faulted at cr2=0x{cr2:x} (error_code=0x{error_code:x})")

    # --- Scenario 6: W^X execute-disable -- call into .data (#14) --------
    log = boot_and_trigger(os.path.join(OUT_DIR, "nx"), "__fault_test_nx")
    assert_boot_markers_once(log, "__fault_test_nx")
    cr2, error_code = assert_wx_fault(
        log, "__fault_test_nx", "nx", PF_PRESENT | PF_FETCH, "execute-disable (NX on every non-code page)"
    )
    if "page fault: instruction fetch, protection violation" not in log:
        raise RuntimeError(
            "[__fault_test_nx] expected the fault screen's decoded line "
            "'page fault: instruction fetch, protection violation'.\nFull debug log:\n" + log
        )
    print(f"==> PASS [__fault_test_nx]: instruction fetch from writable data faulted at cr2=0x{cr2:x} (error_code=0x{error_code:x})")

    print("==> ALL FAULT SCENARIOS PASSED")
    print(f"    artifacts under: {OUT_DIR}")


if __name__ == "__main__":
    main()
