#!/usr/bin/env bash
# Boots ./esp in QEMU under OVMF. Run ./build.sh first.
# WISP_ISO=dist/wisp-os-v<version>-live.iso boots live installation media.
# WISP_INSTALL_DISK=install.img selects its destination image (created at 128M).
# WISP_BOOT_DISK=install.img boots the installed disk without live media.
#
# By default this runs headless (-display none), which is what the sandbox
# this project was built in requires; the shell is still fully interactive
# over QEMU's monitor/serial in that mode (see tests/acceptance_test.py for
# how the automated acceptance test drives it). To get a real graphical
# window when running on a machine that has a display, set:
#   UEFI_SHELL_DISPLAY=gui ./run.sh
set -euo pipefail
cd "$(dirname "$0")"

if [[ -n "${WISP_ISO:-}" && -n "${WISP_BOOT_DISK:-}" ]]; then
    printf 'error: choose WISP_ISO or WISP_BOOT_DISK, not both\n' >&2
    exit 2
fi
if [[ -n "${WISP_INSTALL_DISK:-}" && -z "${WISP_ISO:-}" ]]; then
    printf 'error: WISP_INSTALL_DISK requires WISP_ISO\n' >&2
    exit 2
fi
if [[ -n "${WISP_ISO:-}" && ! -f "$WISP_ISO" ]]; then
    printf 'error: live ISO missing: %s\n' "$WISP_ISO" >&2
    exit 1
fi
if [[ -n "${WISP_BOOT_DISK:-}" && ( ! -f "$WISP_BOOT_DISK" || -L "$WISP_BOOT_DISK" ) ]]; then
    printf 'error: WISP_BOOT_DISK must name an existing regular disk image\n' >&2
    exit 1
fi
if [[ -z "${WISP_ISO:-}" && -z "${WISP_BOOT_DISK:-}" && ! -f esp/EFI/BOOT/BOOTX64.EFI ]]; then
    echo "error: esp/EFI/BOOT/BOOTX64.EFI not found -- run ./build.sh first" >&2
    exit 1
fi

# The project spec's reference invocation names /usr/share/OVMF/OVMF_CODE.fd,
# a single monolithic firmware image (code + variable store in one file), as
# used by some distros. This distro's `ovmf` package instead ships a split
# code/vars pair meant for `-pflash`, under different filenames, plus a
# separate monolithic image that's what actually works with `-bios`. Prefer
# the exact spec'd path and fall back to the real one so this still runs
# unmodified wherever OVMF_CODE.fd exists. See CHANGES.md for details.
OVMF_CODE=/usr/share/OVMF/OVMF_CODE.fd
if [[ ! -f "$OVMF_CODE" ]]; then
    OVMF_CODE=/usr/share/ovmf/OVMF.fd
fi
if [[ ! -f "$OVMF_CODE" ]]; then
    echo "error: no OVMF firmware found (checked OVMF_CODE.fd in /usr/share/OVMF and OVMF.fd in /usr/share/ovmf)" >&2
    exit 1
fi

display_args=(-display none)
if [[ "${UEFI_SHELL_DISPLAY:-}" == "gui" ]]; then
    display_args=()
fi

# Phase 3: an xHCI host controller, so the normal development run exercises
# the same USB path a post-2010 physical machine presents. This is purely
# additive -- the boot stick below stays on QEMU's `-usb` UHCI controller,
# byte-for-byte as before, and the xHCI controller starts with nothing
# attached to it.
#
# An empty xHCI controller is not a dead branch: `src/xhci.rs` initialises
# it, finds no connected port, and reports "USB: xHCI controller ready, no
# keyboard attached" -- exactly the `xhci_no_device` scenario in
# tests/usb_test.py. That is the state a real machine is in when its
# keyboard is PS/2 or is hanging off a hub we do not yet walk (hub support
# is named as deferred in CHANGES.md), so it is worth being the default.
#
# Attaching a USB keyboard is opt-in rather than default on purpose. With a
# `usb-kbd` present, QEMU routes host keystrokes (and monitor `sendkey`) to
# it instead of the emulated PS/2 controller, which silently swaps which of
# our two input drivers is under test. Both configurations are covered by
# tests/usb_test.py (`xhci_no_device` and `usb_keyboard`); this switch just
# makes which one you are running an explicit choice:
#   UEFI_SHELL_USB_KBD=1 ./run.sh
usb_args=(-device qemu-xhci,id=xhci)
if [[ "${UEFI_SHELL_USB_KBD:-}" == "1" ]]; then
    usb_args+=(-device usb-kbd,bus=xhci.0)
fi

# Phase 2, Part B: a second, separate virtual disk attached via AHCI, purely
# for WISP-program save data -- the boot-USB `-drive`/`-device usb-storage`
# pair above stays byte-for-byte unchanged, and this is fully additive.
# `save.img` is generated on demand (never committed -- see .gitignore) so a
# fresh checkout gets a fresh, empty disk rather than one seeded with
# whatever a previous run happened to write.
SAVE_IMG="$(dirname "$0")/save.img"
disk_size=64M
if [[ -n "${WISP_ISO:-}" ]]; then
    SAVE_IMG="${WISP_INSTALL_DISK:-$(dirname "$0")/install.img}"
    disk_size=128M
fi
if [[ -z "${WISP_BOOT_DISK:-}" ]]; then
    if [[ -e "$SAVE_IMG" || -L "$SAVE_IMG" ]]; then
        [[ -f "$SAVE_IMG" && ! -L "$SAVE_IMG" ]] || { printf 'error: destination must be a regular disk image\n' >&2; exit 1; }
    else
        printf '==> Creating %s (%s, raw)\n' "$SAVE_IMG" "$disk_size"
        qemu-img create -f raw "$SAVE_IMG" "$disk_size" >/dev/null
    fi
fi

boot_args=(-drive format=raw,file=fat:esp,if=none,id=stick,readonly=on
           -usb -device usb-storage,drive=stick)
disk_args=(-drive "id=savedisk,if=none,format=raw,file=${SAVE_IMG//,/,,}"
           -device ahci,id=ahci0 -device ide-hd,drive=savedisk,bus=ahci0.0)
if [[ -n "${WISP_ISO:-}" ]]; then
    boot_args=(-cdrom "$WISP_ISO" -boot order=d)
elif [[ -n "${WISP_BOOT_DISK:-}" ]]; then
    boot_args=()
    disk_args=(-drive "id=system,if=none,format=raw,file=${WISP_BOOT_DISK//,/,,}"
               -device ahci,id=ahci0 -device ide-hd,drive=system,bus=ahci0.0)
fi

# The ESP is mounted through QEMU's vvfat driver READ-ONLY (`fat:esp`, not
# `fat:rw:esp`). This was not the original choice -- read-write was, so
# that OVMF could persist its UEFI variable store -- and it broke things
# badly enough to be worth recording here in full.
#
# With `-bios` (a single monolithic firmware image, no separate writable
# pflash for `OVMF_VARS`), this OVMF build persists UEFI variables by
# writing a file named `NvVars` directly onto the first FAT filesystem it
# can reach, i.e. straight into ./esp. That alone sounded like a merely
# cosmetic problem (a stray file, maybe stale boot-manager state), and an
# earlier version of this script papered over it by deleting `esp/NvVars`
# before every boot. That fix was wrong, or at least incomplete: it
# treated the symptom, not the disease. The actual disease is that
# QEMU's vvfat driver in read-write mode is not safe for repeated,
# persistent use against the same host directory -- QEMU's own manual
# describes read/write vvfat support as available "only for (beta)
# testing purposes," and running this project's acceptance test twice
# back to back proved why: a byte-for-byte comparison against a freshly
# built BOOTX64.EFI showed vvfat had silently rewritten 47738 of its
# 90624 bytes on the *host filesystem* -- same file size, wrong bytes
# from partway through the file to the end. Not a timestamp, not a stale
# NVRAM entry: the boot executable itself was corrupted on disk by the
# emulator between runs, which is why OVMF gave up trying to run it and
# fell back to its own built-in EDK II shell instead. Several small
# `esp/APPS/*.wisp` files got rewritten by the same mechanism during that
# same test run too; those happened to come back byte-identical, but
# nothing about vvfat's write path gives any guarantee of that -- it was
# luck, not a property to depend on.
#
# Mounting read-only removes the write path entirely, which removes the
# corruption risk entirely. Nothing this project boots ever needs to
# write to the ESP -- `src/fs.rs` only ever opens files `FileMode::Read`,
# ls and run are the only two things that touch the filesystem, and
# neither writes -- so the only writer was ever OVMF's own NVRAM
# persistence, which we don't want anyway: this project's whole boot
# strategy is the UEFI removable-media fallback path with *no* cached
# boot manager entry required (see USB_INSTALL.md's "Booting the
# stick"). Under a read-only mount, OVMF's variable-store write simply
# fails and it falls back to a volatile, in-memory-only store for that
# boot -- which is a better match for "start clean every time" than the
# delete-before-boot approach ever was, and there is no longer an
# `esp/NvVars` file for a stale one to accumulate into in the first
# place. See CHANGES.md for the full account (including how this was
# caught, and how it was verified fixed across repeated consecutive
# runs, not just one).
#
# One more wrinkle a plain `-drive format=raw,file=fat:esp` hits: with no
# `if=` given, QEMU attaches it through its default IDE hard-disk device
# model, and `ide-hd` unconditionally refuses a read-only backing node
# ("Block node is read-only", even with an explicit `readonly=on` on the
# `-drive` itself) -- disk device models expect to be writable; only
# media models built for read-only use (optical, or removable mass
# storage) accept one. So this attaches the volume as a USB mass-storage
# device instead of a plain IDE disk. That's not a workaround bolted on
# for its own sake, either: it makes the emulated boot path a closer
# match for how this project is actually meant to be used -- booted from
# a real USB stick (see USB_INSTALL.md) -- and UEFI's removable-media
# boot fallback (`\EFI\BOOT\BOOTX64.EFI`) applies to it exactly the same
# way it does to a plain disk.
exec qemu-system-x86_64 \
    -bios "$OVMF_CODE" \
    "${boot_args[@]}" \
    "${usb_args[@]}" \
    "${disk_args[@]}" \
    -net none \
    -m 256M \
    -vga std \
    -debugcon file:debug.log \
    "${display_args[@]}" \
    "$@"
