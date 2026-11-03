# WISP OS v1.2.0 verification record

This sprint began from commit `186a5d3` with existing uncommitted editor and
storage implementation work. That work was preserved. The initial working files
were copied to `/tmp/wisp-sprint-baseline-mjx4uuf2` before edits; the baseline
optimized UEFI build and 291 host tests passed. Eighteen improvements are tracked
in [../TODO.md](../TODO.md), with original failures, acceptance criteria and results.

## Build environment

- Linux 7.0.0-42-generic, x86_64.
- Pinned `nightly-2026-10-21`: rustc 1.101.0-nightly
  (`19833af7224424e8b53e288097d56b6af43d5bcf`, LLVM 23.1.4).
- Cargo 1.101.0-nightly (`3960fa69b`, 2026-10-12).
- Python 3.15.2; QEMU 10.2.1; xorriso 1.5.6.
- OVMF `/usr/share/ovmf/OVMF.fd`; all VM disks were disposable regular files.

Production build: `./build.sh`, optimized/LTO, locked dependencies,
`x86_64-unknown-uefi`, no fault-test features. The executable is 412,672 bytes.
Two clean directories built with `cargo build --release --target x86_64-unknown-uefi
--locked --offline` produced the same executable as the tested working tree:

```text
28c9dacc8ecbf6d41e93f392d8c8391553d63bdb7122fbdd7963f7558f2b1ff9  BOOTX64.EFI
```

## Host and adversarial checks

| Command/check | Result |
| --- | --- |
| `cargo test --manifest-path tests/host/Cargo.toml --release --locked` | 323 passed, 0 failed |
| `python3 -m unittest discover -s tests -p '*_test.py'` | 26 passed, 0 failed |
| AddressSanitizer, optimized Linux host target | 94 distinct tests passed across 15 relevant suites |
| `bash -n build.sh run.sh scripts/*.sh` | Passed |
| `git diff --check` | Passed |
| Locked registry package query against OSV | 16 versions queried, no matching advisories reported; [raw response](DEPENDENCY_AUDIT.json) |

AddressSanitizer was enabled with `RUSTFLAGS=-Zsanitizer=address` and a separate
`CARGO_TARGET_DIR`, covering graphics, keyboard, editor, projects, disk geometry,
PCI/USB/NVMe parsers, storage selection, disk layout, persistence/crash recovery,
malformed-value decoding and allocation fault injection. Initial LeakSanitizer
startup failed because the host's ptrace restrictions (`ptrace_scope=2`) prevent
its thread scan. Address checks were rerun with `ASAN_OPTIONS=detect_leaks=0`;
no host security setting was changed. Leak scanning and bare-metal kernel
sanitization are not claimed.

Adversarial coverage includes mutated mounted superblocks, corrupted record
checksums/generations, exhaustive existing crash-write subsets, zero-space
recovery refusal, failed storage reads, allocator failures followed by recovery,
input-size/token boundaries, combined AST paths and recursive destruction on a
640KiB stack, invalid assignment targets, Unicode input, framebuffer guard
words, extreme integer coordinates, and overlapping keyboard modifiers.
Deterministic seeds: parser `0x574953505f504152`, graphics `0x57495350`, ATA
capacity samples `0xa13c480019375bad`. The original failing conditions were
reproduced before their corrections; the tests retain exact behavior assertions.

Strict Clippy (`cargo clippy --release --target x86_64-unknown-uefi --locked
--offline -- -D warnings`, and the analogous host-crate command) fails on
46 kernel and 10 host-library style/documentation diagnostics. The same commands
against a reconstruction of the initial dirty workspace produced identical
messages and counts. No new diagnostic was introduced or suppressed. This
pre-existing lint debt remains; strict Clippy is not a passing release check.

## Integration and independent review

The VM matrix exercises shell/apps/graphics/cancellation and persistent maps,
AHCI absent/empty/foreign/legacy stores, NVMe 512-byte and native 4KiB geometry,
namespace 7 behind a PCI bridge, USB storage failure/reboot paths, ambiguous
selection, Studio with both PS/2 and USB keyboards, installation cancellation,
small-disk rejection, GPT/FAT validation, standalone boot and persistent saves.
The CPU-fault suite checks breakpoint/page faults, mainline and double-fault
stacks, read-only code and NX heap protection with isolated feature-test binaries.
The production executable does not contain those test hooks.

Executed integration commands (each exited successfully):

| Invocation | Coverage |
| --- | --- |
| `python3 tests/acceptance_test.py` | 24 shell/app/graphics/REPL steps and independent persistence reboot |
| `python3 tests/ahci_test.py` | 5 AHCI absence/error/persistence/foreign/legacy scenarios |
| `python3 tests/nvme_test.py --skip-build` | 5 NVMe scenarios, including namespace 7, bridge and native 4KiB media |
| `python3 tests/usb_storage_test.py --skip-build` | 5 USB transport/failure/boot/reboot scenarios |
| `python3 tests/storage_selection_test.py --skip-build` | Ambiguous disks, explicit choice, isolated stores and unchanged reads |
| `python3 tests/ide_test.py --skip-build` | PS/2 editing, execution, persistence and original parser-failure recovery |
| `python3 tests/ide_test.py --skip-build --usb` | Same workflow with USB HID input |
| `python3 tests/installer_test.py --skip-build` | Cancellation, small-disk rejection, installation, standalone boot and reboot |
| `python3 tests/usb_test.py` | 4 APIC/xHCI/input/cancellation scenarios |
| `python3 tests/fault_test.py` | 6 CPU fault/protection scenarios |
| Extracted runtime-package smoke | Version, bundled hello, Studio syntax and REPL output |

A separate reviewer traced the language changes and another reviewed storage
cleanup/commit/recovery ordering. Release review also examined framebuffer
bounds, modifier state, installer target validation and archive staging. It
found that raw EFI/ISO packaging initially reread live files after snapshotting;
that was corrected, and a fault-injection test now changes the live binary during
packaging while asserting that released binaries remain from one snapshot.
No release-blocking finding remains in those reviewed paths.

Package tests verify runtime exclusions, source ZIP/tar parity, per-file and
asset checksums, executable script permissions, repeated identical packaging,
invalid timestamps, failure preservation and snapshot consistency. ISO tests
compare repeated hashes despite elapsed time and changed source permissions and
mtimes, and inspect the UEFI El Torito entry. Release artifacts are regenerated
from the committed tree; `BUILD_INFO.json` records that commit and the final
binary/source hashes. The obsolete tracked root checksum file (36 mismatches)
was removed; complete manifests are generated inside archives and in `dist/`.

Physical hardware and macOS/Windows installer execution are unverified. Native
4KiB-sector persistence remains read-only. See [RELEASE_NOTES.md](RELEASE_NOTES.md)
for distribution and supported-platform limitations.
