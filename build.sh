#!/usr/bin/env bash
# Builds BOOTX64.efi and assembles it into a bootable ESP directory layout.
set -euo pipefail
cd "$(dirname "$0")"

echo "==> Building BOOTX64.efi (release, x86_64-unknown-uefi)"
if ! command -v rustup >/dev/null; then
    echo "error: rustup not found. This build needs nightly + rust-src (pinned in rust-toolchain.toml)," >&2
    echo "       which a distro-packaged cargo cannot provide. Install: https://rustup.rs" >&2
    exit 1
fi
# rust-toolchain.toml selects nightly, the UEFI target and rust-src automatically.
cargo build --release --target x86_64-unknown-uefi

EFI_BIN="target/x86_64-unknown-uefi/release/BOOTX64.efi"
if [[ ! -f "$EFI_BIN" ]]; then
    echo "error: build succeeded but $EFI_BIN was not produced" >&2
    exit 1
fi

echo "==> Assembling ESP at ./esp"
mkdir -p esp/EFI/BOOT
cp "$EFI_BIN" esp/EFI/BOOT/BOOTX64.EFI

echo "==> Done: esp/EFI/BOOT/BOOTX64.EFI ($(du -h "$EFI_BIN" | cut -f1) bytes)"
