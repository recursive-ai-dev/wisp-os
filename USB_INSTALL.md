# Live installation and USB boot media

## Install the live system onto a disk

Wisp's boot menu offers **T — Try Wisp** and **I — Install Wisp** before
the kernel exits UEFI Boot Services. Waiting at the menu selects Try. The
installer uses firmware disk access, so it can copy the live system onto
eligible SATA, USB or NVMe disks without a host OS.

Build the live ISO from a source checkout:

```bash
./scripts/make_iso.sh
```

This needs Python 3 and `xorriso`, plus the usual Rust build tools. Use
`--skip-build` to package an existing `esp/EFI/BOOT/BOOTX64.EFI`, or
`--output /path/wisp-live.iso` to choose the output file. No root access,
mount operation or additional Python packages are needed. The default
output is `dist/wisp-os-v1.1.0-live.iso` and its `.sha256` file. The ISO has
both an EFI El Torito entry for optical/VM boot and a GPT EFI partition
for USB image-writing tools. Secure Boot requirements still apply.

For a VM trial with a separate, disposable image destination:

```bash
UEFI_SHELL_DISPLAY=gui WISP_ISO=dist/wisp-os-v1.1.0-live.iso ./run.sh
```

The runner creates `install.img` at 128 MiB if missing. Select another
image with `WISP_INSTALL_DISK=/path/destination.img`. It never passes a host
block device to QEMU. To start the installed system without its live ISO:

```bash
UEFI_SHELL_DISPLAY=gui WISP_BOOT_DISK=install.img ./run.sh
```

Installation replaces the selected disk's partition table and files.
Back up anything needed from it first. The installer:

1. Lists writable **whole disks** with 512-byte logical sectors and at
   least 80 MiB capacity. It excludes the boot source and refuses to
   proceed if the source cannot be identified safely.
2. Shows the selected disk's identity and capacity, then asks for the
   exact text `ERASE <number>`. Esc or a different answer cancels without
   writing.
3. Creates primary and backup GPT tables, a 64 MiB FAT32 EFI System
   Partition at the 1 MiB boundary, and a separate Wisp save partition.
4. Copies `EFI/BOOT/BOOTX64.EFI` and the live `APPS/*.wisp` programs,
   including their long filenames; initializes the save store; flushes
   and reads back the written sectors before reporting success.
5. Offers **R** to reboot or **T** to continue the live session. Remove
   the live medium and select the installed disk in firmware to boot it.

This is a whole-disk installer; it does not resize existing partitions or
set up dual boot. An interrupted installation may leave an unbootable
target; boot the live medium and install again. It installs the standard
`\EFI\BOOT\BOOTX64.EFI` fallback file and does not alter firmware NVRAM
boot entries. Some firmware requires choosing that file using its
“boot from EFI file” facility on an internal disk.

The installed save store mounts automatically on supported AHCI disks.
`format` resets only that save region, preserving the boot partition.
Firmware may install and boot Wisp on NVMe or USB even though the kernel
still needs AHCI for persistence after Boot Services ends. The installer
does not add networking or online updates.

## Prepare a USB stick from another OS

`scripts/` contains three installers -- one per host OS you might be
running the build/copy step from -- that turn a USB flash drive into a
bootable copy of this project: a GPT-partitioned, FAT32-formatted disk
containing `EFI/BOOT/BOOTX64.EFI` and the `APPS/*.wisp` programs.

| Host OS | Script | Underlying tools |
|---|---|---|
| Linux | `scripts/install_usb_linux.sh` | `parted`, `mkfs.vfat` |
| macOS | `scripts/install_usb_macos.sh` | `diskutil` |
| Windows | `scripts/install_usb_windows.ps1` | PowerShell `Storage` module |

All three prepare live media with the boot menu above. They follow the
same two-step pattern and the same safety philosophy:

```
<script> list                     # read-only: shows candidate disks
<script> install <device>         # destructive: wipes and writes <device>
```

`esp/EFI/BOOT/BOOTX64.EFI` must exist -- that's what gets copied onto the
stick, along with everything in `esp/APPS/`. A release archive
(`wisp-os-v1.1.0.tar.gz` / `.zip`) ships it prebuilt (check it with
`sha256sum -c SHA256SUMS` in the extracted folder); from a source checkout,
run `./build.sh` first (see the main `README`/`CHANGES.md`).

## Why this is safe to run

Partitioning a disk is inherently destructive, so each script is built
around the same set of guardrails:

- **`list` never touches a disk.** It only reads information back, so
  running it is always safe and is how you find your device's identifier
  in the first place (`/dev/sdb` on Linux, `disk4` on macOS, disk number
  `2` on Windows).
- **The target must look like removable/USB media.** Linux checks the
  kernel's `RM` (removable) flag and USB transport; macOS only ever
  considers disks `diskutil` itself reports as external+physical (which
  categorically excludes your internal disk); Windows checks `BusType
  -eq 'USB'`. Linux and Windows let you override this with `--force`/
  `-Force` for the rare case of a USB enclosure that doesn't report
  itself correctly -- macOS intentionally has no such override.
- **The disk backing your own running system can never be selected**,
  full stop, no override flag bypasses this specific check. Linux
  refuses any disk with a currently-mounted filesystem (checked both via
  `lsblk` and by cross-referencing the root filesystem's backing disk);
  Windows refuses the disk backing `%SystemDrive%` and anything flagged
  `IsBoot`/`IsSystem`; macOS's external+physical filter excludes the
  internal disk by construction.
- **Two separate typed confirmations before anything destructive
  happens:** the exact device identifier, and the literal word `ERASE`.
  Neither can be satisfied by accident, by a stray keypress, or by
  piping in a canned "yes".

None of this makes a USB installer *un*-destructive -- it still
completely erases the target disk, that's the point -- but it makes it
very hard to point at the wrong disk without noticing.

## Linux

```bash
sudo ./scripts/install_usb_linux.sh list
sudo ./scripts/install_usb_linux.sh install /dev/sdb
```

Requires `parted` and `dosfstools` (`mkfs.vfat`); the script tells you
the exact package names for Debian/Ubuntu and Fedora if they're missing.
Must be run as root (needed to partition/format a raw block device).

This was exercised end-to-end against a loopback disk image in
development (partition table + FAT32 filesystem verified with `parted
print` / `blkid` / `fsck.fat`); the final mount-and-copy step is standard
and well-trodden but couldn't be verified inside the sandboxed container
this project was built in, since that container's kernel has no `vfat`
filesystem driver available. On any normal Linux desktop/laptop install
this is a non-issue -- `vfat` support is essentially universal.

## macOS

```bash
./scripts/install_usb_macos.sh list
./scripts/install_usb_macos.sh install disk4
```

Requires only `diskutil`, which ships with macOS. No `sudo` needed in
the common case (`diskutil` handles its own privilege elevation via
prompts if required on your system).

One nuance worth knowing: `diskutil eraseDisk FAT32 ... GPT` creates a
standard FAT32 ("Microsoft Basic Data") partition rather than one
explicitly tagged with the EFI System Partition type GUID -- macOS's
`diskutil` doesn't expose a direct way to set that GUID. This is not a
practical problem: mainstream x86_64 UEFI firmware (this project is
verified against OVMF) supports the UEFI "removable media" fallback boot
path, which looks for `\EFI\BOOT\BOOTX64.EFI` on a FAT filesystem on any
partition of a bootable removable disk, regardless of the partition's
type GUID. This is the same mechanism virtually every USB-installer tool
relies on. It's called out here so you're not caught by surprise on the
rare machine that insists otherwise.

This script could not be executed against real hardware or a real macOS
install during development (this project was built in a Linux sandbox
with no macOS access) -- it was written carefully against documented
`diskutil` behavior and reviewed for correctness, but treat it with
correspondingly more caution than the Linux script on first use.

## Windows

Open PowerShell **as Administrator**, then:

```powershell
.\scripts\install_usb_windows.ps1 -List
.\scripts\install_usb_windows.ps1 -Install -DiskNumber 2
```

Uses the built-in `Storage` module (`Get-Disk`, `Clear-Disk`,
`Initialize-Disk`, `New-Partition`, `Format-Volume`) rather than
`diskpart` scripting, which lets it set the EFI System Partition type
GUID explicitly and correctly (`{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}`)
-- no fallback-path caveat needed here, unlike macOS.

As with the macOS script, this could not be executed against a real
Windows machine during development (Linux-only sandbox); it follows
Microsoft's documented `New-Partition | Format-Volume` pattern and was
reviewed carefully, but treat it with correspondingly more caution on
first use.

## Booting the stick

However you made it, booting is the same everywhere: plug the stick into
an x86_64 PC, and at power-on use that machine's one-time boot-device
menu (commonly `F12`, `F10`, or `Esc` -- check the machine's splash
screen or manual) to boot from the USB stick directly, rather than
relying on it being added to the firmware's permanent boot order. This
works with zero setup on the stick's end because it relies on the UEFI
spec's **removable media fallback path**: any UEFI firmware booting a
removable disk looks for `\EFI\BOOT\BOOTX64.EFI` on it automatically, no
boot manager entry required.

## What the machine has to provide

This section is deliberately specific about what is supported and what
is not, because "it boots on a PC" is the kind of claim that is easy to
make and expensive to be wrong about. Everything below is implemented
and exercised by the test suites; anything not listed is not supported.

**Firmware.** x86_64 UEFI, booting in 64-bit mode. There is no BIOS/CSM
path and no 32-bit build. The stick is not signed -- see Secure Boot
below.

**CPU and interrupts.** The kernel parses ACPI (RSDP -> XSDT/RSDT ->
MADT), brings up the Local APIC and the I/O APIC, and moves every
interrupt in the machine onto them; the timer is the LAPIC timer,
calibrated against the PIT during boot. Interrupt Source Overrides from
the MADT are honoured, so the common firmware remapping of the timer
(ISA IRQ0 arriving as GSI 2) is handled rather than assumed away. This
is what makes the stick usable on a legacy-free machine, where the 8259
PIC and the PIT may be absent or vestigial. If the MADT is missing or
unusable, the kernel says so on screen and falls back to the 8259/PIT
path rather than hanging -- degraded, but still a working shell on an
older machine.

Only the boot processor is used. Other cores are left parked in the
state the firmware handed them over in; there is no SMP bring-up.

**Keyboard.** Two input paths, both live at once:

- A PS/2 keyboard (or a firmware/chipset emulation of one), via ISA
  IRQ1 routed through the I/O APIC.
- A USB HID **boot-protocol** keyboard on an **xHCI** controller,
  plugged directly into a root port. The kernel enumerates the device,
  selects boot protocol, and polls its interrupt endpoint.

The gaps here are real and worth stating plainly. Keyboards behind a USB
**hub** are not found -- hub enumeration is not implemented, and an
external hub between the keyboard and the machine will leave you without
input from it. Hot-plug is not handled: the root ports are scanned once
during boot, so a keyboard plugged in after the shell is up will not
appear. **EHCI and OHCI/UHCI controllers are not driven**, so on a
machine old enough to predate xHCI, USB input depends entirely on the
firmware's own legacy PS/2 emulation. On a post-2010 machine with the
keyboard in a port on the chassis, the xHCI path is the one that runs;
that configuration is what `tests/usb_test.py` covers end to end,
including proving a real keystroke arrives over USB.

The practical advice that follows from all of this: if the shell comes
up but does not respond to typing, move the keyboard to a port directly
on the machine rather than through a hub, dock or monitor, and power-
cycle rather than hot-plugging it.

**Storage.** Saved data (`save`/`load` in WISP) needs an **AHCI** SATA
controller with a disk attached. NVMe is not supported, and neither is
IDE-mode SATA -- a firmware setting of "IDE"/"Compatibility" instead of
"AHCI" will leave the shell fully usable with persistence disabled, and
it says so on screen when that happens rather than failing silently. The
boot stick itself is read through UEFI's own protocols before exit from
Boot Services, so it does not depend on any of this.

**Display.** A UEFI Graphics Output Protocol framebuffer, which is
effectively universal on UEFI machines. The console is drawn by this
project directly into that framebuffer; there is no text mode and no
driver for any specific GPU.

**Memory.** 256 MB is what the emulated runs use and is comfortable.

### Secure Boot

`BOOTX64.EFI` is **not signed**. Machines with Secure Boot enabled will
refuse to run it (you'll typically see a firmware error mentioning an
invalid/unsigned image, or the boot attempt will silently fall through
to the next device). Before booting this stick on real hardware, go into
that machine's firmware setup and either disable Secure Boot, or (if
you'd rather leave Secure Boot on generally) enroll a signature for this
binary yourself -- this project does not sign the binary or attempt to
manage Secure Boot enrollment for you.

### No networking, by design

Every OVMF/QEMU run and every installer script here uses `-net none` /
never touches network configuration -- consistent with this project's
hard requirement that the shell has no networking capability at all.
There is nothing to disable on the stick itself: the firmware binary
never links against or calls any networking protocol, so there's no
setting that could accidentally turn it on.

This did not change when USB support landed. The xHCI driver in
`src/xhci.rs` enumerates a device, checks its interface descriptor for
the HID boot-keyboard class triple, and drives exactly one interrupt
endpoint; a USB network adapter is simply a device it does not
recognise and does not configure. There is no USB networking class
driver, and adding one would be a decision to be made out loud rather
than something that could arrive as a side effect.
