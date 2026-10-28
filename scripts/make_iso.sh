#!/usr/bin/env bash
# Build a UEFI-bootable live ISO. Bash; Python 3 + xorriso; no root/mount.
# Usage: scripts/make_iso.sh [--skip-build] [--output PATH]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
skip_build=0
output=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) skip_build=1; shift ;;
        --output)
            [[ $# -ge 2 && -n "$2" ]] || { printf 'error: --output needs a path\n' >&2; exit 2; }
            output="$2"; shift 2 ;;
        -h|--help)
            printf 'Usage: scripts/make_iso.sh [--skip-build] [--output PATH]\n'
            printf 'Build dist/wisp-os-v<version>-live.iso and its .sha256 checksum.\n'
            exit 0 ;;
        *) printf 'error: unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done
for tool in python3 xorriso; do
    command -v "$tool" >/dev/null 2>&1 || { printf 'error: %s is required\n' "$tool" >&2; exit 1; }
done
version="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' Cargo.toml | head -n 1)"
[[ -n "$version" ]] || { printf 'error: package version is missing\n' >&2; exit 1; }
if [[ -z "$output" ]]; then output="$ROOT/dist/wisp-os-v${version}-live.iso"; fi
# Resolve before creating anything; refuse devices/symlinks even with --output.
output="$(python3 - "$output" <<'PY'
import os
import pathlib
import stat
import sys
p = pathlib.Path(os.path.abspath(sys.argv[1]))
for target in (p, pathlib.Path(str(p) + '.sha256')):
    if target.exists() or target.is_symlink():
        if not stat.S_ISREG(target.lstat().st_mode):
            sys.exit('error: output and checksum must be regular files, never devices/symlinks')
print(p)
PY
)"
if [[ "$skip_build" -eq 0 ]]; then ./build.sh; fi
[[ -f esp/EFI/BOOT/BOOTX64.EFI ]] || { printf 'error: EFI binary missing; run ./build.sh\n' >&2; exit 1; }
mkdir -p -- "$(dirname "$output")"
stage="$(mktemp -d "$(dirname "$output")/.wisp-iso-XXXXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/tree"
python3 tools/make_esp.py esp "$stage/efiboot.img"
cp README.md USB_INSTALL.md "$stage/tree/"
# EFI's El Torito entry and a GPT EFI partition share the same FAT image:
# optical/VM boots and raw-written USB media both load this volume. APPS
# and the installer binary live together on it, independent of ISO9660.
printf '==> Building live ISO: %s\n' "$output"
if [[ -e "$output" ]]; then printf '==> Replacing existing ISO after the build succeeds\n'; fi
xorriso -as mkisofs -quiet -V WISP_LIVE -R -J \
    -append_partition 2 0xef "$stage/efiboot.img" -appended_part_as_gpt \
    -e --interval:appended_partition_2:all:: -no-emul-boot \
    -o "$stage/live.iso" "$stage/tree"
python3 - "$stage/live.iso" "$stage/checksum" "$(basename "$output")" <<'PY'
import hashlib
import pathlib
import sys
source, checksum, name = sys.argv[1:]
digest = hashlib.sha256()
with open(source, 'rb') as stream:
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        digest.update(chunk)
pathlib.Path(checksum).write_text(f'{digest.hexdigest()}  {name}\n', encoding='utf-8')
PY
mv -f -- "$stage/live.iso" "$output"
mv -f -- "$stage/checksum" "$output.sha256"
printf '==> Ready: %s\n    Checksum: %s.sha256\n' "$output" "$output"
