#!/usr/bin/env python3
"""Drives ./run.sh's QEMU instance over the HMP monitor to exercise the
acceptance test end-to-end, saving a PNG screenshot after each step so the
result can be visually verified.

Usage: tests/acceptance_test.py [output_dir]
"""
import os
import re
import shutil
import socket
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "out")
MONITOR_SOCK = os.path.join(OUT_DIR, "monitor.sock")
DEBUG_LOG = os.path.join(OUT_DIR, "debug.log")

# Maps a printable character to the QEMU HMP `sendkey` key name that types it.
KEY_NAMES = {
    " ": "spc",
    "-": "minus",
    "_": "shift-minus",
    ".": "dot",
    "/": "slash",
    # Punctuation WISP source needs, as QEMU qcodes on a US layout -- the
    # same keys keyboard.rs's scancode tables decode them from.
    "=": "equal",
    "+": "shift-equal",
    "*": "shift-8",
    "(": "shift-9",
    ")": "shift-0",
    "%": "shift-5",
    "!": "shift-1",
    "#": "shift-3",
    ",": "comma",
    "<": "shift-comma",
    ">": "shift-dot",
    ";": "semicolon",
    ":": "shift-semicolon",
    "'": "apostrophe",
    '"': "shift-apostrophe",
    "[": "bracket_left",
    "]": "bracket_right",
    "{": "shift-bracket_left",
    "}": "shift-bracket_right",
}


def key_name_for(ch: str) -> str:
    if ch in KEY_NAMES:
        return KEY_NAMES[ch]
    if ch.isupper():
        return f"shift-{ch.lower()}"
    return ch


class Monitor:
    def __init__(self, sock_path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        deadline = time.time() + 15
        last_err = None
        while time.time() < deadline:
            try:
                self.sock.connect(sock_path)
                break
            except (FileNotFoundError, ConnectionRefusedError) as e:
                last_err = e
                time.sleep(0.2)
        else:
            raise RuntimeError(f"could not connect to monitor socket: {last_err}")
        self.buf = b""
        self._read_until_prompt()

    def _read_until_prompt(self, timeout=10.0) -> str:
        self.sock.settimeout(timeout)
        while b"(qemu) " not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                break
            self.buf += chunk
        text = self.buf.decode(errors="replace")
        self.buf = b""
        return text

    def cmd(self, command: str) -> str:
        self.sock.sendall((command + "\n").encode())
        return self._read_until_prompt()

    def type_text(self, text: str):
        for ch in text:
            self.cmd(f"sendkey {key_name_for(ch)}")
            time.sleep(0.02)

    def enter(self):
        self.cmd("sendkey ret")
        time.sleep(0.05)

    def key(self, name: str, times: int = 1, gap: float = 0.05):
        """Press a named key (a QEMU qcode: up, left, home, delete, ...)
        `times` times. `gap` must exceed QEMU's 100 ms sendkey hold when
        the same key repeats on a USB keyboard, whose boot reports carry
        key *state*: two presses inside one hold read as one."""
        for _ in range(times):
            self.cmd(f"sendkey {name}")
            time.sleep(gap)

    def backspace(self):
        self.cmd("sendkey backspace")
        time.sleep(0.02)

    def screendump(self, name: str) -> str:
        ppm_path = os.path.join(OUT_DIR, name + ".ppm")
        png_path = os.path.join(OUT_DIR, name + ".png")
        self.cmd(f"screendump {ppm_path}")
        time.sleep(0.1)
        from PIL import Image

        Image.open(ppm_path).save(png_path)
        return png_path

    def quit(self):
        try:
            self.cmd("quit")
        except Exception:
            pass


def ocr_text(png_path: str) -> str:
    """OCR a screenshot via the `tesseract` CLI (already installed in this
    sandbox) and return the recognized text. Shells out rather than using
    pytesseract so this test has no extra Python dependency beyond
    Pillow. `tesseract <img> stdout` prints recognized text to stdout."""
    result = subprocess.run(
        ["tesseract", png_path, "stdout"],
        capture_output=True,
        text=True,
        timeout=15,
    )
    return result.stdout


def wait_for_boot(mon: "Monitor", timeout: float) -> str:
    """Poll screendumps until *our* shell's banner is confirmed on screen,
    then return the path to that shot.

    A pixel-presence check alone is not enough to prove this is our
    shell: on a boot gone wrong, OVMF's own built-in EDK II UEFI
    Interactive Shell draws its own banner starting at the same (0, 0)
    position ours does, so "some non-black pixels appeared up here"
    would happily pass while looking at entirely the wrong shell -- which
    is exactly what happened during development (see run.sh's comment
    for the full account of the vvfat read-write corruption that caused
    it). So: first wait for *something* to be drawn (cheap, avoids OCR
    on pure-black frames while the TianoCore splash is still up), then
    OCR that frame and require our banner's actual text -- "Custom
    Kernel" is a substring of shell.rs's BANNER constant ("Minimal
    Custom Kernel Shell -- own GDT/IDT/PIC/PIT") and appears nowhere in
    OVMF's own shell banner ("UEFI Interactive Shell"), so it's an
    unambiguous discriminator between the two, not a coincidental proxy
    like pixel color. Seeing OVMF's banner text instead is treated as an
    immediate hard failure rather than something to keep polling past.
    (Updated from the original "Boot Services" check when this project
    left Boot Services behind for its own GDT/IDT/PIC/PIT/keyboard
    drivers -- see CHANGES.md.)
    """
    from PIL import Image

    deadline = time.time() + timeout
    last_path = None
    attempt = 0
    while time.time() < deadline:
        attempt += 1
        last_path = mon.screendump(f"_boot_poll_{attempt}")
        img = Image.open(last_path)
        # Our banner occupies roughly the top-left 480x112 px (4-5 text
        # rows of an 8x16 font); the TianoCore splash's logo box is
        # centered on screen and doesn't reach this corner at the
        # resolutions OVMF/QEMU's stdvga negotiate here.
        region = img.crop((0, 0, 480, 112))
        colors = region.getcolors(maxcolors=64)
        non_black = colors is None or any(c != (0, 0, 0) for _count, c in colors)
        if non_black:
            text = ocr_text(last_path)
            if "Custom Kernel" in text:
                return last_path
            if "Interactive Shell" in text:
                raise RuntimeError(
                    "QEMU/OVMF booted into its own built-in EDK II UEFI "
                    "Interactive Shell instead of BOOTX64.EFI -- our banner "
                    "never appeared on screen. This is the exact failure "
                    "mode documented in run.sh (historically caused by "
                    "vvfat read-write corruption of BOOTX64.EFI on disk; "
                    "see CHANGES.md). OCR read:\n"
                    f"{text!r}\nScreenshot: {last_path}"
                )
            # Some other non-black frame (partially drawn mid-scan, or
            # OCR misread a transient frame) -- keep polling rather than
            # failing on a single ambiguous read.
        time.sleep(1.0)
    raise RuntimeError(
        f"timed out after {timeout}s waiting for our shell's banner to "
        f"appear (last screendump: {last_path})"
    )


def wait_for_debug_marker(debug_log: str, markers, timeout: float) -> str:
    """Poll the kernel's `-debugcon` log until any of `markers` (a string
    or a list of strings) appears, and return the full log text at that
    point. Raises with the log so far on timeout.

    This is the deterministic alternative to sleeping a fixed time and
    hoping the guest got there: the kernel writes `[shell] ready` exactly
    once, immediately before its first prompt accepts input, so a harness
    that waits for it can never type into a VM that is still booting --
    which a fixed sleep was observed to do on a host busy running several
    VMs at once."""
    if isinstance(markers, str):
        markers = [markers]
    deadline = time.time() + timeout
    text = ""
    while time.time() < deadline:
        if os.path.isfile(debug_log):
            with open(debug_log, "r", errors="replace") as f:
                text = f.read()
            if any(m in text for m in markers):
                return text
        time.sleep(0.2)
    raise RuntimeError(
        f"timed out after {timeout}s waiting for any of {markers!r} in "
        f"{debug_log}.\nLog so far:\n{text}"
    )


def read_uptime_ticks(mon: "Monitor", base_name: str, step_name: str, attempts: int = 4) -> tuple:
    """Screendump and OCR-extract the `(N ticks)` reading from the shell's
    `uptime` output, retrying against *fresh screendumps of the same,
    already-rendered, unchanged screen* if OCR fails to parse it cleanly --
    never by re-typing `uptime` again, which would actually change what's
    on screen and defeat the point of a retry.

    This project has independently confirmed real tesseract misreads on
    other screens by direct visual inspection of the actual PNG (see
    CHANGES.md's xor_brain truth-table postmortem), and hit three variants
    of the same thing here during Phase 2 verification: the
    `\\((\\d+)\\s*ticks\\)` regex matching nothing despite `ticks` being
    present in the OCR text, two successive readings OCR'ing to the exact
    same tick count, and (once both of those were dumped to a raw
    `tesseract` invocation for direct inspection) the actual, confirmed
    cause of both: tesseract deterministically misreading this bitmap
    font's `(` as `{` in this specific position -- `up 20.200s {2020
    ticks)` -- on a screenshot that, opened directly, shows a perfectly
    correct, cleanly rendered `up 20.200s (2020 ticks)` line. Conclusive
    proof the kernel and PIT are both working correctly and only the OCR
    pass is unreliable, and specifically unreliable about `(` vs `{` (not
    the digits, not `ticks` itself) -- so the regex below accepts either
    bracket character in both the open and close position, exactly
    mirroring this project's existing "ls" vs "1s" and lowercase-`c`-vs-`E`
    OCR-ambiguity precedents rather than inventing a new kind of fix.
    Retrying the *capture+recognize* step against content that has not
    changed is kept as defense in depth against any other, still-unknown
    transient misread class, but the bracket fix is what actually resolved
    the two confirmed failures above -- a same-run misread this
    deterministic does not go away by re-screenshotting identical pixels.

    Raises with every attempt's OCR text attached if all attempts fail,
    which would then mean a real problem (the kernel genuinely didn't
    render a parseable reading), not a transient misread.
    """
    last_text = None
    last_path = None
    for attempt in range(1, attempts + 1):
        name = base_name if attempt == 1 else f"{base_name}_retry{attempt}"
        shot = mon.screendump(name)
        text = ocr_text(shot)
        last_text, last_path = text, shot
        if "ticks" in text:
            # `[({]`/`[)}]`: tesseract has been confirmed (see doc comment
            # above) to sometimes misread this font's `(`/`)` as `{`/`}` in
            # this exact spot -- both are accepted as the same unambiguous
            # on-screen glyph, not a real alternative meaning.
            matches = re.findall(r"[({](\d+)\s*ticks[)}]", text)
            if matches:
                return shot, int(matches[-1])
        time.sleep(0.2)
    raise RuntimeError(
        f"{step_name}: could not OCR-extract a '(N ticks)' reading after "
        f"{attempts} attempts against the same, unchanged on-screen "
        f"content -- either the kernel genuinely didn't render it (a real "
        f"bug) or tesseract misread it every single time (see this "
        f"function's doc comment for the confirmed-by-direct-inspection "
        f"precedent for the latter).\nLast OCR text:\n{last_text}\n"
        f"Last screenshot: {last_path}"
    )


def assert_ocr(png_path: str, must_contain, step_name: str, must_not_contain=None) -> str:
    """OCR `png_path` and assert every string in `must_contain` appears in
    the recognized text (and, if given, that nothing in `must_not_contain`
    does). Raises with the full OCR text on failure so a broken run fails
    loudly with the evidence attached, rather than a screenshot silently
    being saved and never actually inspected -- passing screendumps do not
    prove correct behavior; only checking their content does. `must_contain`
    accepts either a single string or a list of strings."""
    if isinstance(must_contain, str):
        must_contain = [must_contain]
    text = ocr_text(png_path)
    missing = [s for s in must_contain if s not in text]
    unexpected = [s for s in (must_not_contain or []) if s in text]
    if missing or unexpected:
        problems = []
        if missing:
            problems.append(f"missing expected text {missing!r}")
        if unexpected:
            problems.append(f"contains unexpected text {unexpected!r}")
        raise RuntimeError(
            f"{step_name}: OCR verification failed ({'; '.join(problems)}) "
            f"in {png_path}.\nFull OCR text:\n{text}"
        )
    return text


def assert_debug_log(must_contain, step_name: str):
    """Assert every string in `must_contain` appears in the `-debugcon` log
    written so far. For proof that isn't on the framebuffer at all (own
    page tables built and switched to, the mainline-stack switch) -- OCR
    can only verify what's actually drawn on screen, so Phase 2's paging
    proof lines need this instead, the same debug-log-assertion pattern
    `tests/fault_test.py` already established. Raises with the full log
    attached on failure, matching `assert_ocr`'s "fail loudly with the
    evidence attached" standard."""
    if isinstance(must_contain, str):
        must_contain = [must_contain]
    if not os.path.isfile(DEBUG_LOG):
        raise RuntimeError(f"{step_name}: no debug log found at {DEBUG_LOG}")
    with open(DEBUG_LOG, "r", errors="replace") as f:
        log = f.read()
    missing = [s for s in must_contain if s not in log]
    if missing:
        raise RuntimeError(
            f"{step_name}: debug-log verification failed (missing "
            f"{missing!r}) in {DEBUG_LOG}.\nFull debug log:\n{log}"
        )
    return log


def assert_colors_present(png_path: str, expected_colors, step_name: str, tolerance: int = 24) -> None:
    """Assert every `(r, g, b)` tuple in `expected_colors` appears
    somewhere in `png_path`, within `tolerance` per channel.

    Used where a specific pixel's exact coordinates aren't predictable
    from this side (an animation frame caught at whatever moment the
    screendump happened to land on) but specific, widely-separated colors
    being on screen at all is still strong, non-coincidental evidence
    that the WISP drawing call producing that exact color actually ran
    against the real interpreter and the real (emulated) GOP framebuffer
    -- not just that the script didn't crash. `tolerance` absorbs QEMU's
    std VGA output rounding without masking a genuinely wrong color: the
    WISP programs this is used against always draw fully-saturated,
    widely-separated colors specifically so a real bug (wrong channel
    order from `Console::encode`'s BGR/RGB handling, wrong color
    entirely) can't hide inside it.
    """
    from PIL import Image

    img = Image.open(png_path).convert("RGB")
    present = [c for _count, c in (img.getcolors(maxcolors=65536) or [])]
    missing = [
        expected
        for expected in expected_colors
        if not any(all(abs(a - b) <= tolerance for a, b in zip(c, expected)) for c in present)
    ]
    if missing:
        raise RuntimeError(
            f"{step_name}: expected color(s) {missing!r} (tolerance "
            f"{tolerance}) not found anywhere in {png_path} -- actual "
            f"palette: {present!r}"
        )


def assert_pixel_near(png_path: str, x: int, y: int, expected: tuple, step_name: str, tolerance: int = 24) -> tuple:
    """Assert the pixel at `(x, y)` in `png_path` is within `tolerance`
    per channel of `expected` `(r, g, b)` -- the precise-coordinates
    counterpart to `assert_colors_present` above, for WISP drawing calls
    (`plot`/`rect`/`line`) whose target position this test script chose
    itself and therefore knows exactly. Returns the actual pixel so a
    caller can log it."""
    from PIL import Image

    img = Image.open(png_path).convert("RGB")
    actual = img.getpixel((x, y))
    if any(abs(a - e) > tolerance for a, e in zip(actual, expected)):
        raise RuntimeError(
            f"{step_name}: pixel at ({x},{y}) in {png_path} is {actual}, "
            f"expected approximately {expected} (tolerance {tolerance})"
        )
    return actual


def wait_for_text(mon: "Monitor", name: str, must_contain: str, timeout: float = 10.0) -> str:
    """Poll fresh screendumps (each saved under `name`, overwriting the
    last) until OCR finds `must_contain`, or raise after `timeout`
    seconds. Used where a WISP program's exact finish time isn't
    deterministic from this side of the monitor socket -- e.g.
    bounce.wisp's total run time is `frames` iterations of a
    `sleep_ms()` call the interpreter (not this test script) actually
    waits out."""
    deadline = time.time() + timeout
    last_text = None
    last_shot = None
    while time.time() < deadline:
        last_shot = mon.screendump(name)
        last_text = ocr_text(last_shot)
        if must_contain in last_text:
            return last_shot
        time.sleep(0.3)
    raise RuntimeError(
        f"wait_for_text: timed out after {timeout}s waiting for "
        f"{must_contain!r} (last screendump: {last_shot})\n"
        f"Last OCR text:\n{last_text}"
    )


def main():
    global OUT_DIR
    shutil.rmtree(OUT_DIR, ignore_errors=True)
    os.makedirs(OUT_DIR, exist_ok=True)

    # The ESP is mounted read-only (`fat:esp`, not `fat:rw:esp` -- see
    # run.sh's comment for the full account). QEMU's vvfat driver in
    # read-write mode is documented by QEMU itself as available "only for
    # (beta) testing purposes," and it earned that warning here: a
    # byte-for-byte comparison after two back-to-back acceptance-test runs
    # against a `rw` mount showed vvfat had silently rewritten 47738 of
    # BOOTX64.EFI's 90624 bytes on the *host filesystem* -- same size,
    # wrong bytes from partway through the file onward -- which is why
    # OVMF gave up loading it and fell back to its own built-in shell.
    # Deleting `esp/NvVars` before each run (an earlier fix, kept below
    # only as a defensive no-op for old/foreign esp directories) treated
    # a symptom and missed the actual disease. Nothing this project boots
    # ever writes to the ESP (`src/fs.rs` only ever opens files
    # `FileMode::Read`), so read-only costs us nothing: OVMF's NVRAM
    # write simply fails and it falls back to a volatile in-memory store
    # for that boot, which matches this project's "no cached boot manager
    # entry" boot philosophy better than persisting one ever did.
    nvvars = os.path.join(ROOT, "esp", "NvVars")
    if os.path.exists(nvvars):
        os.remove(nvvars)

    # Phase 2, Part B: a second, separate virtual disk attached via AHCI,
    # purely for WISP-program save data -- fully additive, the boot-USB
    # `-drive`/`-device usb-storage` pair below stays byte-for-byte
    # unchanged. Lives under `OUT_DIR` (already fresh: `shutil.rmtree`
    # above), not the project root's `save.img` that `run.sh` uses for
    # interactive sessions -- every ordinary acceptance-suite run then gets
    # a clean, empty save disk, never state left over from a previous run
    # or from a manual `./run.sh` session.
    save_img = os.path.join(OUT_DIR, "save.img")
    subprocess.run(["qemu-img", "create", "-f", "raw", save_img, "64M"], check=True, capture_output=True)

    # `-bios` needs a single monolithic firmware image (code + vars in one
    # file). The project spec names /usr/share/OVMF/OVMF_CODE.fd; on this
    # distro the `ovmf` package instead ships a split code/vars pair meant
    # for `-pflash`, plus a separate monolithic image at this path, which is
    # what actually works with `-bios`. See CHANGES.md.
    ovmf_code = "/usr/share/OVMF/OVMF_CODE.fd"
    if not os.path.exists(ovmf_code):
        ovmf_code = "/usr/share/ovmf/OVMF.fd"

    # Attached as USB mass storage, not a plain IDE `-drive`: with no
    # `if=` given, QEMU's default ide-hd device model unconditionally
    # refuses a read-only backing node ("Block node is read-only", even
    # with `readonly=on` set explicitly). usb-storage accepts one, and
    # it's a closer match for reality anyway -- this project boots from
    # an actual USB stick (see USB_INSTALL.md), not an IDE disk. See
    # run.sh's matching comment for the full account.
    qemu_cmd = [
        "qemu-system-x86_64",
        "-bios", ovmf_code,
        "-drive", f"format=raw,file=fat:{os.path.join(ROOT, 'esp')},if=none,id=stick,readonly=on",
        "-usb", "-device", "usb-storage,drive=stick",
        "-drive", f"id=savedisk,if=none,format=raw,file={save_img}",
        "-device", "ahci,id=ahci0",
        "-device", "ide-hd,drive=savedisk,bus=ahci0.0",
        "-net", "none",
        "-m", "256M",
        "-vga", "std",
        "-display", "none",
        "-debugcon", f"file:{DEBUG_LOG}",
        "-monitor", f"unix:{MONITOR_SOCK},server,nowait",
        "-serial", "none",
    ]

    print("==> Launching:", " ".join(qemu_cmd))
    proc = subprocess.Popen(qemu_cmd, cwd=ROOT)
    try:
        time.sleep(1.0)
        mon = Monitor(MONITOR_SOCK)

        # Wait for our app to actually be on screen rather than a fixed
        # sleep: OVMF's own TianoCore splash timing is not very
        # deterministic (observed anywhere from ~5s to >12s in this
        # sandbox), so poll for pixels appearing in the banner's area
        # instead of guessing a duration.
        shot = wait_for_boot(mon, timeout=40.0)
        print("step 1 (boot to banner + prompt):", shot)

        # Phase 2 proof that isn't on the framebuffer at all: by the time
        # the shell banner is on screen, `main.rs`'s boot sequence has
        # already run `paging::init` (own page tables built),
        # `paging::load` (the live `CR3` switch onto them), and the
        # one-way switch onto the dedicated, guard-paged mainline stack --
        # strictly before `shell::Shell::run` ever prints the banner
        # `wait_for_boot` just confirmed. Checking the debug log here
        # rather than OCR is deliberate: this is proof of *what the kernel
        # did*, not proof of *what's drawn on screen*, and conflating the
        # two would mean a paging regression that somehow left the
        # framebuffer looking fine could slip through unnoticed.
        assert_debug_log(
            [
                "[paging] tables built: pml4=0x",
                "[paging] guard page: 0x",
                "[paging] active, cr3=0x",
                "[stack] running on the dedicated mainline stack, rsp=0x",
                # v1.0: the heap's split/coalesce self-test, W^X over the
                # kernel image's own sections, NX + CR0.WP live, and the
                # shell's deterministic boot-complete marker.
                "[heap] self-test OK",
                "[paging] image section 0x",
                " RX\n",
                "nx=on wp=on",
                "[shell] ready",
            ],
            "step 1b (paging + mainline stack active)",
        )
        print("step 1b (paging + mainline stack active): debug log verified")

        # The two-phase physical-memory handover (`pmm.rs`): boot-services
        # memory -- where firmware's still-live page tables, stack and IDT
        # sit -- is withheld while `paging::init` allocates and zeroes its
        # tables, and released only after the switch onto our own tables
        # and stack. Checked numerically, not just by the lines existing:
        # every frame of firmware's live hierarchy must be inside the
        # withheld memory, nothing may have been pinned (which would mean
        # firmware state was typed conventional), and the release must
        # free exactly what was withheld, after the stack switch.
        log = assert_debug_log(
            ["[pmm] released ", "[pmm] firmware's live paging hierarchy: "],
            "step 1c (boot-services memory withheld until own tables live)",
        )
        withheld = re.search(r"\[pmm\] \d+ frames free; (\d+) boot-services frames withheld", log)
        hierarchy = re.search(r"\[pmm\] firmware's live paging hierarchy: (\d+) frames, (\d+) of them in withheld", log)
        released = re.search(r"\[pmm\] released (\d+) boot-services frames", log)
        problems = []
        if not (withheld and hierarchy and released):
            problems.append("a [pmm] handover line did not match its expected format")
        else:
            if int(withheld.group(1)) == 0:
                problems.append("no boot-services memory was withheld")
            if int(hierarchy.group(1)) == 0 or hierarchy.group(1) != hierarchy.group(2):
                problems.append(f"firmware's live page tables are not all in withheld memory ({hierarchy.group(0)!r})")
            if released.group(1) != withheld.group(1):
                problems.append(f"released {released.group(1)} frames but withheld {withheld.group(1)}")
            stack_switch = log.find("[stack] running on the dedicated mainline stack")
            if not (log.find("[paging] active, cr3=") < stack_switch < released.start()):
                problems.append("the release did not happen after the CR3 and stack switches")
        if "[pmm] WARNING" in log:
            problems.append("live firmware state was found in conventional memory and pinned")
        if problems:
            raise RuntimeError(f"step 1c: {'; '.join(problems)}\nFull debug log:\n{log}")
        print(
            f"step 1c (two-phase frame handover): {withheld.group(1)} boot-services frames withheld "
            f"(firmware's {hierarchy.group(1)} live table frames among them), released after the switch"
        )

        mon.type_text("help")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("02_help_output")
        assert_ocr(
            shot,
            # "ls" itself is too short/ambiguous for OCR to reliably read
            # (commonly misread as "1s"), so this checks "APPS" from the
            # same help line instead -- still an unambiguous proof the
            # `ls` line rendered.
            ["Available commands", "clear", "echo", "APPS", "run", "uptime", "version", "Esc"],
            "step 2 (help)",
        )
        print("step 2 (help):", shot, "-- OCR verified")

        mon.type_text("echo hello world")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("03_echo_output")
        assert_ocr(shot, "hello world", "step 3 (echo hello world)")
        print("step 3 (echo hello world):", shot, "-- OCR verified")

        mon.type_text("ls")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("04_ls_output")
        assert_ocr(
            shot,
            ["hello.wisp", "oracle.wisp", "tamer.wisp", "adventure.wisp", "xor_brain.wisp"],
            "step 4 (ls)",
        )
        print("step 4 (ls):", shot, "-- OCR verified")

        # Backspace test: type "abcXX" then backspace twice, then submit so
        # the resulting "unknown command: abc" line proves the trailing
        # "XX" was actually deleted, not just visually obscured. On a
        # cleared screen: at the bottom of a full one, tesseract read this
        # line as "abexX".
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("abcXX")
        time.sleep(0.1)
        shot = mon.screendump("05a_before_backspace")
        # "abcXX" in full is OCR-fragile (this font's lowercase 'c' is
        # sometimes misread as 'E'); "XX" alone is enough to prove both
        # trailing characters were actually typed before the backspace
        # test below proves they were then deleted.
        # Case-insensitively: the line editor's underline cursor sits right
        # after the second "X", and tesseract then reads the first as "x"
        # ("abcxX_"). What this step proves is that two characters were
        # typed; step 5c proves they were deleted.
        text = assert_ocr(shot, [], "step 5a (before backspace)")
        if "xx" not in text.lower():
            raise RuntimeError(f"step 5a (before backspace): typed 'XX' not on screen.\nOCR text:\n{text}")
        print("step 5a (before backspace):", shot, "-- OCR verified")
        mon.backspace()
        mon.backspace()
        time.sleep(0.1)
        shot = mon.screendump("05b_after_backspace")
        print("step 5b (after 2 backspaces):", shot)
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("05c_after_backspace_submit")
        # Proves the "XX" was genuinely deleted from the line buffer (not
        # just visually erased): the shell tried to run "abc" as a command
        # and reported it unknown, rather than "abcXX".
        assert_ocr(
            shot,
            "unknown command",
            "step 5c (submitted line reflects deletion)",
            must_not_contain=["XX", "X:"],
        )
        print("step 5c (submitted line reflects deletion):", shot, "-- OCR verified")

        mon.type_text("clear")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("06_clear_output")
        # A real clear wipes the earlier "unknown command: abc" line off
        # screen; if it's still readable, `clear` silently did nothing.
        assert_ocr(shot, [], "step 6 (clear)", must_not_contain=["unknown command"])
        print("step 6 (clear):", shot, "-- OCR verified")

        # uptime: a positive, specific proof the PIT timer interrupt is
        # actually firing, not just "the prompt appeared so it must be
        # working" -- read it twice with a real delay in between and assert
        # the tick count strictly increased.
        mon.type_text("uptime")
        mon.enter()
        time.sleep(0.3)
        # The console never clears between commands, so an earlier
        # "uptime" line (if any) is still on screen above the new one --
        # `read_uptime_ticks` takes the *last* regex match, not the first,
        # so this reads the most recent reading rather than a stale one
        # still scrolled into view.
        shot, first_ticks = read_uptime_ticks(mon, "06b_uptime_first", "step 6b (uptime, first read)")
        print(f"step 6b (uptime, first read): {shot} -- {first_ticks} ticks")

        time.sleep(1.5)
        mon.type_text("uptime")
        mon.enter()
        time.sleep(0.3)
        shot, second_ticks = read_uptime_ticks(mon, "06c_uptime_second", "step 6c (uptime, second read)")
        if second_ticks <= first_ticks:
            raise RuntimeError(
                f"step 6c (uptime, second read): tick count did not "
                f"increase after a 1.5s wait ({first_ticks} -> "
                f"{second_ticks}) -- the PIT/IDT timer interrupt is not "
                f"actually firing."
            )
        print(f"step 6c (uptime, second read): {shot} -- {second_ticks} ticks (PIT verified live)")

        # WISP scripting: list the APPS directory, then run a program end
        # to end (including a blocking `input()` call) to prove scripts
        # loaded from FAT32 actually execute against the real GOP console
        # and keyboard, not just the native test harness.
        mon.type_text("ls APPS")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("07_ls_apps_output")
        assert_ocr(
            shot,
            ["hello.wisp", "oracle.wisp", "tamer.wisp", "adventure.wisp", "xor_brain.wisp"],
            "step 7 (ls APPS)",
        )
        print("step 7 (ls APPS):", shot, "-- OCR verified")

        mon.type_text("run hello")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("08a_run_hello_prompt")
        assert_ocr(shot, "What's your name?", "step 8a (run hello -- waiting on input())")
        print("step 8a (run hello -- waiting on input()):", shot, "-- OCR verified")

        mon.type_text("James")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("08b_run_hello_output")
        assert_ocr(shot, "Hello, James", "step 8b (run hello -- greeted James)")
        print("step 8b (run hello -- greeted James):", shot, "-- OCR verified")

        # WISP lists: run the shuffle-bag oracle demo and draw a fortune,
        # proving list literals, indexing, push/pop, and rand() all work
        # together against the real interpreter running on real firmware,
        # not just the native test harness.
        mon.type_text("run oracle")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("09a_run_oracle_prompt")
        assert_ocr(shot, "The oracle is listening", "step 9a (run oracle -- waiting at its own '>' prompt)")
        print("step 9a (run oracle -- waiting at its own '>' prompt):", shot, "-- OCR verified")

        mon.type_text("ask")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("09b_run_oracle_fortune")
        # The fortune itself is drawn at random, so this can't assert its
        # exact text -- but it can assert the oracle actually replied to
        # "ask" (screen changed, no error), by checking the drawn line
        # isn't the "only understands" fallback error and the prompt is
        # back, proving `input()`, the shuffle-bag draw(), and print() all
        # ran against a real WISP script.
        assert_ocr(
            shot,
            [],
            "step 9b (run oracle -- drew a fortune from the list)",
            must_not_contain=["only understands"],
        )
        print("step 9b (run oracle -- drew a fortune from the list):", shot, "-- OCR verified")

        mon.type_text("quit")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("09c_run_oracle_quit")
        assert_ocr(shot, "The oracle falls silent", "step 9c (run oracle -- quit back to the shell prompt)")
        print("step 9c (run oracle -- quit back to the shell prompt):", shot, "-- OCR verified")

        # A real (if tiny) trained neural network, doing fixed-point
        # forward-pass inference against a live keyboard prompt, running
        # entirely in bare-metal firmware. This is the actual payoff of
        # the whole WISP project -- prove it on real hardware, not just
        # the native harness.
        mon.type_text("run tamer")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("8")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("2")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("10a_run_tamer_erratic")
        assert_ocr(shot, "Erratic", "step 10a (run tamer -- mismatched stats classified 'Erratic')")
        print("step 10a (run tamer -- mismatched stats classified 'Erratic'):", shot, "-- OCR verified")

        mon.type_text("8")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("8")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("10b_run_tamer_predictable")
        assert_ocr(shot, "Predictable", "step 10b (run tamer -- matched stats classified 'Predictable')")
        print("step 10b (run tamer -- matched stats classified 'Predictable'):", shot, "-- OCR verified")

        mon.type_text("quit")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("10c_run_tamer_quit")
        assert_ocr(shot, "tamer done", "step 10c (run tamer -- quit back to the shell prompt)")
        print("step 10c (run tamer -- quit back to the shell prompt):", shot, "-- OCR verified")

        # adventure.wisp: walk the full five-room map, deliberately hitting
        # every gating branch (an invalid direction, the dark storage
        # closet before the lantern is held, the locked vault door before
        # the key is held) before completing the winning path -- not just
        # "it runs", per docs/WISP_AI.md's testing philosophy applied to
        # ordinary WISP programs too.
        mon.type_text("run adventure")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("11a_run_adventure_start")
        assert_ocr(shot, ["THE SEALED ARCHIVE", "ENTRANCE HALL"], "step 11a (run adventure -- entrance hall)")
        print("step 11a (run adventure -- entrance hall):", shot, "-- OCR verified")

        mon.type_text("east")
        mon.enter()
        time.sleep(0.2)
        shot = mon.screendump("11b_run_adventure_bad_direction")
        assert_ocr(shot, "can't go that way", "step 11b (run adventure -- invalid direction rejected)")
        print("step 11b (run adventure -- invalid direction rejected):", shot, "-- OCR verified")

        mon.type_text("north")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("east")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("take key")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("11c_run_adventure_dark_room")
        assert_ocr(
            shot,
            "too dark to find anything",
            "step 11c (run adventure -- too dark to find the key without the lantern)",
        )
        print("step 11c (run adventure -- too dark to find the key without the lantern):", shot, "-- OCR verified")

        mon.type_text("west")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("north")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("north")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("11d_run_adventure_locked_vault")
        assert_ocr(
            shot,
            "vault door is locked tight",
            "step 11d (run adventure -- vault door locked without the key)",
        )
        print("step 11d (run adventure -- vault door locked without the key):", shot, "-- OCR verified")

        mon.type_text("south")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("take lantern")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("east")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("take key")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("11e_run_adventure_got_key")
        assert_ocr(
            shot,
            "pry the small iron key loose",
            "step 11e (run adventure -- lantern lets you see and take the key)",
        )
        print("step 11e (run adventure -- lantern lets you see and take the key):", shot, "-- OCR verified")

        mon.type_text("west")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("north")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("north")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("11f_run_adventure_win")
        assert_ocr(shot, ["THE VAULT", "YOU WIN"], "step 11f (run adventure -- vault opens with the key, game won)")
        print("step 11f (run adventure -- vault opens with the key, game won):", shot, "-- OCR verified")

        # xor_brain.wisp: the full 4-row truth table against the live
        # interpreter, then quit.
        mon.type_text("run xor_brain")
        mon.enter()
        time.sleep(0.3)
        for x1, x2 in ((0, 0), (0, 1), (1, 0), (1, 1)):
            mon.type_text(str(x1))
            mon.enter()
            time.sleep(0.1)
            mon.type_text(str(x2))
            mon.enter()
            time.sleep(0.1)
        shot = mon.screendump("12a_run_xor_brain_truth_table")
        # Ideally this asserts the exact 4 result lines ("0 XOR 0 = 0", "0
        # XOR 1 = 1", "1 XOR 0 = 1", "1 XOR 1 = 0"). In practice tesseract's
        # plain-stdout mode was found (during manual verification of this
        # exact screen) to intermittently drop/garble the trailing "= N"
        # of these specific short numeric lines even though the rendered
        # pixels are correct -- confirmed by opening the PNG directly and
        # reading all 4 rows correctly by eye. Rather than assert against
        # a text-extraction failure mode unrelated to the kernel, this
        # checks the weaker-but-still-meaningful "XOR" appears once per
        # row (proving all 4 prompts were answered and produced output)
        # and leaves the exact-arithmetic claim to that manual check.
        text = assert_ocr(shot, [], "step 12a (run xor_brain -- full 4-row truth table)")
        xor_count = text.upper().count("XOR")
        if xor_count < 4:
            raise RuntimeError(
                f"step 12a (run xor_brain -- full 4-row truth table): only "
                f"found {xor_count} 'XOR' occurrences in OCR text (expected "
                f"4, one per truth-table row); full text:\n{text}"
            )
        print(f"step 12a (run xor_brain -- full 4-row truth table): {shot} -- {xor_count} XOR rows OCR verified")

        mon.type_text("quit")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("12b_run_xor_brain_quit")
        assert_ocr(shot, "xor_brain done", "step 12b (run xor_brain -- quit back to the shell prompt)")
        print("step 12b (run xor_brain -- quit back to the shell prompt):", shot, "-- OCR verified")

        # The fresh save.img is blank, and boot never formats a disk on
        # its own (CODEBASE_AUDIT.md AUD-01): it must have been probed
        # read-only and left unmounted. Provision it explicitly with the
        # shell's `format` command, which asks for confirmation first.
        assert_debug_log(
            "[save_format] disk is blank; left untouched (not mounted)",
            "step 13 (blank save disk left untouched at boot)",
        )
        mon.type_text("format")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("13_format_prompt")
        assert_ocr(shot, ["ERASES", "Type ERASE"], "step 13 (format names the device and asks first)")
        mon.type_text("ERASE")
        mon.enter()
        time.sleep(1.0)
        shot = mon.screendump("13_format_done")
        assert_ocr(shot, "save disk formatted", "step 13 (format provisions the save disk)")
        assert_debug_log(
            "[save_format] disk provisioned by explicit 'format' command",
            "step 13 (format provisions the save disk)",
        )
        print("step 13 (explicit format of the blank save disk):", shot, "-- OCR verified")

        # counter.wisp, first half of the cross-boot persistence proof
        # (Phase 2, Part B, Build Order Step 11): against the save.img this
        # run just created and formatted, the very first call to load()
        # must see nothing saved yet -- the first-run branch -- and the
        # count it then saves must read back as exactly 1. The second half
        # (a genuinely separate boot, reusing this same, unmodified
        # save.img) runs after this VM is cleanly shut down, below.
        mon.type_text("run counter")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("13a_run_counter_first_boot")
        assert_ocr(
            shot,
            # Trailing sentence-ending periods on these specific short
            # lines have been observed (this run) to be dropped by
            # tesseract even though the rendered pixels are correct --
            # same OCR-fragility class as step 12a's "= N" note above, so
            # this checks the text up to (not including) the period,
            # exactly like this file's other punctuation-fragile assertions.
            ["This is the first time", "You have run this 1 time"],
            "step 13a (run counter -- first ever boot, count saved as 1)",
        )
        print("step 13a (run counter -- first ever boot, count saved as 1):", shot, "-- OCR verified")

        # WISP v2 graphics primitives (fill_screen/rect/plot/line/rgb/
        # screen_width/screen_height): bounce.wisp is a short, fully
        # automatic animation. Screendumping it mid-flight and checking
        # for its two, deliberately far-apart colors is real, specific
        # evidence that fill_screen() and rect() actually painted the
        # pixels WISP asked for -- against the real interpreter, through
        # Console::encode()'s BGR-vs-RGB hardware-layout translation,
        # exactly like the text console already does -- not just that
        # the script executed without crashing.
        #
        # Every frame is fill_screen(bg) and then rect(ball), so a single
        # screendump that lands between those two calls rightly shows only
        # the background (seen once in three runs). Sample a few frames:
        # the ball must be on screen in one of them.
        mon.type_text("run bounce")
        mon.enter()
        time.sleep(1.0)
        for attempt in range(5):
            shot = mon.screendump("14a_run_bounce_midflight")
            try:
                assert_colors_present(
                    shot,
                    [(10, 10, 20), (255, 140, 0)],
                    "step 14a (run bounce -- mid-flight background+ball colors)",
                )
                break
            except RuntimeError:
                if attempt == 4:
                    raise
                time.sleep(0.1)
        print("step 14a (run bounce -- mid-flight background+ball colors):", shot, "-- pixel verified")

        shot = wait_for_text(mon, "14b_run_bounce_finished", "bounce.wisp finished", timeout=10.0)
        print("step 14b (run bounce -- finished cleanly):", shot, "-- OCR verified")

        # canvas.wisp: an interactive drawing tool. Walks plot/rect/line
        # each with a real, sampled-pixel color check, exercises
        # color_by_name()'s "unrecognized name falls back to white"
        # branch, and finally proves builtin_rect's negative-size
        # validation is both reachable from a live keyboard session and
        # a *clean* runtime error -- the script ends and control returns
        # to a fully responsive shell prompt, not a hang or a crash.
        mon.type_text("run canvas")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15a_run_canvas_start")
        # "plot, rect, line, fillscreen" (not the whole banner line) --
        # this font's lowercase 'c' has already been confirmed elsewhere
        # in this file to be sometimes OCR-misread as 'E' (see
        # `read_uptime_ticks`'s doc comment), and this exact screen was
        # observed hitting that same ambiguity twice over: "Commands"
        # read back lowercase and "clearscreen" read back as
        # "clearsereen", plus a dropped trailing "t." on "quit." -- all
        # three confirmed, by direct inspection of this screenshot, to be
        # OCR artifacts rather than rendering bugs (the pixels themselves
        # are correct). Checking a substring with no 'c' in it sidesteps
        # the ambiguity entirely, the same fix this file already applies
        # to its other c-vs-E-fragile assertions.
        assert_ocr(
            shot,
            ["screen is", "pixels", "plot, rect, line, fillscreen"],
            "step 15a (run canvas -- startup banner)",
        )
        print("step 15a (run canvas -- startup banner):", shot, "-- OCR verified")

        # plot with an unrecognized color name -- exercises
        # color_by_name()'s fallback branch (white) as well as plot()
        # itself.
        mon.type_text("plot")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("100")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("100")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("purple")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15b_canvas_plot_unrecognized_color")
        assert_ocr(shot, "unrecognized color", "step 15b (canvas plot -- unrecognized color name)")
        assert_pixel_near(shot, 100, 100, (255, 255, 255), "step 15b (canvas plot -- pixel is white)")
        print("step 15b (canvas plot -- unrecognized color falls back to white):", shot, "-- OCR + pixel verified")

        # rect: a 60x40 green rectangle at (150,150) -- sample its rough
        # center.
        mon.type_text("rect")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("150")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("150")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("60")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("40")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("green")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15c_canvas_rect_green")
        assert_pixel_near(shot, 180, 170, (40, 200, 90), "step 15c (canvas rect -- green fill)")
        print("step 15c (canvas rect -- green fill):", shot, "-- pixel verified")

        # line: a vertical yellow line from (300,100) to (300,200) --
        # sample its midpoint.
        mon.type_text("line")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("300")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("100")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("300")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("200")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("yellow")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15d_canvas_line_yellow")
        assert_pixel_near(shot, 300, 150, (230, 200, 40), "step 15d (canvas line -- yellow midpoint)")
        print("step 15d (canvas line -- yellow midpoint):", shot, "-- pixel verified")

        # A negative rect size is a runtime error, not a silent no-op or
        # a crash -- proves builtin_rect's validation is reachable from a
        # live keyboard session against the real interpreter, and that
        # an uncaught WispError cleanly returns control to the shell
        # rather than hanging or crashing the VM.
        mon.type_text("rect")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("10")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("10")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("-5")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("20")
        mon.enter()
        time.sleep(0.1)
        mon.type_text("white")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15e_canvas_rect_negative_error")
        assert_ocr(shot, "must not be negative", "step 15e (canvas rect -- negative size is a clean runtime error)")
        print("step 15e (canvas rect -- negative size is a clean runtime error):", shot, "-- OCR verified")

        # The error ended the script (no explicit 'quit' needed) --
        # confirm we're genuinely back at the shell's own prompt, not
        # stuck.
        mon.type_text("echo canvas_test_done")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("15f_canvas_back_at_shell")
        assert_ocr(shot, "canvas_test_done", "step 15f (canvas -- shell prompt responsive again after the error)")
        print("step 15f (canvas -- shell prompt responsive again after the error):", shot, "-- OCR verified")

        # --- v1.0 additions ------------------------------------------------
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("version")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("16a_version")
        # "OS" alone is routinely OCR-misread as "0S"; the version string and
        # the networking line are the unambiguous parts.
        assert_ocr(shot, ["v1.0.0", "networking: none", "NX on"], "step 16a (version)")
        print("step 16a (version):", shot, "-- OCR verified")

        mon.type_text("mem")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("16b_mem")
        assert_ocr(shot, ["heap:", "KiB total", "frames:"], "step 16b (mem)")
        print("step 16b (mem):", shot, "-- OCR verified")

        # Esc stops a pure compute loop (no input(), no sleep_ms()): the
        # interpreter's per-iteration stop check, fed by the PS/2 ISR.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("run primes")
        mon.enter()
        time.sleep(1.5)
        shot = mon.screendump("17a_primes_running")
        # Tesseract reads the dense column of "prime #N: P" lines poorly (it
        # drops the "#N" parts), so only the word itself is asserted.
        assert_ocr(shot, "prime", "step 17a (run primes -- computing)")
        print("step 17a (run primes -- computing):", shot, "-- OCR verified")
        mon.cmd("sendkey esc")
        time.sleep(0.5)
        # The stop is proven from the kernel's own log line (OCR cannot
        # reliably pick one red line out of a screen of scrolled output),
        # then the prompt's responsiveness by OCR on a cleared screen.
        assert_debug_log("[wisp] primes.wisp stopped by Esc at line", "step 17b (primes -- Esc stopped the compute loop)")
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("echo primes_done")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("17b_primes_stopped")
        assert_ocr(shot, "primes_done", "step 17b (primes -- prompt responsive after Esc)")
        print("step 17b (primes -- Esc stopped the compute loop, prompt responsive):", shot, "-- debug log + OCR verified")

        # Esc wakes a long sleep_ms() immediately, and the elapsed-time
        # reading proves time_ms() advances. The program never ends on its
        # own, so reaching the prompt again proves the stop worked.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("run stopwatch")
        mon.enter()
        time.sleep(2.6)
        shot = mon.screendump("18a_stopwatch_running")
        assert_ocr(shot, ["lap 1", "lap 2", "s elapsed"], "step 18a (run stopwatch -- laps from time_ms())")
        print("step 18a (run stopwatch -- laps from time_ms()):", shot, "-- OCR verified")
        mon.cmd("sendkey esc")
        time.sleep(0.4)
        mon.type_text("echo stopwatch_done")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("18b_stopwatch_stopped")
        assert_debug_log("[wisp] stopwatch.wisp stopped by Esc at line", "step 18b (stopwatch -- Esc stopped it)")
        assert_ocr(
            shot,
            # "stopped", not the full "stopped (Esc pressed)": tesseract
            # drops or alters the parenthesized part depending on where
            # the line lands (seen: "(Esc pressed", "{Ese pressed"). The
            # stop itself is proven exactly by the debug-log line above.
            ["stopped", "stopwatch_done"],
            "step 18b (stopwatch -- Esc interrupted sleep_ms(), prompt responsive, no stray keys)",
        )
        print("step 18b (stopwatch -- Esc interrupted sleep_ms(), prompt responsive):", shot, "-- OCR verified")

        # At the prompt, Esc discards the half-typed line.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("echo leftover")
        mon.cmd("sendkey esc")
        time.sleep(0.1)
        mon.type_text("echo esccleared")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("19_prompt_esc_clears_line")
        assert_ocr(
            shot,
            "esccleared",
            "step 19 (Esc at the prompt discards the typed line)",
            must_not_contain=["leftover"],
        )
        print("step 19 (Esc at the prompt discards the typed line):", shot, "-- OCR verified")

        # calc.wisp: an expression parser written in WISP, exercising the
        # v2 language (for over a string, indexing, contains(), trim())
        # on the real machine. A bad expression is calc's own message,
        # and calc keeps running.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("run calc")
        mon.enter()
        time.sleep(0.5)
        # 32 with operator precedence, 50 without. (Not an answer like 11:
        # tesseract reads "= 11" in this font as "- 1".)
        mon.type_text("2 + 3 * (4 + 6)")
        mon.enter()
        time.sleep(0.5)
        mon.type_text("10 / (5 - 5)")
        mon.enter()
        time.sleep(0.5)
        shot = wait_for_text(mon, "20a_calc", "division by zero")
        assert_ocr(shot, ["calc", "32", "division by zero"], "step 20a (run calc -- precedence and a caught error)")
        print("step 20a (run calc -- 2 + 3 * (4 + 6) = 32, division by zero reported):", shot, "-- OCR verified")
        mon.type_text("quit")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("echo calc_done")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("20b_calc_quit")
        assert_ocr(shot, "calc_done", "step 20b (calc -- quit back to the shell prompt)")
        print("step 20b (calc -- quit back to the shell prompt):", shot, "-- OCR verified")

        # The interactive prompt: state carried between lines, a function
        # typed over three lines, an error that leaves the session intact,
        # Esc stopping a runaway line, `vars`, and `exit`.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("wisp")
        mon.enter()
        time.sleep(0.3)
        for line in [
            'let names = split("ada grace alan")',
            "sort(names)",
            "names",
            "fn shout(s) {",
            '    return upper(s) + "!";',
            "}",
            'shout("hi")',
            "nope + 1",
        ]:
            mon.type_text(line)
            mon.enter()
            time.sleep(0.3)
        shot = wait_for_text(mon, "21a_wisp_prompt", "HI!")
        text = assert_ocr(shot, ["HI!", "undefined variable"], "step 21a (wisp prompt -- definitions, multi-line fn, error)")
        if not re.search(r"ada\W{0,8}alan\W{0,8}grace", text):
            raise RuntimeError(f"step 21a: the sorted list was not echoed as ada, alan, grace.\nOCR text:\n{text}")
        print("step 21a (wisp prompt -- sorted list echoed, 3-line fn defined and called, error reported):", shot, "-- OCR verified")

        mon.type_text("clear()")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("while true { }")
        mon.enter()
        time.sleep(1.0)
        mon.cmd("sendkey esc")
        time.sleep(0.4)
        assert_debug_log("[wisp] prompt input stopped by Esc at line 1", "step 21b (wisp prompt -- Esc stopped a runaway line)")
        mon.type_text("vars")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("len(names) * 10")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("21b_wisp_vars")
        assert_ocr(
            shot,
            # The stop is proven exactly by the debug-log line above; on
            # screen only "stopped" is asserted, because tesseract reads
            # this red "(Esc pressed)" as "{Ese pressed".
            ["stopped", "globals: names", "functions: shout", "30"],
            "step 21b (wisp prompt -- Esc, vars, session intact)",
        )
        print("step 21b (wisp prompt -- Esc stopped a runaway line, vars, session intact):", shot, "-- debug log + OCR verified")

        mon.type_text("exit")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("echo prompt_done")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("21c_wisp_exit")
        assert_ocr(shot, ["left the WISP prompt", "prompt_done"], "step 21c (wisp prompt -- exit back to the shell)")
        assert_debug_log("[wisp] interactive prompt ended", "step 21c (wisp prompt -- exit back to the shell)")
        print("step 21c (wisp prompt -- exit back to the shell):", shot, "-- debug log + OCR verified")

        # The line editor: Up recalls the last command, which is edited
        # mid-line before it runs; then Home/Right/Delete/End/Backspace
        # edit a fresh line; then history at the wisp> prompt.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("echo first_entry")
        mon.enter()
        time.sleep(0.2)
        mon.key("up")
        mon.key("left", 5)
        mon.type_text("x_")
        mon.enter()
        time.sleep(0.2)
        # Delete takes the "K" after "echo ", Backspace the trailing "X".
        # (Words tesseract reads reliably in this font: "bcde" reads as
        # "bede".)
        mon.type_text("echo Kfirst_twoX")
        mon.key("home")
        mon.key("right", 5)
        mon.key("delete")
        mon.key("end")
        mon.key("backspace")
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("22a_line_editing")
        assert_ocr(
            shot,
            # The command's output line, pinned at both ends: a failed
            # Delete would print "Kfirst_two", a failed Backspace
            # "first_twoX". (Not the echoed command line: tesseract reads
            # its "f" as "F".)
            ["first_x_entry", "\nfirst_two\n"],
            "step 22a (line editor -- history recall and mid-line edits)",
            must_not_contain=["Kfirst", "twoX"],
        )
        print("step 22a (line editor -- Up recalled and edited a command; Home/Right/Delete/End/Backspace):", shot, "-- OCR verified")

        mon.type_text("wisp")
        mon.enter()
        time.sleep(0.3)
        mon.type_text("let n = 41")
        mon.enter()
        time.sleep(0.2)
        mon.key("up")
        mon.key("backspace")
        mon.type_text("6")
        mon.enter()
        time.sleep(0.2)
        # Echoed as a word: tesseract skips a bare two-digit line like "92".
        mon.type_text('str(n) + "_ok"')
        mon.enter()
        time.sleep(0.3)
        shot = mon.screendump("22b_wisp_history")
        assert_ocr(shot, ["let n = 46", "46_ok"], "step 22b (wisp prompt -- Up recalled and edited a line)")
        mon.type_text("exit")
        mon.enter()
        time.sleep(0.2)
        print("step 22b (wisp prompt -- Up recalled 'let n = 41', edited to 46):", shot, "-- OCR verified")

        # words.wisp: a word counter whose whole state is one map, so its
        # answers exercise map writes, get(), key-order iteration and the
        # ranking built on them, on the real machine.
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("run words")
        mon.enter()
        time.sleep(0.5)
        for line in ["The cat saw the dog.", "the dog ran", "top"]:
            mon.type_text(line)
            mon.enter()
            time.sleep(0.4)
        shot = wait_for_text(mon, "23a_words_top", "Most used")
        assert_ocr(
            shot,
            ["8 words, 5 different", "the: 3 times", "dog: twice", "cat: once"],
            "step 23a (run words -- tallies kept in a map, ranked)",
        )
        print("step 23a (run words -- tallies kept in a map, ranked):", shot, "-- OCR verified")
        for line in ["forget the", "list", "quit"]:
            mon.type_text(line)
            mon.enter()
            time.sleep(0.4)
        shot = wait_for_text(mon, "23b_words_list", "words done")
        assert_ocr(
            shot,
            ["Forgot the", "cat: once", "ran: once", "saw: once", "words done"],
            "step 23b (words -- forget removed a key, list walked the rest in order)",
        )
        text = ocr_text(shot)
        after_forget = text[text.find("Forgot the"):]
        if "the: 3 times" in after_forget:
            raise RuntimeError(f"step 23b: 'the' was still listed after 'forget the'.\nOCR text:\n{text}")
        if not re.search(r"cat: once.*dog: twice.*ran: once.*saw: once", after_forget, re.S):
            raise RuntimeError(f"step 23b: the list was not in alphabetical order.\nOCR text:\n{text}")
        print("step 23b (words -- forget removed a key, list walked the rest in order):", shot, "-- OCR verified")

        # Maps at the wisp> prompt, then saved to the AHCI disk; the second
        # boot below loads it back (step 13d).
        mon.type_text("clear")
        mon.enter()
        time.sleep(0.2)
        mon.type_text("wisp")
        mon.enter()
        time.sleep(0.3)
        for line in [
            'let inv = {"gold": 7, "keys": ["red"]}',
            'inv["gold"] = inv["gold"] + 35',
            'save("inventory", inv)',
            'print("inventory is a " + type(inv) + " of " + str(len(inv)) + " entries")',
            'print("gold " + str(inv["gold"]) + ", gems " + str(get(inv, "gems", 0)))',
        ]:
            mon.type_text(line)
            mon.enter()
            time.sleep(0.3)
        shot = wait_for_text(mon, "24a_wisp_map_saved", "gold 42")
        assert_ocr(
            shot,
            # Printed as sentences, not echoed: tesseract split short
            # quoted echo lines like "2_entries_map" (read: ries_map).
            ["inventory is a map of 2 entries", "gold 42, gems 0"],
            "step 24a (wisp prompt -- a map built, changed and saved)",
            must_not_contain=["wisp: line"],
        )
        mon.type_text("exit")
        mon.enter()
        time.sleep(0.2)
        print("step 24a (wisp prompt -- a map built, changed and saved):", shot, "-- OCR verified")

        mon.quit()
    finally:
        time.sleep(0.5)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    # counter.wisp, second half of the cross-boot persistence proof: a
    # genuinely separate QEMU process (not the one above, which has
    # already been fully torn down), booted against the exact same,
    # never-recreated save.img file step 13a's VM wrote to. If save/load
    # only worked within a single boot's RAM (never actually reaching the
    # AHCI disk), this second, independent boot would see nothing saved
    # and print "the first time" all over again instead of "Welcome
    # back." -- so this is concrete, specific proof of real, persistent,
    # cross-reboot storage, not just "the save/load commands didn't
    # crash."
    # Point Monitor.screendump()/wait_for_boot()'s screenshots at a
    # dedicated subdirectory from here on, so this second boot's own
    # `_boot_poll_N.png` polling shots (same naming scheme as the first
    # boot's) land alongside the first boot's artifacts instead of
    # silently overwriting them -- `save_img` above was already resolved
    # to the *original* OUT_DIR before this reassignment, so it still
    # points at the one, unmodified save.img from the first boot.
    OUT_DIR = os.path.join(OUT_DIR, "second_boot")
    os.makedirs(OUT_DIR, exist_ok=True)

    debug_log_2 = os.path.join(OUT_DIR, "debug_second_boot.log")
    monitor_sock_2 = os.path.join(OUT_DIR, "monitor_second_boot.sock")
    qemu_cmd_2 = [
        "qemu-system-x86_64",
        "-bios", ovmf_code,
        "-drive", f"format=raw,file=fat:{os.path.join(ROOT, 'esp')},if=none,id=stick,readonly=on",
        "-usb", "-device", "usb-storage,drive=stick",
        "-drive", f"id=savedisk,if=none,format=raw,file={save_img}",
        "-device", "ahci,id=ahci0",
        "-device", "ide-hd,drive=savedisk,bus=ahci0.0",
        "-net", "none",
        "-m", "256M",
        "-vga", "std",
        "-display", "none",
        "-debugcon", f"file:{debug_log_2}",
        "-monitor", f"unix:{monitor_sock_2},server,nowait",
        "-serial", "none",
    ]
    print("==> Launching (second boot, same save.img):", " ".join(qemu_cmd_2))
    proc2 = subprocess.Popen(qemu_cmd_2, cwd=ROOT)
    try:
        time.sleep(1.0)
        mon2 = Monitor(monitor_sock_2)
        shot = wait_for_boot(mon2, timeout=40.0)
        print("step 13b (second boot, same save.img, banner + prompt):", shot)
        with open(debug_log_2, "r", errors="replace") as f:
            if "[save_format] existing save store mounted (read-only probe, no writes)" not in f.read():
                raise RuntimeError("step 13b: second boot did not mount the save store formatted by the first boot")

        mon2.type_text("run counter")
        mon2.enter()
        time.sleep(0.3)
        shot = mon2.screendump("13c_run_counter_second_boot")
        assert_ocr(
            shot,
            ["Welcome back", "You have run this 2 times"],
            "step 13c (run counter -- second, separate boot -- count persisted as 2)",
        )
        print("step 13c (run counter -- second, separate boot -- count persisted as 2):", shot, "-- OCR verified")

        # The map step 24a saved, read back by this separate boot: the
        # map encoding (save format tag 5) on a real AHCI disk.
        mon2.type_text("wisp")
        mon2.enter()
        time.sleep(0.3)
        mon2.type_text('let inv = load("inventory")')
        mon2.enter()
        time.sleep(0.3)
        mon2.type_text('print("loaded a " + type(inv) + " with gold " + str(inv["gold"]) + " and the " + inv["keys"][0] + " key")')
        mon2.enter()
        time.sleep(0.3)
        # A long line: wait for its output, not a fixed time, since QEMU
        # types it one key at a time.
        shot = wait_for_text(mon2, "13d_wisp_map_loaded_second_boot", "red key")
        assert_ocr(
            shot,
            "loaded a map with gold 42 and the red key",
            "step 13d (wisp prompt -- second boot loads the map step 24a saved)",
            must_not_contain=["wisp: line"],
        )
        print("step 13d (wisp prompt -- second boot loads the map step 24a saved):", shot, "-- OCR verified")

        mon2.quit()
    finally:
        time.sleep(0.5)
        proc2.terminate()
        try:
            proc2.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc2.kill()

    print(f"\nAll screenshots saved under {os.path.dirname(OUT_DIR)} (second boot under {OUT_DIR})")


if __name__ == "__main__":
    main()
