# WISP OS engineering sprint

Status flow: `PENDING → IN_PROGRESS → VERIFIED`; `BLOCKED` requires an external dependency.

Reconnaissance: x86_64 UEFI/no_std Rust kernel; pre-exit firmware filesystem snapshot and installer; post-exit owned page tables/allocators/interrupts, native AHCI/NVMe/USB storage, framebuffer shell and WISP interpreter. Existing uncommitted editor/storage work is preserved and included in verification. Initial HEAD: `186a5d3`. Baseline optimized kernel build and host regression suite pass.

Ordering weighs data loss/kernel termination first, then reachable correctness and build/verification reliability. Independent work may proceed concurrently; dependencies govern completion. No performance improvement is claimed without measurements.

Final result: **18 identified, 18 implemented and verified, 0 blocked or incomplete sprint improvements**. Full host suite: 323 passing tests; Python tooling: 26; AddressSanitizer: 94 distinct tests. All applicable QEMU suites and extracted-runtime startup passed. See [verification record](docs/SPRINT_VERIFICATION.md) for reproducibility evidence, independent review and explicitly retained tooling/platform limitations. GitHub publication is the final distribution step, performed after this ledger is committed.

## S16 — P0 / high
- Components: tests/acceptance_test.py and related QEMU harness output directories
- Problem/evidence: acceptance_test.py:428 recursively deletes the caller-supplied output directory; a typo can erase unrelated user files.
- Implementation: create owned per-run scratch directories without deleting caller paths; inspect other harness cleanup paths.
- Acceptance: pre-existing sentinel files survive reruns and explicit output paths; isolated run artifacts remain available.
- Required tests: temporary directory sentinel/rerun tests and full VM suites.
- Dependencies: None
- Status: VERIFIED
- Verification: `python3 tests/test_support_test.py` passed 3 tests (preservation of sentinel/previous run, unique empty children, file/symlink refusal); acceptance/USB/fault runs use fresh children and retain prior artifacts. No harness calls `shutil.rmtree` remain.

## S17 — P0 / high
- Components: scripts/install_usb_linux.sh
- Problem/evidence: installer validates only block-device existence before destructive partitioning; --force accepts partition/mapper targets and can write nested GPT before discovery fails.
- Implementation: verify an unambiguous whole-disk lsblk type before destructive operations; reject partitions and device-mapper nodes even with --force.
- Acceptance: unsupported targets produce an error before wipefs/parted/dd/mkfs; whole disks retain existing confirmations and checks.
- Required tests: stubbed command harness covering disk/partition/mapper/unknown types and ensuring no destructive command executes on rejection.
- Dependencies: None
- Status: VERIFIED
- Verification: 6 isolated Linux installer tests pass; partition/mapper/loop/unknown types and discovery failure reject before mutations, exact whole-disk classification succeeds, invalid argument cases and partition naming pass. Bash syntax check passes. Independent release reviewer inspected the gate, canonical paths, recheck and removal of glob-based unmounts. No physical media writes were performed.

## S01 — P0 / high
- Components: src/save_format.rs
- Problem/evidence: save_in reclaims slots before ensure_current_version; delete_in does not revalidate the mounted superblock. Changed media can be written after mount.
- Implementation: Validate supported media metadata read-only before mounted operations and any cleanup.
- Acceptance: Foreign or unsupported superblocks yield errors with zero writes and byte-identical media.
- Required tests: Mutate superblock after mount; save/delete/load/list must refuse safely.
- Dependencies: None
- Status: VERIFIED
- Verification: Original regression failed (save wrote one stale-slot sector after the superblock changed). Fixed and passed changed-superblock save/delete/nil-save/load/keys regression; media and pending cache remain unchanged. Host save-format/partition/geometry/regression suites pass (74 tests across these suites).

## S02 — P0 / high
- Components: src/save_format.rs
- Problem/evidence: reclaim_stale selects generations from unchecked slot headers; a corrupt stale generation can cause the valid copy to be freed.
- Implementation: Validate candidate records before deciding reclamation winners; retain explicit targeted recovery behavior.
- Acceptance: An unrelated save cannot destroy the valid value when another record is corrupt.
- Required tests: Corrupt stale generation/checksum, unrelated writes, power-loss recovery regressions.
- Dependencies: S01
- Status: VERIFIED
- Verification: Original corrupt-generation/unrelated-save regression failed. All 25 save_format_v2 tests now pass, including exhaustive crash subsets, preserved valid copies before replacement commit, no-space recovery refusal and injected winner-read failure with zero writes. Existing corruption overwrite recovery remains supported.

## S03 — P0 / high
- Components: src/wisp/parser.rs
- Problem/evidence: items[0]() = 1; reaches unreachable! in parse_index_assign_or_expr_stmt, aborting the kernel.
- Implementation: Return a source error for every invalid assignment target.
- Acceptance: Malformed assignments never panic and preserve line diagnostics.
- Required tests: Regression for call-after-index assignment and malformed target matrix.
- Dependencies: None
- Status: VERIFIED
- Verification: Verified malformed-target matrix with source-line assertions and 4000 deterministic malformed token sequences (seed 0x574953505f504152). Original fatal input now reports an error through Studio F6/F5 and the interactive prompt in QEMU with PS/2 and USB keyboards; editor source/session remain usable.

## S14 — P0 / high
- Components: src/wisp/parser.rs; AST lifecycle
- Problem/evidence: Sixty nested parenthesized expressions each with sixty postfix indices parse into depth 3600 despite MAX_SYNTAX_DEPTH=128; recursive traversal/drop can overflow the kernel stack.
- Implementation: Track constructed AST depth across combined grammar paths and reject excessive nesting before constructing unsafe trees.
- Acceptance: All accepted AST paths remain within the documented bound; reject reproducer safely.
- Required tests: Combined postfix/parenthesis/binary nesting and constrained-stack parse/drop tests.
- Dependencies: S03
- Status: VERIFIED
- Verification: Verified by constrained 640KiB-stack regressions for combined postfix/group/binary/lambda/list/map trees, accepted-tree destruction, and the unchanged exact 127-map-depth boundary. Original 3600-depth reproducer now returns a source error. Independent storage-agent review passed; full host suite passed.

## S04 — P1 / high
- Components: src/ahci.rs
- Problem/evidence: IDENTIFY capacity accepts zero or more than 48 address bits; read/write do not bound LBA before command encoding truncates it.
- Implementation: Validate ATA geometry and reject out-of-range requests before DMA/command issue.
- Acceptance: Zero/oversized capacity rejected; LBA equal to capacity and 2^48 cannot wrap.
- Required tests: Host geometry and boundary tests; AHCI QEMU persistence regression.
- Dependencies: None
- Status: VERIFIED
- Verification: Three host geometry tests pass, covering zero/oversized capacities, exact LBA48 boundaries and 10,000 deterministic samples (seed 0xa13c480019375bad). Optimized x86_64 UEFI build passed; all five AHCI QEMU scenarios passed (no controller, empty controller, full persistence stack, untouched foreign disk and legacy-v1 upgrade). Artifacts: tests/out_ahci/run-sm0l7isv; full log: /tmp/wisp-sprint-ahci.log. Final v1.2.0 AHCI suite passed all 5 scenarios (`/tmp/wisp-final-ahci.log`, `tests/out_ahci/run-d7eyneez`); its isolated EFI matches the final production SHA-256 `28c9dacc8ecbf6d41e93f392d8c8391553d63bdb7122fbdd7963f7558f2b1ff9`.

## S05 — P1 / high
- Components: src/save_format.rs value decoder
- Problem/evidence: List/map lengths are checked against fixed slot size rather than remaining bytes; nested malformed lengths allocate disproportionately before rejection.
- Implementation: Bound counts by minimum encoded item size in remaining payload and reserve fallibly.
- Acceptance: Malformed tiny payloads cannot reserve large nested containers; valid format remains compatible.
- Required tests: Truncated/count-limit/nested payload cases and full save-format suite.
- Dependencies: S01
- Status: VERIFIED
- Verification: All four storage_decode tests pass, including malformed counts, 63 nested list headers, valid dense/empty containers and four repeated injected list-allocation failures followed by successful reads. Measured allocations dropped from 68,096 to 4,096 bytes for a five-byte malformed list and from 262,704 to 12,848 bytes for a 571-byte nested malformed payload. Four repeated injected list-allocation failures also recover on subsequent reads. Full affected save/partition/geometry and general regression tests pass.

## S06 — P1 / high
- Components: src/gfx.rs; console initialization and primitives
- Problem/evidence: Console::new clears stride*height pixels without checking fb_len, stride>=width, pointer alignment or multiplication overflow; draw_line wraps endpoint differences before its work bound.
- Implementation: Validate framebuffer geometry before writes and use nonwrapping coordinate magnitudes; correct rectangle conversion at usize extremes.
- Acceptance: Invalid buffers fail before writes; extreme line coordinates return promptly; valid padded buffers retain guards.
- Required tests: Canary buffers, null/alignment/length/overflow cases, endpoint extremes and drawing invariants.
- Dependencies: None
- Status: VERIFIED
- Verification: Original short-framebuffer regression failed before fix (/tmp/wisp-gfx-before.log). All 5 production-renderer host tests pass, including invalid geometry, extreme coordinates, padded guard buffers and 2000 randomized cases (seed 0x57495350).

## S18 — P1 / high
- Components: src/wisp/lexer.rs
- Problem/evidence: lexer materializes all tokens with infallible Vec growth; admitted 256KiB dense source can require the entire 8MiB kernel heap before parser rejection.
- Implementation: enforce a documented token budget and fallible capacity growth while preserving meaningful diagnostics and valid programs.
- Acceptance: dense maximum-sized sources return bounded resource errors; no allocation abort; ordinary applications remain compatible.
- Required tests: token limits, dense oversized input, repeated rejection/recovery and full language corpus.
- Dependencies: S14
- Status: VERIFIED
- Verification: Verified exact 32768-token and 256KiB-source boundaries, 256KiB dense input rejection repeated ten times, recovery, and eight injected token-vector allocation failures. Token vector remains at most 32768 entries/768KiB on x86_64. Language corpus and full host suite passed. Two existing oversized syntax cases were resized to stay below the new token budget (8000 parenthesis pairs, 15000 binary operators); their original depth-error assertions remain unchanged, and separate tests cover larger token streams.

## S15 — P1 / high
- Components: src/shell.rs interactive WISP input
- Problem/evidence: Unfinished multiline REPL input grows source indefinitely despite a 1KiB per-line cap; repeated incomplete blocks can exhaust the 8MiB heap.
- Implementation: Bound aggregate interactive source with fallible, atomic appends and recover to a fresh prompt after refusal.
- Acceptance: Exact limit accepted; excessive input refused without partial state or kernel allocation abort.
- Required tests: Boundary append/recovery tests and interactive QEMU regression.
- Dependencies: S14
- Status: VERIFIED
- Verification: Verified exact 16KiB aggregate boundary including newlines, atomic over-limit and UTF-8-byte rejection, injected append allocation failure with original input preserved, reset/reuse and successful parsing after recovery. QEMU PS/2 and USB prompt/Studio lifecycle and parser-error recovery passed. No 16KiB keyboard injection was attempted; the exact cap is asserted in host tests.

## S07 — P2 / medium
- Components: src/wisp/parser.rs
- Problem/evidence: items[0] + 1; and equality/logical expression statements fail because the index-assignment branch parses only a postfix expression.
- Implementation: Parse the full expression while preserving valid indexed assignment semantics.
- Acceptance: All expression operators work after index/slice/call prefixes; index writes remain valid.
- Required tests: Arithmetic, comparison, logical and chained indexed expression regressions.
- Dependencies: S03,S14
- Status: VERIFIED
- Verification: Verified indexed arithmetic/comparison/logical/slice/call expression statements and precedence, plus valid indexed assignments, in host tests. QEMU PS/2 and USB prompt both evaluate indexed expressions correctly and recover after invalid input.

## S08 — P2 / medium
- Components: src/wisp/mod.rs parse_interactive
- Problem/evidence: print(1) # comment fails because the implicit semicolon is appended inside the trailing comment.
- Implementation: Insert missing terminator in the token stream or beyond the comment without losing source locations.
- Acceptance: Omitted semicolons work with trailing comments; genuine syntax errors retain original diagnostics.
- Required tests: Comment-only, trailing comment, strings containing #, multiline and invalid-input tests.
- Dependencies: S07
- Status: VERIFIED
- Verification: Verified trailing comments (including comments ending in semicolons), multiline/comment-only input, literal # characters and unchanged syntax-error lines/messages. QEMU PS/2 and USB prompt evaluate an unterminated-by-semicolon indexed expression with a trailing comment.

## S09 — P2 / medium
- Components: src/keyboard.rs PS/2 decoder
- Problem/evidence: One shift_held boolean represents both Shift keys; releasing one key clears shift while the other is still held.
- Implementation: Track left/right modifier states independently in the decoded input state.
- Acceptance: Overlapping Shift presses preserve uppercase until both keys release.
- Required tests: Both release orders, repeat makes, extended fake shifts, function keys and queue behavior.
- Dependencies: None
- Status: VERIFIED
- Verification: Original overlap sequence emitted lowercase a while one Shift remained held (/tmp/wisp-keyboard-before.log). All 4 decoder/queue tests now pass, both Shift release orders, repeat/fake shifts, navigation/function keys and queue saturation.

## S10 — P2 / medium
- Components: rust-toolchain.toml; build.sh; build entry points
- Problem/evidence: Toolchain selects floating nightly and build.sh omits --locked, so the same source can resolve different compiler/dependency inputs.
- Implementation: Pin an available compiler date, enforce lockfile use, record exact build versions.
- Acceptance: Clean release builds use the recorded compiler and locked dependencies.
- Required tests: Release build, host suite, fresh-directory binary hash comparison.
- Dependencies: None
- Status: VERIFIED
- Verification: pinned nightly-2026-10-21 installed; locked release build and 321 host tests pass. Two clean offline builds in distinct directories match the final EFI byte-for-byte: 412672 bytes, SHA-256 `28c9dacc8ecbf6d41e93f392d8c8391553d63bdb7122fbdd7963f7558f2b1ff9` (`/tmp/wisp-repro-report.json`).

## S11 — P2 / medium
- Components: scripts/make_iso.sh
- Problem/evidence: Two ISO builds with SOURCE_DATE_EPOCH=1700000000 produced different SHA-256 hashes; timestamps and ISO metadata ignore the supplied epoch.
- Implementation: Normalize ISO/FAT inputs and mastering timestamps deterministically; validate epoch.
- Acceptance: Identical inputs and epoch produce byte-identical bootable ISOs.
- Required tests: Repeated hash comparison, bad epoch validation, ISO inspection and installer boot.
- Dependencies: S10
- Status: VERIFIED
- Verification: `python3 tests/release_test.py` passed reproducibility/checksum/UEFI-entry checks with changed source mtimes and permissions plus five invalid epochs preserving old outputs; `tests/iso_test.py` passed 8 tests. Final v1.2.0 installer integration passed cancellation, GPT/FAT/file-byte validation, standalone boot and persistence (`tests/out_installer/run-nv3ooewr`).

## S12 — P2 / medium
- Components: tests/acceptance_test.py; legacy VM harnesses
- Problem/evidence: Older acceptance/USB harnesses require missing tesseract/Pillow despite an existing exact bitmap font decoder; OCR-specific assertions have known recognition errors.
- Implementation: Reuse exact font decoding and standard-library framebuffer reads without weakening behavior/color assertions.
- Acceptance: Full applicable QEMU suites run with standard Python and enforce exact intended behavior.
- Required tests: Decoder unit tests, acceptance/USB/fault/AHCI/IDE/storage/installer end-to-end suites.
- Dependencies: S03,S04,S06,S07,S08,S09,S14,S15
- Status: VERIFIED
- Verification: Final v1.2.0 full acceptance (24 steps plus separate reboot/persistence), USB/APIC (4 scenarios), fault (6), AHCI (5), both IDE keyboard modes, NVMe (5), USB storage (5), disk selection/reboot, and live installer suites passed. Exact-font/color decoder tests passed 3 cases. Final direct-run logs: `/tmp/wisp-final-{acceptance,usb,fault,ahci}.log`; artifacts: `tests/out/run-elhsqaqf`, `tests/out_usb/run-lh4dlbm6`, `tests/out_fault/run-gujpk2bt`, `tests/out_ahci/run-d7eyneez`. Packaged-runtime boot, bundled app, IDE syntax check and REPL expression/comment smoke also passed.

## S13 — P2 / medium
- Components: scripts/make_release.sh; release documentation
- Problem/evidence: Current release packages mix full development sources with EFI and provide no minimal runtime archive or build provenance file; archives are removed before replacement generation succeeds. The tracked root SHA256SUMS is stale after source changes; only generated package manifests can describe a complete release.
- Implementation: Retain source archives, stage replacements before publishing, add minimal runtime distribution and raw EFI checksums, record environment/version and release verification.
- Acceptance: Runtime package contains only required binary/apps/user docs; all artifacts have verified SHA-256 and provenance.
- Required tests: Archive content/checksum inspection, reproducible packaging, packaged EFI/ISO startup. GitHub publication is a separate final sprint gate.
- Dependencies: S01-S12,S14,S15
- Status: VERIFIED
- Verification: 4 package tests pass (runtime contents and all checksums, byte-identical repeated archives, invalid timestamps and failed compressor preserve prior artifacts, changed live EFI cannot mix snapshots). Generated source ZIP/tar payloads match and all apps/EFI match checkout. Extracted runtime booted in QEMU and passed version, hello, Studio syntax check and indexed/comment REPL checks. EFI SHA-256 is 28c9dacc8ecbf6d41e93f392d8c8391553d63bdb7122fbdd7963f7558f2b1ff9. Final committed-tree packages and release assets are rechecked during publication. Stale root manifest removed after confirming 36 mismatches.
