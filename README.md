# WISP OS v1.2.0

A freestanding x86_64 kernel that boots directly on UEFI firmware (or in
QEMU/OVMF) and *becomes* the machine's whole environment: no underlying
OS, no bootloader handoff beyond UEFI itself, no network stack anywhere
in the binary. It leaves Boot Services behind and runs on its own GDT,
IDT, page tables, physical and heap allocators, interrupt controllers,
timer, keyboard and disk drivers. It owns the screen by writing pixels
straight into the GOP framebuffer with an embedded bitmap font, and it
runs a small scripting language -- **WISP** -- that lets you drop
plain-text programs onto the boot volume and run them with no compiler
and no OS in the loop.

## Why

The point of this project is architectural honesty: build a computer's
worth of software -- console, shell, memory management, interrupt
handling, storage, a real programming language, even (tiny, real,
verified) trained neural network inference -- entirely hand-built from
firmware primitives upward, with nothing borrowed from a hosted OS and
nothing phoned home to a network. "Zero networking capabilities" isn't
a missing feature here, it's the foundational constraint everything else
was designed around: see `USB_INSTALL.md`'s "No networking, by design".

## Quick start

A release archive (`wisp-os-v1.2.0.tar.gz` / `.zip`) already contains a
prebuilt `esp/EFI/BOOT/BOOTX64.EFI`: to boot real hardware without
installing Rust at all, copy the `esp/` directory onto a FAT32 USB stick
(or use the installer scripts -- see `USB_INSTALL.md`). Verify the files
first with `sha256sum -c SHA256SUMS` inside the extracted folder.

To build and run from source:

```bash
./build.sh           # cargo build --locked --release --target x86_64-unknown-uefi,
                     # then assembles esp/EFI/BOOT/BOOTX64.EFI
./run.sh             # boots ./esp in QEMU under OVMF (headless by default;
                     # UEFI_SHELL_DISPLAY=gui ./run.sh for a real window,
                     # UEFI_SHELL_USB_KBD=1 ./run.sh to attach a USB keyboard)
```

To build the live installer ISO (Python 3 and `xorriso` required):

```bash
./scripts/make_iso.sh
UEFI_SHELL_DISPLAY=gui WISP_ISO=dist/wisp-os-v1.2.0-live.iso ./run.sh
```

This creates a separate 128 MiB `install.img` destination for the VM. At
the boot menu, press **T** to try Wisp or **I** to install the running
system. Installation shows eligible disks and requires the exact typed
confirmation `ERASE <number>` before replacing a disk's contents. It copies
the live EFI binary and apps, creates a GPT/FAT32 boot partition and a
separate save-data partition, and verifies the written sectors. Remove the
live media after installation; for the VM, boot the result with:

```bash
UEFI_SHELL_DISPLAY=gui WISP_BOOT_DISK=install.img ./run.sh
```

The ISO boots x86_64 UEFI optical media and includes an EFI partition for
USB image-writing tools. The existing host USB scripts also produce media
with the same boot menu. See [USB_INSTALL.md](USB_INSTALL.md) for supported
targets, firmware boot selection and the destructive-install details.

Run `python3 tests/installer_test.py` to build and verify the full ISO-to-disk
flow in QEMU using disposable image files. It checks cancellation, target
filtering, copied file bytes, standalone boot, apps and saves across reboot.
Its framebuffer checks use the embedded font directly and need no OCR packages.

At the `>` prompt: `help` lists shell commands, `ls APPS` shows the
bundled WISP programs, and `run hello` (or `guess`, `oracle`,
`adventure`, `xor_brain`, `tamer`, `counter`, `bounce`, `canvas`,
`stopwatch`, `primes`, `calc`, `words`, `tasks`, `undo`) runs one. `wisp` opens an interactive
WISP prompt that runs each line as you type it -- the way to try the
language, or write a quick program, on the machine itself. Every line of
input can be edited with the arrow keys, Home, End and Delete, and Up/Down
recall earlier lines at the `>` and `wisp>` prompts.

`ide` opens **Wisp Studio**, the built-in editor, on `main`; `ide hello`
(or `edit hello`) opens a saved program or bundled example. New names
start empty. Studio has line numbers, syntax colors, automatic indentation,
and scrolling with arrows and Page Up/Down. **F2 saves, F3 opens or creates,
F5 runs, F6 checks syntax, F1 shows help, and F10 or Esc leaves.** Leaving
or opening another program with unsaved edits offers Save, Discard or Cancel.
Run output stays visible until a keypress; errors select their source line.

Saved programs use `prog.<name>` keys on the selected save disk and survive
reboots. A blank disk needs the shell's `format` command first. Names are
1-27 ASCII letters, digits, underscores or hyphens; `.wisp` is optional.
Studio edits up to 16 KiB, while current save slots hold **4042 source bytes**;
larger buffers can run but cannot be saved. Without persistence, editing
and running work in memory. `ls` shows bundled and saved origins,
`ls PROGRAMS` lists saved sources, and `run <name>` prefers a saved program
over a bundled app of the same name. Boot-volume `APPS` files stay unchanged.
Remove a saved program with `delete prog.<name>`; source export is not yet available.

`disks` lists discovered disks and marks the selected save disk; `disk <n>`
selects one without formatting it. Ambiguous choices require this explicit
selection. A boot USB containing only FAT files has no save region: use the
live installer to create an ESP plus Wisp save partition on an eligible
destination. `format` refuses foreign partition tables.

For native transport trials in QEMU, set `WISP_SAVE_TRANSPORT=nvme` or
`WISP_SAVE_TRANSPORT=usb` when running `./run.sh`; the default is `ahci`.

**Esc stops any running program.**
`save`/`load` persist key/value pairs to the selected disk (after a one-time
`format`, which names the disk and asks before erasing anything) -- each
save is crash-atomic, and `keys`/`delete` list and remove them -- `uptime` reports
elapsed time from the timer interrupt, `mem` shows heap and physical
memory use, and `version` shows the kernel version and which hardware
protections are live.

To cut a release yourself: `scripts/make_release.sh` builds and writes
`dist/wisp-os-v<version>.tar.gz` and `.zip` (source + prebuilt EFI +
per-file `SHA256SUMS`), reproducibly -- an unchanged tree repackages to
byte-identical archives, and `BOOTX64.EFI` itself builds byte-identically
from the same source and toolchain (`.cargo/config.toml`), so the
prebuilt binary can be checked against your own build.
Pass `--iso` to `scripts/make_release.sh` to include the live ISO and its
checksum in the release output. The source archives include the host test
crate as well as the QEMU suites.

To put this on a real USB stick and boot it on physical x86_64 hardware
-- including exactly what the machine has to provide, and what is not
supported -- see `USB_INSTALL.md`.

For everything in one place -- installing, operating, programming in WISP,
how each layer works, and the roadmap of expansion -- see `docs/MANUAL.md`.

## What it actually does at boot

1. Starts as a UEFI application: opens GOP, reads `APPS/*.wisp` off the
   FAT volume and offers Try or Install. Try starts automatically after
   four seconds; Install copies the live system only after confirmation.
   It then locates the ACPI RSDP.
2. Snapshots everything it will need, then calls `exit_boot_services()`.
   From this line on, nothing from the firmware is running: the RSDP
   address, the framebuffer pointer and the in-memory `APPS` snapshot
   are the only things carried across.
3. Installs its own GDT with a TSS and an IST stack, and a 256-gate IDT
   whose handlers print a real fault screen rather than triple-faulting.
4. Takes over memory: a bitmap physical allocator over the UEFI memory
   map, its own four-level page tables with guard pages, a dedicated
   mainline stack, and an address-ordered, coalescing free-list heap
   (self-tested every boot). The page tables enforce **W^X**: the
   kernel image is mapped section by section from its own PE headers --
   code read-only and executable, constants read-only, everything else
   (data, heap, stacks, DMA buffers, MMIO) non-executable -- with
   `EFER.NXE` and `CR0.WP` on, so a stray write into code or a jump into
   data faults at the offending instruction.
5. Parses ACPI (RSDP -> XSDT/RSDT -> MADT), brings up the Local APIC and
   the I/O APIC, honours the MADT's Interrupt Source Overrides, moves
   every interrupt onto them, and switches the clock to the LAPIC timer
   after calibrating it against the PIT. If the MADT is missing or
   unusable it says so on screen and stays on the 8259/PIT path.
6. Brings up input: a PS/2 keyboard on ISA IRQ1, and -- if an xHCI
   controller with a USB HID boot keyboard is present -- enumerates that
   device and polls its interrupt endpoint. Both paths feed one queue.
7. Brings up AHCI SATA, NVMe (including supported Intel client VMD),
   and xHCI USB Bulk-Only storage. PCI discovery includes firmware-assigned
   buses behind bridges. It probes disks without writing and selects a
   unique mounted Wisp store, or a unique eligible format target. Saves
   are copy-on-write: a power cut leaves the old value or the new one.
   Boot leaves blank and foreign disks untouched. The shell's `format`
   provisions eligible raw disks or an existing Wisp save partition after
   naming the target and requiring confirmation; foreign partitions are refused. Any failure downgrades to "no persistence" with a
   message, never a hang.
8. Hands the machine to the shell.

## Layout

```
src/                  the kernel (no_std, no_main)
  main.rs             entry point, exit_boot_services, bring-up order
  gdt.rs               GDT + TSS + IST stack
  idt.rs               256-gate IDT, fault screens, IRQ dispatch
  pmm.rs                bitmap physical frame allocator
  paging.rs             our own 4-level page tables: W^X, NX, guard pages
  allocator.rs           address-ordered coalescing free-list heap
  acpi.rs               RSDP -> XSDT/RSDT -> MADT parser (no AML)
  lapic.rs               Local APIC: EOI, LVTs, calibrated timer
  ioapic.rs               I/O APIC redirection-table routing
  pic.rs / pit.rs          the 8259/PIT legacy path (and the fallback)
  clock.rs                 the one system timebase: ticks, sleeps, bounded waits
  keyboard.rs              PS/2 keyboard driver and the shared key queue
  xhci.rs                  xHCI host controller: rings, slots, transfers
  usb_hid.rs                USB HID boot-protocol keyboard -> key queue
  pci.rs                     PCI buses, 64-bit BARs and Intel client VMD domains
  ahci.rs / nvme.rs            SATA and polled NVMe disk drivers
  usb_storage.rs / usb_bot.rs   xHCI Bulk-Only SCSI storage and wire formats
  storage.rs / storage_policy.rs shared sector interface and disk selection
  save_format.rs             the hand-designed key/value store on disk
  gfx.rs / font.rs            direct-framebuffer console, 8x16 bitmap font
  fs.rs                        FAT32 reads via UEFI protocols (pre-exit)
  installer.rs                 firmware boot menu, target selection and verification
  disk_layout.rs               streaming GPT/FAT32 writer and save-partition discovery
  shell.rs                      the `>` prompt and its commands
  line_editor.rs                 cursor editing and history for every typed line
  editor.rs / ide.rs             text buffer and native Wisp Studio interface
  projects.rs                    saved WISP sources under prog.<name>
  libc_shims.rs                  freestanding `wcslen`
  wisp/                           lexer -> parser -> tree-walking interpreter
                                   (one Session per program, or per `wisp` prompt)
docs/
  MANUAL.md                the full manual: install, operate, program, internals, roadmap
  WISP_LANGUAGE.md        the complete WISP language reference
  WISP_GRAPHICS.md         the pixel/graphics primitives
  WISP_AI.md                running trained models as WISP forward-pass code
  SAVE_FORMAT.md             the on-disk save format, with a worked example
esp/
  EFI/BOOT/BOOTX64.EFI      build output (produced by build.sh)
  APPS/*.wisp                the bundled WISP demo programs
scripts/                 USB installers, make_iso.sh, make_release.sh
tests/
  ide_test.py               Studio editing, checking, running and reboot persistence (PS/2 or USB)
  nvme_test.py              NVMe discovery, partition protection and reboot persistence
  usb_storage_test.py       USB storage and keyboard coexistence, saves across reboot
  storage_selection_test.py Ambiguous disks, explicit selection and separate stores
  installer_test.py         live ISO installation and standalone boot in QEMU
  iso_test.py               boot-image bytes, names and image-output safeguards
  console_capture.py        exact framebuffer text decoding without OCR
  acceptance_test.py        end-to-end QEMU run, screenshot-verified
  fault_test.py              deliberate CPU faults -> fault screens
  ahci_test.py                storage bring-up and persistence
  usb_test.py                  APIC routing and the USB keyboard path
  host/                        cargo test: interpreter + save format, no VM
tools/train_tiny_nn.py      offline trainer for the demo networks' weights
USB_INSTALL.md            putting this on a stick and booting real hardware
CHANGES.md                 engineering log: the specific decisions and why
```

## The demo programs

- **`hello.wisp`** -- the simplest possible script: one `input()`, one
  greeting. Start here.
- **`guess.wisp`** -- a number-guessing game; also the worked example in
  `docs/WISP_LANGUAGE.md`.
- **`oracle.wisp`** -- a shuffle-bag fortune teller; demonstrates lists,
  `push`/`pop`, and the swap-to-last-then-pop removal pattern.
- **`adventure.wisp`** -- a small five-room text adventure with an
  item-gated locked door, built entirely from globals and whole-line
  string comparisons (it was written for WISP v1, before maps and string
  indexing).
- **`xor_brain.wisp`** -- a 2-2-1 ReLU network computing XOR from
  analytically-derived integer weights. The simplest possible proof that
  real forward-pass inference runs correctly as WISP code.
- **`tamer.wisp`** -- a genuinely trained 2-4-1 ReLU network (full-batch
  Adam, `tools/train_tiny_nn.py`) doing fixed-point inference on two 0-10
  stats. The actual payoff of the WISP-AI pattern: a real trained model,
  running on bare metal, verified against its own exact integer math
  before it ever shipped.
- **`counter.wisp`** -- counts how many times it has ever been run, and
  survives a power cycle. The end-to-end proof of the AHCI save path.
- **`bounce.wisp`** -- a ball animated by writing pixels directly, using
  the graphics primitives and the timer.
- **`canvas.wisp`** -- an interactive drawing prompt: points, rectangles
  and lines, including the clean runtime errors for bad arguments.
- **`stopwatch.wisp`** -- laps measured with `time_ms()`, forever, until
  you press Esc (which wakes its `sleep_ms()` immediately).
- **`primes.wisp`** -- prints primes as fast as it can, forever: a pure
  compute loop that only Esc can end.
- **`calc.wisp`** -- an integer calculator with operator precedence and
  parentheses (`2 + 3 * (4 + 6)` is 32), whose expression parser is
  written in WISP itself: the worked example of `for` over a string,
  string indexing, and the string builtins. Every bad expression is its
  own message, and the calculator keeps running.
- **`words.wisp`** -- a word counter: type text, then ask for the most-used
  words, one word's count, or the whole alphabetical list. Every tally
  lives in one map, so it is the worked example of map reads and writes,
  `get()`, `contains()`, `remove()`, and `for` over a map's keys.
- **`tasks.wisp`** -- a to-do list whose commands are a map from what you
  type to the function that handles it (`commands[word](rest)`): the
  worked example of functions as values, with `filter`, `map`, `reduce`
  and a stable `sort_by` doing the list work.
- **`undo.wisp`** -- a notebook of numbers with undo and redo: each edit is
  a pair of closures (`fn (...) { ... }` as an expression) that remember
  the name, the new value and the value they replaced. The worked example
  of closures, which also hand out edit numbers, and give `filter` and
  `sort_by` the prefix or the notebook they work on.

## Known rough edges (see CHANGES.md for the full story on each)

- `BOOTX64.EFI` is unsigned. Secure Boot must be off, or you enroll your
  own signature.
- USB input needs an **xHCI** controller and a boot-protocol keyboard on
  a **root port**: no hubs, no hot-plug, no EHCI/OHCI/UHCI. Older
  machines fall back to firmware PS/2 emulation. See `USB_INSTALL.md`.
- Persistence supports **AHCI SATA**, **NVMe**, and **xHCI USB Bulk-Only**
  disks with native 512-byte sectors. 4 KiB-native NVMe/USB media can be
  read, but the v2 save layout refuses writes to preserve its atomicity.
  USB storage needs a root port and BOT/SCSI interface (class `08/06/50`);
  UAS, hubs and hot-plug are not implemented. Intel client VMD discovery
  includes `8086:467f`; physical VMD operation remains unverified.
- Only the boot processor is used; there is no SMP bring-up. Interrupts
  are polled for USB rather than delivered by MSI/MSI-X.
- There is no AML interpreter, so ACPI is read for the MADT only.
- The Linux/macOS/Windows USB installer scripts were built and reviewed
  carefully, but only the Linux script's partition/format steps were
  exercised end-to-end in development (against a loopback image, in a
  sandbox with no `vfat` kernel driver) -- see `USB_INSTALL.md`.
- This distro's `ovmf` package doesn't ship the exact
  `/usr/share/OVMF/OVMF_CODE.fd` path the project spec names; `run.sh`
  and the test suites fall back to `/usr/share/ovmf/OVMF.fd`
  automatically.

## Requirements

- Rust `nightly-2026-10-21` with the `x86_64-unknown-uefi` target and
  `rust-src` component. `rust-toolchain.toml` pins the compiler and `rustup`
  installs it automatically. Release builds use `--locked` dependencies.
- `qemu-system-x86_64` and OVMF firmware to run/test without real hardware.
- Python 3.11 or later for the host tools and VM suites. Screen assertions
  decode the embedded bitmap font directly and need no OCR or image packages.
  NumPy is needed only to retrain the bundled neural network.

## Verification and distribution

Run `cargo test --manifest-path tests/host/Cargo.toml --release --locked` for
hardware-independent tests. Run `python3 -m unittest discover -s tests -p '*_test.py'`
for host tooling checks; VM suites also have executable entry points in `tests/`.
The regression matrix and limitations are recorded in [release notes](docs/RELEASE_NOTES.md)
and [TODO.md](TODO.md). VM evidence is kept in separate run directories.

`scripts/make_release.sh --iso` produces source archives, a minimal
`wisp-os-v1.2.0-runtime.zip`, the standalone EFI executable, and a live installer
ISO. The runtime archive contains the boot image, bundled apps, user documentation
and USB installers; it excludes source, tests and build tools. All release assets
have SHA-256 checksums, and `BUILD_INFO.json` records compiler, target, source
identity and binary hash. Set `SOURCE_DATE_EPOCH` to reproduce archive and ISO
metadata. Generation failures leave previously published local packages intact.
