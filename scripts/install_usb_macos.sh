#!/usr/bin/env bash
# install_usb_macos.sh -- write the WISP UEFI shell to a USB flash drive on
# macOS, as a bootable GPT + FAT32 disk.
#
# Safety model (see install_usb_linux.sh for the fuller rationale, this is
# the same approach adapted to diskutil):
#
#   1. `list` mode is read-only and only lists EXTERNAL PHYSICAL disks --
#      diskutil's own "external physical" filter already excludes your
#      internal boot disk and any disk images/virtual volumes, which is a
#      strong built-in safety net on top of everything below.
#   2. `install` mode re-checks the target against that same external-
#      physical list and refuses anything not on it, no override -- this
#      script will never touch your internal disk, full stop.
#   3. `install` mode requires two separate typed confirmations: the exact
#      disk identifier (e.g. disk4), and the literal word ERASE.
#
# Usage:
#   ./install_usb_macos.sh list
#   ./install_usb_macos.sh install diskN [--source DIR]
#
# Run with sudo if diskutil eraseDisk reports a permissions error on your
# system (usually not required, but some macOS versions/configurations
# require it for raw disk erase).
#
# Secure Boot / Mac note: this project targets generic PC UEFI firmware
# (as exercised in QEMU/OVMF). Apple Silicon and Intel Macs do not use the
# same PC-BIOS-compatible UEFI boot flow as a generic PC, so a USB stick
# made by this script is meant for booting on OTHER x86_64 UEFI PCs, not
# for booting the Mac you run this script on. On those PCs: BOOTX64.EFI is
# unsigned, so Secure Boot must be disabled in the target machine's
# firmware setup before it will run.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_SOURCE="$SCRIPT_DIR/../esp"

usage() {
    cat <<EOF
Usage:
  $0 list
  $0 install diskN [--source DIR]

  list                Read-only: show external physical disks and exit.
  install diskN       Destructive: erase and rebuild diskN as a FAT32 stick.
    --source DIR       ESP directory to copy from (default: $DEFAULT_SOURCE).

Examples:
  ./install_usb_macos.sh list
  ./install_usb_macos.sh install disk4
EOF
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: required command '$1' not found (this script needs macOS's diskutil)." >&2
        exit 1
    }
}

# Space-separated list of disk identifiers (e.g. "disk4 disk5") that
# diskutil considers external AND physical -- i.e. real removable media,
# never the internal boot disk, a disk image, or an APFS synthesized
# volume disk. Parses `diskutil list external physical`'s plain-text
# output, which prints one header line per matching disk in the stable,
# well-known form "/dev/diskN (external, physical):".
external_physical_disks() {
    diskutil list external physical 2>/dev/null | \
        grep -Eo '^/dev/disk[0-9]+' | sed 's#^/dev/##'
}

cmd_list() {
    require_cmd diskutil
    echo "==> External, physical disks (internal/virtual disks are never shown here):"
    echo
    diskutil list external physical
    echo
    echo "Identify your USB stick above (e.g. 'disk4'), then run:"
    echo "  $0 install diskN"
}

cmd_install() {
    require_cmd diskutil

    local disk="" source_dir="$DEFAULT_SOURCE"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --source) source_dir="$2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            disk*) disk="$1"; shift ;;
            *) echo "error: unrecognized argument '$1'" >&2; usage; exit 1 ;;
        esac
    done

    if [[ -z "$disk" ]]; then
        echo "error: no target device given (expected something like 'disk4')." >&2
        usage
        exit 1
    fi
    if [[ ! -d "$source_dir" || ! -f "$source_dir/EFI/BOOT/BOOTX64.EFI" ]]; then
        echo "error: source ESP not found at '$source_dir' (expected EFI/BOOT/BOOTX64.EFI there)." >&2
        echo "  Run ./build.sh first, or pass --source DIR." >&2
        exit 1
    fi

    # --- Safety check: must be on diskutil's own external+physical list.
    #     No override flag for this one, deliberately. ---
    local candidates matched=0
    candidates="$(external_physical_disks)"
    for c in $candidates; do
        [[ "$c" == "$disk" ]] && matched=1
    done
    if [[ "$matched" -ne 1 ]]; then
        echo "error: '$disk' is not in diskutil's external+physical disk list, refusing." >&2
        echo "This means it's either your internal disk, a disk image, or doesn't exist." >&2
        echo "Run '$0 list' to see the disks this script is willing to touch." >&2
        exit 1
    fi

    local info size
    info="$(diskutil info "$disk" 2>/dev/null || true)"
    size="$(echo "$info" | awk -F: '/Disk Size/{print $2; exit}' | sed 's/^[[:space:]]*//')"

    echo "=================================================================="
    echo " About to COMPLETELY ERASE and reformat:"
    echo "   Device:  /dev/$disk"
    echo "   Size:    ${size:-unknown}"
    echo "   Source:  $source_dir"
    echo "=================================================================="
    echo
    echo "This will destroy ALL data currently on $disk."
    echo
    diskutil list "$disk" || true
    echo

    read -r -p "Type the disk identifier exactly ($disk) to continue: " confirm1
    if [[ "$confirm1" != "$disk" ]]; then
        echo "Confirmation did not match '$disk'. Aborting, nothing was touched." >&2
        exit 1
    fi
    read -r -p "Type ERASE (all caps) to confirm you want to wipe this disk: " confirm2
    if [[ "$confirm2" != "ERASE" ]]; then
        echo "Confirmation was not 'ERASE'. Aborting, nothing was touched." >&2
        exit 1
    fi

    echo "==> Unmounting $disk..."
    diskutil unmountDisk "/dev/$disk"

    echo "==> Erasing $disk as GPT + FAT32 (volume name WISPUSB)..."
    # `eraseDisk` wipes the whole disk, writes a fresh GUID (GPT) partition
    # map, and creates one FAT32 partition filling it. NOTE on partition
    # type: this creates a standard "Microsoft Basic Data" FAT32 partition,
    # not one explicitly tagged with the EFI System Partition type GUID --
    # diskutil does not expose a direct way to set that GUID. In practice
    # this is not a problem for booting: every mainstream x86_64 UEFI
    # firmware (this project is verified against OVMF, and this matches
    # the behavior real USB-installer tools rely on) supports the
    # "removable media" fallback boot path, which scans for a FAT
    # filesystem containing \EFI\BOOT\BOOTX64.EFI on ANY partition of a
    # bootable removable disk -- it does not require the strict ESP type
    # GUID that a fixed internal boot disk would. If you run into a
    # specific machine that insists on a type-GUID-tagged ESP, you'll need
    # a GPT-editing tool to relabel the partition type by hand; that's a
    # rare enough case that this script does not attempt to handle it
    # automatically.
    diskutil eraseDisk FAT32 WISPUSB GPT "/dev/$disk"

    local mount_point
    mount_point="$(diskutil info "WISPUSB" 2>/dev/null | awk -F: '/Mount Point/{print $2; exit}' | sed 's/^[[:space:]]*//')"
    if [[ -z "$mount_point" || ! -d "$mount_point" ]]; then
        echo "error: could not find the mount point for the newly created WISPUSB volume." >&2
        echo "It should have been auto-mounted after eraseDisk; check 'diskutil list $disk'." >&2
        exit 1
    fi

    echo "==> Copying firmware and apps to $mount_point..."
    mkdir -p "$mount_point/EFI/BOOT" "$mount_point/APPS"
    cp -v "$source_dir/EFI/BOOT/BOOTX64.EFI" "$mount_point/EFI/BOOT/BOOTX64.EFI"
    if compgen -G "$source_dir/APPS/*.wisp" >/dev/null; then
        cp -v "$source_dir"/APPS/*.wisp "$mount_point/APPS/"
    fi

    echo "==> Ejecting..."
    diskutil eject "/dev/$disk"

    echo
    echo "Done. $disk is now a bootable WISP UEFI shell USB stick."
    echo
    echo "IMPORTANT: BOOTX64.EFI is unsigned. On real PC hardware with Secure Boot"
    echo "enabled, the firmware will refuse to run it. Disable Secure Boot in the"
    echo "target machine's firmware setup (or enroll your own signature) first."
    echo "Boot it by choosing this USB stick from the target PC's one-time boot"
    echo "menu (commonly F12/F10/Esc at power-on) -- it uses the UEFI removable-"
    echo "media fallback path (\\EFI\\BOOT\\BOOTX64.EFI), so no OS or boot manager"
    echo "entry needs to be installed for it to run."
}

main() {
    if [[ $# -lt 1 ]]; then
        usage
        exit 1
    fi
    case "$1" in
        list) shift; cmd_list "$@" ;;
        install) shift; cmd_install "$@" ;;
        -h|--help) usage; exit 0 ;;
        *) echo "error: unknown subcommand '$1'" >&2; usage; exit 1 ;;
    esac
}

main "$@"
