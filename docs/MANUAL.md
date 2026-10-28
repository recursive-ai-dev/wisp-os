# The WISP OS Manual

**Covers WISP OS v1.1.0 and the work merged after it (functions, closures and live installation).**
**Companion roadmap: Part VI.**

> WISP OS is a freestanding x86_64 kernel that boots straight from UEFI firmware
> and *becomes* the machine: its own memory manager, interrupt handling, drivers,
> console, shell and programming language, with no host operating system under it
> and no network stack anywhere in the binary.

This manual is the one place that explains the whole system: how to install it,
how to operate it, how to program it, how it works inside, and where it is going.
It is written for four readers, and each can start in a different place:

| If you are... | Start at | Then read |
|---|---|---|
| **Trying it for the first time** | [Chapter 5](#5-getting-the-files) and [Chapter 7](#7-running-in-qemu) | [Chapter 11](#11-the-boot-screen), [Chapter 12](#12-the-shell) |
| **Putting it on real hardware** | [Chapter 8](#8-installing-on-a-usb-stick), [Chapter 9](#9-booting-real-hardware) | [Chapter 10](#10-troubleshooting-boot), [Chapter 40](#40-hardware-bring-up-playbook) |
| **Writing WISP programs** | [Chapter 17](#17-a-guided-tour-of-wisp) | [Chapter 20](#20-recipes), [Chapter 21](#21-graphics), [Chapter 24](#24-limits-at-a-glance) |
| **Hacking on the kernel** | [Chapter 26](#26-architecture-and-boot-order) | [Chapter 36](#36-testing), [Chapter 37](#37-how-changes-are-made-here), [Part VI](#part-vi-roadmap-of-expansion) |

## Contents

- **[Part I: Orientation](#part-i-orientation)**
  - [1. What WISP OS is](#1-what-wisp-os-is)
  - [2. Design principles and the charter](#2-design-principles-and-the-charter)
  - [3. Glossary](#3-glossary)
  - [4. What the machine must provide](#4-what-the-machine-must-provide)
- **[Part II: Getting it running](#part-ii-getting-it-running)**
  - [5. Getting the files](#5-getting-the-files)
  - [6. Host-side checks (optional but recommended)](#6-host-side-checks-optional-but-recommended)
  - [7. Running in QEMU](#7-running-in-qemu)
  - [8. Installing on a USB stick](#8-installing-on-a-usb-stick)
  - [9. Booting real hardware](#9-booting-real-hardware)
  - [10. Troubleshooting boot](#10-troubleshooting-boot)
- **[Part III: Using WISP OS](#part-iii-using-wisp-os)**
  - [11. The boot screen](#11-the-boot-screen)
  - [12. The shell](#12-the-shell)
  - [13. Editing a line, and the keyboard](#13-editing-a-line-and-the-keyboard)
  - [14. Stopping a program](#14-stopping-a-program)
  - [15. Persistence: saving data](#15-persistence-saving-data)
  - [16. The bundled programs](#16-the-bundled-programs)
- **[Part IV: Programming in WISP](#part-iv-programming-in-wisp)**
  - [17. A guided tour of WISP](#17-a-guided-tour-of-wisp)
  - [18. The language on one page](#18-the-language-on-one-page)
  - [19. Built-in function reference](#19-built-in-function-reference)
  - [20. Recipes](#20-recipes)
  - [21. Graphics](#21-graphics)
  - [22. Running trained models](#22-running-trained-models)
  - [23. Error messages](#23-error-messages)
  - [24. Limits at a glance](#24-limits-at-a-glance)
  - [25. Getting programs onto the machine](#25-getting-programs-onto-the-machine)
- **[Part V: Inside WISP OS](#part-v-inside-wisp-os)**
  - [26. Architecture and boot order](#26-architecture-and-boot-order)
  - [27. Memory](#27-memory)
  - [28. Interrupts, ACPI and the clock](#28-interrupts-acpi-and-the-clock)
  - [29. Input: PS/2 and USB](#29-input-ps2-and-usb)
  - [30. Storage: PCI and AHCI](#30-storage-pci-and-ahci)
  - [31. The save format](#31-the-save-format)
  - [32. The console and framebuffer](#32-the-console-and-framebuffer)
  - [33. Failure modes and the fault screen](#33-failure-modes-and-the-fault-screen)
  - [34. The WISP interpreter](#34-the-wisp-interpreter)
  - [35. Building and releasing](#35-building-and-releasing)
  - [36. Testing](#36-testing)
  - [37. How changes are made here](#37-how-changes-are-made-here)
  - [38. Verification ledger](#38-verification-ledger)
  - [39. Security and threat model](#39-security-and-threat-model)
  - [40. Hardware bring-up playbook](#40-hardware-bring-up-playbook)
- **[Part VI: Roadmap of expansion](#part-vi-roadmap-of-expansion)**
  - [How to read the roadmap](#how-to-read-the-roadmap)
  - [41. Horizon 0: Trust the release](#41-horizon-0-trust-the-release)
  - [42. Horizon 1: The machine gets a voice and a door](#42-horizon-1-the-machine-gets-a-voice-and-a-door)
  - [43. Horizon 2: The language grows up](#43-horizon-2-the-language-grows-up)
  - [44. Horizon 3: The machine stops needing another computer](#44-horizon-3-the-machine-stops-needing-another-computer)
  - [45. Horizon 4: Hardware breadth](#45-horizon-4-hardware-breadth)
  - [46. Horizon 5: Graphics, sound and interaction](#46-horizon-5-graphics-sound-and-interaction)
  - [47. Horizon 6: Verified intelligence](#47-horizon-6-verified-intelligence)
  - [48. Horizon 7: The long horizon](#48-horizon-7-the-long-horizon)
  - [49. Non-goals and decisions to make out loud](#49-non-goals-and-decisions-to-make-out-loud)
  - [50. The release train, dependencies and risks](#50-the-release-train-dependencies-and-risks)
- **[Appendices](#appendices)**
  - [Appendix A: Cheat sheet](#appendix-a-cheat-sheet)
  - [Appendix B: A grammar for WISP](#appendix-b-a-grammar-for-wisp)
  - [Appendix C: Quick triage by symptom](#appendix-c-quick-triage-by-symptom)
  - [Appendix D: Glossary of project vocabulary](#appendix-d-glossary-of-project-vocabulary)
  - [Appendix E: Where everything is](#appendix-e-where-everything-is)
  - [Appendix F: Frequently asked questions](#appendix-f-frequently-asked-questions)

### How to read this manual

- **What is verified, and what is not.** This project keeps a standing rule that a
  claim is only made if it was checked, so this manual does the same. Where a
  statement rests on a test, the test is named. Where something has *never been
  exercised* (real hardware, the macOS and Windows installers, LA57 paging, the
  QEMU run of the most recent language features), the manual says so. The full
  ledger is in [Chapter 38](#38-verification-ledger).
- **Sources of truth.** This manual consolidates and extends the repository's
  existing documents rather than replacing them. When this manual and a
  specialised document disagree, the specialised one wins and the disagreement is
  a bug in this manual; [Chapter 38](#38-verification-ledger) lists the known
  stale sentences in the older documents.
  - `README.md`: overview and quick start
  - `docs/WISP_LANGUAGE.md`: the complete language reference
  - `docs/WISP_GRAPHICS.md`: pixel primitives in depth
  - `docs/WISP_AI.md`: running trained models as WISP code
  - `docs/SAVE_FORMAT.md`: the on-disk save format, byte for byte
  - `USB_INSTALL.md`: installing and booting real hardware
  - `CHANGES.md`: the engineering log: every decision, bug and postmortem
  - `CODEBASE_AUDIT.md`: the external audit of v1.0.0, all findings since fixed
- **Conventions.** `monospace` is something you type or a name in the code.
  Shell sessions show the prompt as `>` and WISP prompt as `wisp>`. Sizes are
  binary (KiB = 1024 bytes). A **chapter reference** such as [Chapter 15](#15-persistence-saving-data)
  is a link.

---

# Part I: Orientation

## 1. What WISP OS is

WISP OS is three things stacked on each other:

1. **A kernel.** A `no_std`, `no_main` Rust program built for the
   `x86_64-unknown-uefi` target. UEFI firmware loads it from a FAT volume as a
   normal application. It reads what it needs from the volume, then calls
   `exit_boot_services()` and never looks back: from that point it runs on its own
   GDT, IDT, page tables, physical-frame allocator, heap, interrupt controllers,
   timer, keyboard drivers and disk driver. It draws by writing pixels straight
   into the firmware-provided GOP framebuffer using an embedded 8x16 bitmap font.
2. **A shell.** A `>` prompt with fourteen commands. It is deliberately small: its
   job is to run programs, show the machine's state, and manage the save disk.
3. **A language, WISP.** A small dynamically-typed scripting language with
   integers, strings, lists, maps, functions, closures and pixel graphics. You
   write plain-text `.wisp` files on any ordinary computer, copy them to the boot
   stick's `APPS` folder, and run them from firmware. There is no compiler and no
   operating system in the loop. It also has an interactive prompt, `wisp>`, so you
   can program on the machine itself.

Beside those, the project ships fifteen demonstration programs (a calculator, a
text adventure, a to-do list, an undo/redo notebook, two real neural networks and
more), a crash-atomic key/value store on a SATA disk, installers for three host
operating systems, four QEMU test suites and 229 host-side tests.

### What it is for

The stated purpose is **architectural honesty**: build a computer's worth of
software (console, shell, memory management, interrupt handling, storage, a real
programming language, even verified neural-network inference) from firmware
primitives upward, with nothing borrowed from a hosted OS and nothing sent to a
network. It is a teaching machine, a research platform for "how little does a
useful computer need?", and a place where every layer is small enough to read.

### What it is not

- Not a general-purpose desktop OS. There is no multitasking, no windowing, no
  file manager, no POSIX layer, no dynamic loading.
- Not networked. This is not a missing feature; it is the founding constraint.
  See [Chapter 2](#2-design-principles-and-the-charter).
- Not a language-model host. WISP runs *tiny, fully verified* neural networks
  (hundreds of weights). It cannot load, and will not load, a modern LLM.
- Not yet proven on physical hardware. Everything in the verification ledger was
  exercised in QEMU/OVMF. See [Chapter 38](#38-verification-ledger).

### Version at a glance

| Item | Value |
|---|---|
| Kernel version | 1.1.0 (`Cargo.toml`; banner and `version` read it at compile time) |
| Save format | version 2 (version 1 disks still mount and upgrade on first save) |
| Binary | `BOOTX64.EFI`, about 308 KB, unsigned |
| Toolchain | Rust nightly, `x86_64-unknown-uefi`, `uefi` crate pinned `=0.30.0` |
| Last full QEMU verification | 2026-10-19 nightly (`rustc 1.101.0-nightly (07e526db9)`) |
| Host tests | 229 (`cd tests/host && cargo test --release`) |
| Bundled programs | 15 in `esp/APPS/` |

## 2. Design principles and the charter

These principles are not decoration. Each one has already decided real questions in
`CHANGES.md`, and Part VI uses them to rank work and to say no.

1. **Nothing borrowed, nothing phoned home.** Every layer is hand-built from
   firmware primitives. There is no network code in the binary: not disabled,
   *absent*. A USB network adapter is simply a device the xHCI driver does not
   recognise. Adding networking "would be a decision to be made out loud rather than
   something that could arrive as a side effect" (`USB_INSTALL.md`).
2. **Boot never writes.** A freshly booted machine reads the disk it finds and
   modifies nothing. The only code path that writes outside an existing save store
   is the `format` command, which names the device and the exact destructive range
   and requires the word `ERASE`. (This is the lesson of audit finding AUD-01.)
3. **Degrade, never hang.** Every hardware path has a failure mode that ends in a
   working shell and a sentence on the screen: no MADT means the 8259/PIT fallback;
   no AHCI disk means `save`/`load` are disabled and say so; no USB keyboard means
   PS/2 only. A buggy script cannot crash the machine; Esc always stops it.
4. **Measure, don't assume.** Stack frame sizes were read from assembly. Boot
   memory ownership was measured by walking live page tables. A hang bug was found
   only because the project insisted on testing its own documented failure paths.
   When a claim cannot be tested, the documentation says it is untested.
5. **Pixels are ground truth.** The QEMU suites screenshot the framebuffer and check
   sampled pixel colours and OCR'd text. A test that only checks "the screendump
   succeeded" is considered a non-test.
6. **Small and complete over large and partial.** A feature ships with its
   errors, its limits, its tests (including *mutation tests*, which deliberately
   break the code to prove the tests notice), its documentation and its log entry,
   or it does not ship. No `TODO`, no placeholder branches.
7. **Integer-only where unverified.** No floating point exists anywhere in the
   kernel, because FPU/SSE exception handling has never been exercised end to end
   (`docs/WISP_GRAPHICS.md`). Floats arrive with their own verification or not at all.

### The charter: lines that are crossed only out loud

| Line | Current position | How it could ever change |
|---|---|---|
| Networking | Absent by design | A written decision record in `CHANGES.md` plus an update to the README's foundation statement. Not a side effect of a driver. |
| Writing to the boot stick | Never written | A new filesystem decision, tested against the vvfat-style corruption failure documented in `CHANGES.md`. |
| Floating point | Absent | Ships together with FPU/SSE fault tests. |
| LLM-scale inference | Out of scope | Nothing here is a path to it; the realistic ceiling is tiny verified models. |
| Catchable Esc | Esc cannot be caught or ignored by a script | Never: always-available stop is a safety property. |

## 3. Glossary

| Term | Meaning |
|---|---|
| **ACPI / MADT** | Firmware tables. The kernel parses the MADT (Multiple APIC Description Table) to learn the interrupt topology. There is no AML interpreter. |
| **AHCI** | The SATA controller interface. The only storage the kernel drives. |
| **APPS snapshot** | Every `APPS/*.wisp` file, read into RAM *before* Boot Services exit. `ls` and `run` serve from it. |
| **Boot Services** | The services UEFI firmware offers while it is still in charge. Gone after `exit_boot_services()`. |
| **debugcon** | QEMU's debug console port (`0xE9`). Boot and driver log lines go here. It exists only under QEMU; real hardware has no equivalent yet. |
| **ESP** | EFI System Partition: the FAT volume the firmware boots from. `esp/` in the repo is a ready-made one. |
| **GOP** | UEFI Graphics Output Protocol: gives the kernel a linear framebuffer. |
| **IST** | Interrupt Stack Table: a dedicated stack the CPU switches to for specific exceptions, such as double fault. |
| **LAPIC / I/O APIC** | The per-CPU and external interrupt controllers that replace the legacy 8259 on modern machines. |
| **OVMF** | The open-source UEFI firmware QEMU uses. |
| **PMM** | Physical Memory Manager: a bitmap of 4 KiB frames. |
| **Session** | One WISP interpreter state: one per `run`, or one per `wisp` prompt. |
| **Slot** | One 4096-byte record in the save store; there are 64, one always kept free, so 63 keys. |
| **W^X** | Write xor Execute: a page is never both writable and executable. |
| **WISP** | The scripting language (and the project's name for it). |
| **xHCI** | The USB 3 host controller interface. The only USB controller the kernel drives. |

## 4. What the machine must provide

Everything here is implemented and exercised by the test suites; anything not
listed is not supported.

| Component | Required | Notes |
|---|---|---|
| **CPU** | x86_64 | Only the boot processor is used; other cores stay parked. |
| **Firmware** | 64-bit UEFI | No BIOS/CSM path, no 32-bit build. |
| **Secure Boot** | Must be off | The binary is unsigned ([Chapter 9](#9-booting-real-hardware)). |
| **Display** | A UEFI GOP framebuffer | Effectively universal. Resolution must fit an 8x16 text console. No GPU driver. |
| **Keyboard** | PS/2 (or firmware emulation of it), **or** a USB HID *boot-protocol* keyboard on an **xHCI** root port | US layout only. No hubs, no hot-plug. |
| **Storage (optional)** | An **AHCI** SATA controller with a disk | Only needed for `save`/`load`. NVMe and IDE-mode SATA are not supported. |
| **Memory** | 256 MB is comfortable | The kernel heap is a fixed 8 MiB; the rest is frame-allocated for page tables and buffers. |
| **Network** | None | By design. |

Interrupts use the LAPIC and I/O APIC when the MADT is usable, with the clock on
the LAPIC timer calibrated against the PIT. If the MADT is missing, the kernel says
so and stays on the 8259/PIT path.

---

# Part II: Getting it running

## 5. Getting the files

There are two ways to obtain WISP OS.

### Option A: a release archive (no Rust needed)

A release is `wisp-os-v1.1.0.tar.gz` or `.zip`. It contains the full source, the
bundled programs, and a **prebuilt** `esp/EFI/BOOT/BOOTX64.EFI`, so you can boot
real hardware without installing a toolchain.

Verify before you trust it. Inside the extracted folder:

```bash
sha256sum -c SHA256SUMS
```

Every file is listed. The release builder (`scripts/make_release.sh`) is
reproducible: an unchanged tree repackages to byte-identical archives.

> **Source checkouts are different.** In a plain git checkout, the `SHA256SUMS`
> file at the repository root describes the v1.0.0 release and is regenerated
> only by `scripts/make_release.sh` (`CHANGES.md` records this). Do not use it to
> validate a binary you built from a checkout.

### Option B: build from source

You need `rustup`. The repository's `rust-toolchain.toml` selects the `nightly`
channel, the `x86_64-unknown-uefi` target and the `rust-src` component, and
`rustup` fetches them on first use.

```bash
./build.sh
```

This runs `cargo build --release --target x86_64-unknown-uefi` and assembles
`esp/EFI/BOOT/BOOTX64.EFI`. The release profile uses `opt-level = "s"` and LTO, and
`.cargo/config.toml` passes `/Brepro` and `/DEBUG:NONE` to the linker so the same
source and toolchain produce a byte-identical binary regardless of directory.

**Toolchain note.** `rust-toolchain.toml` says plain `nightly`, which floats. v1.1.0
was built and fully tested with `rustc 1.101.0-nightly (07e526db9 2026-10-19)`. If a
later nightly ever breaks the build:

```bash
rustup toolchain install nightly-2026-10-20
cargo +nightly-2026-10-20 build --release --target x86_64-unknown-uefi
```

(At the time of writing, a build on `rustc 1.101.0-nightly (16d1ffac1 2026-10-26)`
succeeded with zero warnings. Pinning the toolchain file is a Part VI item.)

Because the build is reproducible only for a *fixed* toolchain, a binary built on a
different nightly will differ from the release binary even when the source is
identical. That is expected, not corruption.

## 6. Host-side checks (optional but recommended)

The hardware-independent half of the project (the whole language, the save format,
and the line editor) compiles on an ordinary computer and has its own test suite:

```bash
cd tests/host
cargo test --release
```

`--release` matters: the stack-depth tests run scripts on a deliberately small
thread stack, and only optimised frame sizes are meaningful. Expect 229 tests across
seven files: `language` (41), `maps` (43), `functions` (43), `closures` (45),
`regressions` (29), `save_format_v2` (20), `line_editor` (8). The kernel sources are
compiled in unmodified; only the AHCI driver is replaced by an in-memory disk model
with a volatile write cache.

## 7. Running in QEMU

Requirements: `qemu-system-x86_64`, `qemu-img`, and an OVMF firmware image.

```bash
./run.sh
```

What it does, and the switches that matter:

| Variable | Effect |
|---|---|
| *(none)* | Headless (`-display none`). The shell is still reachable through QEMU's monitor/serial machinery; this is how the test suites drive it. |
| `UEFI_SHELL_DISPLAY=gui` | Opens a real window. **Use this for interactive play.** |
| `UEFI_SHELL_USB_KBD=1` | Attaches a USB keyboard to the xHCI controller, so your keystrokes exercise the USB driver instead of PS/2. |
| extra arguments | Passed through to QEMU (`./run.sh -m 512M`). |

```bash
UEFI_SHELL_DISPLAY=gui ./run.sh
```

The emulated machine has: the `esp/` directory as a **read-only** USB mass-storage
boot device (via vvfat, read-only on purpose, see below), an empty xHCI controller,
a 64 MiB raw disk `save.img` on an AHCI controller, **no network** (`-net none`),
256 MB RAM, and a debug console written to `debug.log`.

`save.img` is created on first run and never committed. It starts blank, so the first
time you want `save`/`load` you must run `format` ([Chapter 15](#15-persistence-saving-data)).
Delete `save.img` for a fresh disk.

**Why the stick is mounted read-only.** Mounting vvfat read-write silently corrupted
the boot executable on the host between runs (47,738 of 90,624 bytes rewritten, found
by a byte-for-byte comparison). The kernel never writes the boot volume, so read-only
removes the whole class of failure. The project's rule since: *the boot stick is
never written.*

**OVMF paths.** `run.sh` prefers `/usr/share/OVMF/OVMF_CODE.fd` and falls back to
`/usr/share/ovmf/OVMF.fd`. Ubuntu 24.04's `ovmf` package ships only split code/vars
images; the project's verified run formed a monolithic image the way they are always
formed:

```bash
cat /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_CODE_4M.fd > /usr/share/ovmf/OVMF.fd
```

(Adjust the paths for your distribution, and use `sudo` if the directory is
root-owned.)

### Running the QEMU test suites

```bash
python3 tests/acceptance_test.py /path/to/NEW_EMPTY_DIR
python3 tests/ahci_test.py       /path/to/ANOTHER_NEW_DIR
python3 tests/fault_test.py      /path/to/ANOTHER_NEW_DIR
python3 tests/usb_test.py        /path/to/ANOTHER_NEW_DIR
```

> **Danger.** Each harness deletes its output directory at startup
> (`shutil.rmtree(OUT_DIR)`). Always pass a *new, dedicated* directory, never the
> repository root, never a folder with anything you want to keep.

The suites need Python 3, `Pillow` (screenshots) and `pytesseract` with the
`tesseract` binary (OCR assertions). They are development-only dependencies.

## 8. Installing on a USB stick

### Live installation

The boot menu now offers **T** to try Wisp (also selected after a short
timeout) or **I** to install the live system. Build its hybrid UEFI ISO
with `./scripts/make_iso.sh` (Python 3 and `xorriso` required), then boot
the ISO as optical media or write it to USB with an image-writing tool.
The existing host USB scripts below also produce media with this menu.

The installer lists writable whole disks with 512-byte sectors and at
least 80 MiB capacity. It excludes the live boot source and asks for the
exact confirmation `ERASE <number>` before writing. It replaces that disk
with primary/backup GPT tables, a 64 MiB FAT32 EFI System Partition, the
live EFI binary and apps, and an initialized Wisp save partition. It
flushes and verifies the sectors before reporting success. It does not
resize existing partitions, arrange dual boot or change firmware NVRAM
entries. Remove the live medium after installation and select the disk
or its `\EFI\BOOT\BOOTX64.EFI` file in firmware.

For an isolated VM trial:

```bash
./scripts/make_iso.sh
UEFI_SHELL_DISPLAY=gui WISP_ISO=dist/wisp-os-v1.1.0-live.iso ./run.sh
# After installing onto the automatically created 128 MiB install.img:
UEFI_SHELL_DISPLAY=gui WISP_BOOT_DISK=install.img ./run.sh
```

Firmware can install and boot from NVMe or USB, but persistence after
Boot Services still needs a supported AHCI disk. An interrupted install
can leave its target unbootable; rerun installation from live media.
See [USB_INSTALL.md](../USB_INSTALL.md) for the complete procedure.

### Preparing live USB media from a host OS

Three installers do the same job on three host operating systems: make a
GPT-partitioned, FAT32-formatted stick containing `EFI/BOOT/BOOTX64.EFI` and
`APPS/*.wisp`. `esp/EFI/BOOT/BOOTX64.EFI` must exist first (from a release archive,
or `./build.sh`).

| Host OS | Script | Needs |
|---|---|---|
| Linux | `scripts/install_usb_linux.sh` | `parted`, `mkfs.vfat` (package `dosfstools`), root |
| macOS | `scripts/install_usb_macos.sh` | `diskutil` (ships with macOS) |
| Windows | `scripts/install_usb_windows.ps1` | PowerShell `Storage` module, Administrator |

All three follow the same two-step pattern:

```bash
sudo ./scripts/install_usb_linux.sh list              # read-only: shows candidates
sudo ./scripts/install_usb_linux.sh install /dev/sdb  # destructive
```

```bash
./scripts/install_usb_macos.sh list
./scripts/install_usb_macos.sh install disk4
```

```powershell
.\scripts\install_usb_windows.ps1 -List
.\scripts\install_usb_windows.ps1 -Install -DiskNumber 2
```

### The guardrails

Partitioning is destructive, so every script is built around the same safety rules:

- **`list` never touches a disk.** It only reads information back.
- **The target must look like removable/USB media.** Linux checks the kernel's `RM`
  flag and USB transport; macOS considers only disks `diskutil` reports as
  external and physical; Windows checks `BusType -eq 'USB'`. Linux and Windows have
  a `--force`/`-Force` override for a USB enclosure that misreports itself; macOS
  intentionally has none.
- **The disk backing your running system can never be selected.** No override
  bypasses this.
- **Two typed confirmations** before anything destructive: the exact device
  identifier, then the literal word `ERASE`.

### Honest status of the installers

| Installer | Exercised |
|---|---|
| Live boot installer | QEMU: cancellation and small targets leave images unchanged; every copied file matches; installed disk boots without live media, runs apps, excludes itself from targets and preserves saves across reboot. Real hardware remains untested. |
| Linux | Partition and format steps verified end to end against a loopback image (`parted print`, `blkid`, `fsck.fat`). The final mount-and-copy step could not be verified in the build sandbox (no `vfat` kernel driver), though it is standard. |
| macOS | **Never run** on a real Mac. Written against documented `diskutil` behaviour. Creates a "Microsoft Basic Data" FAT32 partition rather than one tagged with the ESP type GUID, which works because firmware's removable-media path ignores the type GUID. |
| Windows | **Never run** on a real Windows machine. Sets the ESP type GUID explicitly. |

Treat the macOS and Windows scripts with extra caution on first use, and read what
`list` shows before you type a device.

### Manual installation

You do not need a script. Any FAT32 volume works if it contains:

```text
EFI/BOOT/BOOTX64.EFI
APPS/hello.wisp          (and any other .wisp files)
```

## 9. Booting real hardware

1. Plug the stick into an x86_64 PC.
2. At power-on, open the machine's **one-time boot menu** (commonly `F12`, `F10`,
   `F8` or `Esc`; check the splash screen) and pick the USB stick. This works with
   no setup because UEFI firmware looks for `\EFI\BOOT\BOOTX64.EFI` on any
   removable disk (the **removable-media fallback path**).
3. Boot with the keyboard plugged **directly into a port on the machine**: not a
   hub, dock or monitor port.

### Firmware settings that matter

| Setting | Needed value | Why |
|---|---|---|
| **Secure Boot** | **Disabled** | The binary is unsigned. A Secure Boot machine will refuse it, often by printing an invalid-image error or by silently falling through to the next boot device. Alternatively enrol your own signature for the binary; this project does not sign or manage enrolment. |
| **SATA mode** | **AHCI** (not "IDE" / "Compatibility" / "RAID") | Persistence needs AHCI. IDE mode leaves the shell fully usable with `save`/`load` disabled, and it says so on screen. |
| **CSM / Legacy boot** | Off or irrelevant | There is no BIOS path. |
| **USB legacy support / xHCI hand-off** | Try both settings if USB input is dead | Not verified on hardware. The kernel does not implement the xHCI ownership hand-off ([Chapter 29](#29-input-ps2-and-usb)), so these firmware settings can change whether USB input works. On machines with no xHCI-visible keyboard, the firmware's PS/2 emulation is how typing works. |

### Before you boot on a machine with disks you care about

Choosing Try Wisp never provisions a disk. Installation requires the
separate `ERASE <number>` confirmation. The kernel selects the first
usable AHCI device for persistence, mounts a valid Wisp save partition
or legacy raw store, and otherwise leaves the disk alone. `format` shows
the device and range and asks for `ERASE`; it refuses foreign partition
tables rather than overwriting boot files. Read each displayed target
carefully before confirming.

## 10. Troubleshooting boot

| Symptom | Likely cause | What to do |
|---|---|---|
| Firmware shows an "invalid image" / "security violation" error, or boots the next device | Secure Boot is on | Disable Secure Boot. |
| The stick is not in the boot menu | Not GPT/FAT32; file at the wrong path; firmware hides the stick | Confirm `EFI/BOOT/BOOTX64.EFI` (exact upper-case name) exists on a FAT32 partition. Re-run the installer's `list`. |
| Boots to OVMF's own "EDK II" shell (QEMU) | `BOOTX64.EFI` missing or corrupt | Re-run `./build.sh`. Never mount `esp/` read-write in QEMU. |
| Shell appears, **typing does nothing** | Keyboard behind a hub/dock; hot-plugged after boot; USB keyboard on a non-xHCI controller | Plug the keyboard directly into a chassis port and power-cycle; try a PS/2 keyboard or enable firmware USB legacy emulation. |
| Typing does nothing, and the boot screen says `USB: controller up, but no HID boot keyboard attached.` | The keyboard is not a *boot-protocol* HID device, is on a hub, or is not on a root port | Same as above. |
| `save` says "no AHCI save disk available this boot" | No AHCI controller, or SATA set to IDE mode, or an NVMe-only machine | Set SATA to AHCI, or accept running without persistence. |
| Boot says `AHCI disk is blank -- not formatted for WISP` or `AHCI disk holds non-WISP data` | A disk that is not a WISP save store (left untouched on purpose) | Run `format` if (and only if) you intend to dedicate that disk. |
| A fault screen with a red exception name and register dump | A genuine kernel fault | Photograph the screen and report the vector, RIP and CR2 ([Chapter 33](#33-failure-modes-and-the-fault-screen)). |
| Screen is black after the firmware logo | Resolution too small for the 8x16 console, or no GOP | The kernel prints a fatal message through firmware text output if it cannot initialise the console. |
| `ls` is empty | `APPS` missing, or all files over budget | Check the file names, sizes and the budgets in [Chapter 24](#24-limits-at-a-glance). |


---

# Part III: Using WISP OS

## 11. The boot screen

From power-on to prompt takes a moment, and the lines that scroll past are the
machine telling you what it found. They are worth learning to read, because on
real hardware they are your diagnostic output ([Chapter 40](#40-hardware-bring-up-playbook)).

A representative boot under QEMU looks like this. Exact numbers, device names and
the presence of each line vary with the machine:

```text
Exiting UEFI Boot Services...
Physical frame allocator self-test: OK
Heap allocator self-test: OK
Interrupts, timer, and keyboard online.
ACPI: 1 CPU(s), 1 I/O APIC(s), LAPIC at 0xfee00000.
APIC: interrupts now routed through the I/O APIC; clock = LAPIC timer.
AHCI controller found and ABAR mapped.
AHCI disk identified: 'QEMU HARDDISK' on port 0, 131072 sectors (64 MiB).
AHCI disk is blank -- not formatted for WISP. Left untouched; run 'format' to use it for save/load.
USB: controller up, but no HID boot keyboard attached.
===========================================================
 WISP OS v1.1.0 -- Minimal Custom Kernel Shell
 hand-built from firmware up; no network stack, by design
===========================================================
Type 'help' for a list of commands. Esc stops a running program.

>
```

### Reading the lines

| Line | What it tells you |
|---|---|
| `Exiting UEFI Boot Services...` | The point of no return. Firmware is gone; everything after runs on the kernel's own tables. |
| `...allocator self-test: OK` | The physical-frame and heap allocators passed their boot-time self-tests. A failure here is logged to `debugcon` and is a real bug. |
| `ACPI: ... CPU(s), ... I/O APIC(s)` | The MADT parsed. The kernel counts the other CPUs but never starts them. |
| `APIC: interrupts now routed through the I/O APIC; clock = LAPIC timer.` | The modern interrupt path is live. If the MADT is missing or unusable, the kernel says so here instead and stays on the 8259/PIT; the shell still works. |
| `AHCI controller found and ABAR mapped.` / `AHCI disk identified: ...` | A SATA controller and disk were found. The model name, port and size printed are what `format` would erase. |
| `AHCI save disk mounted (save/load available).` | The disk already carries a valid WISP store. |
| `AHCI disk is blank ...` | Blank disk, left untouched. `format` provisions it. |
| `AHCI disk holds non-WISP data. Left untouched ...` | A foreign disk (a partition table, another filesystem). Left untouched; **do not `format` it unless you mean to erase it.** |
| `AHCI disk has a WISP save store this version does not understand.` | A WISP store with an unknown version or geometry. Left untouched, never reinterpreted. |
| `AHCI: ... (persistent storage unavailable)` | No usable AHCI controller or disk. `save`/`load` will say so. |
| `USB HID keyboard ready.` | A USB boot keyboard was found on an xHCI root port and is being polled. |
| `USB: controller up, but no HID boot keyboard attached.` | xHCI is fine; nothing keyboard-shaped was found. PS/2 still works. |
| `USB: ... (PS/2 keyboard only).` | No xHCI controller or it failed to initialise. |

Lines about an unmounted disk or a failed probe are drawn in red. The `>` prompt itself is green.

## 12. The shell

The shell is a single loop: print `> `, read a line, run it. Commands are separated
from their argument by the first space. There is no pipe, no redirection, no
variable expansion, no quoting, no history file, no scripting in the shell itself.
(Scripting is what WISP is for.) Command names are case-sensitive and lower case.
An unknown command prints `unknown command: <name>` in red.

### Command summary

| Command | Purpose |
|---|---|
| `help` | List the commands. |
| `clear` | Clear the screen. |
| `echo <text>` | Print `<text>` back. |
| `ls [APPS]` | List the in-memory snapshot of `APPS/*.wisp`, with sizes. |
| `run <name>` | Run `APPS/<name>.wisp`. |
| `wisp` | Open the interactive WISP prompt. |
| `uptime` | Time since boot. |
| `mem` | Heap and physical-memory use. |
| `version` | Kernel version and which hardware protections are live. |
| `save <key> <value>` | Store a string on the save disk. |
| `load <key>` | Print a saved value. |
| `keys` | List every saved key. |
| `delete <key>` | Remove a key. |
| `format` | Provision the save disk. **Erases.** Asks first. |

There is no `reboot`, `halt` or `poweroff` command. To leave, use your machine's
power button or the emulator's quit. (This is a roadmap item: [Chapter 42](#42-horizon-1-the-machine-gets-a-voice-and-a-door).)

### `help`

Prints the command list and the editing reminder:

```text
Available commands:
  help          Show this list of commands
  clear         Clear the screen
  echo <text>   Print <text> back to the screen
  ls [APPS]     List the in-memory APPS snapshot
  run <name>    Run APPS/<name>.wisp (a WISP program)
  wisp          Interactive WISP prompt ('exit' to leave)
  uptime        Show elapsed time since boot (timer ticks)
  mem           Show heap and physical memory usage
  version       Show the kernel version and protections
  save <k> <v>  Save <v> under key <k> on the AHCI save disk
  load <k>      Print the value saved under key <k>
  keys          List every key on the save disk
  delete <k>    Delete key <k> from the save disk
  format        Set up the AHCI disk for save/load (ERASES it; asks first)
Press Esc to stop a running program. Arrows, Home, End and Delete edit
a line; Up and Down recall earlier ones.
```

### `echo <text>`

Prints everything after the first space, verbatim. `echo` alone prints a blank line.

### `ls [APPS]`

Lists the **APPS snapshot**: the files read into RAM at boot. Each line shows the
file name and its size in bytes. `ls` and `ls APPS` are identical; any other path
prints an honest error, because there is no live filesystem after boot:

```text
> ls
     adventure.wisp  (5338 bytes)
     bounce.wisp  (2014 bytes)
     ...
> ls /somewhere
ls: '/somewhere': no live filesystem after boot -- only the in-memory APPS snapshot is available (try 'ls' or 'ls APPS')
```

> **The snapshot is taken once, at boot.** A file you add to the stick afterwards,
> or edit, does not appear until the next boot. This is the single most common
> surprise for new users. See [Chapter 25](#25-getting-programs-onto-the-machine).

### `run <name>`

Runs a WISP program from the snapshot. The name may be written with or without the
`.wisp` extension, and matching ignores case: `run guess`, `run guess.wisp` and
`run GUESS` are the same program.

- Without a name: `run: expected a program name, e.g. 'run hello'`.
- Unknown name: `run: '<name>.wisp' not found in the APPS snapshot`.
- A parse or runtime error ends the program with one line in the form
  `wisp: line N: <message>`, and you are back at `>`.
- **Esc stops the program** at any moment ([Chapter 14](#14-stopping-a-program)).

Each `run` is a fresh interpreter: global variables do not survive from one run to
the next. (Use `save`/`load` for that, [Chapter 15](#15-persistence-saving-data).)

### `wisp`

Opens the interactive prompt; the last step of [Chapter 17](#17-a-guided-tour-of-wisp) walks through it.
`vars` lists what the session has defined; `exit` or `quit` leaves and frees
everything the session built.

### `uptime`

```text
> uptime
up 73.420s (7342 ticks)
```

The clock ticks at 100 Hz (10 ms per tick), driven by the LAPIC timer when
available and the PIT otherwise.

### `mem`

```text
> mem
heap:   312 KiB used / 8192 KiB total, 7880 KiB free in 3 block(s), largest 7600 KiB
frames: 231 MiB free of 255 MiB spanned (59136 free 4 KiB frames)
```

The heap is a fixed 8 MiB arena, address-ordered and coalescing. `largest` shows
fragmentation: if `free` is large but `largest` is small, the heap has fragmented.
Frames are 4 KiB physical pages used for page tables and device buffers. (Numbers
above are illustrative.)

### `version`

```text
> version
WISP OS v1.1.0 (x86_64 UEFI, no_std Rust)
memory protection: W^X, NX on, kernel write-protect on, guard-paged stacks
clock: Local APIC timer at 100 Hz
networking: none (by design)
```

On a CPU without the NX bit it says `NX unsupported by this CPU`. If the APIC path
was not available it says `clock: PIT at 100 Hz`. Include this output in any bug
report.

### `save`, `load`, `keys`, `delete`, `format`

See [Chapter 15](#15-persistence-saving-data), which covers them together with
their limits and failure modes.

## 13. Editing a line, and the keyboard

Every line of typed input (the `>` prompt, the `wisp>` prompt, and a program's
`input()`) goes through one line editor with a visible cursor.

| Key | Effect |
|---|---|
| Printable ASCII (`0x20`-`0x7E`) | Insert at the cursor. |
| Left / Right | Move the cursor. |
| Home / End | Jump to the start / end of the line. |
| Backspace | Delete the character before the cursor. |
| Delete | Delete the character under the cursor. |
| Up / Down | Recall earlier / later lines. **Only at `>` and `wisp>`** (a game's `input()` must not recall shell commands). |
| Enter (including keypad Enter) | Submit the line. |
| Esc | Context-dependent, see below. |

**What Esc does, by context**

| Where | Esc does |
|---|---|
| The `>` prompt | Erases the half-typed line. |
| The `wisp>` prompt | Erases the half-typed line. |
| A `...` continuation prompt | Discards the whole unfinished input. |
| Inside a program's `input()` | Stops the program; the half-typed line is discarded. |
| Anywhere a program is running | Stops the program ([Chapter 14](#14-stopping-a-program)). |

**Limits.** A line is at most 1024 characters; further keystrokes are ignored.
History keeps the last 32 lines at the shell prompt. The `wisp>` prompt keeps its own
history, separate from the shell's, and it survives from one `wisp` session to the
next until the machine restarts. The editor handles lines longer than the screen
width and screens that scroll while you type.

**Layout.** The keyboard tables are **US layout only**, scancode set 1 for PS/2 and
the HID usage table for USB. Shift works. Caps Lock, Num Lock, Scroll Lock, Ctrl,
Alt and function keys are recognised and discarded rather than mis-decoded, and Tab reaches the
line editor as a byte it ignores.
Characters outside printable ASCII cannot be typed. (Other layouts: Part VI.)

**Display.** The console is an 8x16 bitmap font drawn straight into the
framebuffer, ASCII only. Text in a program may contain other UTF-8 (a string's
`len` counts bytes, and the value is preserved exactly) but non-ASCII characters
render as a placeholder.

## 14. Stopping a program

Press **Esc**. It works on both PS/2 and USB keyboards, whatever the program is doing:

- inside any loop, including a pure compute loop with no I/O (the interpreter checks
  before every `while`/`for` iteration and every function call);
- inside `sleep_ms()` (it wakes within one 10 ms tick);
- while waiting in `input()` (the half-typed line is discarded);
- inside `map`, `filter`, `reduce` and `sort_by`.

The program ends with an ordinary runtime error naming the line it was on, which
doubles as a cheap profiler for "where was it stuck?":

```text
wisp: line 31: stopped (Esc pressed)
```

Two guarantees keep this clean: anything you typed into a program after it stopped
is discarded, so it never leaks onto the `>` prompt; and an Esc pressed at the
prompt itself never stops the *next* program you run. A program cannot catch or
ignore Esc: WISP has no exception handling, by design, so stopping is always
available. `primes.wisp` and `stopwatch.wisp` exist to be stopped this way.

## 15. Persistence: saving data

### The model

Persistent data lives in a **Wisp save partition** on an installed AHCI
disk, or in a legacy raw store on a separate AHCI disk when booted live.
The live boot medium is read once and never written. The save region
holds a **key/value store**:

- keys are strings of 1 to 32 bytes (no NUL bytes);
- values are any WISP value that does not contain a function: `nil`, `bool`, `int`,
  `string`, `list`, `map` (nested to the usual depth limit);
- at most **63 keys**;
- one value's encoding must fit in **4047 bytes**;
- the namespace is **flat and global**: every program that ever runs on a machine
  shares one key space.

The store is deliberately simple. It is a flat array of 64 fixed 4096-byte slots,
with no directories and no fragmentation. Chapter 31 explains why that was the
right call and exactly how a save survives a power cut.

### First-time setup: `format`

The live installer initializes its save partition, so installed AHCI
systems can use `save`/`load` immediately. A separate blank disk used by a
live session must first be provisioned with `format`:

```text
> format
format: this ERASES sectors 0-512 (256 KiB, including any partition table) of:
  AHCI port 0  model 'QEMU HARDDISK'  serial 'QM00001'  64 MiB
Type ERASE to continue, anything else to cancel:
```

(The model, serial and size are whatever your disk reports; the ones above are an example.) Read it. It names the device and the exact destructive range. It proceeds **only**
if you type `ERASE` in full. Anything else prints `format: cancelled, nothing was
written`. If the disk already holds a WISP store it adds `It currently holds a WISP
save store; every saved value will be lost.` On success: `format: save disk
formatted (save/load available)`.

`format` writes in three flushed steps: it invalidates the old superblock first,
then clears the slots, then publishes the new superblock last. A power cut at any
point therefore leaves a disk the next boot reports as blank or foreign, never one
it accepts with garbage in it.

On an installed disk, `format` displays the save partition's physical
sector range instead. It erases only the save store, preserving GPT and
the EFI files. A partitioned foreign disk with no valid Wisp save
partition is refused; create a Wisp installation on an intentionally
selected whole disk using the boot menu instead.

### Using it from the shell

```text
> save greeting hello world
saved 'greeting'
> load greeting
hello world
> save mood content
saved 'mood'
> keys
     greeting  (16 bytes)
     mood  (12 bytes)
2 of 63 keys in use
> delete mood
deleted 'mood'
> load mood
load: 'mood' not found
```

Two details worth knowing:

- `save <key> <value>` stores everything after the key as a **string**, verbatim,
  spaces included. `save n 5` then WISP `load("n")` gives the *string* `"5"`, not the
  int `5`. Use `int()` in your program.
- `load` prints the value the way `print()` would, so a string shows without quotes
  and you cannot tell `5` from `"5"` there. `keys` shows the stored size in bytes
  (the encoded size, which includes a tag and, for strings, a length prefix).

### Using it from WISP

```wisp
let n = load("visits");
if n == nil { n = 0; }
n = n + 1;
save("visits", n);
print("visit number " + str(n));
```

`load` returns `nil` for a missing key (not an error). `save(key, nil)` deletes the
key and frees its slot. `counter.wisp` is the canonical example: run it, reboot,
run it again, and the number is higher.

### Crash safety

Each `save` and `delete` is **atomic per key**: if power fails at any point, the key
holds either its old value or its new one, never neither, never garbage. A save
reports success only after the data is flushed to the device. This is tested by
stopping a simulated device after every possible write and flush and checking every
possible subset of unflushed writes reaching the media.

What it does *not* promise:

- **No cross-key transactions.** Two related `save` calls can be interrupted
  between them. Put related values in one list or map under one key if they must
  change together.
- **Durability needs a working cache flush.** A device that has a volatile write
  cache but supports neither flush command makes `save` *fail* rather than report
  an unbacked success.
- **Torn sectors** are assumed not to happen (the convention filesystems rely on);
  if one does, the checksum reports corruption instead of returning a wrong value.

### Errors you may see

| Message (summary) | Meaning |
|---|---|
| `no AHCI save disk available this boot` | No AHCI controller/disk was found at boot. |
| Not mounted / blank / foreign | The disk has no valid WISP store; `format` provisions it. |
| key empty or longer than 32 bytes | Pick a shorter key. |
| value too large for one slot | The encoding exceeds 4047 bytes. Split it across keys. |
| store full (63 keys) | Delete a key to make room. |
| corrupt data | The winner copy failed its checksum. `load` never falls back to an older copy, because that would silently return a value you had already replaced. |

### Key naming convention

Because the namespace is global, prefix keys with the program name: `tasks.list`,
`counter.boots`. The shell's `keys` will then group related keys visually.

### Backup and restore

A WISP store occupies exactly sectors 0-512 (513 sectors, 262,656 bytes) of the disk.

- **QEMU:** `save.img` is an ordinary file. Copy it.
- **Real disk (Linux, as root, source disk read only):**

```bash
sudo dd if=/dev/sdX of=wisp-save-backup.img bs=512 count=513
```

Restoring is the same command with `if` and `of` swapped, and **overwrites** the first
513 sectors of the target: triple-check the device. Because the store is atomic
per key but not across keys, take the backup while the machine is idle at the
prompt.

### Upgrading from the v1.0.0 format

A format-version-1 disk mounts and reads unchanged. The first `save` to it rewrites
one byte of the superblock (version 1 to 2) and flushes, before any version-2 data
exists. After that, a v1.0.0 kernel will refuse the disk (it reports an unsupported
store and touches nothing). A v1 store that was full (64 keys) must have a key deleted
before the first `save` can proceed, because version 2 needs one free slot.

## 16. The bundled programs

Fifteen programs ship in `esp/APPS/`. Each is also a worked example: read the source
alongside this table. All transcripts below were produced by running the real
interpreter sources with scripted input (the `tests/host` harness), so the output is
exactly what the program prints; only `guess`, `oracle` and `counter` involve
randomness or saved state.

| Program | What it is | Demonstrates | Ends by |
|---|---|---|---|
| `hello` | One `input()`, one greeting | The three basic moves | Itself |
| `guess` | Number-guessing game | `while`, `rand`, `if/elif/else`, `int()` | Guessing, or Esc |
| `oracle` | Fortune teller with a shuffle bag | Lists, `push`/`pop`, swap-then-pop | `quit` |
| `adventure` | Five-room text adventure with a locked door | Globals, whole-line string comparison | `quit` |
| `xor_brain` | 2-2-1 ReLU net computing XOR | Forward-pass inference, exact integer weights | `quit` |
| `tamer` | Trained 2-4-1 ReLU net, fixed-point | Real trained model on bare metal | `quit` |
| `counter` | Counts how often it has ever run | `save`/`load` across reboots | Itself |
| `bounce` | A square bouncing for 240 frames | `fill_screen`, `rect`, `sleep_ms` | Itself |
| `canvas` | Interactive drawing prompt | Every graphics builtin and its errors | `quit` |
| `stopwatch` | Laps once a second, forever | `time_ms`, `sleep_ms`, Esc | Esc |
| `primes` | Prints primes forever | Pure compute loop; Esc | Esc |
| `calc` | Integer calculator with precedence | A recursive-descent parser written in WISP | `quit` |
| `words` | Word-frequency counter | Maps, `get`, `contains`, `remove`, `for` over keys | `quit` |
| `tasks` | To-do list driven by a command table | Functions as values, `map`/`filter`/`reduce`/`sort_by` | `quit` |
| `undo` | Notebook with undo and redo | Closures capturing state | `quit` |

### `hello`

```text
> run hello
What's your name? Ada
Hello, Ada! Welcome to WISP, running straight from firmware.
```

### `guess`

The secret is `rand(1, 20)`; each wrong guess answers `Higher.` or `Lower.`. This is one possible game, and the secret is different every run:

```text
> run guess
I'm thinking of a number from 1 to 20.
Your guess: 10
Lower.
Your guess: 5
Got it in 2 tries!
```

### `oracle`

Type `ask` for a fortune, `quit` to leave. Fortunes are drawn without replacement
from a ten-line bag, so you see every fortune once before any repeats. It predates
`remove()` and shows the older, fastest idiom for removing an arbitrary element:
swap it with the last element, then `pop()`.

### `adventure`

*The Sealed Archive.* Commands: `look`, `north`, `south`, `east`, `west`,
`take lantern`, `take key`, `inventory`, `help`, `quit`. The route to the vault is:
north, take lantern, east (the closet is dark without the lantern), take key, west,
north, north. It was written for WISP v1, before maps and string indexing, and is
built entirely from globals and whole-line comparisons, which makes it a good
example of what the *minimal* language could already do.

### `xor_brain` and `tamer`

```text
> run xor_brain
xor_brain: a tiny hand-built neural net computing XOR.
Enter two bits (0 or 1); type 'quit' for either to exit.
x1: 0
x2: 1
0 XOR 1 = 1
```

`xor_brain` uses analytically derived integer weights, so there is no rounding to
explain away. `tamer` classifies a creature from two 0-10 stats (aggression and
size): a *matched* pair (both low or both high) is `Predictable`, a *mismatched* pair
is `Erratic`. That rule is XOR in disguise (not linearly separable), which is why it
needs a hidden layer:

```text
Aggression (0-10): 2
Size (0-10): 3
Predictable
Aggression (0-10): 9
Size (0-10): 1
Erratic
```

The weights were trained offline with full-batch Adam (`tools/train_tiny_nn.py`) and
checked at 100% accuracy using WISP's own integer arithmetic before being
transcribed. See [Chapter 22](#22-running-trained-models).

### `counter`

Run it twice, with a reboot in between. The first run says `This is the first time
counter.wisp has ever been run.`; later runs say `Welcome back.` and a higher count.
It needs a formatted save disk.

### `bounce` and `canvas`

`bounce` needs no input and ends after 240 frames. `canvas` is an interactive
drawing prompt: `plot`, `rect`, `line`, `fillscreen`, `clearscreen`, `quit`. Its `rect`
command lets you provoke the negative-size error and watch the shell recover.

### `calc`

```text
> run calc
calc: integer arithmetic with + - * / % and parentheses.
Type an expression, or 'quit' to leave.
calc> 2 + 3 * (4 + 6)
= 32
calc> (7 - 2) * -3
= -15
calc> 10 / 0
error: division by zero
calc> 17 % 5
= 2
calc> 2 +
error: the expression ends too early
calc> quit
```

Every bad expression gets its own message and the calculator keeps running. Numbers
are limited to 18 digits and results to 10^18, which keeps every operation far from
integer overflow.

### `words`

```text
> run words
WORDS -- type some text and I'll count the words in it.
Commands: top, list, count WORD, forget WORD, quit.
words> the cat and the hat
Counted 5 words, 5 words in all.
words> top
5 words, 4 different. Most used:
  the: twice
  and: once
  cat: once
  hat: once
words> count the
the: twice
words> forget the
Forgot the.
words> list
  and: once
  cat: once
  hat: once
```

Because a map keeps its keys sorted, `list` is just `for w in counts`.

### `tasks`

```text
tasks> add 3 write manual
Added task 1.
tasks> add 1 fix bug
Added task 2.
tasks> add 2 ship it
Added task 3.
tasks> sort
Sorted by priority; equal priorities keep their order.
tasks> done 2
Done: ship it
tasks> open
[ ] p1 fix bug
[ ] p3 write manual
tasks> stats
3 task(s), 2 open, open priority load 4.
```

Look at `done 2` after `sort`: it marks task number 2 *in the current list order*, which
after sorting is "ship it". The command table at the heart of the program is a map from
what you type to the function that handles it: `commands[word](rest)`.

### `undo`

Every edit is a pair of closures that remember what that edit needs in order to be
undone and redone:

```text
undo> set a 10
set a = 10
undo> add a 5
add 5 to a (now 15)
undo> set b 3
set b = 3
undo> undo
Undid #3: set b = 3
undo> undo
Undid #2: add 5 to a (now 15)
undo> show
a = 10
undo> redo
Redid #2: add 5 to a (now 15)
undo> history
#1 set a = 10
#2 add 5 to a (now 15)
(1 undone, ready to redo)
```

(The history keeps the last 50 edits.)

> **Verification note.** `tasks` and `undo` (and the functions-as-values and closures
> features they showcase) were merged after v1.1.0 and exercised by the host harness
> and the real `.wisp` files driven through scripted sessions. The resulting kernel
> was built with zero warnings but has **not yet been booted under QEMU**, and no
> acceptance step covers them. Closing that gap is the first item of the roadmap
> ([Chapter 41](#41-horizon-0-trust-the-release)).


---

# Part IV: Programming in WISP

## 17. A guided tour of WISP

This tour takes you from `print` to closures in fourteen short steps. **Every program
and every output in this chapter was run against the real interpreter sources** (the
same `src/wisp` code the kernel uses, compiled for the host by `tests/host`), so what you
see is what the machine prints. Type them into the `wisp>` prompt, or save them as
`.wisp` files ([Chapter 25](#25-getting-programs-onto-the-machine)).

Two rules to remember from the start:

1. **Statements end with `;`.** Newlines mean nothing; you can format a program however
   you like. (At the `wisp>` prompt only the *last* `;` of a line may be left off.)
2. **`#` starts a comment** that runs to the end of the line.

### Step 1: Hello

```wisp
print("Hello, WISP!");
```

`print` writes its arguments, converted to text and separated by single spaces, then a
newline. `print()` alone prints a blank line. A string is written in double quotes with
the escapes `\"`, `\\`, `\n` and `\t`.

### Step 2: Variables and arithmetic

```wisp
let apples = 7;
let price = 3;
let total = apples * price;
print("total:", total);
print(7 / 2, 7 % 2, -7 / 2, -7 % 2);
print(2 + 3 * 4, (2 + 3) * 4);
apples = apples - 1;
print(apples);
```

```text
total: 21
3 1 -3 -1
14 20
6
```

- `let` declares; plain `=` reassigns. Assigning to an undeclared name is an error, on
  purpose: it catches typos instead of silently creating a new variable.
- Numbers are **64-bit signed integers**. There are no floats.
- `/` and `%` **truncate toward zero**: `-7 / 2` is `-3` and `-7 % 2` is `-1` (the
  remainder takes the sign of the left operand).
- Overflow is an **error**, not a wraparound (`9223372036854775807 + 1` stops the
  program), and division by zero is an error.

### Step 3: Decisions and loops

```wisp
let n = 12;
if n % 2 == 0 {
    print(n, "is even");
} elif n % 3 == 0 {
    print(n, "is a multiple of three");
} else {
    print(n, "is neither");
}

let i = 1;
while i <= 3 {
    print("while", i);
    i = i + 1;
}

for k in range(5) {
    if k == 1 { continue; }
    if k == 4 { break; }
    print("for", k);
}
```

```text
12 is even
while 1
while 2
while 3
for 0
for 2
for 3
```

Conditions must be real booleans. There is **no truthiness**: `if 1 { }` is an error,
because silently treating numbers or empty strings as true or false is a classic source
of bugs. Write the comparison you mean: `if n != 0 { }`. `and`, `or` and `not` work on
booleans only and short-circuit.

`range(n)` is the list `[0, 1, ..., n-1]`; `range(a, b)` is `[a, ..., b-1]`. `break` and
`continue` act on the innermost loop.

### Step 4: Strings

```wisp
let s = "hello, world";
print(len(s), s[0], s[7:], s[:5]);
print(upper(s), find(s, "world"), replace(s, "l", "L"));
let words = split("  go   north now ");
print(words, len(words));
print(join(words, "-"));
print(chars("abc"));
print("n=" + str(42));
print(trim("  padded  ") + "|");
print(starts_with(s, "hell"), ends_with(s, "x"), contains(s, ", "));
print(repeat("ab", 3), chr(65), ord("A"));
for c in "héy" { print(c); }
print(len("é"));
```

```text
12 h world hello
HELLO, WORLD 7 heLLo, worLd
["go", "north", "now"] 3
go-north-now
["a", "b", "c"]
n=42
padded|
true false true
ababab A 65
h
é
y
2
```

- Strings are **immutable**: every operation makes a new string, and `s[0] = "x"` is an
  error.
- **Offsets are bytes.** For ASCII text (everything the console can draw) a byte is a
  character. For other characters (2-4 bytes in UTF-8) an offset must land on the first
  byte of a character: `"é"[1]` is an error rather than half a character. `for c in s`
  and `chars(s)` walk **characters**, so use them for text that may not be ASCII.
- `+` joins two strings. It never converts for you: `"n=" + 42` is an error; write
  `"n=" + str(42)`.
- `split(s)` splits on runs of whitespace and never yields empty strings;
  `split(s, sep)` splits on every occurrence of `sep` and keeps empty pieces, so
  `join(split(s, sep), sep) == s`.

### Step 5: Lists

```wisp
let a = [3, 1, 2];
push(a, 10);
print(a, len(a), a[0], a[1:3], a[:2], a + [7]);
sort(a);
print(a);
reverse(a);
print(a);
print(pop(a));
print(a);
insert(a, 1, 99);
print(a);
print(remove(a, 0));
print(a);
print(find(a, 99), contains(a, 5), range(3), range(2, 5));
let grid = [[0, 0], [0, 0]];
grid[1][0] = 5;
print(grid);
print([1, 2] == [1, 2], [[1], "x", true, nil]);
```

```text
[3, 1, 2, 10] 4 3 [1, 2] [3, 1] [3, 1, 2, 10, 7]
[1, 2, 3, 10]
[10, 3, 2, 1]
1
[10, 3, 2]
[10, 99, 3, 2]
10
[99, 3, 2]
0 false [0, 1, 2] [2, 3, 4]
[[0, 0], [5, 0]]
true [[1], "x", true, nil]
```

- Index from 0. An index outside `0 <= i < len` is an error: there is no negative
  indexing and no silent clamping. Slices `a[i:j]` have the same strictness.
- `push`, `pop`, `insert`, `remove`, `sort` and `reverse` change the list **in place**.
  Slicing and `+` make **new** lists.
- **Lists are shared, not copied.** This is the single most important thing to know:

```wisp
let a = [1];
let b = a;          # b is the same list, not a copy
push(b, 2);
print(a);           # [1, 2]
let c = a[:];       # a[:] is a real copy
push(c, 3);
print(a, c);        # [1, 2] [1, 2, 3]
```

  Every other value (int, string, bool) is copied on assignment; only lists and maps are
  shared. Equality (`==`) is *structural*: two different lists with equal contents are `==`.

### Step 6: Maps

```wisp
let hero = {"name": "ada", "hp": 12, "items": ["lamp"]};
hero["hp"] = hero["hp"] - 3;
hero["gold"] = 5;
print(hero);
for key in hero {
    print(key, hero[key]);
}
print(keys(hero), values(hero), len(hero));
print(get(hero, "mana"), get(hero, "mana", 0), contains(hero, "hp"));
print(remove(hero, "gold"), hero);
let mixed = {2: "two", 1: "one", "b": 0, "a": 1};
print(mixed);
```

```text
{"gold": 5, "hp": 9, "items": ["lamp"], "name": "ada"}
gold 5
hp 9
items ["lamp"]
name ada
["gold", "hp", "items", "name"] [5, 9, ["lamp"], "ada"] 4
nil 0 true
5 {"hp": 9, "items": ["lamp"], "name": "ada"}
{1: "one", 2: "two", "a": 1, "b": 0}
```

- Keys are **ints or strings** (`1` and `"1"` are different keys). Values can be anything.
- A map **keeps its keys sorted**: int keys first in numeric order, then string keys in
  byte order. `print`, `for`, `keys` and `values` all use that order, so a program's
  output never depends on the order entries were added in.
- Reading a missing key is an **error**, not `nil`. Ask first with `contains`, or read
  with `get(map, key)` / `get(map, key, default)`.
- Maps are shared like lists. WISP has no map-copy builtin; copy with a loop:
  `let c = {}; for k in m { c[k] = m[k]; }` (this is a shallow copy).

### Step 7: Functions

```wisp
let greeting = "hi";

fn shout(name) {
    let text = greeting + ", " + upper(name) + "!";
    return text;
}

fn fact(n) {
    if n <= 1 { return 1; }
    return n * fact(n - 1);
}

print(shout("ada"));
print(fact(10), fact(20));
print(later(4));

fn later(x) { return x * 2; }
```

```text
hi, ADA!
3628800 2432902008176640000
8
```

- `fn name(params) { ... }` declares a function. A declaration is **top-level only**, and
  is *hoisted*: `later` is callable before the line that defines it.
- A named function sees **its own parameters and locals, and the globals**, never the
  local variables of whoever called it. That keeps functions predictable. (A function
  that must see surrounding variables is a closure, step 9.)
- `return` with no value, or falling off the end, gives `nil`. Recursion works to 256
  nested calls; deeper is a clean `stack overflow` error, not a crash. (`fact(21)`
  overflows an `int`, and says so.)

### Step 8: Functions are values

```wisp
fn double(x) { return x * 2; }
fn is_even(x) { return x % 2 == 0; }
fn add(a, b) { return a + b; }
fn square(x) { return x * x; }

let f = double;
print(f(21), type(f), f);
print(map(double, [1, 2, 3]));
print(filter(is_even, range(10)));
print(reduce(add, [1, 2, 3, 4], 0));
let people = ["grace", "al", "ada"];
sort_by(people, len);
print(people);
print(reduce(add, filter(is_even, map(square, range(6))), 0));
let table = {"d": double, "sq": square};
print(table["sq"](9));
```

```text
42 func <fn double>
[2, 4, 6]
[0, 2, 4, 6, 8]
10
["al", "ada", "grace"]
20
81
```

A function's name, used where a value is expected, *is* that function. It can sit in a
variable, a list or a map, be passed or returned. Built-ins are function values too
(`sort_by(people, len)` above). `map`, `filter`, `reduce` and `sort_by` take a function:

| Call | Result |
|---|---|
| `map(f, list)` | New list of `f(x)` for each element. |
| `filter(f, list)` | New list of the elements where `f(x)` is `true` (must return a bool). |
| `reduce(f, list, start)` | Folds from the left: `start`, then `f(result, x)` for each `x`. |
| `sort_by(list, f)` | Reorders **in place** by the key `f(x)` (all ints or all strings). **Stable.** |

`tasks.wisp` is the worked example: a command table `{"add": cmd_add, ...}` called as
`commands[word](rest)`.

### Step 9: Closures

```wisp
fn make_counter() {
    let n = 0;
    return fn () {
        n = n + 1;
        return n;
    };
}
let a = make_counter();
let b = make_counter();
print(a(), a(), a(), b());

let adders = [];
for i in range(3) {
    push(adders, fn (x) { return x + i; });
}
print(adders[0](10), adders[1](10), adders[2](10));
let double = fn (x) { return x * 2; };
print(double(21), double);
print(fn (a, b) { return a + b; }(1, 2));
```

```text
1 2 3 1
10 11 12
42 <fn (x)>
3
```

`fn (params) { body }` written where an expression is expected is a nameless function
that **remembers the variables around it**, and shares them (it does not copy them).
Each `make_counter()` call gets its own `n`. Each pass of a loop body is a fresh scope,
so the three closures in `adders` each keep their own `i`. `undo.wisp` is the worked
example: every edit is a pair of closures remembering what that edit needs.

### Step 10: Talking to the user

```wisp
let DIGITS = "0123456789";

fn is_number(text) {
    if len(text) == 0 { return false; }
    for c in text {
        if not contains(DIGITS, c) { return false; }
    }
    return true;
}

let total = 0;
while true {
    let line = input("number (or done): ");
    if line == "done" { break; }
    if not is_number(line) {
        print("not a number: " + line);
        continue;
    }
    total = total + int(line);
}
print("sum: " + str(total));
```

A session with the inputs `5`, `12`, `abc`, (empty), `30`, `done`:

```text
number (or done): 5
number (or done): 12
number (or done): abc
not a number: abc
number (or done): 
not a number: 
number (or done): 30
number (or done): done
sum: 47
```

`input(prompt)` prints the prompt, then waits for a line (with full cursor editing)
and returns it as a string. **`int("abc")` is an error**, and WISP has no exceptions, so
validate *before* converting, as `is_number` does. That is the idiom for every program
that reads numbers.

### Step 11: Remembering things

```wisp
let best = load("snake.best");
if best == nil { best = 0; }
let score = rand(0, 100);
print("score", score, "best", best);
if score > best {
    save("snake.best", score);
    print("new record!");
}
```

`load` gives `nil` for a key never saved. This needs a formatted save disk
([Chapter 15](#15-persistence-saving-data)); without one `save`/`load` are runtime errors
that say so.

### Step 12: Drawing

```wisp
let w = screen_width();
let h = screen_height();
fill_screen(rgb(10, 10, 30));
rect(w / 2 - 50, h / 2 - 50, 100, 100, rgb(220, 40, 40));
line(0, 0, w - 1, h - 1, rgb(255, 255, 255));
plot(w / 2, h / 2, 16777215);
```

Paints a dark background, a red square, a white diagonal, and one white pixel at the
centre. See [Chapter 21](#21-graphics) for the rules. The one to remember now: **text and
graphics share one framebuffer**, so draw your background *before* printing text you
want to keep.

### Step 13: When things go wrong

```text
wisp: line 1: undefined variable 'x' (did you forget 'let'?)
wisp: line 1: list index 5 out of bounds (length 2)
wisp: line 1: integer overflow in '21 * 2432902008176640000'
```

Every error is one line, `wisp: line N: <message>`, and you are back at the prompt. A
buggy script cannot crash or lock the machine. Press **Esc** to stop a runaway loop.
[Chapter 23](#23-error-messages) lists every message class.

### Step 14: The interactive prompt

Type `wisp` at the shell. Each line runs immediately against everything you defined so
far:

```text
wisp> let names = split("ada grace alan")
wisp> sort(names)
wisp> names
["ada", "alan", "grace"]
wisp> fn shout(s) { return upper(s) + "!"; }
wisp> shout(names[0])
"ADA!"
wisp> let total = 0
wisp> for n in range(1, 5) {
...   total = total + n;
...   }
wisp> total
10
wisp> 1 / 0
wisp: line 1: division by zero
wisp> let add = fn (a, b) { return a + b; }
wisp> add(2, 3)
5
wisp> let total = 100
wisp> total
100
```

- A line that is just an expression **shows its value** (strings in quotes, so `"1"` and
  `1` differ); `nil` shows nothing.
- The **final** `;` of the input may be left off. Inside a `{ ... }` block every statement
  still needs its `;` (note `total = total + n;` above).
- A line that opens more `{`, `(` or `[` than it closes **continues** at a `...` prompt
  until they balance, so `fn`, `if`, `while` and `for` can be typed over several lines.
- Unlike in a program file, `let` may **re-declare** a global and `fn` may **redefine**
  a function: that is how you fix a typo. Closures keep working across lines.
- Error line numbers count within the piece of input you just typed.
- Commands: `vars` lists the session's globals and functions; `exit` or `quit` leaves
  and frees everything the session built. Esc erases a half-typed line, or discards a
  whole unfinished `...` input. Up/Down recall earlier lines.
- A session holds everything it builds until you `exit`. If it holds most of the free
  heap the prompt says so (`this session holds most of the free heap`).

## 18. The language on one page

### Values: exactly seven types

| Type | Literals | Notes |
|---|---|---|
| `nil` | `nil` | Absence of a value; default return; what `load` gives for a missing key. |
| `bool` | `true` `false` | The only thing `if`, `while`, `and`, `or`, `not` accept. |
| `int` | `0`, `42` | 64-bit signed. `-5` is unary minus applied to `5`. |
| `string` | `"text"` | Immutable UTF-8. Escapes `\"` `\\` `\n` `\t`. One line only. |
| `list` | `[1, "a", [2]]` | Growable, mutable, **shared**. |
| `map` | `{"k": v}` | Keys int or string, sorted, mutable, **shared**. |
| `func` | named function, built-in, `fn (x) { }` | A first-class value. Cannot be a map key or be saved. |

There are no user-defined types: a map with fixed keys is how you group named fields.
`type(v)` returns `"nil"`, `"bool"`, `"int"`, `"string"`, `"list"`, `"map"` or `"func"`.

### Keywords (reserved)

`let if elif else while for in fn return break continue true false nil and or not`

### Operators, lowest precedence first

| Level | Operators | Associativity |
|---|---|---|
| 1 | `or` | left |
| 2 | `and` | left |
| 3 | `==` `!=` | left |
| 4 | `<` `<=` `>` `>=` | left |
| 5 | `+` `-` | left |
| 6 | `*` `/` `%` | left |
| 7 | `not`, unary `-` | right (prefix) |
| 8 | grouping `( )`, calls, `[ ]` and `{ }` literals, closures, indexing, slicing | |

There are **no bitwise operators**, no `**`, no `++`, no ternary, no compound assignment
(`+=`).

### Semantics that surprise people

| Rule | Detail |
|---|---|
| `+` | Two ints, two strings or two lists. Any other mix is an error; use `str()`. |
| `==` / `!=` | Same-typed values compare by value (lists and maps structurally); different types are simply unequal. Never an error. |
| `<` `<=` `>` `>=` | Two ints or two strings (byte order). Anything else is an error, lists included. |
| `and` `or` `not` | Booleans only; `and`/`or` short-circuit. |
| `/` `%` | Truncate toward zero; division by zero and overflow are errors. |
| Assignment | `let` declares, `=` reassigns, `x[i] = v` writes a list element or map entry, chains work: `world["rooms"][0]["exits"]["n"] = 1;`. |
| Sharing | Lists and maps are handles. `let b = a;` and passing to a function share one object. Copy a list with `a[:]`. |
| Order of evaluation | Left to right. Note that because lists are shared, `print(a, remove(a, 0), a)` shows `a` in its *final* state for both arguments. |

### Scoping

- A `let` inside a block (`if`, `while`, `for` body, function body) lives until the block
  ends. A block can read and reassign (with `=`) any outer variable.
- Shadowing is legal across levels. A second `let` of the same name in the **same**
  block is an error (`'x' is already declared in this scope`): reassign instead.
- Each pass through a loop body is a **fresh scope**, so `let` inside a loop is clean
  every iteration.
- `fn name(...)` is top-level only, hoisted, and sees only parameters, locals and
  globals. `fn (...) { }` as an expression is a closure and sees every enclosing scope,
  *sharing* (not copying) the variables.
- Variables and functions share one lookup: the nearest **variable** first, then the
  function of that name. A parameter named `f` holding a function is what `f(x)` calls.
  A variable holding something *else* (`let f = 5;`) does not hide a function called
  `f`.

### Statements

```text
let NAME = EXPR;
NAME = EXPR;
NAME[INDEX] = EXPR;
if EXPR { ... } elif EXPR { ... } else { ... }
while EXPR { ... }
for NAME in EXPR { ... }      # EXPR: a list, a map (visits keys) or a string (visits characters)
break;   continue;
fn NAME(PARAM, ...) { ... return EXPR; }
EXPR;                          # a call, usually
```

`for` evaluates its expression **once** and visits the elements the collection held when
the loop started: growing or shrinking it inside the body neither extends nor shortens
the loop.

### Ten gotchas, each verified

1. **Forgetting `;`** is the most common parse error: `expected ';', found identifier 'print'`.
2. **No truthiness.** `if 1 { }`, `while list { }` and `1 and 2` are errors.
3. **No `else if`.** The keyword is `elif`; `else if` fails with `expected '{', found 'if'`.
4. **Strings can't be edited in place.** Build a new one with slices and `+`.
5. **Aliasing.** `let b = a;` on a list or map shares it. Use `a[:]` for lists.
6. **Missing map key** is an error. Use `get()` or `contains()` first.
7. **`int("12a")` is an error**, and there is no `try`. Validate first.
8. **Integer overflow stops the program.** Factorials past 20! do.
9. **Named functions don't see caller locals.** Pass arguments, use globals, or write a closure.
10. **A UTF-8 byte-order mark** at the start of a file breaks the lexer
    (`unexpected character 'ï'`). Save program files as UTF-8 *without BOM*
    ([Chapter 25](#25-getting-programs-onto-the-machine)).

### Syntax traps

Each of these is a parse error; all were checked.

| You wrote | Why it fails |
|---|---|
| `[1, 2,]`, `{"a": 1,}`, `f(1, 2,)`, `fn f(a, b,)` | No trailing commas anywhere. |
| `if x { ... };` | A `;` after a closing `}` is a stray token (`unexpected ';'`). Blocks need no terminator. |
| `{ print(1); }` on its own | There are no bare blocks; a `{` where an expression is expected starts a map literal. |
| `x += 1;`, `list[0] += 1;` | No compound assignment. Write `x = x + 1;`. |
| `0x10`, `1_000` | Integer literals are plain decimal digits. |
| `"a" "b"` | No implicit string concatenation. Use `+`. |
| `else if` | Use `elif`. |
| `fn f() { fn g() { } }` | A named function is top-level only. Use `let g = fn () { };`. |

## 19. Built-in function reference

Fifty built-ins exist. They are always available and cannot be shadowed by `let` or
redefined by `fn`. Every one is also a function value (`let n = len;`). "Error" below
means a runtime error in the standard `wisp: line N: ...` form.

### Output, input, time, system

| Function | Description |
|---|---|
| `print(a, b, ...)` | Writes each argument as `str()` would, separated by spaces, then a newline. Strings inside lists/maps appear quoted; a bare string is unquoted. Output for one value stops after 16 KiB with `...(truncated)`. |
| `input(prompt)` | Writes `prompt`, waits for a line (editable), returns a `string`. Esc stops the program. |
| `str(v)` | Any value to its text. Error if the text would exceed 16 KiB. |
| `int(v)` | An int unchanged; a string parsed as an optionally `-`-prefixed decimal (error if invalid); `bool` and `nil` are errors. |
| `rand(lo, hi)` | Pseudo-random int in `[lo, hi]` (xorshift64, seeded once at boot from the CPU cycle counter). **Not cryptographically secure.** |
| `sleep_ms(n)` | Pause at least `n` ms, rounded up to the 10 ms tick; idles the CPU with `hlt`. `0` returns at once. Negative or non-int is an error. Esc ends it and stops the program. |
| `time_ms()` | Milliseconds since boot, monotonic, in 10 ms steps. |
| `clear()` | Clears the screen and homes the cursor (same as the shell's `clear`). |
| `type(v)` | `"nil"`, `"bool"`, `"int"`, `"string"`, `"list"`, `"map"`, `"func"`. |

### Persistence

| Function | Description |
|---|---|
| `save(key, value)` | Persist under a string key of 1-32 bytes. Atomic per key and flushed before it returns. `value` may be any value without a function inside. `save(key, nil)` deletes the key. Errors: no save disk, bad key, encoding over 4047 bytes, 63 keys already stored. |
| `load(key)` | The saved value (a fresh independent copy), or `nil` if never saved. Errors: no save disk, bad key, corrupt data. |

### Numbers

| Function | Description |
|---|---|
| `abs(n)` | Absolute value. `abs` of the most negative int is an overflow error. |
| `min(a, b)` / `max(a, b)` | Of two ints, or of two strings. |

### Strings (offsets are bytes; every result is a new string up to 256 KiB)

| Function | Description |
|---|---|
| `len(s)` | Bytes in a string. (Also lists and maps.) |
| `split(s)` / `split(s, sep)` | Words on whitespace runs / pieces on a non-empty separator, keeping empties. At most 32,768 pieces. |
| `join(list, sep)` | Join a list of **strings** with `sep` (use `str()` first for others). |
| `find(s, part)` | Byte offset of the first `part`, or `-1`. (Also `find(list, value)`.) |
| `contains(s, part)` | `bool`. (Also lists and maps.) |
| `replace(s, old, new)` | Replace every occurrence of a non-empty `old`. |
| `upper(s)` / `lower(s)` | ASCII letters only; all else unchanged. |
| `trim(s)` | Strip leading and trailing whitespace. |
| `starts_with(s, p)` / `ends_with(s, p)` | `bool`. |
| `chars(s)` | List of one-character strings, **by character**. |
| `repeat(s, n)` | `s` repeated `n >= 0` times. |
| `chr(n)` / `ord(s)` | Code point to one-character string, and back. |

### Lists (in place unless noted)

| Function | Description |
|---|---|
| `len(list)` | Element count. |
| `push(list, v)` | Append. Returns `nil`. |
| `pop(list)` | Remove and return the last element (error if empty). |
| `insert(list, i, v)` | Insert at `0 <= i <= len`. Returns `nil`. |
| `remove(list, i)` | Remove and return element `i`. |
| `sort(list)` | Ascending, all ints or all strings. Returns `nil`. |
| `reverse(list)` | Reverse. Returns `nil`. |
| `range(n)` / `range(a, b)` | New list of ints; at most 32,768 elements. |
| `find` / `contains` | See strings. |

### Maps

| Function | Description |
|---|---|
| `len(map)` | Number of entries. |
| `keys(map)` / `values(map)` | New lists, in key order. |
| `get(map, k)` / `get(map, k, default)` | Value or `nil`/`default` (never an error for a missing key). |
| `contains(map, k)` | `bool`. |
| `remove(map, k)` | Remove and return the value (error if missing). |

### Functions as arguments

`map(f, list)`, `filter(f, list)`, `reduce(f, list, start)`, `sort_by(list, f)`: see
[Chapter 17, step 8](#step-8-functions-are-values). Each works from a snapshot of the
list taken when the call began, honours Esc, and reports an error inside `f` at `f`'s
own line.

### Graphics

`screen_width()`, `screen_height()`, `rgb(r, g, b)`, `plot(x, y, c)`,
`rect(x, y, w, h, c)`, `line(x1, y1, x2, y2, c)`, `fill_screen(c)`: see
[Chapter 21](#21-graphics).

## 20. Recipes

Every recipe here was run through the interpreter; the output shown is real.

### Formatting numbers: padding, money, hex

There is no `printf`. Build text from `str`, `repeat` and arithmetic:

```wisp
fn pad_left(s, width, fill) {
    let text = str(s);
    if len(text) >= width { return text; }
    return repeat(fill, width - len(text)) + text;
}

fn money(cents) {
    let sign = "";
    if cents < 0 {
        sign = "-";
        cents = -cents;
    }
    return sign + "$" + str(cents / 100) + "." + pad_left(cents % 100, 2, "0");
}

fn hex(n) {
    if n == 0 { return "0"; }
    let digits = "0123456789abcdef";
    let out = "";
    while n > 0 {
        out = digits[n % 16] + out;
        n = n / 16;
    }
    return out;
}

print(pad_left(7, 3, "0"), pad_left("abcd", 3, " "));
print(money(1205), money(5), money(-250));
print(hex(255), hex(4096), hex(0));
```

```text
007 abcd
$12.05 $0.05 -$2.50
ff 1000 0
```

Money is kept as integer **cents** and divided only for display. That is the standard way
to do decimal quantities with no floats.

### Math you do not get for free

There is no `sqrt`, `pow`, `gcd` or bit manipulation built in; each is a few lines:

```wisp
fn gcd(a, b) {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    return a;
}

fn isqrt(n) {
    if n < 2 { return n; }
    let lo = 1;
    let hi = n;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if mid <= n / mid { lo = mid; } else { hi = mid - 1; }
    }
    return lo;
}

fn power(base, exp) {
    let result = 1;
    for i in range(exp) { result = result * base; }
    return result;
}

fn binary_search(sorted, target) {
    let lo = 0;
    let hi = len(sorted) - 1;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        if sorted[mid] == target { return mid; }
        if sorted[mid] < target { lo = mid + 1; } else { hi = mid - 1; }
    }
    return -1;
}

print(gcd(84, 36), isqrt(1000000), isqrt(99), power(2, 40));
let primes = [2, 3, 5, 7, 11, 13, 17];
print(binary_search(primes, 11), binary_search(primes, 4));
```

```text
12 1000 9 1099511627776
4 -1
```

`isqrt` divides (`n / mid`) rather than multiplying (`mid * mid`) so it can never
overflow, even near the top of the `int` range.

### Stack, queue, set, records

```wisp
let stack = [];
push(stack, "a"); push(stack, "b"); push(stack, "c");
print(pop(stack), pop(stack), stack);

let queue = [1, 2, 3];
push(queue, 4);
print(remove(queue, 0), remove(queue, 0), queue);

let seen = {};
for w in split("to be or not to be") {
    seen[w] = true;
}
print(keys(seen), contains(seen, "or"), contains(seen, "xyz"));

let people = [
    {"name": "ada", "age": 36},
    {"name": "alan", "age": 41},
    {"name": "grace", "age": 36}
];
sort_by(people, fn (p) { return p["name"]; });
sort_by(people, fn (p) { return p["age"]; });
for p in people { print(p["age"], p["name"]); }
```

```text
c b ["a"]
1 2 [3, 4]
["be", "not", "or", "to"] true false
36 ada
36 grace
41 alan
```

- A **set** is a map whose values are all `true`.
- A queue's `remove(queue, 0)` shifts the rest down, which is fine for small queues.
- To sort by two keys, sort by the **secondary** key first and the **primary** last:
  `sort_by` is *stable*, so equal primary keys keep the secondary order (the example
  above leaves Ada before Grace).

### Memoizing with a closure

```wisp
fn memoize(f) {
    let cache = {};
    return fn (n) {
        if contains(cache, n) { return cache[n]; }
        let v = f(n);
        cache[n] = v;
        return v;
    };
}
let calls = 0;
let slow_square = fn (n) { calls = calls + 1; return n * n; };
let fast_square = memoize(slow_square);
print(fast_square(12), fast_square(12), fast_square(12), calls);
```

```text
144 144 144 1
```

### Shuffling

```wisp
fn shuffle(list) {
    let i = len(list) - 1;
    while i > 0 {
        let j = rand(0, i);
        let t = list[i];
        list[i] = list[j];
        list[j] = t;
        i = i - 1;
    }
}
```

This is Fisher-Yates and is unbiased as long as `rand` is. For "draw without replacement"
(a shuffle bag) see `oracle.wisp`.

### Holding a frame rate

```wisp
let frame = 0;
while frame < 5 {
    let t0 = time_ms();
    print("frame", frame);
    let spent = time_ms() - t0;
    sleep_ms(100 - spent);
    frame = frame + 1;
}
```

Measure the work and sleep for the remainder. `sleep_ms` rounds up to 10 ms, so the
effective resolution is 10 ms. Keep `100 - spent` non-negative (a negative argument is an
error): use `max(0, 100 - spent)` for work that can exceed the budget.

### A command table

The shape of `tasks.wisp` and `undo.wisp`, and the best way to structure any
command-driven program:

```wisp
fn cmd_add(args) { print("adding", join(args, " ")); }
fn cmd_list(args) { print("listing"); }
fn cmd_help(args) { print("commands: add, list, help, quit"); }

let commands = {"add": cmd_add, "list": cmd_list, "help": cmd_help};

while true {
    let parts = split(input("> "));
    if len(parts) == 0 { continue; }
    let word = lower(parts[0]);
    if word == "quit" { break; }
    if contains(commands, word) {
        commands[word](parts[1:]);
    } else {
        print("Unknown command: " + parts[0] + ". Type help.");
    }
}
```

A session typing `add milk and eggs`, `list`, `dance` and `quit`:

```text
> add milk and eggs
adding milk and eggs
> list
listing
> dance
Unknown command: dance. Type help.
> quit
```

Adding a command is writing a function and adding one line to the table: no `if/elif`
chain grows. Passing `parts[1:]` hands each handler its arguments as a list.

### A saved high score

```wisp
let best = load("snake.best");
if best == nil { best = 0; }
let score = rand(0, 100);
print("score", score, "best", best);
if score > best {
    save("snake.best", score);
    print("new record!");
}
```

Remember to prefix keys with the program name ([Chapter 15](#15-persistence-saving-data)).

### Keeping saved state in one key

Because writes are atomic **per key** but not across keys, store related state together:

```wisp
save("game.state", {"level": 3, "hp": 12, "items": ["lamp", "key"]});
let s = load("game.state");
```

One 4047-byte value holds a lot of small integers and names; if you outgrow it, split by
*independent* concerns, never by values that must change together.

### Copying a map

```wisp
fn copy_map(m) {
    let c = {};
    for k in m { c[k] = m[k]; }
    return c;
}
```

Shallow: nested lists and maps in the values stay shared.

## 21. Graphics

WISP draws straight into the framebuffer the text console uses. There are seven
built-ins; this chapter is the working summary, and `docs/WISP_GRAPHICS.md` is the
complete contract.

### The pixel model

- `(0, 0)` is the **top-left** pixel. `x` grows right, `y` grows **down**.
- A **colour is an `int`** packed `0xRRGGBB`: red in bits 16-23, green 8-15, blue 0-7.
  `rgb(255, 0, 0)` and the literal `16711680` are the same pure red. `rgb()` is the only
  way to build one from channels at the language level, because WISP has no bitwise
  operators. If you know the hex, write the number.
- Any colour argument outside `0..=0xFFFFFF` is an error, never silently masked.
- `screen_width()` and `screen_height()` are fixed for the program's life (the display
  mode is set once at boot, and there is no mode switching).

### The primitives

| Call | Behaviour |
|---|---|
| `plot(x, y, c)` | One pixel. Off-screen is a silent no-op. |
| `rect(x, y, w, h, c)` | Filled rectangle, clipped to the screen. `w` or `h` of `0` is a silent no-op. **Negative `w` or `h` is an error** (always a swapped-argument or arithmetic bug). |
| `line(x1, y1, x2, y2, c)` | One-pixel Bresenham line, each pixel clipped independently. **Coordinates beyond +/-200,000 of the origin are an error**, because a line's cost depends on its endpoints before any clipping. |
| `fill_screen(c)` | Every pixel to one colour. Does **not** move the text cursor. |
| `clear()` | Screen to the console background (black) **and** cursor to top-left. |
| `rgb(r, g, b)` | Pack channels, each `0..=255`, else an error naming the channel. |

The asymmetry (silent clipping for position, errors for nonsensical sizes) is
deliberate. An object animating in from off-screen is ordinary; a negative width never is.

### Text and graphics share one framebuffer

There is no layer, no compositing and no z-order: whichever drew a pixel last owns it.
**`fill_screen` erases printed text as completely as it erases graphics.** An early draft
of `canvas.wisp` printed its banner and then painted the background, and the banner never
appeared. The rule:

> **Draw your background first; print the text you want to keep afterwards.**

Use `clear()` (not `fill_screen`) just before a closing message, to land it at a known spot.

### Animation without flicker

There is no double buffering and no vertical-sync wait, so a `fill_screen` every frame
flashes. Redraw only what moves: erase the old rectangle with the background colour, draw
the new one.

```wisp
let w = screen_width();
let h = screen_height();
let bg = rgb(10, 10, 20);
let ball = rgb(255, 140, 0);
fill_screen(bg);

let x = 40; let y = 40; let dx = 4; let dy = 3; let size = 30;
let frame = 0;
while frame < 240 {
    rect(x, y, size, size, bg);          # erase the old position
    x = x + dx;
    y = y + dy;
    if x < 0 or x + size > w { dx = -dx; }
    if y < 0 or y + size > h { dy = -dy; }
    rect(x, y, size, size, ball);        # draw the new one
    sleep_ms(16);
    frame = frame + 1;
}
clear();
print("done");
```

(`bounce.wisp` does the simpler `fill_screen` per frame, which is fine for a demo.)
The drawing primitives are not batched: one `rect` call is one trip through the
interpreter, so a program drawing thousands of shapes per frame will be slow. Draw few,
large shapes.

### A bar chart

```wisp
let data = [3, 9, 4, 7, 1];
let bar_w = 40;
let gap = 10;
let base = screen_height() - 40;
fill_screen(rgb(0, 0, 0));
for i in range(len(data)) {
    let bar_h = data[i] * 20;
    rect(40 + i * (bar_w + gap), base - bar_h, bar_w, bar_h, rgb(60, 120, 240));
}
```

### Limits worth knowing

- The font is 8x16 and ASCII only. There is currently no built-in way to draw text at a
  pixel position; `print` writes at the text cursor. (Roadmap: [Chapter 46](#46-horizon-5-graphics-sound-and-interaction).)
- There is no way to read a pixel back, no sprites, no blit, no circles.
- There is no non-blocking keyboard read: `input()` waits for Enter. Real-time games are
  therefore limited to timed animation plus a typed command between rounds.

## 22. Running trained models

`docs/WISP_AI.md` is the full reference; this is the working method. WISP runs **inference
only** (a forward pass) of tiny models trained elsewhere. It cannot train, and it cannot
load a `.safetensors` or `.gguf` file (a script cannot read files at all). What it can do is
run a small, fully owned, fully verified network, correctly, on bare metal, with no
floating point.

### The pipeline

1. **Train or derive** the model on a real computer (Python and numpy are enough;
   `tools/train_tiny_nn.py` is the worked example).
2. **Verify accuracy in float**.
3. **Quantize** every weight to an integer: `round(weight * SCALE)`. Both demos use
   `SCALE = 1000` (three decimal digits).
4. **Re-verify using the exact integer arithmetic WISP will run.** This is the step that
   matters. It caught a real bug: Python's `//` floors, but WISP's `/` **truncates toward
   zero**, so the two disagree on negative operands. `tools/train_tiny_nn.py` has a
   `_trunc_div` helper for exactly this.
5. **Bake the weights into the `.wisp`** as nested list literals and write the forward
   pass.
6. **Test the shipped WISP**, not just the Python that produced the constants, including a
   boundary case where being off by one quantization step flips the answer.

### Fixed-point in WISP

A real number `r` is stored as the integer `round(r * SCALE)`. Adding fixed-point values
is plain `+`. Multiplying two of them needs a divide-back-down, with rounding to nearest:

```wisp
let SCALE = 1000;
fn fixed_mul(a, b) {
    let prod = a * b;
    if prod >= 0 { return (prod + SCALE / 2) / SCALE; }
    return (prod - SCALE / 2) / SCALE;
}
fn relu(x) { if x > 0 { return x; } return 0; }
fn dense(W, B, x) {
    let out = [];
    for j in range(len(B)) {
        let acc = B[j];
        for i in range(len(x)) {
            acc = acc + fixed_mul(W[j][i], x[i]);
        }
        push(out, acc);
    }
    return out;
}
let W = [[500, -250], [1000, 1000]];
let B = [100, -500];
let x = [2000, 1000];
print(dense(W, B, x), map(relu, dense(W, B, [-3000, 500])));
print(fixed_mul(1500, 1500), fixed_mul(-1500, 1500));
```

```text
[850, 2500] [0, 0]
2250 -2250
```

Read it as: weights `0.5, -0.25` and `1.0, 1.0`, biases `0.1, -0.5`, input `(2.0, 1.0)` gives
`(0.85, 2.5)`. And `1.5 * 1.5 = 2.25`.

- **ReLU is scale-invariant** (`max(0, x)` works identically at any scale), which is why
  both demos use it. Sigmoid and tanh need a lookup table or polynomial.
- Multiplying a fixed-point value by a **plain integer** (a loop counter, a raw 0-10 stat)
  needs no `fixed_mul`.
- To threshold at 0.5 compare against `SCALE / 2`.
- The weights live in the source, so a model is limited by how much you will paste into a
  `.wisp` file (the 256 KiB per-file cap) and how many `fixed_mul` calls a tree-walking
  interpreter can do while a human waits.

### What this is not

There is no tokenizer, no attention and no LLM path. Real LLM inference leans on
SIMD-tuned matrix kernels and usually a GPU, none of which exist before an OS. What is
here is the other end of the size spectrum on purpose: tiny, fully owned, verified models.

## 23. Error messages

Every error has the shape `wisp: line N: <message>`. This catalog was produced by running
one deliberately broken program per row.

### Parse errors (the program never starts)

| Cause | Message |
|---|---|
| Missing `;` | `expected ';', found identifier 'print'` |
| Unclosed bracket | `expected ')', found end of input` |
| Unterminated string | `unterminated string literal (newline before closing '"')` |
| `break` outside a loop | `'break' outside of a loop` |
| Named `fn` inside a block | `'fn b' is a declaration and can only appear at the top level of a program (for a function value here, write 'fn (parameters) { ... }' with no name)` |
| Nesting beyond 128 levels | `program is nested too deeply (over 128 levels of blocks/expressions)` |
| A stray non-ASCII character outside a string | `unexpected character '...'` |

### Runtime errors (the program stops at that line)

| Cause | Message |
|---|---|
| Unknown variable (read or assign) | `undefined variable 'x' (did you forget 'let'?)` |
| Unknown function | `undefined function 'nosuch'` |
| Redeclared in the same block | `'x' is already declared in this scope` |
| Wrong argument count | `'f' expects 2 argument(s), got 1` |
| Calling a non-function | `cannot call 'n': it holds int, not a function` |
| Division by zero | `division by zero` |
| Overflow | `integer overflow in '9223372036854775807 + 1'` |
| `+` on mismatched types | `'+' expects two ints, two strings or two lists, found string and int` |
| `*` etc. on non-ints | `'*' expects two ints, found string and int` |
| Ordering on non-int/string | `comparison expects two ints or two strings, found list and list` |
| Non-boolean condition | `expected bool here, found int (1)` and `'and' expects bool, found int` |
| Index out of range | `list index 5 out of bounds (length 2)` |
| Missing map key | `key "b" is not in the map (check with contains(), or read it with get())` |
| Bad map key | `map keys must be ints or strings, found list` |
| Assigning into a string | `cannot assign into a string: strings are immutable (build a new one with slices and '+')` |
| Inside a UTF-8 character | `string index 1 falls inside a multi-byte character (string offsets are in bytes)` |
| `int("abc")` | `int() could not parse "abc" as an integer` |
| `len(5)` | `len() expects a string, list or map, found int` |
| `pop([])` | `pop() called on an empty list` |
| `chr(-1)` | `chr() expects a Unicode code point (0 to 1114111, excluding 55296 to 57343), found -1` |
| `range(100000)` | `range() would make 100000 elements; the limit is 32768` |
| `sleep_ms(-1)` | `sleep_ms() expects a non-negative int` |
| `rgb(256, 0, 0)` | `rgb() expects r in 0..=255, found 256` |
| `rect` negative size | `rect() width and height must not be negative, found w=-5, h=20` |
| `line` far away | `line() coordinates must be within +/-200000 of the origin, found x2=5000000` |
| Runaway recursion | `stack overflow: too much recursion (over 256 nested calls)` |
| Too much nesting | `stack overflow: program nests too deeply (expressions, blocks and calls combined)` |
| Value too large to show | `str(): the result would be longer than 16384 bytes` |
| Esc pressed | `stopped (Esc pressed)` |
| Heap reserve reached | `out of memory (the script used up the heap reserve and was stopped)` |

One known rough edge: a condition that is a bare **literal** (`if 1 { }`, `while 5 { }`)
reports `line 0`, because a literal carries no line number. A condition that is a
variable or an expression reports the right line. (Tracked in
[Chapter 41](#41-horizon-0-trust-the-release).)

## 24. Limits at a glance

Every limit below is enforced with a clean error rather than a crash or an unbounded
allocation.

| Limit | Value | When exceeded |
|---|---|---|
| Integer range | -2^63 to 2^63-1 | `integer overflow` error |
| Nested function calls | 256 | `stack overflow ... over 256 nested calls` |
| Combined nesting budget (calls, statements, expressions) | 1024 units | `stack overflow ... nests too deeply` |
| Syntax nesting depth (parens, blocks, each operator in a chain) | 128 | parse error |
| String length | 256 KiB | error from the builtin that would exceed it |
| List elements | 32,768 | error |
| Map entries | 32,768 | error |
| `split` pieces | 32,768 | error |
| `print` output for one value | 16 KiB | truncated with `...(truncated)` |
| `str()` result | 16 KiB | error |
| `==` work on large shared graphs | 100,000 steps | error |
| Free-heap reserve a script may not dip below | 1.5 MiB | `out of memory`, script stopped |
| `line()` coordinates | +/-200,000 | error |
| Save key | 1-32 bytes | error |
| Save value encoding | 4047 bytes | error |
| Save keys | 63 | error |
| Kernel heap | 8 MiB (fixed) | |
| Mainline stack | 1 MiB (guard-paged) | |
| Typed line | 1024 characters | extra keystrokes ignored |
| Shell history | 32 lines | oldest dropped |
| `APPS` file | 256 KiB | skipped at boot (logged to the debug port only) |
| `APPS` total | 1 MiB | later files skipped at boot |
| `APPS` file count | 64 | later files skipped at boot |
| `APPS` file name | 64 bytes | skipped at boot |

The bundled programs total 41,532 bytes, about 4% of the 1 MiB snapshot budget.

## 25. Getting programs onto the machine

### The workflow today

1. Write a program in any text editor on any computer.
2. Save it as `something.wisp` (the extension match ignores case).
3. Copy it into the **`APPS` folder at the top level** of the boot stick's FAT volume.
   Only files directly in `APPS` are read: subfolders are skipped.
4. **Reboot** from the stick. The snapshot is taken once, as the last act before Boot
   Services exit.
5. `ls` to confirm it is there, `run something` to run it.

To iterate faster, develop in QEMU (`./run.sh` re-reads `esp/APPS` on every start) or at
the `wisp>` prompt, then copy the finished program to the stick.

### Format rules

| Rule | Detail |
|---|---|
| Encoding | UTF-8. A file that is not valid UTF-8 is skipped. |
| **No BOM** | A UTF-8 byte-order mark at the start of the file makes the lexer fail with `unexpected character 'ï'`. Save as "UTF-8" without BOM. (Some Windows editors add one by default.) |
| Line endings | LF and CRLF both work (verified). Tabs are fine. |
| Name | At most 64 bytes, ending in `.wisp`. Long (VFAT) names work. |
| Size | At most 256 KiB each, 1 MiB for all, 64 files. |

### Why a skipped file is silent

A file that is too big, too long-named, not UTF-8 or past the count is skipped, and the
reason is written **only to the debug port** (`debug.log` under QEMU). On real hardware
that port does not exist, so the only symptom is that `ls` lacks the file. If a program
you copied is missing from `ls`, check the table above first. (A visible boot-log screen
and serial output are roadmap items: [Chapter 42](#42-horizon-1-the-machine-gets-a-voice-and-a-door).)

### Programs cannot read or write files

A script cannot open other files on the stick or write anything except through
`save`/`load`. There is also no `import`: one `.wisp` file is one whole program. Share
code between programs by pasting, until modules exist ([Chapter 43](#43-horizon-2-the-language-grows-up)).

### Writing a good program for this machine

- Start from the recipes in [Chapter 20](#20-recipes) and the bundled programs.
- Validate every `int(...)` conversion; there are no exceptions.
- Remember Esc: loops are stoppable for free, so a `while true` is fine.
- Put state you want to keep in **one key** with a program-name prefix.
- Keep functions small and global-light; closures are the clean way to carry state.
- Test on the host first: `tests/host` runs the real interpreter on a PC, and the existing
  test files are templates for scripted sessions (a `Host` that feeds `input()` lines).


---

# Part V: Inside WISP OS

This part explains how the machine works, one layer at a time, with enough detail to
read the source with confidence. Each chapter names the file that implements the
layer. For the full reasoning behind any decision, the matching section of
`CHANGES.md` is the primary source.

## 26. Architecture and boot order

### The layers

```text
 ┌──────────────────────────────────────────────────────────────┐
 │  WISP programs (.wisp text)        APPS snapshot in RAM      │
 ├──────────────────────────────────────────────────────────────┤
 │  wisp/: lexer → parser → AST → tree-walking interpreter      │
 │         values · builtins · Session · Host trait             │
 ├──────────────────────────────────────────────────────────────┤
 │  shell.rs   line_editor.rs   (commands, prompts, ShellHost)  │
 ├───────────────┬──────────────────┬───────────────────────────┤
 │  gfx.rs       │  keyboard.rs     │  save_format.rs           │
 │  font.rs      │  usb_hid.rs      │  (key/value store)        │
 │  (console,    │  xhci.rs         │  ahci.rs  pci.rs          │
 │  pixels)      │  (input drivers) │  (storage driver)         │
 ├───────────────┴──────────────────┴───────────────────────────┤
 │  idt.rs  gdt.rs  lapic.rs  ioapic.rs  pic.rs  pit.rs         │
 │  acpi.rs  clock.rs        (interrupts, time)                 │
 ├──────────────────────────────────────────────────────────────┤
 │  pmm.rs  paging.rs  allocator.rs  sync.rs     (memory)       │
 ├──────────────────────────────────────────────────────────────┤
 │  main.rs  panic.rs  debugcon.rs  fs.rs  libc_shims.rs        │
 │  (entry, exit_boot_services, fault screen, boot-volume read) │
 ├──────────────────────────────────────────────────────────────┤
 │              UEFI firmware (gone after exit)                 │
 └──────────────────────────────────────────────────────────────┘
```

### Source map

| File | Lines (approx.) | Role |
|---|---|---|
| `main.rs` | 855 | Entry point, `exit_boot_services`, bring-up order, stack switch |
| `gdt.rs` | 350 | GDT, TSS, IST stack, the 1 MiB mainline stack and its guard |
| `idt.rs` | 697 | 256-gate IDT, fault stubs, IRQ stubs, EOI routing, fault screen text |
| `pmm.rs` | 383 | Bitmap physical frame allocator, withheld/released boot-services memory |
| `paging.rs` | 782 | Our own 4-level page tables, W^X, NX, guard pages, MMIO mappings |
| `allocator.rs` | 408 | Address-ordered coalescing free-list heap, self-test, `free_bytes` |
| `acpi.rs` | 496 | RSDP, XSDT/RSDT, MADT parser (no AML) |
| `lapic.rs` | 558 | Local APIC, calibrated timer, EOI |
| `ioapic.rs` | 286 | I/O APIC redirection entries, staged-masked routing |
| `pic.rs` / `pit.rs` | 162 / 42 | The legacy 8259 and 8253/54, and the fallback path |
| `clock.rs` | 155 | The one timebase: ticks, sleeps, bounded `poll_until` waits |
| `keyboard.rs` | 318 | PS/2 driver, shared key queue, Esc stop-request flag |
| `xhci.rs` | 1655 | xHCI host controller: rings, slots, enumeration, transfers |
| `usb_hid.rs` | 219 | HID boot keyboard report decoding, polling loop |
| `pci.rs` | 235 | Legacy config-space access, bus-0 enumeration |
| `ahci.rs` | 1043 | AHCI SATA driver, IDENTIFY, command recovery, flush |
| `save_format.rs` | 972 | The key/value store: probe, provision, save, load, delete |
| `gfx.rs` / `font.rs` | 584 / 134 | Framebuffer console, 8x16 bitmap font |
| `fs.rs` | 227 | Boot-volume reads via UEFI protocols (pre-exit only) |
| `shell.rs` | 1016 | The `>` prompt, commands, `ShellHost` |
| `line_editor.rs` | 371 | Cursor editing and history |
| `panic.rs` | 167 | Independent fault/panic screen drawing |
| `wisp/*.rs` | ~4,700 | The language |

About 12,300 lines of Rust in the kernel (`src/*.rs`) and 4,700 in the language (`src/wisp`), plus
roughly 3,300 lines of Python test harness and 1,400 lines of WISP demos. There is exactly one
dependency: the `uefi` crate, pinned `=0.30.0`, used only before the exit.

### Boot order, in full

The order is not cosmetic: several steps would fault, hang or corrupt if reordered, and
`CHANGES.md` records the bugs that taught each ordering.

**Phase A: still a UEFI application (firmware services available)**

1. `allocator::init()`: the very first statement, because the `uefi` crate may allocate
   and our allocator must already be installed. The heap is a static 8 MiB array in
   `.bss`, never backed by firmware.
2. `uefi::helpers::init`, then open **GOP** and build the framebuffer `Console`. If this
   fails the kernel prints one line through firmware text output and returns.
3. Register the raw framebuffer with the **fault screen** (`panic.rs`), which draws
   independently of the console's state.
4. **Build** (not load) the GDT and IDT contents.
5. `fs::snapshot_apps`: read every `APPS/*.wisp` into RAM, within budget. This is the
   last use of Boot Services.
6. Record the loaded image's base and size (needed to map the kernel's own sections
   under W^X) and the ACPI **RSDP** address. Only these integers, the framebuffer
   fields and the APPS snapshot cross the boundary.
7. Print `Exiting UEFI Boot Services...` and call `exit_boot_services`.

**Phase B: after the exit, still on firmware's page tables, IDT and stack**

8. `pmm::init`: builds the frame bitmap from the final memory map, freeing
   **conventional memory only**. Firmware's live tables and stack sit in
   boot-services memory, which is **withheld**. Then the frame self-test, then the
   **heap self-test**.
9. `paging::init`: build the new 4-level tables with W^X flags, guard pages for the
   double-fault stack and mainline stack, and the framebuffer as an MMIO range.
   (Allocates thousands of frames, which is why the withholding in step 8 matters.)
10. `gdt::load`, `idt::load`, then `paging::load`: the first moment live CPU
    descriptor-table state changes. The IDT goes in *before* the `CR3` switch so a
    mistake in the new mapping is caught by our own fault screen, not firmware's.
11. Switch `RSP` to the dedicated mainline stack and continue there.

**Phase C: on the kernel's own tables, GDT, IDT and stack**

12. `pmm::release_boot_services_memory`: firmware's tables, stack and IDT are no longer
    live, so their frames are finally freed.
13. Initialise the 8259 and PIT, enable interrupts (`sti`):
    `Interrupts, timer, and keyboard online.`
14. Parse ACPI (`ACPI: ... CPU(s) ...`). If it parses, **bring up the APIC**: calibrate
    the LAPIC timer against the PIT, stage the I/O APIC routes *masked*, then in one
    interrupts-disabled section mask the 8259, hand EOI ownership to the LAPIC, unmask
    the routes and drain any PS/2 byte that arrived during the switch. Any failure
    before that section leaves the 8259/PIT path live, and says so.
15. Enumerate **PCI bus 0**.
16. Bring up the **save disk** (AHCI): find the controller, IDENTIFY the first disk,
    read LBA 0 only, and *probe* for a store. Never writes.
17. Bring up **USB**: find an xHCI controller, enumerate a boot keyboard on a root port,
    install it as a second input source.
18. Hand the machine to the shell: print the banner, emit the `[shell] ready` marker on
    the debug port, and loop forever.

There is no return path to firmware. The only ways out are a panic or CPU exception
(which draw the fault screen and halt) and the power button.

## 27. Memory

### Three allocators and who uses them

| Allocator | Granularity | Consumers |
|---|---|---|
| **Heap** (`allocator.rs`) | Bytes. 8 MiB fixed arena in `.bss`. | Every `Vec`, `String`, `Rc`, the APPS snapshot, the interpreter's whole world. |
| **PMM** (`pmm.rs`) | 4 KiB frames, a bitmap over every usable page. | Page-table pages and DMA structures (one whole frame each so alignment is free). |
| **Paging** (`paging.rs`) | Maps virtual to physical. Identity mapping. | MMIO windows (AHCI, xHCI) are mapped as needed. |

### The heap

An intrusive, **address-ordered**, singly linked free list with first-fit allocation,
block splitting, and **coalescing on free** (an O(1) check against the neighbours,
because the list is sorted by address). The earlier unordered, non-merging list degraded
into thousands of small fragments over a long session, since every `run` builds and
tears down an AST, scope maps and value lists of many sizes. `mem` shows `largest` free
block as a fragmentation gauge. The allocator **self-tests every boot** and keeps an
O(1) free-bytes counter that the shell uses to stop a runaway script.

Allocations that a script can trigger are **fallible** (`try_reserve`), so a script that
asks for too much gets an `out of memory` error rather than halting the kernel; when free
heap falls below a 1.5 MiB reserve the interpreter stops the script while there is still
room to report and clean up.

### Page tables and W^X

The kernel builds its own 4-level tables and switches `CR3` to them. Everything is mapped
with plain **4 KiB pages** (no 2 MiB or 1 GiB leaves, no higher half), identity (virtual
equals physical) for every region the kernel relies on, so the switch is invisible to
running code while the permissions become real.

- The kernel image is mapped **section by section from its own PE headers**: code
  read-only and executable, constants read-only, **everything else (data, heap, stacks,
  DMA buffers, MMIO) non-executable**.
- `EFER.NXE` and `CR0.WP` are on, so even ring 0 faults on a write to read-only memory
  or a jump into data. This is checked in `tests/fault_test.py` by a write to a constant
  and a call into a `ret`-filled data buffer; each must produce a page fault that the
  fault screen decodes as `write, protection violation` or `instruction fetch`.
- **Guard pages** (unmapped) sit below the mainline stack and the double-fault stack, so
  an overflow faults at the offending instruction instead of silently corrupting memory.
- If firmware has enabled 5-level paging (`CR4.LA57`), the new PML4 is wrapped in a PML5
  (it cannot be turned off in long mode). **This path has never been exercised**: no
  available OVMF enables LA57.
- `version` reports whether NX is in force; on a CPU without NX it says so.

### Stacks

| Stack | Size | Notes |
|---|---|---|
| Mainline | 1 MiB, guard-paged | The shell and interpreter run here. Raised from 256 KiB after the interpreter's native stack use was *measured* (Chapter 34). |
| Double-fault (IST1) | dedicated | Lets the CPU deliver `#DF` after a real stack overflow instead of triple-faulting into a silent reset. Proven by `__fault_test_stack_overflow`. |

### The firmware-memory hazard (audit verification gap 6)

`exit_boot_services` makes boot-services memory *reusable* but not immediately safe to
overwrite: until the kernel has its own tables, GDT/IDT and stack, the CPU is *running
on* firmware's, which live in exactly that memory. Under QEMU/OVMF the live `CR3`
hierarchy is 1,029 frames, all `BOOT_SERVICES_DATA`, and so are the live stack and IDT.
`paging::init` allocates about 6,300 frames first-fit from the bottom of RAM, so on
firmware that puts those structures low it would zero a live page table and reset the
machine with nothing on screen. It never fired on OVMF only because OVMF places them
near the top.

The fix is by construction, not by layout: `pmm::init` frees only conventional memory
and withholds the rest; `release_boot_services_memory` frees it as the first act on the
new stack. A second line of defence walks the live paging hierarchy and pins any frame of
it (and the live GDT, IDT and stack) that the map wrongly calls conventional, with a
warning. Test step 1c requires that boot-services frames were withheld, that every frame
of firmware's hierarchy was among them, that nothing needed pinning, and that exactly the
withheld count was released; reverting the fix fails it.

## 28. Interrupts, ACPI and the clock

### GDT, TSS, IDT

Long mode needs a GDT for the code-segment descriptor's 64-bit bit and for the TSS (which
now only carries stack pointers and the IST). The IDT has **all 256 gates populated**,
each with its own tiny stub, because a gate cannot otherwise know which vector invoked it.

- **Fault stubs (vectors 0-31):** normalise the stack (ten of the 32 exceptions push a
  hardware error code; the rest get a dummy 0), save the registers and call the fault
  handler, which draws the fault screen and halts.
- **IRQ stubs (vectors 32-255):** the same save sequence, then dispatch.
- The Windows x64 calling convention is what UEFI uses, not System V; getting that wrong
  once made IRQ handlers read garbage arguments (`CHANGES.md`, "Microsoft x64 calling
  convention").
- **One place decides where an EOI goes**: `idt::end_of_interrupt` picks the LAPIC or the
  8259 from a single mode flag, so the ownership switch is atomic.

### ACPI and the APIC pair

`acpi.rs` parses RSDP, then XSDT (or RSDT), then the **MADT** from scratch against the ACPI
spec: the LAPIC address, each I/O APIC and its GSI base, and the **Interrupt Source
Overrides**. Overrides matter because a legacy ISA IRQ number is not generally its Global
System Interrupt; the classic case is the timer (ISA IRQ0 arriving as GSI 2) on many
firmwares. There is **no AML interpreter**, so `_PRT`-style routing, power buttons and
battery information are out of reach.

The handover (Chapter 26, step 14) was reordered after audit finding AUD-05: routes are
first programmed fully but masked (`ioapic::stage_isa_irq`), and only the final switch,
in one interrupts-disabled section, unmasks them. If staging IRQ0 fails after IRQ1
succeeded, IRQ1 is masked again. IRQ1 is edge-triggered, so an unread PS/2 byte during the
switch would suppress every later edge; the section drains it.

### The clock

Both time sources (the LAPIC timer and the PIT) deliver the same vector at the same
**100 Hz** (`TICK_HZ`), and `idt.rs` routes it to the single counter in `clock.rs`.
*What keeps time* is a per-boot decision; *what time it is* is one counter. `uptime`,
`time_ms()` and `sleep_ms()` all read it.

`clock::poll_until` is where **every hardware wait in the kernel** lives: a bounded poll
against a tick deadline. A hardware or driver bug therefore produces a reported timeout,
never a silent hang. The ordering rule that comes with it: waits can only expire once timer
interrupts are being delivered, which is why `sti` precedes the AHCI and xHCI bring-up (a
mistake here hung boot once, and is how the project found out its failure paths needed
testing).

### Fallbacks

| Condition | Result |
|---|---|
| No ACPI tables at all | `ACPI: ... (staying on the legacy 8259 PIC)`; PIT clock; PS/2 on IRQ1. |
| MADT unusable | The same, with the reason logged. |
| LAPIC timer calibration fails | `keeping the PIT as this kernel's clock`. |
| APIC bring-up stops partway | `legacy 8259 remains the live interrupt controller`. |

## 29. Input: PS/2 and USB

Two drivers feed **one queue** of key bytes, so either keyboard works at any time.

### PS/2 (`keyboard.rs`)

IRQ1, scancode set 1 (what every controller speaks unless reprogrammed). The handler reads
the 8042 only when the status register says the output buffer is full and the byte is not
from the auxiliary (mouse) port. It tracks Shift, decodes printable ASCII through two
tables (unshifted and shifted, US layout), turns the `0xE0`-prefixed arrows, Home, End and
Delete into the line editor's key codes (`0x80`-`0x86`, above anything ASCII can produce),
and **discards** everything else (function keys, Ctrl/Alt, lock keys, the "fake shift"
codes some keyboards wrap arrows in) rather than mis-decode it.

The queue and Esc: Esc sets a **stop-request flag** that the interpreter polls; it is also
queued as a normal key for the line editor. A missing compiler fence once made the
keyboard-driven shell hang forever with keystrokes queued but never delivered, and is the
reason the project insists on `compiler_fence` around ISR-shared state (`CHANGES.md`).

### USB (`xhci.rs`, `usb_hid.rs`)

Built from scratch against the xHCI 1.2 specification:

- Finds the controller by **PCI class code** (class `0x0C`, subclass `0x03`, prog-if
  `0x30`), not by vendor ID, so any conformant controller qualifies.
- Maps BAR0, handling a 64-bit BAR (real controllers invariably have one).
- Command, event and transfer rings; slot enable; device address; endpoint configuration.
- Enumerates the device on a **root port**, checks its interface for the HID **boot
  keyboard** class triple, sends SET_PROTOCOL(boot), and drives exactly one interrupt
  endpoint.
- **Polled**, not interrupt-driven: the event ring is drained from `keyboard::wait_for_key`'s
  idle loop, which already wakes on every timer tick. That costs up to 10 ms of latency
  and avoids MSI/MSI-X.
- Boot reports are **eight bytes**: a modifier bitmap, a reserved byte, up to six key codes.
  The driver requests eight bytes (not the endpoint's max packet size) and accepts both
  Success and Short Packet completions when a full report arrived (audit AUD-11).
- The event ring is read **control dword first**, payload only after ownership is proven by
  the cycle bit (AUD-06).

### What it does not do (and what that means for you)

| Gap | Effect |
|---|---|
| **No hubs** | A keyboard behind a hub, dock or monitor is invisible. |
| **No hot-plug** | Ports are scanned once. A keyboard plugged in later is only noted in the log. |
| **Boot-protocol keyboards only** | No HID report-descriptor parser, so no mice, gamepads or exotic keyboards. |
| **First xHCI on PCI bus 0 only** | See Chapter 30; a controller on another bus is not found. |
| **No xHCI BIOS-to-OS ownership handoff** | The driver does not request ownership through the controller's USB Legacy Support extended capability. Many firmwares release the controller at exit; some may not. On a machine where USB input is dead but PS/2 emulation works, this is a candidate cause (unproven). |
| **EHCI/OHCI/UHCI not driven** | Pre-xHCI machines depend on firmware PS/2 emulation. |

## 30. Storage: PCI and AHCI

### PCI

`pci.rs` uses the legacy configuration mechanism (ports `0xCF8`/`0xCFC`) and enumerates
**bus 0 only**, with no recursion through PCI-to-PCI bridges and no ECAM/MCFG. Each
function is classified by class/subclass/programming-interface. AHCI is `01:06:01`.

Why this was acceptable: chipset AHCI controllers and, usually, xHCI controllers live on
bus 0. **The consequence for real hardware is real:** a SATA or USB controller that
firmware places behind a bridge (some add-in cards, some AMD and server layouts,
Thunderbolt-attached controllers) is not found, and the kernel reports "no xHCI/AHCI
controller found on PCI bus 0". Both drivers also use only the **first** matching
controller. Full enumeration is a roadmap item.

### AHCI (`ahci.rs`)

Built against AHCI 1.3.1:

- **Single outstanding command, slot 0, no NCQ.** Issue, then poll to completion with a
  bounded timeout. Ample for a key/value store's occasional sector writes.
- ABAR (BAR5) must be a 32-bit memory BAR, else the controller is rejected as non-conformant.
- Picks the first port with a SATA disk, IDENTIFYs it (model, serial, sector count, and
  cache/flush capability words 82/83/85/87).
- **`Hba::issue` owns the whole command lifecycle** (audit AUD-02). On a timeout or task-file
  error it runs the AHCI 6.2.2.1 recovery: clear `PxCMD.ST`, wait for `PxCMD.CR` to drop and
  verify `PxCI` is clear (only then has the controller released the command table and data
  buffer), clear `PxSERR`/`PxIS`, COMRESET if the device is still BSY/DRQ, restart. If
  quiescence cannot be proven the port is marked **failed**: every later request fails
  *before touching MMIO or DMA memory*, and the frames stay allocated.
- **`flush_cache`** issues FLUSH CACHE EXT (or FLUSH CACHE), chosen from IDENTIFY. `save`
  reports success only after the write and the flush.
- Every sector access goes through `save_format::region_lba`, which rejects anything
  outside LBA 0-512.

## 31. The save format

`docs/SAVE_FORMAT.md` is the byte-level spec; this is the working model.

```text
LBA 0               superblock: "WISPSAVE1", version 2, slot count 64, slot size 4096
LBA 1 .. 512        64 slots of 8 sectors (4096 bytes) each
                       slot: state | key(32) | length | checksum | generation | value...
```

- **63 keys**, because one slot is always kept free so an update can be written
  copy-on-write.
- A key may briefly have two copies. Every reader uses the **winner**: the highest
  generation (ties go to the lowest slot index).
- The checksum is **FNV-1a/32** over everything the copy says except the checksum field
  itself: header *and* value. Covering the header matters: a flipped generation bit could
  otherwise promote a stale copy to winner.
- **Value encoding:** a tag byte then a payload. `0` nil, `1` bool, `2` int (8 bytes LE),
  `3` string (u32 length + bytes), `4` list (u32 count + elements), `5` map (u32 count +
  key/value pairs in ascending key order). Decoding validates counts against the remaining
  bytes *before* looping and requires map keys strictly ascending.

### Saving, step by step

1. Scan headers; free every stale copy of any key; flush.
2. Refuse if the key is new and 63 keys already exist.
3. If the store is version 1, upgrade it (one superblock byte, flushed).
4. Write the new copy (generation = old winner's + 1) into the first free slot: value
   sectors first, **flush**, then the first sector (header + checksum), **flush**. That
   second flush is the **commit point**.
5. Free the old winner; flush.

Whatever subset of unflushed writes a power cut lets through, the key holds the old value or
the new one. The host tests prove this exhaustively: for every operation they stop the
simulated device after every possible number of writes and flushes and build the disk image
for *every subset* of unflushed writes reaching the media, then check every key. A mutation
that restores the old superblock-first order of `provision` makes the tests fail.

### What it deliberately does not do

No wear levelling, no cross-key atomicity, no directory structure, no namespacing, no
encryption, and the store owns the first 513 sectors of a **whole disk** rather than a
partition (a roadmap item).

## 32. The console and framebuffer

`gfx.rs` writes pixels directly into the GOP framebuffer, using the 8x16 bitmap font in
`font.rs` (generated, no font file read at runtime).

- Pixel layout is probed once (`Bgr` in every environment tested, `Rgb` also supported) and
  hidden behind `Color::from_rgb24`, so `0xRRGGBB` always means the same logical colour.
- Console palette: foreground `#D0E8D0`, accent (prompt) `#33FF88`, error `#FF5555`,
  background black.
- Scrolling copies the framebuffer up one text row, and the console counts how many times it
  has scrolled so the line editor can find a line that scrolled while it was being typed.
- Because text and WISP graphics write into the same pixels, there is no layering ([Chapter
  21](#21-graphics)).
- The console refuses a display too small for an 8x16 text grid.
- Raw drawing methods used by WISP (`plot_pixel`, `draw_rect_filled`, `draw_line`,
  `fill_screen`) do real, always-compiled bounds checks, because the release profile has no
  `debug_assert!` and no overflow checks, and use `unsigned_abs()` on script-supplied values
  (`i64::MIN.abs()` cannot be represented).
- `draw_line` is integer-only Bresenham with its own 500,000-step backstop beneath the
  builtin's 200,000-coordinate rejection.

## 33. Failure modes and the fault screen

### The fault screen

`panic.rs` draws independently of the console: from a raw framebuffer snapshot taken at boot,
with plain atomics instead of locks, because a crash can happen mid-update of the console or
while holding any lock. A fault or panic therefore cannot deadlock against the code that just
failed. It never returns.

For a CPU exception, the red-on-black screen reads like this (values are illustrative):

```text
CPU EXCEPTION
#14 Page Fault
page fault: write, protection violation
error_code=0x3  cr2=0x1a4000
rip=0x...  cs=0x...
rflags=0x...
rsp=0x...  ss=0x...
rax=0x... rbx=0x... rcx=0x... rdx=0x...
rsi=0x... rdi=0x... rbp=0x...
r8=0x... r9=0x... r10=0x... r11=0x...
r12=0x... r13=0x... r14=0x... r15=0x...
```

For `#PF` the error code is decoded into *what kind of access hit what kind of page* (read,
write or instruction fetch; page not present or protection violation; reserved bit set),
which since W^X is usually the whole diagnosis. `CR2` (the faulting address) is shown for
`#PF`. `RSP` and `SS` are reconstructed correctly for every vector (an early version printed fabricated
values for most exceptions). A Rust panic produces
the same style of screen with the panic message. **Photograph it** and report the vector,
`rip` and `cr2`: with the `BOOTX64.EFI` you ran, `rip` can be traced to a function.

### What happens when things go wrong

| Failure | Behaviour | Proven by |
|---|---|---|
| WISP syntax or runtime error | One-line `wisp: line N:` message, back to `>` | host tests, acceptance steps |
| Runaway script | Esc stops it | acceptance 17b, 18b, 21b |
| Script runs the heap to its reserve | Stopped with `out of memory`; everything it built is reclaimed | host tests |
| Native stack exhaustion by deep recursion | Impossible by construction (nesting budget); a stray overflow hits the guard page | host tests; `fault_test` |
| Breakpoint / page fault / double fault / write to read-only data / jump into data | Fault screen with the right vector and `cr2` | `fault_test.py` (six hooks) |
| No MADT / bad LAPIC | Falls back to 8259/PIT, says so | `usb_test` `no_xhci`, `apic` |
| No AHCI controller / no disk | `save`/`load` disabled, says so | `ahci_test` `no_controller`, `controller_no_disk` |
| Foreign or damaged disk | Left byte-for-byte untouched | `ahci_test` `foreign_disk_untouched` (hashes the whole image) |
| AHCI command timeout | Port recovery; a port that cannot be proven quiet is marked failed and refuses further work | Implemented to the AHCI spec; no injected-timeout test is recorded |
| Kernel panic | `panic = "abort"`, fault screen, halt | `panic-screen-smoke-test` feature build |

There is **no watchdog and no reboot-on-fault**: a fault ends in a halted screen until you
power-cycle. (Roadmap: [Chapter 42](#42-horizon-1-the-machine-gets-a-voice-and-a-door).)

### The hidden test hooks

A binary built with `--features fault-test-hooks` adds hidden shell commands that
deliberately fault, so `tests/fault_test.py` can prove each safety net catches a *real*
exception: `__fault_test_int3` (breakpoint), `__fault_test_pagefault` (unmapped address),
`__fault_test_guard_overflow` (stack guard page), `__fault_test_stack_overflow` (`RSP` walks
into the guard, which needs IST1 to survive), `__fault_test_wp` (write to read-only data),
`__fault_test_nx` (call into non-executable data). The normal release binary does not contain
them. The `panic-screen-smoke-test` feature panics right after the console exists, to check
the panic screen on its own.

## 34. The WISP interpreter

### Pipeline

```text
source text ──lexer──► tokens ──parser──► AST (Program) ──interp──► effects via Host
                                   │                         │
                         precedence climbing         tree-walking, Session state
                         MAX_SYNTAX_DEPTH = 128      MAX_NESTING = 1024 units
```

`wisp::run_source` lexes, parses and interprets one program in one `Session`. The `wisp>`
prompt keeps one `Session` across lines, parsing each with `parse_interactive`.

### The Host boundary

The language core never touches hardware. Everything outside the language (printing, input,
clear, sleep, random numbers, save/load, Esc, memory pressure, the clock and the pixels)
goes through the `Host` trait, which the shell implements as `ShellHost`. That is why the
whole language compiles for a PC and runs in `tests/host` unmodified, with a scripted
host that feeds `input()` lines, records output and models an in-memory disk. It is also
where a future platform port would plug in.

### Values and memory

- `Value` is `Nil | Bool | Int | Str | List | Map | Func | Closure`. Strings are `Rc<String>`.
  Lists and maps are `Rc<ListCell>`-style shared handles.
- Collections are **shared by reference**, so cycles are possible (`push(a, a)`, a closure
  stored in a variable it captured). Reference counting alone would leak them (audit AUD-09),
  so every list, map and closure a run creates is recorded as a **weak handle in the
  session's `ObjectRegistry`**, and `reclaim()` **empties them all when the run ends**,
  which breaks every cycle. Dropping a list is *iterative*, so a 200,000-deep chain frees
  without recursing on the kernel stack. Allocation-counted tests assert that repeated runs
  creating 10,000 cycles return to the same live-byte count.
- Every structural operation is bounded by **work or output, not just depth** (AUD-08):
  display stops at 16 KiB, equality memoises list-identity pairs and gives up after 100,000
  steps, serialization checks the 4047-byte budget before every append.

### Scopes and closures

A named function call starts a brand-new stack of **frames** holding only its parameters, so
it cannot see the caller's locals. A **closure** call starts from the stack that was live where
the `fn` expression ran and adds a frame for the parameters. Frames are shared cells, so a
closure and its creator see and assign the *same* variables. Every block entry (and every
loop pass) makes a fresh frame, which is why a `let` in a loop body declares cleanly each
time and why closures made in different passes keep different variables.

### The native stack budget (audit AUD-10)

A tree-walker backs every WISP call with real Rust stack frames, so depth must be bounded by
what the kernel stack can hold. This was **measured**, not guessed. Frame sizes were read from
the UEFI-target assembly:

| Item | Before | After |
|---|---|---|
| `eval_expr` / `exec_stmt` frame | 480 bytes each | `eval_expr` 160 (arms split into never-inlined functions) |
| Parser | ~300 bytes per precedence level, six levels per sub-expression; `parse_stmt` 1,088 | precedence climbing: one frame per sub-expression; a parenthesis level is ~1 KiB |
| One nested WISP call | ~1.9 KiB, so 256 calls needed ~470 KiB against a **256 KiB** stack | stack raised to 1 MiB; budget bounds the worst case at ~770 KiB |

The limits that result: `MAX_SYNTAX_DEPTH` 128, `MAX_NESTING` 1024 units shared by
statements, expressions and calls (a `for` costs two, because its native chain is larger),
`MAX_CALL_DEPTH` 256. Tested on a 640 KiB host stack: 250-deep recursion succeeds; runaway
recursion, 50 nested `while` bodies per call, 20,000 parentheses, 50,000-term chains and
5,000 nested blocks all end in a WISP error. Function bodies are shared (`Rc<FuncDecl>`), not
deep-cloned.

### Where Esc and memory are polled

The interpreter asks the host `stop_requested()` before every `while` iteration, every `for`
iteration and every user-function call, and the blocking built-ins check after they return.
`map`, `filter`, `reduce` and `sort_by` poll too, even when given a *built-in* callback that
has no check of its own (a tested case). `memory_low()` is polled at exactly the same points (`check_interrupts`), and builtins that build
large lists also check it every 256 elements.

### Builtins

`builtins.rs` dispatches on the name; `is_builtin` is the single list that stops `let` and
`fn` from shadowing one. Adding a builtin means: add the name there, add the dispatch arm,
implement it with explicit bounds and typed errors, add host tests, document it in
`docs/WISP_LANGUAGE.md`, and (if it can run long) make it poll the host.

## 35. Building and releasing

### Build

- `rust-toolchain.toml`: channel `nightly`, target `x86_64-unknown-uefi`, component `rust-src`.
- `Cargo.toml`: binary named `BOOTX64` (upper case on purpose, to match the UEFI fallback
  filename every script expects), `panic = "abort"`, release `opt-level = "s"`, `lto = true`.
  LTO at this opt level makes LLVM rewrite a `uefi`-internal length loop into a call to libc's
  `wcslen`, which does not exist here; `libc_shims.rs` provides it rather than degrade codegen.
- `.cargo/config.toml`: `/Brepro` and `/DEBUG:NONE`, for a byte-identical binary from the same
  source and toolchain in different directories (verified).
- `uefi = "=0.30.0"` is **hard-pinned**; its API shifts across releases and every call was
  checked against that exact source.
- Features: `fault-test-hooks`, `panic-screen-smoke-test`. Neither is in the release binary.

### Cutting a release

```bash
scripts/make_release.sh              # builds, then packages
scripts/make_release.sh --skip-build # package the existing esp/EFI/BOOT/BOOTX64.EFI
scripts/make_release.sh --iso        # also build the hybrid live ISO (xorriso)
```

It writes `dist/wisp-os-v<version>.tar.gz` and `.zip` (source, docs, scripts, test harnesses,
bundled programs, the prebuilt EFI and a per-file `SHA256SUMS`), plus `dist/SHA256SUMS` over the
two archives. The version comes from `Cargo.toml`, the single source of truth that the banner,
`version` and the archive name all read. Archives are reproducible: entries sorted, owners
zeroed, timestamps pinned to `SOURCE_DATE_EPOCH` (default: the newest modification time among
the packaged files).

`scripts/make_iso.sh [--skip-build] [--output PATH]` builds just the live
ISO plus a `.sha256` file. Its EFI boot image is generated by
`tools/make_esp.py` using only Python's standard library, with long
filenames and deterministic FAT metadata. It requires neither root nor
a mounted filesystem. Release source archives now include the complete
`tests/host` crate so the host checks can run from an extracted release.

### Release checklist

1. Bump `version` in `Cargo.toml`; update `Cargo.lock`, README and `USB_INSTALL.md` archive names,
   and acceptance step 16a (which checks the banner).
2. Build with the pinned toolchain; confirm **zero warnings**.
3. `cd tests/host && cargo test --release`: all green.
4. Run all four QEMU suites with fresh output directories; read the screenshots, do not trust
   OCR alone.
5. Run `scripts/make_release.sh`; verify `sha256sum -c SHA256SUMS` inside an extracted archive.
6. Rebuild from the archive in a different directory and compare `BOOTX64.EFI` byte for byte.
7. Write the `CHANGES.md` entry: what changed, what was verified, **what was not**.
8. Boot the stick on at least one real machine and record the result ([Chapter 40](#40-hardware-bring-up-playbook)).

## 36. Testing

### The pyramid

| Layer | Where | Count | Runs on |
|---|---|---|---|
| Host unit and behaviour tests | `tests/host/tests/*.rs` | 240 | any PC, seconds |
| Fault injection | `tests/fault_test.py` | 6 hook scenarios | QEMU |
| Storage | `tests/ahci_test.py` | `no_controller`, `controller_no_disk`, `full_stack`, `foreign_disk_untouched`, `legacy_v1_store` | QEMU |
| Interrupts and USB | `tests/usb_test.py` | `apic`, `no_xhci`, `xhci_no_device`, `usb_keyboard` | QEMU |
| End to end | `tests/acceptance_test.py` | 55 numbered checks: steps 1-24 plus second-boot persistence | QEMU |
| ISO image | `tests/iso_test.py` | FAT chains, long names, source bounds, output protection | host Python |
| Live installer | `tests/installer_test.py` | cancellation, small targets, byte comparisons, standalone boot, source protection, persistence | QEMU |
| Console capture | `tests/console_capture_test.py` | exact glyphs, cursor handling, PPM boundaries | host Python |

`python3 tests/iso_test.py` needs only Python's standard library. It
decodes the generated image independently of the writer and verifies
the EFI binary, app payloads, directory chains, duplicate-name handling,
snapshot budgets and refusal to replace a device/symlink output.

Run `python3 tests/installer_test.py` for a complete live installation
trial. It builds the kernel and ISO, creates disposable disk images under
`tests/out_installer`, and checks installation followed by two boots
without live media. `--skip-build` reuses the current EFI binary and ISO.
It compares GPT checksums, FAT directory chains and every installed app
and executable byte independently of the installer. Screen assertions
decode `src/font.rs` directly using `tests/console_capture.py`, so this
suite needs neither Pillow nor Tesseract. Other QEMU suites retain their
existing image/OCR dependencies.

### Host tests

| File | Tests | Covers |
|---|---|---|
| `language.rs` | 41 | `for`, slicing, string indexing, list `+`, string/list/number built-ins, interactive sessions |
| `maps.rs` | 43 | Map literals, indexing, order, builtins, display, equality, limits, saving |
| `functions.rs` | 43 | Functions as values, `map`/`filter`/`reduce`/`sort_by`, shadowing rules, `tasks.wisp` sessions |
| `closures.rs` | 45 | Capture, sharing, loop scopes, recursion, leaks, deep chains, `undo.wisp` sessions |
| `regressions.rs` | 29 | The audit findings: shared graphs, cycles, deep syntax, UTF-8, memory accounting |
| `save_format_v2.rs` | 20 | Crash-atomicity over every write/flush boundary and subset, v1 upgrade |
| `line_editor.rs` | 8 | Editing against a simulated character grid, keystroke by keystroke |
| `disk_layout.rs` | 7 | GPT/FAT32 and long names, installation IDs, validation before writes, interrupted writes, readback corruption |
| `save_partition.rs` | 4 | Save-partition isolation, raw API offsets, foreign MBR and malformed GPT refusal |

The tests use a **counting global allocator** to assert that runs return to the same live-byte
count (leak detection) and run deep-recursion cases on a deliberately small thread stack, which
is why `--release` is required.

### Mutation testing as practice

New features were checked by deliberately breaking them: eleven deliberate bugs for functions as
values (no Esc check in `map`, unstable `sort_by`, a non-function variable hiding a function...),
nine for closures. Surviving mutants were **treated as test gaps**: two survived the first draft
for functions and one for closures (captured frames in reverse order, which matters only when two
enclosing scopes declare the same name), and each now has a test that fails with the bug in. This is
the project's practice of proving the tests can fail.

### QEMU suites: how they work

Each suite boots the real kernel under QEMU/OVMF, types through QEMU's monitor `sendkey`, takes
screenshots, and asserts on three kinds of evidence: **debug-console log lines** (the kernel prints
markers such as `[shell] ready`, so tests wait for a condition instead of sleeping a fixed time; a
fixed sleep was seen to lose keystrokes under load), **OCR** of screenshots (a convenience), and
**sampled pixel colours** (ground truth: `assert_pixel_near`, `assert_colors_present`). Two apparent
failures were tesseract misreads, not bugs, which is why pixels outrank OCR in this project.

### Adding a test

- A language feature: add a test file under `tests/host/tests/`, driven by a `TestHost`. Include
  every documented error path, resource limits, an allocation-counted leak check, and a mutation
  pass.
- A driver behaviour: add a scenario to the matching QEMU suite with a debug-console marker.
- A new bundled program: add an acceptance step that runs it and OCR-checks (or pixel-checks) the
  result. (`tasks` and `undo` are missing this.)

## 37. How changes are made here

The conventions below are not written in one place in the repository; they are visible in every
`CHANGES.md` entry. Following them is what keeps the claims in this manual true.

1. **Investigate before building.** Read the relevant source and spec; measure rather than assume.
2. **Decide out loud.** Each non-obvious choice gets a paragraph in `CHANGES.md` saying what was
   chosen, what was rejected, and why.
3. **Bound everything.** Every loop, wait, allocation and recursion has a limit and a clean
   error. Script-controlled integers are untrusted input: use checked arithmetic and `unsigned_abs()`.
4. **Degrade, don't hang.** A failed device is a message, not a stop.
5. **Test the failure paths** as hard as the success path, including with a mutation pass.
6. **Zero warnings** in the project's own code, including with the feature builds.
7. **Update the docs in the same change**: `docs/WISP_LANGUAGE.md` for any language change,
   `docs/SAVE_FORMAT.md` for any format change, and this manual.
8. **Say what was not verified.** Every entry ends with what could not be run (no QEMU in the session,
   no hardware). Over-claiming is the failure this project guards against hardest.
9. **No `TODO`s, placeholders or half-finished branches** in committed code.
10. **Version honestly.** A format change bumps the format version and keeps old disks readable.

## 38. Verification ledger

What has been exercised, and what has not, as of this manual.

### Verified

| Claim | Evidence |
|---|---|
| 240 host tests pass | Original 229 language/save/editor tests plus seven disk-layout and four save-partition tests. |
| Live ISO installs a standalone system | `tests/installer_test.py`: cancelled and undersized targets unchanged, EFI/apps byte-identical, GPT/FAT validated, installed disk boots and runs `xor_brain`, saves survive reboot, boot disk excluded from install targets. |
| Current AHCI storage stack passes QEMU checks | All five `tests/ahci_test.py` scenarios passed, using exact bitmap text decoding for screenshots: missing controller, empty controller, full stack, foreign disk untouched and legacy-v1 upgrade. |
| The kernel builds with zero warnings | Run for this manual with `rustc 1.101.0-nightly (16d1ffac1 2026-10-26)`: `./build.sh` finished, no warnings, `BOOTX64.EFI` 308 KB. |
| v1.1.0 passes the QEMU suites | Recorded in `CHANGES.md`: fault, ahci and usb suites pass every scenario; acceptance runs to its last step with every OCR check passing (nightly 2026-10-19). |
| The bundled programs print what this manual shows | `hello`, `guess`, `calc`, `words`, `tasks`, `undo`, `adventure`, `tamer`, `xor_brain` were run through the real interpreter for this manual. |
| Every code sample and error message in Part IV | Run through the interpreter for this manual. |
| The 13 audit findings are fixed in code | `CHANGES.md`, "Audit remediation". AUD-01, 03, 04, 08, 09, 10, 12 and 13 have host tests or QEMU scenarios. AUD-02, 05, 06, 07 and 11 (command timeout, interrupt handover, event-ring ordering, boot-time `APPS` budgets, short USB packets) are fixed to the specifications and covered only by the QEMU suites still passing, not by fault injection. |
| Saves are crash-atomic | Exhaustive host model, plus `legacy_v1_store` and `full_stack` QEMU scenarios. |
| The build is reproducible | Byte-identical output from two directories (v1.0.0 record). |

### Not verified

| Item | Why it matters |
|---|---|
| **Any real hardware** | No physical machine has run this. All hardware claims are QEMU/OVMF. Biggest open risk. |
| **`tasks` and `undo` in QEMU** | Functions and closures have host coverage; the installed binary boots in QEMU and runs `xor_brain`, but these two apps still lack a QEMU acceptance step. |
| macOS and Windows installers | Never executed on real systems. |
| LA57 / 5-level paging path | No OVMF build enables it. |
| xHCI behaviour with real keyboards of every descriptor shape | QEMU's `usb-kbd` only. |
| AHCI timeout recovery on real hardware | Designed against the spec, not hardware-injected. |
| Behaviour with more than one AHCI disk | Only the first is used; foreign-disk safety is tested, selection is not. |

### Known documentation inconsistencies

The older documents were written as the project grew; these sentences are now stale:

| Where | Stale statement | Truth |
|---|---|---|
| `docs/WISP_LANGUAGE.md`, Lists, last bullet | "There is no `map`/`filter`: write the loop." | `map`, `filter`, `reduce`, `sort_by` exist (functions as values). |
| `docs/WISP_LANGUAGE.md`, "Size limits" | `save()` rejects an encoding over "4055 bytes" | The version-2 limit is **4047** (4055 was version 1). The `save()` entry in the same file says 4047. |
| `docs/WISP_LANGUAGE.md`, intro | "v2 is landing in pieces..." | All four pieces have landed. |
| `docs/WISP_AI.md` | Refers to "what's not in v1" and says a script cannot read files | Still true that scripts cannot read files; the section it points to is now "What's still not in WISP". |
| `esp/APPS/canvas.wisp`, header comment | "WISP has no string-splitting... no string indexing/slicing at all in v1" | It does now; the comment is historical. |
| `SHA256SUMS` at the repo root | Describes v1.0.0 | Regenerated only by `scripts/make_release.sh`. |
| `CHANGES.md`, "Not addressed" in the audit entry | Lists slot atomicity and the firmware-memory overlap as open | Both were fixed later (the entry says so in a trailing note). |

### Known defects found while writing this manual

| Defect | Evidence | Severity |
|---|---|---|
| A UTF-8 **BOM** at the start of a `.wisp` file fails to lex | `unexpected character 'ï'` on line 1 | Low, but trips Windows users; fix is one line in the lexer or the snapshot reader. |
| A bare-literal condition reports **`line 0`** | `while 5 { }` gives `wisp: line 0: expected bool here...` because literals carry no line | Cosmetic; fix by passing the statement's line. |
| Programs skipped at boot are **silent on real hardware** | Reasons go to the debug port only | Usability; needs an on-screen or serial log. |

None of these affect safety; all are cheap and are scheduled in [Chapter 41](#41-horizon-0-trust-the-release).

## 39. Security and threat model

This chapter is an *analysis*, not an audit result: the only external audit covered v1.0.0 and
its 13 findings are fixed. Anything below marked "unaudited" is reasoning from the design.

### What is being protected

The machine's integrity while it runs a program you did not write; the data on the save disk and
on any other disk attached; and the host you used to prepare the stick.

### Trust boundaries (there are fewer than in a normal OS)

- **There is no kernel/user boundary.** Everything runs in ring 0, and a WISP program is *data
  interpreted by kernel code*. The interpreter is the only isolation. A memory-safety bug in a
  built-in is therefore a kernel-level bug. This is why every built-in is bounded, why the
  language has no foreign-function interface, and why the interpreter is `no_std` Rust.
- **A WISP program can**: print, read the keyboard, draw, sleep, read the clock, and
  save/load its own key space on the save disk. It **cannot**: read other files, touch ports or
  memory, load code, or reach the network (there is none). It can overwrite *any* key on the
  save disk, since keys are one global namespace.
- **The save disk is not authenticated or encrypted.** Anyone with the disk can read and edit it;
  the checksum detects accident, not tampering.

### Findings and mitigations by area

| Area | Status |
|---|---|
| Hostile scripts: memory exhaustion, deep recursion, shared-graph blowups, cycles | Mitigated and tested (AUD-07 to AUD-10); limits in [Chapter 24](#24-limits-at-a-glance). |
| Script-controlled integers reaching drawing code | Checked arithmetic and `unsigned_abs()`; bounds checks always compiled. |
| `rand()` | Xorshift64 from the cycle counter: **not for security**. Documented. |
| Boot-time data destruction | Boot never writes; `format` needs `ERASE` (AUD-01). |
| Write/execute confusion in the kernel | W^X with NX and `CR0.WP`, proven by fault tests. |
| Unsigned binary | `BOOTX64.EFI` is unsigned; Secure Boot must be off or you enrol a key. Anyone who can write the stick can replace it. |
| **Hostile USB device** | **Unaudited.** The xHCI driver parses descriptors and runs transfers from a device it does not trust, in ring 0, with DMA. A malformed descriptor reaching a buggy parser is a plausible kernel compromise. Fuzzing it is a roadmap item. |
| **DMA from devices** | No IOMMU. A malicious PCI device already has full access; out of scope. |
| **Hostile disk image** | Probe validates the superblock completely; `load` validates counts against remaining bytes before looping and requires ordered map keys. Reasonable; **unfuzzed**. |
| Physical attacker | Can boot their own stick, read the save disk, replace the binary. Not a design goal to resist. |
| Supply chain | One pinned dependency (`uefi 0.30.0`) plus the Rust nightly toolchain. A reproducible build lets you check a shipped binary, *given the same toolchain*. |

### Reporting problems

Open an issue on the repository with the `version` output, what you did, and for a crash a photo
of the fault screen.

## 40. Hardware bring-up playbook

Real hardware is the largest unproven claim in this project. This chapter turns "try it on a
machine" into a procedure that produces a *useful* report whether it works or not.

### Before you start

- Pick a machine you can afford to lose a boot cycle on. **Disconnect any SATA disk whose
  contents matter**, or at minimum do not run `format`.
- Prepare the stick with a release archive's prebuilt binary and `sha256sum -c SHA256SUMS`.
- Put a **PS/2 or USB keyboard directly into the chassis**. Have a phone ready to take photos:
  there is no serial output yet, so the screen is your only log.
- In firmware setup: **Secure Boot off**, SATA mode **AHCI**.

### Known real-hardware risks, in the order they are likely to bite

| Risk | Symptom | First check |
|---|---|---|
| Secure Boot on | Firmware refuses or skips the stick | Disable it |
| Controller not on PCI bus 0 | `no xHCI controller found on PCI bus 0` / no AHCI | Note the vendor/model; this needs the bridge-walking enumeration (roadmap) |
| xHCI owned by firmware (no handoff) | Shell appears, no USB key works, PS/2 emulation does or doesn't | Try a PS/2 keyboard or firmware "legacy USB" option |
| Keyboard behind a hub or dock | No input | Plug directly into the chassis |
| NVMe-only machine | `AHCI: ... (persistent storage unavailable)` | Expected; persistence is unavailable until an NVMe driver exists |
| Very large reserved or MMIO ranges in the memory map | Slow or failed boot at the "Exiting UEFI Boot Services" step, or `paging` running out of frames | Page tables use 4 KiB pages only: mapping a 12 GiB reserved region took ~6,300 frames under OVMF. Note total RAM and any huge regions |
| Odd GOP pixel format | Wrong colours | The console handles `Bgr` and `Rgb`; report the machine |
| MADT quirks | `ACPI: ... (staying on the legacy 8259 PIC)` | Fine on a machine that still has an 8259; report the message |
| Interrupt routing on a legacy-free board | Typing works then stalls; timers stop | Photograph the boot log; report the board |

### What to record

| Item | Where it comes from |
|---|---|
| Machine make, model, BIOS version, CPU, RAM | The machine |
| Firmware settings changed | Your notes |
| Every line of the boot screen, in order | Photograph before the prompt appears |
| `version` output | The shell |
| `mem` output | The shell |
| `uptime` twice, ten seconds apart | Confirms the clock ticks at about 100 Hz (about 1000 ticks per 10 s) |
| Keyboard type and port; did typing work? | Your notes |
| `run hello`, `run calc`, `run bounce`, `run primes` then Esc | The shell: input, compute, graphics, Esc |
| If a disk is present: the AHCI lines; do **not** `format` without reading the device line | The boot screen |
| A fault screen, if any | Photograph; include `rip`, `cr2` and the vector |

### A report template

```text
WISP OS hardware report
Version: (output of `version`)
Machine: (make, model, year, CPU)
Firmware: (vendor, version; Secure Boot off? SATA mode?)
Display: (resolution if known, colours correct?)
Keyboard: (PS/2 or USB, port, worked?)
Boot screen: (attach photos, or transcribe the ACPI/APIC/AHCI/USB lines)
Shell: (help, ls, run hello, run bounce, run primes + Esc: each OK/fail)
Persistence: (disk present? mounted? format tested?)
Clock: (uptime at T and T+10s)
Faults: (screen photo with vector/rip/cr2)
Notes:
```

### Recording results back into the project

A short `HARDWARE.md` table (model, firmware, what worked, what did not, date, version) is the
cheapest possible way to turn anecdotes into a compatibility matrix. It is the first
deliverable of Horizon 0 ([Chapter 41](#41-horizon-0-trust-the-release)).


---

# Part VI: Roadmap of expansion

This roadmap says where WISP OS goes next, in what order, and why. It is built from
evidence in the repository, not from a wish list: every item points at a documented gap, a
finding from this manual's verification pass, or a limit the code states about itself.

## How to read the roadmap

### Ranking rules

Work is ranked by five questions, in this order. A lower-ranked item never jumps a
higher-ranked one without a written reason.

1. **Trust.** Does it make an existing claim true, or prove it? (Verification beats features:
   the project's value is that its claims hold.)
2. **Reach.** Does it unblock real hardware, where the biggest unproven risk lives?
3. **Autonomy.** Does it let the machine be used without another computer?
4. **Capability.** Does it let people write programs they could not write before?
5. **Delight.** Is it fun, and does it show off what the machine is?

### Sizes

Effort is relative, not calendar time. A one-person project's calendar depends on how often
real hardware is available, so sizes describe *scope*:

| Size | Meaning |
|---|---|
| **S** | One module, a handful of tests. A focused sitting or two. |
| **M** | Several modules, new test scenarios, docs. Roughly a week of focused work. |
| **L** | A new subsystem. Multi-week, with its own `CHANGES.md` chapter. |
| **XL** | Changes the architecture or needs new verification infrastructure. Months. |

### Definition of done (every item)

An item is done only when all of this is true ([Chapter 37](#37-how-changes-are-made-here)):
it is bounded and degrades instead of hanging; its failure paths are tested (mutation pass for
logic); zero warnings; QEMU or host test coverage appropriate to the layer; the docs, this
manual and `CHANGES.md` are updated; and the entry states **what was not verified**.

### Compatibility promises for 1.x

Growth must not break what people already wrote. Proposed promises, to be adopted at v1.2.0:

| Surface | Promise |
|---|---|
| **WISP source** | A program that runs on 1.1 runs the same on every 1.x. Behaviour is only ever *extended*. |
| **New builtins** | Today builtins cannot be shadowed, so *adding* a builtin named `date` would break any program that defines `fn date()`. **Policy: a builtin added after 1.1 is "soft"**: a user `fn` or `let` of the same name wins, so old programs keep working. Prefer prefixes for families (`key_*`, `file_*`, `time_*`). |
| **New keywords** | Not added in 1.x. New features use builtins (`pcall`, `import`) or syntax that was already an error. Candidate keywords stay un-reserved. |
| **Save format** | Version 2 stores stay readable by every later kernel. A new version upgrades in place on first save (as 1 to 2 did), with a documented rollback note. |
| **Disk safety** | Boot never writes. `format` always names the device and the range and requires `ERASE`. |
| **Shell** | Existing command names and meanings do not change. |
| **Boot layout** | `EFI/BOOT/BOOTX64.EFI` plus `APPS/*.wisp` keeps working. |
| **Reserved names** | Save keys beginning `sys.` are reserved for the system from 1.3. |

### The charter, restated

The roadmap does **not** contain networking, a hosted-OS compatibility layer, a windowing
system, multi-user accounts, or LLM-scale models. See [Chapter 49](#49-non-goals-and-decisions-to-make-out-loud).

---

## 41. Horizon 0: Trust the release

**Target release: v1.2.0.** Theme: turn "verified in QEMU at v1.1.0" into "verified for the tree
you are holding, on a machine you can name".

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H0-1** | Boot the current tree under QEMU and run all four suites; fix whatever breaks | Functions as values and closures were merged after v1.1.0 and the binary was never booted (`CHANGES.md`, "Not verified here"). | S |
| **H0-2** | Acceptance steps for `tasks`, `undo`, and a `wisp>` session using functions and closures | `tasks` and `undo` have no acceptance step; the acceptance suite stops at maps (step 24). | S |
| **H0-3** | First real-hardware reports, recorded in a `HARDWARE.md` matrix | Zero physical machines have run this (Ch. 38). Use the playbook ([Chapter 40](#40-hardware-bring-up-playbook)). | M, ongoing |
| **H0-4** | Reconcile the stale documentation sentences | Seven stale statements are listed in Ch. 38. | S |
| **H0-5** | Fix the three small defects: UTF-8 BOM, `line 0` for literal conditions, the stale `canvas.wisp` comment | Found while writing this manual (Ch. 38). | S |
| **H0-6** | Pin `rust-toolchain.toml` to a dated nightly; keep a CI canary on the latest nightly | The toolchain file says plain `nightly`, which floats; the reproducible-build claim holds only for a fixed toolchain. | S |
| **H0-7** | Continuous integration: host tests, kernel build, QEMU suites, artifact upload | There is no `.github` directory; everything runs by hand. The QEMU suites need OVMF assembly, `tesseract` and `Pillow`, which a container can fix. | M |
| **H0-8** | Executable documentation: a checker that runs this manual's code samples and diffs their output | This manual's samples were run by hand against the interpreter; that should be a test so it cannot rot. | S |
| **H0-9** | Fuzz the host-compilable parts: lexer, parser, interpreter, `save_format` decode, line editor | Threat model (Ch. 39): hostile scripts and disk images are the main attack surface, and everything involved compiles on a PC. | M |
| **H0-10** | Host runner `wisp-run` / `wisp-check`, built from the existing `tests/host` sources | Authoring on a PC currently needs a QEMU boot to run a script. The interpreter already compiles on the host. | S |
| **H0-11** | Cut v1.2.0: regenerate `SHA256SUMS`, release archives, adopt the compatibility promises | The repository-root `SHA256SUMS` still describes v1.0.0. | S |

### H0-1 and H0-2: boot what we built

**Do:** `./build.sh`; run the four suites with fresh output directories; read the screenshots
(do not trust OCR alone). Add acceptance steps for `tasks` and `undo` that feed the same scripted
sessions the host tests use and OCR-check the key lines (`Sorted by priority...`, `Undid #3: set b
= 3`). Add one `wisp>` step that defines a closure on one line and calls it on the next.
**Done when:** all suites pass on the tree that becomes v1.2.0 and the ledger (Ch. 38) loses its
"not booted" row.

### H0-3: the first real machines

**Do:** follow [Chapter 40](#40-hardware-bring-up-playbook) on at least three different machines
(ideally one Intel desktop, one AMD, one laptop). Start `HARDWARE.md` with a table:

| Machine | Firmware | Result | Keyboard | Disk | Notes | Version |
|---|---|---|---|---|---|---|

**Why this is first among equals:** every other roadmap item assumes the base works on metal. The
predicted failures (PCI bus-0-only enumeration, no xHCI handoff, huge reserved regions) are
*hypotheses*. The hardware reports decide how soon H4-1, H4-2 and H4-5 move up.
**Done when:** the matrix has entries and every failure has a linked issue.

### H0-5: the small defects

- **BOM:** strip a leading U+FEFF in the lexer *and* tolerate it in `fs::read_file_to_string`
  (the lexer fix is enough for the shell; the kernel reads the same bytes). Test: a BOM file
  runs; a BOM in the *middle* of a program is still an error.
- **`line 0`:** `eval_bool` falls back to `0` for literal expressions; pass the enclosing
  statement's line instead. Test: `while 5 { }` reports its own line.
- **Comment:** the `canvas.wisp` header predates string indexing.

### H0-7: continuous integration

A workflow with three jobs: (1) host tests (`cargo test --release`); (2) kernel build on the pinned
toolchain, failing on any warning, plus a non-blocking canary on latest nightly; (3) QEMU suites in a
container with `qemu-system-x86_64`, OVMF (assembled into the monolithic image), `tesseract` and
`Pillow`. Upload `BOOTX64.EFI` and its SHA-256 as an artifact. **Done when:** a pull request that breaks
a test or introduces a warning cannot merge.

### H0-8: executable documentation

Fence convention for new docs: a block tagged ` ```wisp run ` is executed; the next block tagged
` ```text expect ` is its exact expected output; ` ```wisp ` alone is illustration. A small Python
tool extracts blocks, runs them through `wisp-run` (H0-10) with scripted input, and diffs. Retrofit this
manual's samples. **Done when:** CI runs it and a deliberately wrong sample fails.

### H0-9: fuzzing

The pieces that take untrusted bytes (`.wisp` source, the save-disk image, keystrokes) all compile for
the host. Add `cargo-fuzz` targets: `lex_parse(&[u8])`, `run(&[u8])` with a step limit and a fake host,
`decode_value(&[u8])` for the save format, and `line_editor(&[u8])` against the simulated grid. Seed with
the bundled programs. **Done when:** each target runs a fixed budget in CI nightly and any crash becomes
a regression test.

---

## 42. Horizon 1: The machine gets a voice and a door

**Target release: v1.3.0.** Theme: make the machine *observable* and make it *possible to leave*.
Both are needed for real-hardware work and for being a pleasant daily machine.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H1-1** | Serial console, an in-memory log ring, and a `dmesg` command | Real hardware has no debug console; skipped programs are invisible; bring-up relies on photographs. | M |
| **H1-2** | `reboot`, `poweroff` and `halt` | There is no way to leave the machine except the power button; the kernel discards the UEFI runtime-services table at the exit. | M |
| **H1-3** | Wall-clock time: `date` command and `date()` builtin | The only clock is uptime. | S-M |
| **H1-4** | Shell comfort: tab completion, `cat`, `help <cmd>`, scrollback, `lspci`, `disks` | The shell is deliberately small but lacks basics people expect, and the diagnostics help hardware reports. | M |
| **H1-5** | Keymaps and modifiers: UK, DE, FR, ES, Dvorak, Colemak, AltGr, Caps Lock, key repeat | Tables are US-only, everything else is discarded (Ch. 13). | M |
| **H1-6** | `beep(hz, ms)` through the PC speaker | Sound at near-zero cost: PIT channel 2 and port `0x61`. | S |

### H1-1: serial, the log ring and `dmesg`

**Problem.** Boot and driver messages go to QEMU's `debugcon` (port `0xE9`), which does not exist on a
real machine. Bring-up reports depend on photographing a scrolling screen; programs skipped at
boot ([Chapter 25](#25-getting-programs-onto-the-machine)) leave no trace.

**Design.**
- A `log` module owns a fixed ring buffer (for example 32 KiB, allocated statically) and the one
  function every `debugcon::write_*` call is routed through.
- Sinks: `debugcon` (as now), a **16550 UART on COM1** (`0x3F8`, 115200 8N1) detected with the
  scratch-register loopback test and silently skipped if absent, and the ring.
- `dmesg` prints the ring; `dmesg | page` is not needed if scrollback (H1-4) lands.
- `fs::snapshot_apps` hands its skip reasons to the shell; `ls` appends `(2 skipped: see dmesg)`.
- No dynamic allocation in the log path (it must work in the fault handler); the fault screen also
  writes the same text to serial.

**Tests.** Unit tests for the ring (wraparound, truncation); QEMU `-serial file:serial.log` scenario
asserting boot lines appear there; acceptance step running `dmesg` and OCR-checking a known line.

**Risks.** A UART at `0x3F8` that is absent or a different device on some machines: loopback detection
before use; never block on transmit (bounded `poll_until`). **Done when:** a boot on QEMU produces the
same log on serial, in `dmesg` and in `debug.log`.

### H1-2: `reboot`, `poweroff`, `halt`

**Problem.** `main.rs` keeps `_runtime_st` from `exit_boot_services` but discards it. UEFI Runtime
Services (including `ResetSystem`) stay valid after the exit, which is the portable, AML-free way to
reset or power off.

**Design.**
- Carry the runtime-services pointer across the boundary with the other three values.
- Under our page tables everything outside the kernel image is **non-executable** (W^X), including the
  firmware's `RUNTIME_SERVICES_CODE` regions. `paging::init` must map those descriptors read+execute and
  `RUNTIME_SERVICES_DATA` read+write, with no `SetVirtualAddressMap` needed because the mapping is identity.
- `reboot` = `ResetSystem(Cold)`, `poweroff` = `ResetSystem(Shutdown)`, `halt` = disable interrupts and
  `hlt` in a loop with a message.
- Fallbacks if the pointer is missing or the call returns: `reboot` through the 8042 reset pulse
  (`0xFE` to port `0x64`), then a deliberate triple fault; `poweroff` says it could not and leaves the
  machine running (power-off without AML needs the `_S5` object, which is out of scope).
- Optional: after a fault screen, a configured "press any key to reboot".

**Tests.** QEMU with `-no-reboot` and `-no-shutdown` so the monitor can observe the reset/shutdown
request; a regression test that the Runtime mapping does not make any *data* region executable (walk the
tables as the firmware-memory diagnostic did). **Risks.** Firmware runtime services that assume
firmware-chosen virtual addresses or that touch the NVRAM variable store; verify on real hardware before
claiming it ([Chapter 40](#40-hardware-bring-up-playbook)). **Done when:** `reboot` restarts under QEMU and the
suite can use it between boots.

### H1-3: wall-clock time

**Design.** Read the real-time clock once, *before* exit, through UEFI `GetTime` (portable, handles BCD
and time zone fields), store the boot-time wall clock, and add the monotonic tick count thereafter. A
direct CMOS read (`0x70`/`0x71`, with the update-in-progress guard and BCD/binary handling) is the
fallback. Expose `date` in the shell and `date()` in WISP returning a map
`{"year":..., "month":..., "day":..., "hour":..., "minute":..., "second":...}`. There is no time-zone
database; an offset in minutes may be kept in the reserved key `sys.tz`. `date()` is a **soft** builtin
([compatibility promises](#compatibility-promises-for-1x)) because `date` is a likely user function name.
**Tests.** Host tests for date arithmetic (leap years, rollover) kept pure; QEMU `-rtc base=...`
scenario asserting the printed date. **Risks.** Clocks that are wrong or in local time: print what was
read and say so. **Size:** S for the builtin and command, M with the CMOS fallback.

### H1-4: shell comfort

- **Tab completion** over command names and, after `run`, app names. Needs a hook in the line editor's
  key handler and a completion callback on `Screen`-agnostic text; unit-testable in the host harness.
- **`cat <app>`**: print an APPS file's source (it is in RAM). Also the way to read a program on a
  machine with no other computer. Paged output.
- **`help <command>`** with usage and examples; `help` stays as the list.
- **Scrollback** (PageUp/PageDown): retain the last N text rows (for example 200) in a ring beside the
  framebuffer; redraw from it. Graphics drawn by WISP are not scrollback (documented).
- **`lspci`** (the bus-0 list already gathered at boot, extended after H4-1) and **`disks`** (AHCI port,
  model, serial, size, mounted?).
- **App descriptions in `ls`** from a first-line comment convention (`# name -- description`).
- **`sys.autorun`**: a save key naming a program to run at boot (reserved `sys.` namespace).

**Tests.** Editor unit tests for completion; acceptance steps for `cat`, scrollback (pixel-check that an
earlier line returns) and `lspci`. **Size M.**

### H1-5: keymaps and modifiers

**Problem.** Layout is US-only and Ctrl, Alt, Caps Lock and function keys are discarded. Anyone with a
non-US keyboard cannot type `;` `{` `}` `[` `]` reliably, and WISP needs them.

**Design.** A table-driven `keymap` module: each layout is a pair of tables for scancode set 1 and the HID
usage table, plus an AltGr layer. `keymap <name>` selects and persists in `sys.keymap`; the boot-stick
configuration file (H4-7) can preselect. Add modifier state (Ctrl, Alt, AltGr), Caps Lock with LED
update, and **software key repeat** for the USB path (the HID boot report carries no repeat). Decode
function keys and PageUp/PageDown into new key codes above `0x86`. Keep Esc as the stop key; do **not** map
Ctrl-C to it (the always-available stop is a safety property).
**Tests.** Tables are pure data: exhaustive host tests that every layout maps every printable key and
shifted variant; QEMU keyboard scenarios per layout via `sendkey`. **Risks.** A wrong table silently types
the wrong character: verify against the published layouts and keep a visible `keymap` status line.
**Size M.**

### H1-6: `beep(hz, ms)`

PIT channel 2 programmed to the square-wave mode, gated through port `0x61` bits 0-1. Blocks for `ms`
(polling Esc, restoring the gate on exit), clamps frequency to a safe range, and does nothing on machines
without a speaker. A soft builtin. **Tests.** QEMU can model the speaker with `-audiodev`; at minimum, assert
the gate bits toggle and restore. **Size S.** It also gives `hello.wisp` a reason to say hello out loud.

**Release exit criteria for v1.3.0:** boot on real hardware produces a readable log over serial and in
`dmesg`; `reboot`/`poweroff` verified on at least one real machine; non-US keymap demonstrated on
hardware.

---

## 43. Horizon 2: The language grows up

**Target release: v1.4.0.** Theme: the gaps programs actually trip over (the bundled demos show
them) without breaking any 1.1 program. Every item here is additive and respects the compatibility
promises: no new keywords, new builtins are soft.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H2-1** | Bitwise builtins | WISP has no bitwise operators; `rgb()` exists only because a script cannot pack a colour; saving flags and hashing need bits. | S |
| **H2-2** | String and number formatting | `calc`, `tasks`, `words` and the recipes all hand-roll padding and hex. | S-M |
| **H2-3** | `pcall` / `error`: catchable errors that never catch Esc | No exception handling: `calc`, `tasks` and `undo` each hand-roll number validation before `int()`, while `guess` and `tamer` do not, so a mistyped answer ends them with an error. | M |
| **H2-4** | Diagnostics: call stacks, source line display, column numbers | Errors give only a line number, and `line 0` bugs hide behind that. | M |
| **H2-5** | Modules: `import(name)` | One file is one program: no code reuse, so `is_number`, `pad_left`, `copy_map` are re-pasted everywhere. | L |
| **H2-6** | A standard library written in WISP | The recipes in Ch. 20 are the seed. | M |
| **H2-7** | Integer helpers: `divmod`, `pow`, `isqrt`, `muldiv`, variadic `min`/`max`, `sum` | Fixed-point models multiply and divide (`fixed_mul`) and can overflow; `isqrt` and `power` were hand-written. | S |
| **H2-8** | Small conveniences: `+=` family, `enumerate`, `last` | Each must prove boilerplate reduction in real programs before it is added. | S-M |
| **H2-9** | Grammar, conformance suite and a 2.0 stability statement | The language is now large enough to deserve a spec independent of the implementation. | M |

### H2-1: bitwise builtins

**Choice.** Operators (`& | ^ ~ << >>`) are cleaner but touch the lexer, the precedence table and the
parser, and a bad choice is permanent. Builtins are zero-syntax-risk. **Recommendation:** ship builtins
first: `band(a,b)`, `bor`, `bxor`, `bnot(a)`, `shl(a,n)`, `shr(a,n)` (arithmetic) and `lshr(a,n)`
(logical), plus `rgb_parts(c)` returning `[r,g,b]`. Revisit operators after real use.
**Semantics.** Two's-complement `i64`; shift counts outside `0..=63` are errors; no implicit overflow
(shifts that lose bits are defined, not errors, since that is what shifting means). All soft builtins.
**Tests.** Property tests against Rust's operators over a large random sample and the `i64` boundary
values; mutation pass (swap `shr`/`lshr`). **Size S.**

### H2-2: formatting

A minimal, bounded set rather than a format mini-language: `pad_left(s,w[,fill])`, `pad_right`,
`hex(n)`, `bin(n)`, `int(s, base)`, `fixed(n, scale)` (so `fixed(1205,100)` is `"12.05"`, replacing the
hand-written `money` in Ch. 20) and `fmt("{} scored {}", a, b)` with only `{}` placeholders, converting
each argument as `str` does and erroring on a count mismatch. Results obey the 256 KiB string cap. All
soft. **Tests.** Table-driven, including negative numbers and the `i64` extremes. **Size S-M.**

### H2-3: `pcall` and `error`

**Why it is hard.** The charter says Esc can never be caught. So the design must make the stop request
(and out-of-memory) **uncatchable by construction**.

**Design.** `pcall(f, a, b, ...)` calls `f` and returns a map: `{"ok": true, "value": v}` or
`{"ok": false, "error": "message", "line": n}`. `error(message)` raises a runtime error from script code.
Errors that are *stop* (`STOPPED_MESSAGE`) or *out of memory* propagate through `pcall` as errors of a
distinct kind the interpreter refuses to convert. On unwind, the nesting budget, scope frames and
registered objects must be restored exactly as on a normal return (the tests that today assert
"budget given back after 3,000 loop iterations" are the template).
**Tests.** `pcall` of every error class in [Chapter 23](#23-error-messages); `pcall` inside loops 10,000
times (leak and budget); **Esc inside a `pcall` still stops the program** (mutation: let `pcall` swallow
it and require a failure); cycles through error values. **Risks.** Subtle: `pcall` of a closure that
captured frames; handled by the same frame ownership rules as a normal call. **Size M.**

### H2-4: diagnostics

Today: `wisp: line N: message`. Add (without changing the first line's format, so existing tooling and
OCR tests keep matching) a second, dimmer line showing the source text and a caret, and a call chain such
as `  in tokenize (called from line 40)`. Column numbers require tokens to carry them. Fix `line 0` as part
of this. For modules (H2-5) errors must also name the file. **Tests.** Every error in Ch. 23 has its line,
column and chain asserted. **Size M.**

### H2-5: modules

**Design.** `import("name")` returns a value, conventionally a map of exported functions:

```wisp proposed
let strings = import("strings");
print(strings["pad_left"](7, 3, "0"));
```

- A module is a file `LIB/<name>.wisp` in the snapshot. The snapshot currently reads only files directly in
  `APPS`; extend it to read `LIB` under the same budgets, so libraries count toward the same 1 MiB.
- It runs **once** per session in its **own global scope**, as if its top level were a function body,
  and its final `return` is the export value. (Reuses the closure machinery: a module is a function
  evaluated once.) Because there is no `from x import y`, there is no name pollution.
- Import cycles are an error naming the cycle; nesting depth is capped; each module's functions keep the
  module's globals alive (closures already model this).
- Errors inside a module name the module file (needs H2-4).

**Tests.** Cycles, missing module, a module that errors, size budgets, memory returned when the session
ends, `wisp>` use. **Risks.** The "named `fn` is top-level only, sees only globals" rule gets a module
dimension; define it as "module globals". **Size L.**

### H2-6: a standard library

Ship `LIB/strings.wisp` (pad, centre, wrap, `join_fmt`), `LIB/math.wisp` (gcd, lcm, clamp, `isqrt`, `power`,
`abs_diff`), `LIB/collections.wisp` (`copy_map`, `copy_list`, `zip`, `enumerate`, `group_by`, `unique`,
`flatten`), `LIB/fixed.wisp` (`fixed_mul`, `fixed_div`, `to_text`) and `LIB/text.wisp` (word wrap for the
console width). Each function is host-tested against its documentation, and the bundled demos are migrated
to use them (proving the API). **Size M.**

### H2-7: integer helpers

`divmod(a,b)` returning `[q, r]`, `pow(base, exp)` with overflow error, `isqrt(n)`, and
`muldiv(a, b, c)` computing `a*b/c` in 128-bit with truncation toward zero and an error only if the *result*
overflows, which is exactly what fixed-point code wants and cannot express without risking overflow
(`fixed_mul` today errors when `a*b` overflows even if the answer would fit). Variadic `min`/`max`, and
`sum(list)`. All soft. **Tests.** Property tests against 128-bit reference arithmetic; the `tamer`
inference results unchanged when rewritten with `muldiv`. **Size S.**

### H2-8: conveniences, under a strict rule

Syntax is permanent, so each proposal must show, from the bundled programs, that it removes real
boilerplate: **compound assignment** (`+=`, `-=`, `*=`, `/=`, `%=`; today `list[0] += 1` is a parse error),
**`enumerate(list)`** returning `[[i, x], ...]`, **`last(list)`**. Explicitly *not* proposed: negative
indexing (changes a safety property: out-of-range today is always an error), a ternary operator (closures
and `if` cover it), operator overloading, classes. **Size S-M.**

### H2-9: grammar, conformance, and the 2.0 statement

Write the grammar in EBNF (Appendix B is the starting draft), then a **conformance suite**: a directory of
`.wisp` programs each with expected output and expected error, run by the host runner and, in CI, by QEMU
for a sample. The suite is the oracle for the bytecode VM (H7-1). Publish what "WISP 2.0" promises.
**Size M.**

**Release exit criteria for v1.4.0:** the bundled demos use the standard library and `pcall`; no 1.1
program changes behaviour (run all demos and the host suite from 1.1 unmodified as a regression gate).

---

## 44. Horizon 3: The machine stops needing another computer

**Target release: v1.5.0.** Theme: **autonomy**. Today a program is written on another computer and
copied to a read-only stick, a reboot at a time. This horizon makes the machine self-sufficient: it
can hold programs and files, edit them, run them, and share them back out.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H3-1** | Save format v3: multi-slot values | A value is capped at 4047 bytes: too small for a program, a model or a document. | L |
| **H3-2** | A partition-aware store | The store owns a *whole disk's* first 513 sectors; audit "Not addressed". | L |
| **H3-3** | Disk selection by identity, never "first disk" | Boot uses the first AHCI port; the audit's own multi-disk test is still unwritten. | M |
| **H3-4** | The program store and shell file commands | No way to create or keep a program on the machine. | M |
| **H3-5** | `edit`: an on-device text editor | Authoring needs another computer. | L |
| **H3-6** | Sandboxed file builtins | A script cannot read or write anything but `save`/`load`. | M |
| **H3-7** | Reserved `sys.*` settings | Several roadmap items need persistent settings. | S |

### H3-1: save format v3

**Goal.** Let one key hold up to the string cap (256 KiB) while keeping the guarantees: copy-on-write,
a single commit point, atomic per key, corruption detected, every old store still readable.

**Design sketch.**
- A value is stored as a **chain of data slots** plus a **head slot**. The head carries the key, total
  length, generation, the list of data-slot numbers (or a link to a chain) and the checksum over *all*
  of it. Data slots carry only bytes and a back-reference to the head's generation.
- **Commit rule:** write all data slots, flush, then write the head, flush. The head sector is the commit
  point exactly as the first sector of a slot is today. Free the old head and its data slots afterwards.
- **Mount** scans headers (one sector per slot, as now) and rebuilds the free map. A data slot with no
  referencing winner head is free (an interrupted save leaves orphans, harmless and reclaimed).
- **Capacity** becomes a superblock field: more slots than 64 (a store region sized to the disk, up to a
  cap), with the superblock version bumped to 3 and v2 stores upgraded in place on first save, exactly
  as v1 became v2.
- **Compatibility:** a v2-only kernel must refuse a v3 store untouched (it already does for unknown
  versions).

**Tests.** The existing exhaustive crash model (stop after every write and flush; every subset of
unflushed writes) generalised to chains; chain-length edge cases (1, 2, max); orphan reclamation; upgrade
from v2 with the store full; mutation (commit the head before flushing data) must fail. **Risks.** This is
the roadmap item with the most potential for data loss; hence the exhaustive model first, the kernel
second. **Done when:** the model proves old-or-new for every operation at every failure point, QEMU
scenarios cover v2-to-v3, and `docs/SAVE_FORMAT.md` is rewritten as v3.

### H3-2 and H3-3: partitions and disk identity

**Today.** `bring_up_save_disk` takes the first AHCI port with a disk; the store owns sectors 0-512 of the
whole disk and `format` erases any partition table there. That is safe because boot never writes and
`format` is explicit, but it cannot share a disk with anything.

**Design.**
- **Identity:** the superblock records a store UUID; `disks` lists every AHCI port (model, serial, size,
  *store present?*). Boot mounts the disk that holds a valid store; **never** picks one just because it is
  first. `format <port>` is explicit. Persist "this serial is my save disk" in the store itself.
- **Partitions:** a GPT reader (header, entries, CRC32) finds a partition by a WISP type GUID; the store's
  LBA translation already lives in one function (`save_format::region_lba`), which is the seam: make it
  relative to the partition start. Creating a partition table from the OS is high-risk; instead `format`
  *refuses* a disk with a partition table unless asked for the whole disk, and the **installer scripts**
  learn to create the save partition on the stick (a second partition on the same USB stick, usable once
  USB mass storage (H4-3) exists).

**Tests.** The audit's own proposed regression: a QEMU scenario with a foreign partitioned disk first and a
valid WISP disk second, then reversed, hashing every sector before and after boot, requiring every
unselected disk to stay byte-identical. GPT parser fuzzing (H0-9). **Size L total.**

### H3-4: the program store and file commands

Programs live in the store under keys `prog.<name>` (using v3). `run <name>` searches the APPS snapshot,
then the program store; `ls` shows both with their origin; new commands `rm`, `cp`, `mv`, `cat`, and
`export <name>` (prints the source to the screen and the serial console, base64 on request, so a program
can leave the machine without writing the boot stick). With H1-1 and a serial cable, `export` is the
copy-out path; with H4-3 it can write a second USB stick. **Tests.** Round-trip a 100 KiB program; name
collisions between APPS and the store; power-loss during `cp`. **Size M.**

### H3-5: `edit`

**Goal.** Edit a program on the machine, run it, see the error, jump to the failing line, fix it.

**Design.**
- A **full-screen editor** over the console: needs `Console` cursor addressing (`set_cursor(col,row)`,
  `rows()`, `cols()`), which also enables TUI programs (H5-2).
- The core is a text buffer (a vector of lines or a gap buffer) with an operations API and a `Screen`
  trait, **exactly the structure of `line_editor.rs`**, so it is host-testable against a simulated grid,
  keystroke by keystroke.
- Keys: arrows, Home/End, PageUp/PageDown, Backspace/Delete, Enter, Tab (insert spaces), and commands on
  Esc-prefixed or function keys (these need H1-5's decoding): save, run, quit, search, go to line.
- **Syntax highlighting** from the real lexer's tokens (keywords, strings, comments, numbers).
- **Run from the editor** (`F5`): runs the buffer in a fresh session, and on a runtime error reopens the
  editor at the error's line (needs H2-4's line/column).
- Bounds: the 256 KiB file cap, 32,768 lines, a line length cap, all with clean errors.

**Tests.** Host tests for every editing operation and for the screen after each key; an acceptance step
that types a program with the editor, runs it, and OCR-checks the output. **Size L.**

### H3-6: sandboxed file builtins

`read_file(name)`, `write_file(name, text)`, `append_file(name, text)`, `list_files()`, `delete_file(name)`
operate on the program store's `file.<name>` namespace only: never raw sectors, never the boot stick. Names
are bounded, sizes bounded, errors typed. This is what lets a WISP program keep a log, a document or a
model's weights as text instead of pasting them into source (the "no `load weights.bin`" gap in
[Chapter 22](#22-running-trained-models)). All soft builtins. **Tests.** Limits, collisions with
`save` keys (separate prefixes), power loss. **Size M.**

### H3-7: reserved `sys.*`

Reserve the `sys.` key prefix for the system: `sys.keymap`, `sys.tz`, `sys.autorun`, `sys.theme`.
`save("sys...", v)` from a script is allowed but documented as affecting the machine; `keys` marks them.
**Size S.**

**Release exit criteria for v1.5.0:** a person with *only* the machine and a keyboard can write, save,
edit, run and delete a program, power-cycle, and find it still there; the crash-atomicity model covers v3.

---

## 45. Horizon 4: Hardware breadth

**Target release: v1.6.0.** Theme: **reach**. The predicted real-hardware failures are all here. The
order inside this horizon is set by H0-3's results; the order below is the order the code suggests.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H4-1** | Full PCI enumeration: bridges, ECAM/MCFG, multiple controllers | Enumeration is **bus 0 only**; a controller behind a bridge is invisible (Ch. 30). | M |
| **H4-2** | xHCI hardening: BIOS handoff, hubs, hot-plug, multiple devices, MSI | No handoff code exists; no hubs; no hot-plug (Ch. 29). | L |
| **H4-3** | USB mass storage (Bulk-Only Transport) | Persistence needs AHCI; many machines have only NVMe or a stick. | L |
| **H4-4** | NVMe driver and a `BlockDevice` trait | NVMe is unsupported; the save store is welded to the AHCI type. | L-XL |
| **H4-5** | Large-page mappings for non-RAM regions | 4 KiB-only paging used ~6,300 frames for one 12 GiB OVMF region. | M |
| **H4-6** | HID report-descriptor parser; mouse and non-boot devices | Only boot keyboards are understood. | L |
| **H4-7** | Boot configuration file and display modes | Nothing can be configured before boot; the stick is read-only but readable. | M |
| **H4-8** | Secure Boot: signing script and enrolment guide | The binary is unsigned (Ch. 9). | S-M |
| **H4-9** | ACPI without full AML: FADT reset, optional minimal AML | Power control and PCI routing are out of reach without AML. | M / XL |
| **H4-10** | Intel HDA audio | `beep` is the only sound. | XL |

### H4-1: full PCI enumeration

**Design.** Keep the 0xCF8/0xCFC mechanism as the fallback; add **ECAM** via the ACPI `MCFG` table
(extending `acpi.rs`, which parses only the MADT). Walk **PCI-to-PCI bridges** recursively: read the
secondary and subordinate bus numbers; if firmware left a bridge unconfigured, assign numbers. Return all
matching controllers, not the first. Log a bus/device/function tree to the log ring. `lspci` (H1-4) prints
it.
**Tests.** QEMU with `-device pci-bridge` / `pcie-root-port` placing an xHCI and an AHCI controller behind
bridges (the exact failure case), plus a no-bridge regression. **Risks.** Bridges with unassigned
resources; scope: do not reprogram BARs unless a controller has none (record and report).
**Done when:** a QEMU machine with both controllers behind a bridge boots with USB and persistence.

### H4-2: xHCI hardening

In order of risk reduction:
1. **BIOS-to-OS ownership handoff** through the USB Legacy Support extended capability (set the OS-owned
   semaphore, wait, with a bounded timeout): small, and the most likely cause of "USB dead on real
   hardware".
2. **Multiple devices and ports**, then **hot-plug** by handling Port Status Change events (enumerate on
   connect; tear down the slot on disconnect), so a keyboard plugged in after boot appears.
3. **Hubs:** hub-class enumeration (descriptor, port power, reset, per-port status), route strings, and
   transaction translators for low/full-speed devices behind a high-speed hub.
4. **MSI/MSI-X** for the event ring, retiring the 10 ms polling latency (optional; the poll is fine for
   keyboards).

**Tests.** QEMU `usb-hub` with `usb-kbd` behind it, `device_add`/`device_del` through the monitor, and a
descriptor-fuzz target (H0-9) because this code trusts a device. **Size L.**

### H4-3: USB mass storage

Bulk endpoints and the Bulk-Only Transport protocol with SCSI `READ(10)`, `WRITE(10)`,
`SYNCHRONIZE CACHE`, `TEST UNIT READY`, `READ CAPACITY(10)`. Prerequisite: the xHCI driver drives only an
interrupt endpoint today; add bulk-IN/OUT rings. With `BlockDevice` (H4-4) the store and, later, a FAT
reader sit on top. This gives persistence on SATA-less machines and a **write path off the machine**
without ever writing the *boot* stick (use a second stick). **Size L.**

### H4-4: NVMe and the `BlockDevice` seam

**Refactor first.** `save_format` is written against the concrete AHCI type; the host tests work around that
by substituting an in-memory `ahci` module. Introduce `trait BlockDevice { sector_size, total_sectors,
read, write, flush }`; implement it for AHCI, for the host test double (replacing the substitution
trick) and later NVMe and USB. Doing this *before* NVMe is a pure refactor with the existing tests as the
safety net.

**NVMe driver.** PCI class `01:08:02`; 64-bit BAR0; controller reset and enable; one admin queue pair
(Identify controller and namespace); one I/O queue pair; polled completions (no MSI initially); PRP1/PRP2 for
transfers up to a few pages; Read, Write and Flush; the `clock::poll_until` bound on every wait and a failed
state on timeout mirroring AHCI's recovery rule. QEMU `-device nvme,serial=...,drive=...`. The same
all-failures-are-visible rule applies. **Why it matters:** most laptops sold in the last several years have
only NVMe, so without this the machine can never persist data there. **Size L-XL.**

### H4-5: large pages for non-RAM regions

Keep 4 KiB leaves for the **kernel image** (W^X needs page granularity) and for RAM that the kernel hands
out, and use 2 MiB (and 1 GiB where aligned) leaves for large MMIO and reserved regions, which need only
non-executable, uncached-or-device permissions. Measure frames used before and after on the 12 GiB OVMF
region (baseline: ~6,300 frames, about 24.6 MiB). Pay attention to the interaction with
`pmm::init`'s live-hierarchy pinning logic (it already handles 2 MiB and 1 GiB leaves when *walking*).
**Tests.** Host-side tests of the mapper on synthetic memory maps (the mapper is pure arithmetic over a
frame source); the existing fault tests must still pass unmodified. **Size M.**

### H4-6: HID descriptors, mouse and more

A report-descriptor parser (the subset that matters: usage pages Generic Desktop and Keyboard/Keypad and
Button; input items; report IDs; logical min/max), which generalises the boot-keyboard special case and
unlocks mice, trackpads in HID mode, gamepads and non-boot keyboards. PS/2 mouse (aux port) in parallel.
Expose to WISP through H5-6. Fuzz the parser (H0-9). **Size L.**

### H4-7: boot configuration and display modes

A plain text file read **before the exit**, from the stick (`WISP.CFG` or `APPS/boot.wispcfg`), keeping the
"boot stick is read-only" rule: keymap, preferred GOP mode, console scale, autorun program, log level. GOP
mode selection (choose from `QueryMode` before exit; fall back to the firmware's choice) and an integer
**font scale** (2x for 4K panels). Parsing is trivial and bounded; errors go to the log ring.
**Tests.** QEMU `-vga std` with different resolutions; a malformed config file leaves defaults and logs.
**Size M.**

### H4-8: Secure Boot

A `scripts/sign_efi.sh` using `sbsign` (or `pesign`) with a user-generated key; a guide to enrol the key
in the firmware `db` or through MOK, and to test with OVMF's Secure Boot variables in QEMU. Document, do
not manage, enrolment (the charter keeps key management out of the OS). A *signed release* is H7-6.
**Size S-M.**

### H4-9: ACPI power and routing

First the cheap, AML-free part: the FADT's reset register and `SLP_TYP` values for S5 cannot be read
without AML, so H1-2's runtime-services path stays primary; add the FADT reset register as a `reboot`
fallback. Only if H0-3 shows machines that need `_PRT` routing or `_S5`, build a **minimal AML interpreter**
that evaluates just those objects (namespace walk, `Package`/`Method` basics): XL, with its own
verification effort. Until the data demands it, this stays off the critical path.

### H4-10: audio

An Intel HDA codec driver (CORB/RIRB, stream descriptors, buffer descriptor list) playing PCM from WISP
(`tone(hz, ms)`, `play(samples)` with a bounded buffer). Large, hardware-sensitive; only after `beep` has
shown that people want sound. QEMU's `intel-hda` + `hda-duplex` model makes it testable. **Size XL.**

**Release exit criteria for v1.6.0:** the HARDWARE.md matrix shows persistence and keyboard input
working on machines with NVMe-only storage and on machines with the xHCI behind a hub or bridge; QEMU
scenarios reproduce each fixed failure.

---

## 46. Horizon 5: Graphics, sound and interaction

**Target release: v1.7.0.** Theme: **delight**, and the missing primitives that real-time programs need.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H5-1** | Non-blocking keyboard: `key()`, `key_wait(ms)` | `input()` blocks until Enter, so no real-time game is possible (Ch. 21). | S-M |
| **H5-2** | Text placement and the screen grid: `at`, `cols`, `rows`, `color`, `text` | `print` writes at the cursor only; there is no pixel text. | S-M |
| **H5-3** | Offscreen buffers, `get_pixel`, `blit`, `present` | No readback, no double buffering: animation flickers (Ch. 21). | M-L |
| **H5-4** | More primitives: circle, ellipse, triangle, polyline, outline, clip rect | Seven primitives are enough for a demo and not for a game. | S-M |
| **H5-5** | Sprites and tile maps | Games spend their time here. | M |
| **H5-6** | Mouse and cursor | Needs H4-6. | M |
| **H5-7** | Sound: `tone`, `play` | Needs H1-6, then H4-10. | M |
| **H5-8** | Timers: `every`, `after` | Simplifies animation; needs care. | M |
| **H5-9** | A demonstration suite | Programs prove the primitives. | M |

### H5-1: non-blocking input

`key()` returns the next pending key as a string (`"a"`, `"enter"`, `"up"`, `"down"`, `"left"`, `"right"`,
`"home"`, `"end"`, `"delete"`, `"backspace"`, `"tab"`, `"f1"`...) or `nil` if none; `key_wait(ms)` waits up to
`ms` (polling Esc) and returns a key or `nil`. **Esc is never delivered**: the stop flag always wins, so
no program can trap the user. Needs the key queue to carry the new key codes (H1-5). **Tests.** QEMU
typing during a running program; verify that Esc still stops a `while true { key(); }` loop. **Size S-M.**

### H5-2: the screen grid and text

`cols()`, `rows()`, `at(col, row)` (move the text cursor), `color(r, g, b)` (set text foreground), and
`text(x, y, s, c)` drawing a string at a *pixel* position with the font (clipped, ASCII). Together with
console cursor addressing (also needed by H3-5) these make TUI programs possible. A `font_scale` setting
from H4-7. **Tests.** Pixel assertions of glyph rows; clipping at every edge. **Size S-M.**

### H5-3: buffers, readback and `present`

- `get_pixel(x, y)` returns the colour (or an error off-screen).
- `canvas(w, h)` makes an offscreen buffer handle; `draw_to(buf)` redirects primitives; `blit(buf, x, y)`
  copies it; `present()` copies the back buffer to the screen in one pass: **flicker-free animation**.
- Buffers come from the **PMM frame pool**, not the 8 MiB heap, with a hard total cap and an
  `out of memory` error that names it. Each handle is freed when the session ends (the object registry
  already exists to do exactly this).

**Tests.** Pixel checks that `present` is atomic from the viewer's side (no partial frames in a
screenshot); leak tests; a 1024x768 buffer within budget. **Size M-L.**

### H5-4 to H5-8 in brief

- **Primitives:** `circle`, `fill_circle`, `ellipse`, `triangle`, `polyline`, `rect_outline`, `clip(x,y,w,h)`;
  all integer-only (midpoint algorithms), all with the same position-clips-but-nonsense-errors rule.
- **Sprites and tile maps:** a sprite is a list of rows plus a palette; `blit_sprite`, with transparency;
  tile maps as a list of lists drawn by one native call (a pure-WISP loop of `rect` calls is slow).
- **Mouse:** `mouse()` returns `{"x":, "y":, "buttons":}`, plus a software cursor drawn over the
  framebuffer and restored from `get_pixel` backups.
- **Sound:** `tone(hz, ms)` first on the PC speaker, then on HDA; `play(list_of_samples)` bounded.
- **Timers:** `every(ms, f)` and `after(ms, f)` run callbacks at the interpreter's safe points (loop
  iterations, `sleep_ms`). The hazard is re-entrancy (a callback firing while another runs) so callbacks
  are queued and run only between statements; evaluate after the bytecode VM (H7-1), which makes safe
  points cheap and explicit.

### H5-9: the demonstration suite

Each demo is a bundled program with an acceptance step (pixel-checked where it draws): **snake**,
**pong**, a **tetris**-style falling blocks game, **game of life**, **mandelbrot** (integer
fixed-point), a **plasma** effect, a **clock** (needs `date()`), **paint** (mouse), and a text-mode
**file manager** over the program store. Demos are the roadmap's proof: any primitive that no demo needs
should be questioned.

---

## 47. Horizon 6: Verified intelligence

**Target: rolling, starting after v1.4.0.** Theme: keep the project's most distinctive claim, *real,
verified, tiny neural networks on bare metal*, growing in an honest direction.

| ID | Item | Why (evidence) | Size |
|---|---|---|---|
| **H6-1** | Native integer vector kernels: `dot`, `matvec`, `fixed_dense` | A forward pass is dominated by `fixed_mul` calls in a tree-walker (Ch. 22). | M |
| **H6-2** | An activation library: lookup-table sigmoid, tanh, softmax | The AI doc names "a lookup-table activation" as the next step. | M |
| **H6-3** | A model packer and verification harness | `tools/train_tiny_nn.py` is two hard-coded models. | M |
| **H6-4** | Flagship demo: an 8x8 handwritten-digit recogniser you draw on screen | The best single demonstration of bare-metal ML. | M |
| **H6-5** | More tiny models: k-NN, decision trees, naive Bayes, a character n-gram generator | Shows range at an honest scale. | M |
| **H6-6** | A written ceiling on model size, and a research note on a toy transformer | Keeps the "not an LLM" promise precise. | S |

### H6-1: native kernels with identical semantics

`dot(a, b)`, `matvec(W, x)`, `vadd`, `vmax`, and `fixed_dense(W, B, x, scale)` implemented natively but
with **exactly WISP's integer semantics**: `i64` with overflow as an error, and division that *truncates
toward zero* (the very trap `WISP_AI.md` documents). Bounded sizes, polling Esc every row. **The trust
argument is the test:** a property test generates random weights and inputs and requires the native result
to equal the pure-WISP reference implementation (`fixed_mul` loops) bit for bit, including negative
operands and rounding. The speed-up is then free of any accuracy question. **Size M.**

### H6-2: activations

Generate (with a tool, not by hand) lookup tables for sigmoid, tanh and the exponential used by softmax,
over a documented input range with linear interpolation, at fixed scales, each with a *published maximum
error* checked by a test against high-precision reference values. Ship them as `LIB/nn.wisp` (H2-6) or
natively. Softmax subtracts the max for stability and normalises with `muldiv` (H2-7). **Size M.**

### H6-3: packing and verification

Extend `tools/` with `pack_model.py`: from numpy weights (an MLP of any depth) to a quantized `.wisp` file
or a store blob (H3-6 `read_file`), plus an **automatic verification report**: float accuracy,
fixed-point accuracy using WISP's truncating division (`_trunc_div`), the worst-case activation magnitude
(so overflow headroom is proven, not hoped for), and generated boundary test cases that flip if a weight is
off by one quantization step. The report is checked into the repo beside the model. **Size M.**

### H6-4: the digit recogniser

An 8x8 grid you draw on with the arrow keys (H5-1) or the mouse (H5-6); a **64-16-10 MLP** (about 1,200
weights, trained offline on the public 8x8 digits dataset) classifies it and shows ten probability bars.
It needs H5-1 and H5-2, benefits enormously from H6-1, and its weights fit comfortably within the 256 KiB
source cap. **Acceptance:** feed canonical patterns for each digit and assert the classification; a pixel
check that the bar for the winner is the longest. Ship the training script and the verification report.
**Size M.**

### H6-5 and H6-6

Small models that need no neural net at all, shown honestly: **k-nearest-neighbours** on a built-in
dataset, a **decision tree** exported from a trainer as nested maps, **naive Bayes** for a toy
classifier, and a **character n-gram generator** (a Markov model over a text embedded in the program):
"language model flavoured", labelled as such. H6-6 writes down the ceiling: the 256 KiB source cap, the
interpreter's speed and the heap bound the practical model at tens of thousands of weights after H6-1; a
*toy* character-level transformer is a research note, attempted only after H6-1 and the bytecode VM
(H7-1) make a forward pass take seconds, and clearly named a toy. **WISP will not train, and will not
host a modern LLM.**

---

## 48. Horizon 7: The long horizon

**Target: v2.0 and beyond.** Theme: the large changes that need the earlier horizons first. None of these
starts without a measured need.

| ID | Item | Precondition | Size |
|---|---|---|---|
| **H7-1** | A bytecode VM | H2-9 conformance suite as the oracle; a benchmark showing the need | XL |
| **H7-2** | Floating point, done properly | FPU/SSE fault infrastructure and tests; a decision on semantics | XL |
| **H7-3** | SMP | A concrete workload that wants a second core | XL |
| **H7-4** | Cooperative multitasking | H7-1 | XL |
| **H7-5** | An architecture seam and an aarch64 port | The `BlockDevice` and `Host` seams (H4-4) | XL |
| **H7-6** | Signed releases and supply-chain hygiene | H4-8 | M |
| **H7-7** | Documentation as a product | Ongoing | M |

### H7-1: a bytecode VM

**Why not now.** The tree-walker is simple, has been audited and its native stack use was measured to the
byte. A VM is a rewrite of the heart of the project. **When.** If benchmarks (add them: `primes.wisp`,
`words`-style map workloads, a 1,000-weight inference) show that interpretation speed limits real programs
after H6-1's native kernels.

**Shape.** Compile the AST to compact bytecode with **resolved variable slots** (removing the per-access
frame search), run on an **explicit heap-allocated stack** (which *retires* the native-stack budget problem
entirely: depth becomes a counter, not a kernel-stack risk), keep Esc polling at back-edges and calls, and
keep the **tree-walker as the reference implementation**. **Differential testing:** run every conformance
program, every bundled demo and the fuzz corpus on both and require identical output and identical error
messages. Target: a measured 5-10x on `primes.wisp`. **Size XL.**

### H7-2: floating point

**Precondition list (all must be true before a single float exists).** Initialise the FPU/SSE state
(`CR0.MP/EM/TS`, `CR4.OSFXSR`, `CR4.OSXMMEXCPT`, `MXCSR`), install and **test** handlers for `#NM`, `#XM`
and `#MF` with new fault hooks that provoke each, confirm no interrupt handler or stub touches SSE state
without saving it (the kernel is single-threaded, but ISRs interrupt mainline code), and decide the
semantics: IEEE-754 `f64` with defined NaN, infinity and `-0` behaviour, **deterministic** results (no
libm: `sqrt` is correctly rounded in hardware; `sin`/`cos`/`exp` implemented in software with documented
error), and a shortest-round-trip **printing** algorithm (a Ryu-style routine is real work).

**Alternative to weigh first.** A decimal **fixed-point type** or the `fixed`/`muldiv` library (H2-6, H2-7)
might cover plotting and physics without any of the above. Decide from real programs that cannot be
written with fixed-point. **Size XL.**

### H7-3: SMP

INIT-SIPI-SIPI start-up from the MADT's processor list, per-CPU stacks, GDT/TSS and IDT, spinlocks in
place of the interrupt-disable `sync.rs`, AP idle with `hlt`, TLB shootdown for any future page-table
change. **There is no workload for it today.** One speculative use: running a program on an application
processor while the boot processor serves input and the screen, so Esc and a clock stay responsive. Do
this only for a measured reason; a second core multiplies the verification surface of every driver.

### H7-4: cooperative multitasking

`spawn(f)`, `yield()`, channels as maps, scheduled at safe points on the bytecode VM, no preemption: split
panes, a background clock, a logger. Isolation stays by language semantics, not hardware. Evaluate against
simply running one program at a time, which the charter's simplicity favours.

### H7-5: portability

Introduce a `hal` seam: port and MMIO access, interrupt controller, timer, framebuffer, block device,
input. The `Host` and `BlockDevice` traits are the first two seams. Then an **aarch64 UEFI** target
(`aarch64-unknown-uefi`) with GICv3, the generic timer, a PL011 UART, PCIe ECAM (already generic after
H4-1) and the same xHCI/AHCI/NVMe drivers. It brings the machine to ARM servers, UEFI Raspberry Pi
images and Apple-silicon VMs. The host test suite must pass **unchanged**, which is why the language and
save format stay hardware-independent.

### H7-6: signed releases

Sign `BOOTX64.EFI` with a project key, publish the fingerprint and detached signatures for release
archives, record build provenance, and audit the one dependency (`uefi 0.30.0`) with `cargo vet` or an
equivalent. Pair with H4-8. **Size M.**

### H7-7: documentation as a product

This manual kept current by the executable-docs checker (H0-8); on-device `help <topic>` pages generated
from the same sources; a getting-started path that ends in H3-5's editor. **Rolling.**

---

## 49. Non-goals and decisions to make out loud

A roadmap that cannot say no ends up with a different project. These are *not* planned. The right column
says what would have to be true to reconsider, and the process: **a written decision in `CHANGES.md`
and an update to the README's foundation statement, before any code.**

| Non-goal | Reason | What would reconsider it |
|---|---|---|
| **Networking** (any stack, USB or PCI NICs, Wi-Fi) | The founding constraint; there is *no network code in the binary* today, which is a stronger guarantee than "disabled". | A deliberate project decision to change what the machine is, made out loud, with an offline-by-default design. Not a side effect of any driver. |
| **A POSIX or Linux layer; running native executables** | Defeats "nothing borrowed". | Never planned. |
| **Dynamic loading, plug-in code, a JIT** | Adds a code-execution surface the W^X design avoids. | A VM-only sandbox with the same bounds as WISP. |
| **A windowing system or GUI toolkit** | Out of scope for a single-program machine. | Text-mode TUIs and framebuffer apps (H5) cover the need. |
| **Multi-user accounts, passwords, permissions** | One machine, one operator, physical access is the trust boundary. | A concrete multi-operator use. |
| **LLM-scale inference** | Needs SIMD kernels and usually GPUs, none of which exist pre-OS. | Hardware-independent breakthroughs, not engineering effort here. |
| **Training on the device** | WISP is an inference host (`WISP_AI.md`). Tiny online learners are fine as demos. | Never planned. |
| **Writing the boot stick, or writable FAT32** | The vvfat corruption postmortem; no atomicity in FAT. | A tested, crash-atomic design (H3's store plus H4-3 on a *separate* device is the supported path). |
| **A package manager or online repository** | Needs a network. | Offline bundles and the `LIB/` directory (H2-5) cover sharing. |
| **Catchable Esc** | The always-available stop is a safety property. | Never. |
| **Floats before their verification** | Charter principle 7. | H7-2's preconditions. |

---

## 50. The release train, dependencies and risks

### Releases

| Release | Theme | Items | Exit criteria |
|---|---|---|---|
| **v1.2.0** | Trust | H0-1 to H0-11 | All suites green on the released tree; first real-machine reports in `HARDWARE.md`; CI blocks regressions; compatibility promises adopted |
| **v1.3.0** | Voice and door | H1-1 to H1-6 | Readable log over serial and `dmesg`; `reboot`/`poweroff` on real hardware; non-US keymap on hardware |
| **v1.4.0** | Language | H2-1 to H2-9 | No 1.1 program changes behaviour; demos use the stdlib and `pcall`; conformance suite in CI |
| **v1.5.0** | Autonomy | H3-1 to H3-7 | Write, save, edit, run, delete a program with only the machine; crash model covers v3 |
| **v1.6.0** | Reach | H4-1 to H4-8 (+H4-9/10 as justified) | NVMe-only and bridged-controller machines work; every fixed failure reproduced in QEMU |
| **v1.7.0** | Delight | H5-1 to H5-9 | A real-time game and the digit recogniser run on hardware |
| **v2.0.0** | Maturity | H7-1 (if needed), H2-9's 2.0 statement, H7-6 | Language specification frozen; bytecode VM (if built) proven identical by differential testing |

H6 items ride along from v1.4.0 whenever their dependencies land. Releases are cut when exit criteria are
met, not on a date.

### Dependency map

```text
H0-10 wisp-run ──► H0-8 executable docs
H0-9 fuzzing ────► (guards) H2-5, H3-2, H4-2, H4-6
H0-3 hardware reports ──► orders all of Horizon 4

H1-1 serial/dmesg ──► H0-3 (better reports), H3-4 export, H4-*
H1-2 reboot ───────► test suites between boots
H1-5 keymaps/modifiers ──► H3-5 editor, H5-1 key()
H1-4 console addressing ─► H3-5 editor, H5-2 text

H2-4 diagnostics ──► H2-5 modules ──► H2-6 stdlib
H2-7 muldiv ───────► H6-1, H6-2
H2-9 conformance ──► H7-1 bytecode VM ──► H7-4, H5-8

H4-4 BlockDevice ──► H4-3 USB storage, H3-2 partitions, H7-5
H3-1 save v3 ──► H3-4 program store ──► H3-5 editor, H3-6 files ──► H6-3
H4-1 PCI ──► H4-2 xHCI, H4-4 NVMe
H4-6 HID ──► H5-6 mouse ──► H6-4 digit recogniser (optional)
H5-1, H5-2, H6-1 ──► H6-4
```

### The critical path to self-hosting

`H1-1 → H1-5 → H4-1 → H4-4 → H3-1 → H3-4 → H3-5`. Everything else can proceed in parallel; the editor is
the milestone at which the project can claim *"a computer you can use without another computer"*.

### Suggested order of work, first ten

1. **H0-1** boot what exists. 2. **H0-2** acceptance for `tasks`/`undo`. 3. **H0-5** the three defects.
4. **H0-10** `wisp-run`. 5. **H0-3** real hardware, starting now and continuing. 6. **H0-7** CI.
7. **H1-1** serial and `dmesg`. 8. **H0-9** fuzzing. 9. **H1-2** `reboot`/`poweroff`. 10. **H4-1** PCI
enumeration, moved up if H0-3 reports a missing controller.

### Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Real hardware exposes failures QEMU hides (bus-0-only PCI, xHCI handoff, huge reserved regions, odd firmware) | High | High | H0-3 first; H1-1 for visibility; H4-1, H4-2, H4-5 targeted at the predicted failures |
| R2 | Data loss from a storage-format change (H3-1) | Medium | Severe | Build the exhaustive crash model before the kernel; v2 stays readable; mutation tests |
| R3 | Scope creep crosses the charter (networking via a "harmless" USB driver, floats "just for sqrt") | Medium | High | Non-goals table; any crossing needs a written decision first |
| R4 | Nightly toolchain drift breaks the build | Medium | Medium | H0-6 pin plus a canary job |
| R5 | One maintainer cannot hand-verify every layer | High | Medium | CI (H0-7), executable docs (H0-8), fuzzing (H0-9) |
| R6 | Interpreter speed limits ambitious programs | Medium | Medium | H6-1 native kernels first; H7-1 only on measured need, guarded by differential testing |
| R7 | Adding builtins breaks old programs that defined the same name | Medium | Medium | Soft-builtin policy; prefix families |
| R8 | USB complexity (hubs, hot-plug, mass storage) introduces kernel bugs reachable by a hostile device | Medium | High | Descriptor fuzzing, bounded everything, tests with `usb-hub`, treat as untrusted input |
| R9 | Documentation drift | High | Medium | Executable docs; documentation updates are part of the definition of done |
| R10 | The unsigned binary blocks adoption on Secure Boot machines | High | Medium | H4-8 now, H7-6 later |
| R11 | `format` run on the wrong disk | Low | Severe | Name the device and range, require `ERASE`, H3-3 identity, and the playbook's "disconnect disks you care about" |

### What success looks like

Three milestones, each checkable:

- **Credible:** a stranger downloads a release, boots it on a machine they own, and the result is recorded in
  the hardware matrix. (v1.2.0.)
- **Self-sufficient:** the same stranger writes, saves, edits and runs a program with nothing but the
  machine. (v1.5.0.)
- **Remarkable:** a hand-drawn digit is recognised by a network whose every weight was verified in the exact
  arithmetic the machine runs, on bare metal, with no OS and no network. (v1.7.0.)

---

# Appendices

## Appendix A: Cheat sheet

### Shell

```text
help  clear  echo <text>  ls [APPS]  run <name>  wisp
uptime  mem  version
save <k> <v>  load <k>  keys  delete <k>  format
```

### Keys

| Key | Action |
|---|---|
| Left Right Home End Delete Backspace | Edit the line |
| Up Down | History (at `>` and `wisp>` only) |
| Enter | Submit |
| **Esc** | Stop a running program; erase the line at a prompt |

### WISP in a screenful

```text
let x = 1;  x = x + 1;                    # declare, assign
if a { } elif b { } else { }               # conditions are bools
while c { }  for v in list_or_map_or_string { }  break;  continue;
fn name(a, b) { return a + b; }            # top level only
let f = fn (a) { return a * 2; };          # closure
let l = [1, 2];  l[0];  l[1:];  push(l, 3);
let m = {"k": 1};  m["k"];  get(m, "z", 0);  contains(m, "k");
"text" + str(5);  len(s);  split(s);  join(l, ",");
input("? ");  print(a, b);  int("5");  rand(1, 6);
save("k", v);  load("k");                   # nil if missing
sleep_ms(100);  time_ms();
rgb(r, g, b);  plot  rect  line  fill_screen  clear
map(f, l)  filter(f, l)  reduce(f, l, 0)  sort_by(l, f)
```

### Limits you will actually hit

`256` nested calls · `128` syntax depth · `32,768` list/map entries · `256 KiB` string · `16 KiB` print/str ·
`32` byte key · `63` keys · `4047` byte value · `1 MiB`/`256 KiB`/`64` APPS budgets.

## Appendix B: A grammar for WISP

An informal EBNF, consistent with the parser's observed behaviour (every rule below was checked against the
errors in [Chapter 23](#23-error-messages) and the traps in [Chapter 18](#18-the-language-on-one-page)).
H2-9 turns this into the normative grammar.

```ebnf
program     = { top_item } ;
top_item    = fn_decl | statement ;
fn_decl     = "fn" IDENT "(" [ params ] ")" block ;
params      = IDENT { "," IDENT } ;             (* no trailing comma *)
block       = "{" { statement } "}" ;

statement   = let_stmt | assign_stmt | if_stmt | while_stmt | for_stmt
            | "break" ";" | "continue" ";" | return_stmt | expr_stmt ;
let_stmt    = "let" IDENT "=" expr ";" ;
assign_stmt = target "=" expr ";" ;
target      = IDENT { "[" expr "]" } ;
if_stmt     = "if" expr block { "elif" expr block } [ "else" block ] ;
while_stmt  = "while" expr block ;
for_stmt    = "for" IDENT "in" expr block ;
return_stmt = "return" [ expr ] ";" ;
expr_stmt   = expr ";" ;                        (* a stray ";" after a "}" is an error *)

expr        = or_expr ;
or_expr     = and_expr  { "or"  and_expr } ;
and_expr    = eq_expr   { "and" eq_expr } ;
eq_expr     = cmp_expr  { ( "==" | "!=" ) cmp_expr } ;
cmp_expr    = add_expr  { ( "<" | "<=" | ">" | ">=" ) add_expr } ;
add_expr    = mul_expr  { ( "+" | "-" ) mul_expr } ;
mul_expr    = unary     { ( "*" | "/" | "%" ) unary } ;
unary       = ( "not" | "-" ) unary | postfix ;
postfix     = primary { call | index | slice } ;
call        = "(" [ expr { "," expr } ] ")" ;   (* no trailing comma *)
index       = "[" expr "]" ;
slice       = "[" [ expr ] ":" [ expr ] "]" ;
primary     = INT | STRING | "true" | "false" | "nil" | IDENT
            | "(" expr ")" | list | map | closure ;
list        = "[" [ expr { "," expr } ] "]" ;   (* no trailing comma *)
map         = "{" [ expr ":" expr { "," expr ":" expr } ] "}" ;
closure     = "fn" "(" [ params ] ")" block ;

IDENT       = ( letter | "_" ) { letter | digit | "_" } ;
INT         = digit { digit } ;                 (* no hex, no underscores, no leading sign *)
STRING      = '"' { char | escape } '"' ;       (* escapes \" \\ \n \t; one line *)
COMMENT     = "#" { any-but-newline } ;
```

Notes: comparison and equality are left-associative (`1 == 1 == true` is `true`); there are no compound
assignments; a `fn NAME` is legal only at the top level; strings compare in byte order (`"B" < "a"`).

## Appendix C: Quick triage by symptom

| You see | Look at |
|---|---|
| Nothing on screen after boot | Ch. 10: resolution/GOP; Ch. 40 |
| Prompt, typing dead | Ch. 10 and Ch. 29: hub, hot-plug, boot keyboard, handoff |
| `unknown command` | Ch. 12: there are 14 commands, case-sensitive |
| `run: ... not found in the APPS snapshot` | Ch. 12 and 25: the snapshot is taken at boot; names; budgets |
| `expected ';'` | Ch. 18 gotcha 1 |
| `unexpected character 'ï'` | Ch. 25: UTF-8 BOM |
| `expected bool here` | Ch. 17 step 3: no truthiness |
| `save: ...` | Ch. 15: format, size, key rules |
| Stuck program | Esc (Ch. 14) |
| Red fault screen | Ch. 33: photograph vector, `rip`, `cr2` |

## Appendix D: Glossary of project vocabulary

**Winner**: the live copy of a key with the highest generation. **Stale copy**: any other copy of the same
key, invisible and freed on the next write. **Commit point**: the flush after the header sector is written.
**Withheld memory**: boot-services frames kept out of the allocator until firmware is truly gone.
**Soft builtin**: a builtin that a user definition of the same name overrides (proposed). **Seam**: an
interface (`Host`, `BlockDevice`, `Screen`) behind which hardware-dependent code is replaced by a test
double so the logic can run on a PC. **Ledger**: [Chapter 38](#38-verification-ledger).

## Appendix E: Where everything is

| I want to... | Go to |
|---|---|
| Install and boot | Chapters 5-10 |
| Operate the shell and save disk | Chapters 11-15 |
| Learn WISP | Chapter 17, then 18-23 |
| Know a limit | Chapter 24 |
| Understand a subsystem | Chapters 26-34 |
| Contribute | Chapters 35-37, Part VI |
| Test on hardware | Chapter 40 |
| Find a quick answer | Appendix F |
| See what is coming | Chapters 41-50 |
| The raw reference documents | `docs/WISP_LANGUAGE.md`, `docs/WISP_GRAPHICS.md`, `docs/WISP_AI.md`, `docs/SAVE_FORMAT.md`, `USB_INSTALL.md`, `CHANGES.md`, `CODEBASE_AUDIT.md` |


## Appendix F: Frequently asked questions

**I copied a program to the stick but `ls` does not show it.**
The `APPS` snapshot is taken once, at boot: reboot. Then check the file sits directly in `APPS` (not a
subfolder), ends in `.wisp`, has a name of at most 64 bytes, is valid UTF-8 *without* a byte-order mark,
is under 256 KiB, and that the total stays under 1 MiB and 64 files. A skipped file is only logged to the
debug port, so `ls` simply lacks it ([Chapter 25](#25-getting-programs-onto-the-machine)).

**The shell appears but typing does nothing.**
Plug the keyboard directly into a port on the machine (no hub, dock or monitor), power-cycle instead of
hot-plugging, and try a PS/2 keyboard or the firmware's legacy-USB setting ([Chapter 10](#10-troubleshooting-boot)).

**Can I run it in VirtualBox, VMware, Hyper-V or UTM?**
The verified environment is QEMU with OVMF. Anything that boots a 64-bit UEFI image from a FAT volume and
provides a GOP framebuffer should boot it, but input needs PS/2 emulation or an xHCI controller, and
persistence needs an AHCI disk. None of those other hypervisors has been tested.

**Does it run on a Raspberry Pi, or an Apple-silicon Mac?**
No. It is x86_64 UEFI only today. An aarch64 port is on the long horizon (H7-5, [Chapter 48](#48-horizon-7-the-long-horizon)).
Intel Macs are untested.

**Why is there no networking, and can I add it?**
It is the founding constraint: there is no network code in the binary at all. Adding it would be a
deliberate project decision made out loud ([Chapter 49](#49-non-goals-and-decisions-to-make-out-loud)), not a patch.

**How do I get a program *off* the machine?**
There is no way today. Scripts live on a read-only stick, and nothing on the machine can write one out. The
roadmap adds an on-device program store, `cat`, and `export` over serial ([Chapter 44](#44-horizon-3-the-machine-stops-needing-another-computer)).

**Can a program read or write files?**
No. Only `save`/`load`, into the 63-key store. There is no `import` either.

**How big can a saved value be?**
Its encoding must fit in 4047 bytes, and there are at most 63 keys. Larger values need the planned save
format v3 (H3-1).

**What does `format` actually erase?**
Sectors 0-512 (513 sectors, about 256 KiB) of the first AHCI disk, including any partition table, after
showing you the model and serial and requiring you to type `ERASE`. Nothing else on the disk is touched.

**How do I clear one saved value, or all of them?**
`delete <key>` (or `save(key, nil)` from a program) for one; `format` for all.

**Do variables survive between runs, or between `wisp` sessions?**
No. Each `run` is a fresh interpreter and each `wisp` session ends with `exit`. Use `save`/`load`.

**Why does `sleep_ms(5)` take 10 ms?**
The clock ticks at 100 Hz and sleeps round up to a whole tick.

**Can I run two programs at once?**
No. One program runs at a time, and it owns the keyboard and screen until it ends or you press Esc.

**Is `rand()` safe to use for anything that matters?**
No. It is a xorshift64 generator seeded from the CPU cycle counter, fine for dice and loot, not for secrets.

**Is the save disk encrypted or authenticated?**
No. The checksum detects accidents, not tampering ([Chapter 39](#39-security-and-threat-model)).

**How do I update to a new version?**
Replace `EFI/BOOT/BOOTX64.EFI` (and `APPS`) on the stick. The save disk keeps working. A version-1 store is
upgraded to version 2 on its first save, after which a v1.0.0 kernel will refuse it.

**Why can't I use floating point?**
There is none anywhere in the kernel, by principle: FPU/SSE fault handling has never been verified. Use
integers and fixed-point ([Chapter 20](#20-recipes), [Chapter 22](#22-running-trained-models)).

**I found a bug or a crash.**
Open an issue with the output of `version`, what you did, and for a crash a photograph of the fault screen
(vector, `rip`, `cr2`) ([Chapter 33](#33-failure-modes-and-the-fault-screen)).

*End of manual.*
