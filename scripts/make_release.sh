#!/usr/bin/env bash
# Builds BOOTX64.EFI and packages a WISP OS release as both .tar.gz and
# .zip under ./dist, each containing:
#
#   wisp-os-v<version>/
#     README.md, CHANGES.md, USB_INSTALL.md, Cargo.toml, Cargo.lock, ...
#     src/ docs/ scripts/ tests/*.py tools/     the full source
#     esp/APPS/*.wisp                           the bundled programs
#     esp/EFI/BOOT/BOOTX64.EFI                  prebuilt, ready to copy to a stick
#     SHA256SUMS                                checksums of every file above
#
# plus dist/SHA256SUMS covering the archives and optional live ISO.
#
# The version comes from Cargo.toml, the single source of truth that the
# kernel banner and `version` command also read (via CARGO_PKG_VERSION), so
# the archive name can never disagree with what the binary reports.
#
# Archives are reproducible for a given tree and SOURCE_DATE_EPOCH: entries
# are sorted, owners zeroed and every timestamp pinned to SOURCE_DATE_EPOCH
# (default: the newest modification time among the packaged files, so an
# unchanged tree repackages to byte-identical archives).
#
# Usage: scripts/make_release.sh [--skip-build] [--iso]
#   --skip-build   package the BOOTX64.EFI already in esp/EFI/BOOT instead
#                  of rebuilding (it must exist).
#   --iso          also build the live ISO (needs xorriso).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SKIP_BUILD=0
INCLUDE_ISO=0
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=1 ;;
        --iso) INCLUDE_ISO=1 ;;
        -h|--help)
            sed -n '2,27p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown argument '$arg' (try --help)" >&2
            exit 2
            ;;
    esac
done

if [[ "$INCLUDE_ISO" -eq 1 ]] && ! command -v xorriso >/dev/null 2>&1; then
    printf 'error: --iso needs xorriso\n' >&2
    exit 1
fi

for tool in tar gzip sha256sum python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: '$tool' is required but not installed" >&2
        exit 1
    fi
done

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
    echo "error: could not read the package version from Cargo.toml" >&2
    exit 1
fi
NAME="wisp-os-v${VERSION}"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    ./build.sh
fi
EFI="esp/EFI/BOOT/BOOTX64.EFI"
if [[ ! -f "$EFI" ]]; then
    echo "error: $EFI not found (run ./build.sh, or drop --skip-build)" >&2
    exit 1
fi

# Everything that is source or documentation, and nothing that is build
# output, test scratch, local save disks or caches.
FILES=()
for f in .gitignore .cargo/config.toml Cargo.toml Cargo.lock rust-toolchain.toml \
         build.sh run.sh README.md CHANGES.md USB_INSTALL.md; do
    [[ -f "$f" ]] || { echo "error: expected file '$f' is missing" >&2; exit 1; }
    FILES+=("$f")
done
while IFS= read -r -d '' f; do
    FILES+=("${f#./}")
done < <(find ./src ./docs ./scripts ./tools ./esp/APPS -type f \
            -not -path '*/__pycache__/*' -not -name '*.pyc' -print0)
while IFS= read -r -d '' f; do
    FILES+=("${f#./}")
done < <(find ./tests -maxdepth 1 -type f -name '*.py' -print0)
# Ship the host test crate too; source archives should run the same checks
# as a checkout, without generated target/ trees or test output images.
while IFS= read -r -d '' f; do
    FILES+=("${f#./}")
done < <(find ./tests/host -type f -not -path './tests/host/target/*' -print0)
FILES+=("$EFI")

DIST="$ROOT/dist"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/$NAME" "$DIST"

for f in "${FILES[@]}"; do
    mkdir -p "$STAGE/$NAME/$(dirname "$f")"
    cp -p "$f" "$STAGE/$NAME/$f"
done

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    SOURCE_DATE_EPOCH="$(find "$STAGE/$NAME" -type f -printf '%T@\n' | sort -n | tail -n1 | cut -d. -f1)"
fi
export SOURCE_DATE_EPOCH

(
    cd "$STAGE/$NAME"
    find . -type f -not -name SHA256SUMS -printf '%P\n' | LC_ALL=C sort | xargs -d '\n' sha256sum > SHA256SUMS
)

rm -f "$DIST/$NAME.tar.gz" "$DIST/$NAME.zip"

tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    --mode='u+rw,go+r,go-w' -C "$STAGE" -cf - "$NAME" | gzip -n -9 > "$DIST/$NAME.tar.gz"

# zip via Python's standard library, so no `zip` binary is required. Entries
# are written in sorted order with a fixed timestamp, and executable files
# keep their mode bits (scripts stay runnable after unzipping on Unix).
python3 - "$STAGE" "$NAME" "$DIST/$NAME.zip" "$SOURCE_DATE_EPOCH" <<'PY'
import os
import sys
import time
import zipfile

stage, name, out, epoch = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
# ZIP timestamps cannot predate 1980.
stamp = time.gmtime(max(epoch, 315532800))[:6]
root = os.path.join(stage, name)
paths = []
for dirpath, _dirs, files in os.walk(root):
    for f in files:
        paths.append(os.path.join(dirpath, f))
paths.sort(key=lambda p: os.path.relpath(p, stage))
with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for path in paths:
        arc = os.path.relpath(path, stage).replace(os.sep, "/")
        info = zipfile.ZipInfo(arc, date_time=stamp)
        mode = 0o755 if os.access(path, os.X_OK) else 0o644
        info.external_attr = (0o100000 | mode) << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        info.create_system = 3  # Unix, so the mode bits above are honoured
        with open(path, "rb") as fh:
            z.writestr(info, fh.read(), compresslevel=9)
PY

if [[ "$INCLUDE_ISO" -eq 1 ]]; then
    scripts/make_iso.sh --skip-build
fi

(
    cd "$DIST"
    artifacts=("$NAME.tar.gz" "$NAME.zip")
    if [[ "$INCLUDE_ISO" -eq 1 ]]; then artifacts+=("$NAME-live.iso"); fi
    sha256sum "${artifacts[@]}" > SHA256SUMS
)

echo "==> Release $NAME"
echo "    $(du -h "$DIST/$NAME.tar.gz" | cut -f1)  dist/$NAME.tar.gz"
echo "    $(du -h "$DIST/$NAME.zip" | cut -f1)  dist/$NAME.zip"
if [[ "$INCLUDE_ISO" -eq 1 ]]; then echo "    dist/$NAME-live.iso"; fi
echo "    dist/SHA256SUMS:"
sed 's/^/      /' "$DIST/SHA256SUMS"
