#!/usr/bin/env python3
"""Build Order Step 11 of the Phase 2, Part B plan: a permanent, isolated
regression test covering the whole AHCI persistent-storage stack end to
end -- PCI enumeration -> ABAR mapping -> HBA/port bring-up -> IDENTIFY ->
raw sector read/write -> `save_format.rs`'s on-disk layout -> the shell's
`save`/`load` commands -- via debugcon-log assertions and, for the on-disk
format itself, direct host-side inspection of the save-disk image file
after a clean shutdown. Matches `tests/fault_test.py`'s isolated
build/ESP/target-dir pattern (never touches the project's real `./esp` or
`./target`), and reuses `tests/acceptance_test.py`'s `Monitor`/OCR helpers
the same way `tests/manual_save_load_check.py` already proved out
manually.

Four independent scenarios, each its own fresh boot (the AHCI driver, like
the fault handler in `fault_test.py`, is only ever exercised once per boot
here -- no scenario depends on any other having run first):

  1. `no_controller` -- no AHCI PCI device attached at all. Proves the
     "zero AHCI-class PCI devices found" soft-fail path (Phase 2 plan,
     Part B) reports cleanly and the kernel still boots to a working shell
     prompt, rather than blocking boot or panicking.
  2. `controller_no_disk` -- an AHCI controller is attached (QEMU's
     `ich9-ahci`) but no drive is wired to any of its ports. Proves the
     "no device detected on this port" (`PxSSTS.DET != 3`) soft-fail path
     for every implemented port, and -- critically -- that this completes
     within the bounded timeout instead of hanging. This is a direct,
     permanent regression test for the real sti-ordering bug found and
     fixed during this phase's own verification (see `CHANGES.md`):
     `ahci::init()`'s `pit::ticks()`-based bounded-timeout polling only
     works once `sti` has already run, and this exact scenario -- a
     controller present, no device on any port -- was the one that hung
     forever before that fix landed, because the polling deadline could
     never be reached.
  3. `full_stack` -- a real 64 MiB save disk attached. Exercises every
     remaining layer against real (QEMU-emulated) hardware:
       - PCI enumeration finds the AHCI controller by class/subclass/
         prog-if (`01:06:01`), not a hardcoded vendor:device ID.
       - ABAR is mapped and the HBA's generic registers (CAP/GHC/PI/VS)
         are sane (`PI != 0`).
       - The port starts and reports a plain SATA disk (`PxSIG ==
         SIG_ATA`).
       - IDENTIFY DEVICE reports the exact sector count a 64 MiB image
         must have (131072 512-byte sectors) -- an exact match, not just
         "some positive number."
       - A raw single-sector read of LBA 0 is verified byte-for-byte
         against a host-written, non-trivial known pattern seeded into the
         disk image before boot -- reconstructed from `ahci::
         debug_dump_sector`'s hex-dump log lines, the same rigor
         `pmm::self_test`'s pattern write/read-back already established.
       - Boot leaves the (foreign-looking) disk untouched and unmounted;
         `save` refuses it; a cancelled `format` writes nothing; `format`
         + `ERASE` provisions it. No boot-time write self-test runs, and
         the disk's last sector and everything past LBA 512 are verified
         unchanged afterward (CODEBASE_AUDIT.md AUD-01/AUD-03).
       - The on-disk superblock this test then reads directly from the
         save-image file
         (magic/version/slot-count/slot-size) matches `save_format.rs`'s
         own constants exactly -- proof of the actual bytes on "disk," not
         just that the call returned `Ok`.
       - The shell-level `save`/`load` commands round-trip a real value
         end to end against the running interpreter-independent surface
         (mirrors `tests/manual_save_load_check.py`'s proven interactive
         check, now permanent), and the resulting slot this test decodes
         directly from the save-image file afterward -- key, value length,
         FNV-1a/32 checksum, and the type-tagged encoded payload -- matches
         `save_format.rs`'s documented on-disk layout exactly, not just
         what the shell happened to print.
  4. `foreign_disk_untouched` -- a disk with an MBR signature and data
     across the whole save region and its last sector. Boot plus save/load
     attempts must leave the image's SHA-256 unchanged (AUD-01).

Usage: tests/ahci_test.py [output_dir]
"""
import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "out_ahci")

# Fully isolated from the project's real ./esp and ./target, exactly like
# tests/fault_test.py -- this test builds its own binary and boots it from
# its own ESP directory so it can never interfere with (or be confused by
# stale artifacts from) an ordinary `./build.sh`/`./run.sh` session.
BUILD_TARGET_DIR = os.path.join(OUT_DIR, "target")
ESP_DIR = os.path.join(OUT_DIR, "esp")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import acceptance_test  # noqa: E402 -- screendump() writes under this module's OUT_DIR global
from acceptance_test import Monitor, wait_for_boot, ocr_text  # noqa: E402

BOOT_MARKERS = ["[pmm] self-test OK", "[boot] interrupts enabled"]

# --- save_format.rs's on-disk layout, mirrored here as plain constants so
# this test can decode the raw disk image directly rather than trusting
# the driver's own claim that it wrote the right thing. Kept in sync with
# src/save_format.rs by hand (a small, stable, already-`# Build order`-
# frozen format -- see that file's own module doc comment) rather than
# parsed out of the Rust source, matching this project's existing
# precedent of small hand-mirrored constants in test code (e.g.
# fault_test.py's EXCEPTION_NAMES table).
SAVE_MAGIC = b"WISPSAVE1"
SAVE_FORMAT_VERSION = 1
SAVE_SLOT_COUNT = 64
SECTOR_SIZE = 512
SECTORS_PER_SLOT = 8
SAVE_SLOT_SIZE = SECTORS_PER_SLOT * SECTOR_SIZE  # 4096
SAVE_KEY_MAX_LEN = 32
SAVE_SLOT_HEADER_LEN = 1 + SAVE_KEY_MAX_LEN + 4 + 4  # in_use + key + value_len + checksum
SUPERBLOCK_LBA = 0
SLOTS_START_LBA = 1
TAG_STR = 3


def fnv1a32(data: bytes) -> int:
    """Exact port of save_format.rs's fnv1a32 -- FNV-1a, 32-bit."""
    h = 0x811C_9DC5
    for b in data:
        h ^= b
        h = (h * 0x0100_0193) & 0xFFFF_FFFF
    return h


def encode_str_value(s: str) -> bytes:
    """Exact port of save_format.rs's serialize_value for Value::Str."""
    payload = s.encode("utf-8")
    return bytes([TAG_STR]) + struct.pack("<I", len(payload)) + payload


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
    # No WISP app is run in any scenario here (the shell-level save/load
    # commands are enough to exercise the interpreter-independent surface;
    # WISP's own save()/load() builtins already have their own coverage
    # via APPS/counter.wisp and the acceptance suite) -- an empty APPS
    # directory keeps fs::snapshot_apps's boot-time walk well-defined
    # rather than exercising a path nothing here checks, matching
    # fault_test.py's identical choice.
    os.makedirs(os.path.join(ESP_DIR, "APPS"), exist_ok=True)
    print(f"==> Done: {os.path.join(ESP_DIR, 'EFI', 'BOOT', 'BOOTX64.EFI')}")


def ovmf_code_path() -> str:
    path = "/usr/share/OVMF/OVMF_CODE.fd"
    if not os.path.exists(path):
        path = "/usr/share/ovmf/OVMF.fd"
    return path


def build_qemu_cmd(debug_log: str, monitor_sock: str, save_img: str = None, controller_only: bool = False) -> list:
    cmd = [
        "qemu-system-x86_64",
        "-bios", ovmf_code_path(),
        "-drive", f"format=raw,file=fat:{ESP_DIR},if=none,id=stick,readonly=on",
        "-usb", "-device", "usb-storage,drive=stick",
    ]
    if save_img is not None:
        # Full stack: a real disk attached via AHCI, same shape as
        # run.sh/acceptance_test.py's savedisk args.
        cmd += [
            "-drive", f"id=savedisk,if=none,format=raw,file={save_img}",
            "-device", "ahci,id=ahci0",
            "-device", "ide-hd,drive=savedisk,bus=ahci0.0",
        ]
    elif controller_only:
        # Controller present, but no drive wired to any port -- the "no
        # device detected" soft-fail path, per port.
        cmd += ["-device", "ahci,id=ahci0"]
    # else: no AHCI device at all -- the "no controller found" soft-fail path.
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
                f"[{scenario}] expected boot marker {marker!r} to appear "
                f"exactly once in the debug log (found {count}) -- 0 means "
                f"the kernel never got this far (a hang or an early crash); "
                f"2+ means the machine silently reset and rebooted, which "
                f"is exactly the failure mode this check exists to catch.\n"
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


def read_debug_log(path: str) -> str:
    if not os.path.isfile(path):
        raise RuntimeError(f"no debug log produced at {path}")
    with open(path, "r", errors="replace") as f:
        return f.read()


def extract_sector0_dump(log: str) -> bytes:
    """Reconstruct the 512 raw bytes ahci::debug_dump_sector logged for
    LBA 0, in whatever order the "[ahci] sector +0xOFF: xx xx ..." lines
    appear (there is exactly one such 32-line dump anywhere in this
    scenario's whole boot log -- bring_up_save_disk calls
    debug_dump_sector exactly once, on read_sector(0)'s result)."""
    rows = {}
    for m in re.finditer(r"\[ahci\] sector \+0x([0-9a-f]{3}): ((?:[0-9a-f]{2} )+)", log):
        offset = int(m.group(1), 16)
        byte_strs = m.group(2).split()
        rows[offset] = bytes(int(b, 16) for b in byte_strs)
    if not rows:
        raise RuntimeError("no '[ahci] sector +0x...:' hex-dump lines found in the debug log")
    total_len = sum(len(v) for v in rows.values())
    if total_len != SECTOR_SIZE:
        raise RuntimeError(
            f"hex-dump lines reconstructed to {total_len} bytes, expected "
            f"exactly {SECTOR_SIZE} (found rows at offsets: {sorted(rows)})"
        )
    out = bytearray(SECTOR_SIZE)
    for offset, chunk in rows.items():
        out[offset:offset + len(chunk)] = chunk
    return bytes(out)


def run_and_capture(scenario: str, scenario_dir: str, save_img: str = None, controller_only: bool = False,
                     interact=None) -> tuple:
    """Boot one scenario, optionally run `interact(mon)` once the shell
    banner is confirmed on screen, then cleanly shut the VM down and
    return (debug_log_text, screenshot_of_banner_path). `interact` is
    responsible for any of its own screendumps/assertions; this function
    only guarantees a clean `quit` afterward so a save disk's writes are
    flushed to the host file before the caller inspects it."""
    os.makedirs(scenario_dir, exist_ok=True)
    debug_log = os.path.join(scenario_dir, "debug.log")
    monitor_sock = os.path.join(scenario_dir, "monitor.sock")

    qemu_cmd = build_qemu_cmd(debug_log, monitor_sock, save_img=save_img, controller_only=controller_only)
    print(f"==> [{scenario}] Launching:", " ".join(qemu_cmd))
    proc = subprocess.Popen(qemu_cmd, cwd=ROOT)
    banner_shot = None
    try:
        time.sleep(1.0)
        # Monkey-patch acceptance_test's module-level OUT_DIR: Monitor.
        # screendump() and wait_for_boot()'s own screendumps both write
        # under that global, exactly as tests/manual_save_load_check.py
        # already established.
        acceptance_test.OUT_DIR = scenario_dir
        mon = Monitor(monitor_sock)
        banner_shot = wait_for_boot(mon, timeout=40.0)

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

    return read_debug_log(debug_log), banner_shot


def assert_save_load_report_unavailable(mon, scenario: str):
    """Prove `save`/`load` report "unavailable" rather than crashing, by
    issuing them fresh and OCR-checking the immediate result -- not by
    trying to OCR the boot-time console lines main.rs printed before the
    shell banner, which (like every other pre-banner console line -- see
    acceptance_test.py's step 1b comment) may well have already scrolled
    off the top of the visible framebuffer by the time the banner itself
    is on screen. Typing a fresh command and reading the line it produces
    sidesteps that entirely."""
    mon.type_text("save x y")
    mon.enter()
    time.sleep(0.3)
    acceptance_test.assert_ocr(
        mon.screendump("save_unavailable"),
        "no AHCI save disk available this boot",
        f"{scenario} (save reports unavailable, not a crash)",
    )
    mon.type_text("load x")
    mon.enter()
    time.sleep(0.3)
    acceptance_test.assert_ocr(
        mon.screendump("load_unavailable"),
        "no AHCI save disk available this boot",
        f"{scenario} (load reports unavailable, not a crash)",
    )


def scenario_no_controller():
    scenario = "no_controller"
    log, _banner_shot = run_and_capture(
        scenario, os.path.join(OUT_DIR, scenario),
        interact=lambda mon: assert_save_load_report_unavailable(mon, scenario),
    )
    assert_boot_markers_once(log, scenario)
    assert_log_contains(
        log,
        "[ahci] no AHCI controller found on PCI bus 0 -- persistent storage unavailable",
        scenario,
    )
    if "[ahci] controller found at" in log:
        raise RuntimeError(f"[{scenario}] unexpectedly found an AHCI controller with no AHCI device attached")
    print(f"==> PASS [{scenario}]: no AHCI controller present -- reported cleanly, boot unaffected, "
          f"save/load correctly report unavailable")


def scenario_controller_no_disk():
    scenario = "controller_no_disk"
    log, _banner_shot = run_and_capture(
        scenario, os.path.join(OUT_DIR, scenario), controller_only=True,
        interact=lambda mon: assert_save_load_report_unavailable(mon, scenario),
    )
    assert_boot_markers_once(log, scenario)
    assert_log_contains(log, "[ahci] controller found at", scenario)
    # Every implemented port on QEMU's ich9-ahci (PI = 0x3f, ports 0-5 --
    # see this module's own doc comment and the interactively-observed
    # `pi=0x0000003f` value captured during Build Order Step 4's
    # verification) must report "no device detected" -- proves the
    # bounded-timeout DET poll actually completes, for every port, rather
    # than hanging on the first one. This is the exact scenario that
    # exposed the sti-ordering bug fixed earlier this phase (see
    # CHANGES.md): without that fix, this whole scenario would never
    # reach this point at all.
    for port in range(6):
        assert_log_contains(
            log,
            f"[ahci] port {port}: no device detected (PxSSTS.DET != 3), skipping",
            scenario,
        )
    assert_log_contains(
        log,
        "[ahci] no implemented port reported a usable SATA disk -- persistent storage unavailable",
        scenario,
    )
    print(f"==> PASS [{scenario}]: AHCI controller present with no device on any of its 6 ports -- "
          f"all timed out cleanly and boot completed (sti-ordering regression guard), "
          f"save/load correctly report unavailable")


def scenario_full_stack():
    scenario = "full_stack"
    scenario_dir = os.path.join(OUT_DIR, scenario)
    os.makedirs(scenario_dir, exist_ok=True)
    save_img = os.path.join(scenario_dir, "save.img")
    subprocess.run(["qemu-img", "create", "-f", "raw", save_img, "64M"], check=True, capture_output=True)

    # Seed LBA 0 with a distinctive, non-trivial known pattern before boot
    # -- deliberately not all-zero, not the eventual "WISPSAVE1" magic,
    # and not the self-test's own i^0xA5 pattern, so a correct byte-for-
    # byte match against this specific pattern is real proof read_sector()
    # returns exactly what's on "disk," not a coincidence. bring_up_save_
    # disk() reads and hex-dumps LBA 0 at boot, and boot never writes,
    # so this exact pattern is what the dump must show.
    known_pattern = bytes((i * 31 + 7) & 0xFF for i in range(SECTOR_SIZE))
    last_sector_pattern = bytes((i * 17 + 3) & 0xFF for i in range(SECTOR_SIZE))
    with open(save_img, "r+b") as f:
        f.write(known_pattern)
        f.seek(-SECTOR_SIZE, os.SEEK_END)
        f.write(last_sector_pattern)

    def interact(mon):
        # The seeded LBA 0 is not a WISP superblock, so boot must have left
        # the disk unmounted: save refuses until it is explicitly formatted.
        mon.type_text("save greeting too early")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("01_save_before_format"),
            "not formatted",
            f"{scenario} (save refuses an unformatted disk)",
        )
        # A cancelled format writes nothing.
        mon.type_text("format")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("no")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("01_format_cancelled"),
            "cancelled",
            f"{scenario} (format can be cancelled)",
        )
        mon.type_text("format")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("ERASE")
        mon.enter()
        time.sleep(1.0)
        acceptance_test.assert_ocr(
            mon.screendump("01_format_done"),
            "save disk formatted",
            f"{scenario} (explicit format)",
        )

        # The AHCI status lines main.rs prints to the console (controller
        # found, disk identified, self-test OK, save disk formatted) are
        # NOT re-checked here via OCR: by the time the shell banner scrolls
        # into view (what wait_for_boot just confirmed), those earlier
        # lines have very likely already scrolled off the top of the
        # visible framebuffer -- the debug-log assertions below are the
        # real verification for this pipeline (exact, deterministic, and
        # not subject to scroll timing), matching this project's existing
        # "debugcon for what the kernel did, OCR for what's drawn right
        # now" split (see assert_debug_log's doc comment in
        # acceptance_test.py). The checks below issue fresh commands and
        # immediately OCR the result, so there's no scrolled-content risk.

        # Shell-level save/load round trip -- an interpreter-independent
        # verification surface, matching tests/manual_save_load_check.py's
        # already-proven interactive check, now made permanent.
        mon.type_text("save greeting hello world")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("02_after_save"),
            ["saved", "greeting"],
            f"{scenario} (save confirmation)",
        )

        mon.type_text("load greeting")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("03_after_load"),
            "hello world",
            f"{scenario} (load echoes back the saved value)",
        )

        mon.type_text("load nonexistent_key_xyz")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("04_after_load_missing"),
            "not found",
            f"{scenario} (load reports a clean 'not found' for an unsaved key)",
        )

    log, banner_shot = run_and_capture(scenario, scenario_dir, save_img=save_img, interact=interact)

    assert_boot_markers_once(log, scenario)

    # --- PCI enumeration -------------------------------------------------
    assert_log_contains(log, "[pci] devices found:", scenario)
    assert_log_contains(log, "class:subclass:prog_if=01:06:01", scenario)
    assert_log_contains(log, "[ahci] controller found at", scenario)

    # --- ABAR mapping + HBA generic registers -----------------------------
    m = re.search(r"\[ahci\] abar=0x[0-9a-f]+ cap=0x[0-9a-f]+ ghc=0x[0-9a-f]+ pi=0x([0-9a-f]+) vs=0x[0-9a-f]+", log)
    if not m:
        raise RuntimeError(f"[{scenario}] expected an '[ahci] abar=... pi=...' line in the debug log")
    pi = int(m.group(1), 16)
    if pi == 0:
        raise RuntimeError(f"[{scenario}] controller reported PI=0 (zero implemented ports) with a disk attached")

    # --- Port bring-up: a plain SATA disk signature -----------------------
    m = re.search(r"\[ahci\] port (\d+): started -- cmd=0x[0-9a-f]+ ssts=0x[0-9a-f]+ sig=0x([0-9a-f]+)", log)
    if not m:
        raise RuntimeError(f"[{scenario}] expected an '[ahci] port N: started ...' line in the debug log")
    sig = int(m.group(2), 16)
    if sig != 0x0000_0101:
        raise RuntimeError(f"[{scenario}] expected PxSIG=0x00000101 (SIG_ATA), found 0x{sig:08x}")

    # --- IDENTIFY DEVICE: exact sector count for a 64 MiB image ------------
    m = re.search(r"\[ahci\] identify: model='[^']*' serial='[^']*' total_sectors=(\d+) \(512-byte sectors\) = (\d+) bytes", log)
    if not m:
        raise RuntimeError(f"[{scenario}] expected an '[ahci] identify: total_sectors=...' line in the debug log")
    total_sectors, total_bytes = int(m.group(1)), int(m.group(2))
    expected_sectors = (64 * 1024 * 1024) // SECTOR_SIZE
    if total_sectors != expected_sectors:
        raise RuntimeError(
            f"[{scenario}] expected total_sectors={expected_sectors} for a 64 MiB image, "
            f"found {total_sectors}"
        )
    if total_bytes != total_sectors * SECTOR_SIZE:
        raise RuntimeError(f"[{scenario}] identify's own byte count {total_bytes} doesn't match total_sectors*512")

    # --- Raw sector read, byte-for-byte against the known seeded pattern ---
    dumped = extract_sector0_dump(log)
    if dumped != known_pattern:
        first_diff = next(i for i in range(SECTOR_SIZE) if dumped[i] != known_pattern[i])
        raise RuntimeError(
            f"[{scenario}] raw read of LBA 0 did not byte-for-byte match the seeded pattern -- "
            f"first mismatch at offset 0x{first_diff:03x}: "
            f"dumped=0x{dumped[first_diff]:02x} expected=0x{known_pattern[first_diff]:02x}"
        )

    # --- Boot left the foreign disk alone; `format` provisioned it ---------
    # (CODEBASE_AUDIT.md AUD-01/AUD-03: no automatic format, no boot-time
    # write self-test.)
    assert_log_contains(log, "[save_format] disk holds foreign data; left untouched (not mounted)", scenario)
    assert_log_contains(log, "[save_format] disk provisioned by explicit 'format' command", scenario)
    if "[ahci] self-test" in log:
        raise RuntimeError(f"[{scenario}] boot ran a disk self-test; boot must not write to the save disk")

    # --- Direct, host-side verification of the actual on-disk bytes --------
    # (QEMU has already been cleanly `quit`+terminated by run_and_capture,
    # so every write the guest issued is flushed to this file.)
    with open(save_img, "rb") as f:
        raw = f.read()

    superblock = raw[SUPERBLOCK_LBA * SECTOR_SIZE:(SUPERBLOCK_LBA + 1) * SECTOR_SIZE]
    magic = superblock[0:9]
    version = superblock[9]
    slot_count = struct.unpack_from("<H", superblock, 10)[0]
    slot_size = struct.unpack_from("<I", superblock, 12)[0]
    if magic != SAVE_MAGIC:
        raise RuntimeError(f"[{scenario}] on-disk superblock magic {magic!r} != expected {SAVE_MAGIC!r}")
    if version != SAVE_FORMAT_VERSION:
        raise RuntimeError(f"[{scenario}] on-disk superblock version {version} != expected {SAVE_FORMAT_VERSION}")
    if slot_count != SAVE_SLOT_COUNT:
        raise RuntimeError(f"[{scenario}] on-disk slot_count {slot_count} != expected {SAVE_SLOT_COUNT}")
    if slot_size != SAVE_SLOT_SIZE:
        raise RuntimeError(f"[{scenario}] on-disk slot_size {slot_size} != expected {SAVE_SLOT_SIZE}")

    # Nothing outside the save region may have been written: in particular
    # the disk's last sector, which the old boot self-test used to clobber.
    if raw[-SECTOR_SIZE:] != last_sector_pattern:
        raise RuntimeError(f"[{scenario}] the disk's last sector was modified; only LBA 0-512 belong to WISP")
    if any(raw[513 * SECTOR_SIZE:-SECTOR_SIZE]):
        raise RuntimeError(f"[{scenario}] bytes beyond the save region (LBA 512) were written")

    # slot 0 must hold the "greeting" -> "hello world" save issued above --
    # the first (and only) key saved this boot, so it must have landed in
    # the very first free slot.
    slot0_lba = SLOTS_START_LBA
    slot0 = raw[slot0_lba * SECTOR_SIZE: slot0_lba * SECTOR_SIZE + SAVE_SLOT_SIZE]
    in_use = slot0[0]
    key_field = slot0[1:1 + SAVE_KEY_MAX_LEN]
    key = key_field.rstrip(b"\x00").decode("ascii")
    value_len = struct.unpack_from("<I", slot0, 1 + SAVE_KEY_MAX_LEN)[0]
    checksum = struct.unpack_from("<I", slot0, 1 + SAVE_KEY_MAX_LEN + 4)[0]
    payload = slot0[SAVE_SLOT_HEADER_LEN:SAVE_SLOT_HEADER_LEN + value_len]

    expected_encoded = encode_str_value("hello world")
    if in_use != 1:
        raise RuntimeError(f"[{scenario}] on-disk slot 0's in-use flag is {in_use}, expected 1")
    if key != "greeting":
        raise RuntimeError(f"[{scenario}] on-disk slot 0's key is {key!r}, expected 'greeting'")
    if value_len != len(expected_encoded):
        raise RuntimeError(
            f"[{scenario}] on-disk slot 0's value_len is {value_len}, expected {len(expected_encoded)}"
        )
    if checksum != fnv1a32(expected_encoded):
        raise RuntimeError(
            f"[{scenario}] on-disk slot 0's checksum is 0x{checksum:08x}, "
            f"expected 0x{fnv1a32(expected_encoded):08x}"
        )
    if payload != expected_encoded:
        raise RuntimeError(
            f"[{scenario}] on-disk slot 0's encoded value {payload!r} != expected {expected_encoded!r}"
        )

    print(
        f"==> PASS [{scenario}]: PCI->ABAR->port->IDENTIFY->raw read/write->save_format, "
        f"all verified end to end (total_sectors={total_sectors}, PI=0x{pi:02x}, "
        f"slot 0 = key='greeting' value='hello world' checksum=0x{checksum:08x})"
    )


def scenario_foreign_disk_untouched():
    """CODEBASE_AUDIT.md AUD-01 regression: a disk that does not carry a
    WISP save store -- here, an MBR partition table plus filesystem-like
    data across the whole region WISP would use, and the final sector --
    must come out of a boot, and of save/load attempts, byte-for-byte
    identical."""
    scenario = "foreign_disk_untouched"
    scenario_dir = os.path.join(OUT_DIR, scenario)
    os.makedirs(scenario_dir, exist_ok=True)
    save_img = os.path.join(scenario_dir, "save.img")
    subprocess.run(["qemu-img", "create", "-f", "raw", save_img, "64M"], check=True, capture_output=True)
    with open(save_img, "r+b") as f:
        for lba in range(520):
            f.write(bytes(((lba * 7 + i) | 1) & 0xFF for i in range(SECTOR_SIZE)))
        f.seek(510)
        f.write(b"\x55\xaa")
        f.seek(-SECTOR_SIZE, os.SEEK_END)
        f.write(b"\xee" * SECTOR_SIZE)
    with open(save_img, "rb") as f:
        before = hashlib.sha256(f.read()).hexdigest()

    def interact(mon):
        mon.type_text("save x y")
        mon.enter()
        time.sleep(0.3)
        acceptance_test.assert_ocr(
            mon.screendump("save_refused"), "not formatted", f"{scenario} (save refuses a foreign disk)"
        )
        mon.type_text("load x")
        mon.enter()
        time.sleep(0.3)

    log, _ = run_and_capture(scenario, scenario_dir, save_img=save_img, interact=interact)
    assert_boot_markers_once(log, scenario)
    assert_log_contains(log, "[save_format] disk holds foreign data; left untouched (not mounted)", scenario)
    with open(save_img, "rb") as f:
        after = hashlib.sha256(f.read()).hexdigest()
    if after != before:
        raise RuntimeError(f"[{scenario}] the foreign disk was modified (sha256 {before} -> {after})")
    print(f"==> PASS [{scenario}]: foreign disk byte-identical after boot and save/load attempts")


def main():
    shutil.rmtree(OUT_DIR, ignore_errors=True)
    os.makedirs(OUT_DIR, exist_ok=True)

    build_test_binary()

    scenario_no_controller()
    scenario_controller_no_disk()
    scenario_full_stack()
    scenario_foreign_disk_untouched()

    print("==> ALL AHCI SCENARIOS PASSED")
    print(f"    artifacts under: {OUT_DIR}")


if __name__ == "__main__":
    main()
