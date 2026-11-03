# WISP OS v1.2.0

Release engineering sprint, 2026-11-03. Target: x86_64 UEFI.

## Changes

- Integrates Wisp Studio editing, syntax checks, execution and persisted programs,
  native NVMe and USB mass storage, and explicit save-disk selection. Existing
  work in these components was preserved, audited and regression-tested.
- Protects save media against changed superblocks and corrupt generation headers.
  Corrupt keys can be explicitly replaced without reclaiming their valid older
  copies before the replacement commits. ATA geometry and request addresses are
  checked before issuing DMA commands.
- Removes a kernel panic from invalid indexed-call assignments, fixes indexed
  expression statements and REPL comments, and bounds constructed syntax depth,
  token allocation and accumulated interactive source. Errors leave a usable prompt.
- Validates framebuffer mappings before drawing, prevents extreme-coordinate
  wraparound, and tracks both PS/2 Shift keys independently.
- Refuses partition/mapper targets in the Linux USB installer, preserves test
  output directories, replaces OCR dependencies with exact bitmap decoding, and
  pins locked builds to nightly-2026-10-21.
- Adds deterministic live ISOs, minimal runtime packages and build provenance.
  Packaging uses a common source snapshot, stages complete results before replacing
  local artifacts, and includes SHA-256 manifests.

The architectural boundaries remain: firmware access ends before kernel handoff;
editor state and language logic are tested independently from framebuffer/input;
storage transports share the sector interface and persistence format. No network
stack was added.

## Measured resource improvement

The same optimized allocation-tracking tests measured malformed save-value decoding:

| Payload | Before | After |
| --- | ---: | ---: |
| 5-byte invalid list | 68,096 allocated bytes | 4,096 allocated bytes |
| 571-byte nested invalid value | 262,704 allocated bytes | 12,848 allocated bytes |

These are cumulative allocation measurements for malformed input, not throughput
benchmarks. No runtime speedup is claimed.

## Verification

Detailed commands, results and independent review evidence are recorded in
[SPRINT_VERIFICATION.md](https://github.com/recursive-ai-dev/wisp-os/blob/v1.2.0/docs/SPRINT_VERIFICATION.md)
and [TODO.md](https://github.com/recursive-ai-dev/wisp-os/blob/v1.2.0/TODO.md).
The production binary uses optimized LTO with all fault-test features disabled.
The final host suite passed 323 tests, Python tooling passed 26 tests, and 94
distinct host tests passed AddressSanitizer checks. All applicable QEMU suites
and an extracted-runtime startup check passed.
The lockfile's 16 registry package versions were checked against the OSV advisory
API on 2026-11-03; the response reported no matching advisories. This does not
constitute a proof that dependencies contain no vulnerabilities.

## Distribution

- `wisp-os-v1.2.0-BOOTX64.EFI`: standalone executable; install as `EFI/BOOT/BOOTX64.EFI`.
- `wisp-os-v1.2.0-runtime.zip`: EFI, bundled apps, user documentation and USB installers.
- `wisp-os-v1.2.0-live.iso`: UEFI live environment and installer.
- `wisp-os-v1.2.0.tar.gz` and `.zip`: source plus prebuilt EFI for development/rebuilding.
- `wisp-os-v1.2.0-BUILD_INFO.json` and `SHA256SUMS`: source/compiler identity and checksums.

Verify downloaded assets with `sha256sum -c SHA256SUMS`. The archives contain
additional per-file checksums. Builds and packages are reproducible with the same
source, toolchain and `SOURCE_DATE_EPOCH` in the recorded environment.

## Limitations

Validation is on Linux x86_64 with QEMU/OVMF; physical hardware, macOS and Windows
installer execution were not tested in this sprint. Only x86_64 UEFI is supported;
there are no ARM or legacy BIOS binaries. EFI is unsigned, so Secure Boot requires
a separately trusted signature or suitable firmware configuration.

Strict Clippy retains 46 kernel and 10 host-library diagnostics reproduced from
the original workspace; no new diagnostics were introduced. LeakSanitizer could
not scan threads under the host ptrace policy; AddressSanitizer ran with leak
detection disabled and without changing host security settings.

Native 4KiB-sector save media remains read-only because the 512-byte commit format
cannot guarantee atomic writes there. Controller/device coverage remains the
existing bounded implementation; QEMU coverage is not a guarantee for every real
USB/NVMe controller. Studio's 16KiB edit buffer still exceeds the 4,042-byte source
capacity of one persisted program slot. See USB_INSTALL.md and README.md for the
supported media and boot paths.
