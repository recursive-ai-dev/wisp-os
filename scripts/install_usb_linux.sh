#!/usr/bin/env bash
# install_usb_linux.sh -- write the WISP UEFI shell to a USB flash drive on
# Linux, as a bootable GPT + FAT32 ESP.
#
# This script is destructive to whatever device you point it at (it wipes
# the partition table), so it is built around a "measure twice, cut once"
# safety model:
#
#   1. `list` mode is entirely read-only and is the only way to discover
#      device names -- it never touches a disk.
#   2. `install` mode refuses to run against any disk that is currently
#      mounted at a sensitive mountpoint (/, /boot, /boot/efi, /home) or
#      backing active swap, with NO override for that specific check --
#      this is the check that stops you from nuking the machine you're
#      running the script on.
#   3. `install` mode additionally refuses non-removable / non-USB-transport
#      disks unless you pass --force, on top of the mountpoint check above
#      (which still applies even with --force).
#   4. `install` mode requires two separate typed confirmations: the exact
#      device path, and the literal word ERASE. Piping in "yes" or similar
#      cannot satisfy this by accident.
#
# Usage:
#   sudo ./install_usb_linux.sh list
#   sudo ./install_usb_linux.sh install /dev/sdX [--force] [--source DIR]
#
# --source DIR defaults to the ESP directory this script's project builds
# (../esp relative to this script), i.e. run ./build.sh first.
#
# Secure Boot: BOOTX64.EFI produced by this project is NOT signed. Real
# hardware with Secure Boot enabled will refuse to run it. Either disable
# Secure Boot in the target machine's firmware setup before booting this
# USB stick, or enroll your own signature -- this script does not attempt
# to do either for you.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_SOURCE="$SCRIPT_DIR/../esp"

usage() {
    cat <<EOF
Usage:
  $0 list
  $0 install /dev/sdX [--force] [--source DIR]

  list                 Read-only: show candidate USB disks and exit.
  install /dev/sdX     Destructive: partition, format, and copy onto /dev/sdX.
    --force            Allow a disk that isn't flagged removable/USB-transport.
                        Does NOT bypass the mounted-filesystem safety check.
    --source DIR        ESP directory to copy from (default: $DEFAULT_SOURCE).
EOF
}

require_root() {
    if [[ "$(id -u)" -ne 0 ]]; then
        echo "error: this action requires root (partitioning/formatting a disk). Re-run with sudo." >&2
        exit 1
    fi
}

require_cmds() {
    local missing=()
    for c in "$@"; do
        command -v "$c" >/dev/null 2>&1 || missing+=("$c")
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "error: missing required command(s): ${missing[*]}" >&2
        echo "  Debian/Ubuntu: sudo apt install parted dosfstools util-linux" >&2
        echo "  Fedora:        sudo dnf install parted dosfstools util-linux" >&2
        exit 1
    fi
}

cmd_list() {
    require_cmds lsblk
    echo "==> Candidate disks (removable and/or USB-attached are marked):"
    echo
    lsblk -d -o NAME,SIZE,TYPE,TRAN,RM,MODEL | awk 'NR==1{print; next} {print}'
    echo
    echo "Disks with RM=1 or TRAN=usb are the ones this script will accept"
    echo "without --force. Identify your USB stick above, then run:"
    echo "  sudo $0 install /dev/sdX"
}

# Print every mountpoint currently in use anywhere under a whole-disk device
# (i.e. across all of its partitions), one per line. Empty output means
# nothing on the disk is mounted.
disk_mountpoints() {
    local disk="$1"
    lsblk -no MOUNTPOINT "$disk" 2>/dev/null | sed '/^$/d'
}

# Returns the /dev path for partition N of a whole-disk device, handling
# both "sdX" -> "sdXN" and "nvme0n1"/"mmcblk0"/loop-style -> "...pN" naming.
partition_path() {
    local disk="$1" num="$2"
    if [[ "$disk" =~ [0-9]$ ]]; then
        echo "${disk}p${num}"
    else
        echo "${disk}${num}"
    fi
}

cmd_install() {
    require_root
    require_cmds lsblk parted mkfs.vfat blockdev findmnt

    local disk="" force=0 source_dir="$DEFAULT_SOURCE"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --force) force=1; shift ;;
            --source) source_dir="$2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            /dev/*) disk="$1"; shift ;;
            *) echo "error: unrecognized argument '$1'" >&2; usage; exit 1 ;;
        esac
    done

    if [[ -z "$disk" ]]; then
        echo "error: no target device given." >&2
        usage
        exit 1
    fi
    if [[ ! -b "$disk" ]]; then
        echo "error: '$disk' is not a block device. Run '$0 list' to see candidates." >&2
        exit 1
    fi
    if [[ ! -d "$source_dir" || ! -f "$source_dir/EFI/BOOT/BOOTX64.EFI" ]]; then
        echo "error: source ESP not found at '$source_dir' (expected EFI/BOOT/BOOTX64.EFI there)." >&2
        echo "  Run ./build.sh first, or pass --source DIR." >&2
        exit 1
    fi

    # --- Safety check #1: never touch a disk with anything mounted on it,
    #     no override. This is what stops you from wiping your own OS. ---
    local mounts
    mounts="$(disk_mountpoints "$disk")"
    if [[ -n "$mounts" ]]; then
        echo "error: '$disk' has mounted filesystem(s), refusing to touch it:" >&2
        echo "$mounts" | sed 's/^/    /' >&2
        echo "Unmount everything on this disk yourself first if you are certain," >&2
        echo "or (much more likely) you've picked the wrong device -- re-check with '$0 list'." >&2
        exit 1
    fi
    # Belt-and-suspenders: also refuse if this disk backs the running
    # system's root filesystem, even if lsblk somehow didn't show it as
    # mounted (e.g. an unusual overlay/pivot-root setup).
    local root_disk
    root_disk="$(lsblk -no PKNAME "$(findmnt -no SOURCE / 2>/dev/null || true)" 2>/dev/null || true)"
    if [[ -n "$root_disk" && "/dev/$root_disk" == "$disk" ]]; then
        echo "error: '$disk' appears to back this machine's root filesystem. Refusing." >&2
        exit 1
    fi

    # --- Safety check #2: removable/USB only, unless --force. ---
    local rm_flag tran
    rm_flag="$(lsblk -dno RM "$disk")"
    tran="$(lsblk -dno TRAN "$disk")"
    if [[ "$rm_flag" != "1" && "$tran" != "usb" ]]; then
        if [[ "$force" -ne 1 ]]; then
            echo "error: '$disk' is not flagged removable and isn't USB-attached (RM=$rm_flag, TRAN=$tran)." >&2
            echo "This looks like it might be an internal/fixed disk. If you're certain it's the" >&2
            echo "right USB device, re-run with --force." >&2
            exit 1
        fi
        echo "warning: proceeding on a non-removable/non-USB disk because --force was given." >&2
    fi

    local size_bytes size_human model
    size_bytes="$(blockdev --getsize64 "$disk")"
    size_human="$(lsblk -dno SIZE "$disk")"
    model="$(lsblk -dno MODEL "$disk" | sed 's/[[:space:]]*$//')"

    echo "=================================================================="
    echo " About to COMPLETELY ERASE and reformat:"
    echo "   Device:  $disk"
    echo "   Size:    $size_human ($size_bytes bytes)"
    echo "   Model:   ${model:-unknown}"
    echo "   Source:  $source_dir"
    echo "=================================================================="
    echo
    echo "This will destroy ALL data currently on $disk."
    echo

    read -r -p "Type the device path exactly ($disk) to continue: " confirm1
    if [[ "$confirm1" != "$disk" ]]; then
        echo "Confirmation did not match '$disk'. Aborting, nothing was touched." >&2
        exit 1
    fi
    read -r -p "Type ERASE (all caps) to confirm you want to wipe this disk: " confirm2
    if [[ "$confirm2" != "ERASE" ]]; then
        echo "Confirmation was not 'ERASE'. Aborting, nothing was touched." >&2
        exit 1
    fi

    echo "==> Unmounting any partitions of $disk (should be none, already checked)..."
    for part in "$disk"?*; do
        [[ -b "$part" ]] || continue
        umount "$part" 2>/dev/null || true
    done

    echo "==> Writing GPT partition table with a single FAT32 EFI System Partition..."
    parted --script "$disk" mklabel gpt
    parted --script "$disk" mkpart ESP fat32 1MiB 100%
    parted --script "$disk" set 1 esp on
    # Let the kernel notice the new partition before we try to format it.
    partprobe "$disk" 2>/dev/null || true
    sleep 1

    local part
    part="$(partition_path "$disk" 1)"
    if [[ ! -b "$part" ]]; then
        # udev can lag partprobe on some systems; give it one more chance.
        sleep 2
        if [[ ! -b "$part" ]]; then
            echo "error: expected partition '$part' did not appear after partitioning." >&2
            exit 1
        fi
    fi

    echo "==> Formatting $part as FAT32 (label WISPUSB)..."
    mkfs.vfat -F32 -n WISPUSB "$part"

    # Deliberately NOT `local`: an EXIT trap runs after bash has already
    # unwound this function's local-variable scope (errexit aborts the
    # function first, *then* fires the trap), so a trap body referencing a
    # `local` would blow up with "unbound variable" under `set -u` instead
    # of doing its job. Plain globals survive to see the trap run.
    mount_dir="$(mktemp -d)"
    mounted=0
    # However this function exits (success, a failed command under `set
    # -e`, or a signal), make sure we don't leak a mounted filesystem or an
    # empty scratch directory under /tmp.
    trap 'if [[ "${mounted:-0}" -eq 1 ]]; then umount "$mount_dir" 2>/dev/null || true; fi; rmdir "$mount_dir" 2>/dev/null || true' EXIT

    echo "==> Mounting $part at $mount_dir..."
    mount "$part" "$mount_dir"
    mounted=1

    echo "==> Copying firmware and apps..."
    mkdir -p "$mount_dir/EFI/BOOT" "$mount_dir/APPS"
    cp -v "$source_dir/EFI/BOOT/BOOTX64.EFI" "$mount_dir/EFI/BOOT/BOOTX64.EFI"
    if compgen -G "$source_dir/APPS/*.wisp" >/dev/null; then
        cp -v "$source_dir"/APPS/*.wisp "$mount_dir/APPS/"
    fi

    echo "==> Syncing and unmounting..."
    sync
    umount "$mount_dir"
    mounted=0
    trap - EXIT
    rmdir "$mount_dir"

    echo
    echo "Done. $disk is now a bootable WISP UEFI shell USB stick."
    echo
    echo "IMPORTANT: BOOTX64.EFI is unsigned. On real hardware with Secure Boot"
    echo "enabled, the firmware will refuse to run it. Disable Secure Boot in the"
    echo "target machine's firmware setup (or enroll your own signature) first."
    echo "Boot it by choosing this USB stick from your firmware's one-time boot"
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
