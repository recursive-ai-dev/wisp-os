# Codebase Quality Audit

## Executive Summary

**Overall risk: unsafe to boot with valuable SATA disks attached; significant kernel-wide reliability risks remain even in a disposable VM.** The supplied WISP OS v1.0.0 snapshot contains **13 findings: 1 Critical, 10 High, and 2 Medium**. The most urgent problems are destructive storage behavior and failure to isolate application or hardware errors from the rest of the kernel. This is a correctness and user-impact assessment, not a claim of demonstrated remote exploitation.

The top three issues are:

1. **Unrequested disk destruction during boot — AUD-01.** Boot selects the first usable device on the discovered AHCI controller and formats it when its first sector lacks WISP's magic. No explicit device selection, ownership check, or confirmation protects existing data. Formatting replaces LBA 0–512, including the disk's partition-table area. The separate boot self-test also writes the disk's final sector; its error-path damage is AUD-03.
2. **A timed-out disk operation does not relinquish DMA ownership — AUD-02.** The driver returns an error while its command can remain active, then permits later operations to overwrite the same command table and data buffer. This can turn a recoverable device failure into data corruption or incorrect completion reporting.
3. **Tiny list graphs can exhaust the entire kernel heap — AUD-08.** Serialization checks its 4,055-byte output limit only after constructing the complete encoding. Display has no output/work limit. A 23-list shared graph implies approximately 79.7 MB of serialized output or 29.4 MB of display output against an 8 MiB kernel heap. Function-call and value-depth limits do not prevent this.

**Immediate containment:** use only disposable virtual disks, keep production data disconnected from physical test machines, and do not treat a successful `save` message as a durability guarantee until AUD-01–04 are addressed. Increasing the heap or timeout values is not a substitute for the identified fixes.

### Scope and audit integrity

Audited input: `Archive.zip`, SHA-256:

```text
92b324800b8ab196768bb2d8a3d97d1f7b73ca45494ee36730fb78f8cf9cf84a
```

The archive contains 67 files. Paths and line numbers below are relative to its extracted root, not to a moving repository branch. No Git revision was supplied. Source contents and the uploaded archive were not modified; all 67 extracted file hashes matched the initial manifest after verification. The report and independent audit evidence were created outside the source tree.

The deep trace covered boot and controller initialization, memory ownership/allocation, interrupt-controller transition, application ingestion, shell dispatch, lexer/parser/interpreter behavior, list operations, AHCI persistence, and xHCI/HID input. Build configuration, installation scripts, and available test harnesses were inspected for context. This is not a claim that every path in every file has been exhaustively verified.

### Runtime and verification map

| Area | Entry points / configuration | Audit relevance |
|---|---|---|
| Kernel | `Cargo.toml:7–17`; `src/main.rs:72–284,369–528` | `BOOTX64`, a freestanding UEFI binary; firmware exit, memory setup, controller initialization, then shell. |
| Build | `rust-toolchain.toml:1–4`; `Cargo.toml:26–61`; `build.sh:7–17` | Rolling nightly; `x86_64-unknown-uefi`; pinned `uefi = 0.30.0`; aborting panic profiles; release LTO. |
| Application execution | `src/fs.rs:81–102`; `src/shell.rs:504–550`; `src/wisp/mod.rs:53–56` | All application sources are snapshotted before firmware exit; `run` clones the selected source, then lexes, parses, and interprets it. |
| Storage | `src/main.rs:676–769`; `src/ahci.rs`; `src/save_format.rs` | Automatic device acquisition, destructive self-test, format/mount, then shell and WISP save/load. |
| Input | `src/main.rs:545–648`; `src/xhci.rs:727–768,1427–1516`; `src/usb_hid.rs:157–213` | PIC-to-APIC handover and asynchronous USB completion processing. |
| Integration tests | `tests/acceptance_test.py`; `tests/ahci_test.py`; `tests/fault_test.py`; `tests/usb_test.py` | QEMU/OVMF integration harnesses; hardware absence, normal persistence, deliberate kernel faults, and normal USB input are represented. |

Available commands discovered, **not executed in this audit**:

```sh
# Build writes generated output, including the source tree's esp directory.
./build.sh

# Run only in a separate disposable working copy with disposable disk images.
# Each harness deletes its output directory at startup: supply a NEW, dedicated
# directory, never the repository root or a directory containing valuable files.
python3 tests/acceptance_test.py /tmp/wisp-audit-acceptance
python3 tests/ahci_test.py       /tmp/wisp-audit-ahci
python3 tests/fault_test.py      /tmp/wisp-audit-fault
python3 tests/usb_test.py        /tmp/wisp-audit-usb
```

`rustc`, `cargo`, `rustup`, and `qemu-system-x86_64` were unavailable. **No compilation, Rust unit test, kernel boot, or hardware fault-injection test is claimed.** Five shell files passed `bash -n`; five Python files passed `ast.parse`. These are syntax checks, not functional tests. Seven bounded Python models checked sector ordering, interrupted initialization, self-test cleanup, graph-size arithmetic, an event-ring interleaving, USB completion dispatch, and UTF-8 conversion. Their results are recorded in `AUDIT_EVIDENCE.json`.

Evidence labels used below: **S** = source/control-flow or mathematical proof; **M** = independently executed Python model of the relevant operations, not execution of the Rust implementation. Hardware-dependent outcomes are stated with their triggering conditions, not presented as observed failures on a physical device.

## Findings

Rows are ordered by severity, then destructive impact, breadth of failure, and trigger accessibility. Local script findings require loading/running a script; controller findings require the stated hardware event. No finding depends on an exposed network service.

| ID | Severity | Location | Trigger | Impact | Evidence |
|---|---|---|---|---|---|
| AUD-01 | **Critical** | `src/main.rs:736–754`; `src/ahci.rs:291–405,427–434`; `src/save_format.rs:282–309` | Boot with a usable AHCI disk whose LBA 0 lacks WISP magic | Unrequested replacement of the disk's first 513 sectors | **S+M:** automatic call chain; no ownership/blankness check; model replaced exactly LBA 0–512. |
| AUD-02 | **High** | `src/ahci.rs:564–618,766–790`; `src/shell.rs:423–440,747–758` | An ATA command times out, followed by another operation | In-flight DMA structures/data are overwritten; corruption or false completion is possible | **S:** timeout returns without quiescing/offlining; later writes reuse the same buffer and slot. |
| AUD-03 | **High** | `src/ahci.rs:816–850`; `src/main.rs:736–748` | Readback fails/mismatches after the self-test pattern write, or power is lost before restoration | Original final-sector contents can remain destroyed | **S+M:** restoration is after early-return branches; injected readback failure left the pattern in place. |
| AUD-04 | **High** | `src/ahci.rs:135–139,654–710,766–792`; `src/save_format.rs:121–130,314–329`; `src/shell.rs:433–435` | Successful save followed by power loss while a device's volatile cache remains dirty | Acknowledged saves can disappear or become inconsistent | **S:** no flush, FUA write, or verified cache-disable path before success. |
| AUD-05 | **High** | `src/main.rs:581–601`; `src/ioapic.rs:219–236`; `src/idt.rs:550–555` | I/O APIC delivers an interrupt after a route is unmasked but before ownership switches | Wrong-controller EOI can stall interrupts, timers, and timeout-dependent boot paths | **S:** routes enabled with interrupts live while `APIC_MODE` still selects PIC EOIs. |
| AUD-06 | **High** | `src/xhci.rs:727–767` | Controller publishes an event between payload reads and the cycle-bit read | Driver consumes a valid-looking event containing stale completion data | **S+M:** payload-before-ownership reads; a publication interleaving produced completion code 0 instead of 1. |
| AUD-07 | **High** | `src/fs.rs:49–52,92–100`; `src/main.rs:141`; `src/allocator.rs:53–58` | One oversized `.wisp` file or an oversized aggregate APPS snapshot | Heap exhaustion prevents boot from reaching the shell | **S:** eager, infallible allocation from a fixed shared heap without file/aggregate budgets. |
| AUD-08 | **High** | `src/save_format.rs:168–202,350–353`; `src/wisp/value.rs:60–94,104–128`; `src/wisp/builtins.rs:154–163` | Print, convert, save, or structurally compare a compact, heavily shared list graph | Kernel OOM or uninterruptible exponential work | **S+M:** exact size recurrence; output limit checked after expansion; depth bound does not bound breadth. |
| AUD-09 | **High** | `src/wisp/value.rs:40`; `src/wisp/builtins.rs:205–210`; `src/wisp/interp.rs:297–299,346–356` | Cyclic lists become unreachable or their script ends | Memory remains allocated across scopes/runs until the kernel heap is exhausted | **S:** all graph edges are strong `Rc`; mutation permits cycles; no cycle collection or run-owned arena. |
| AUD-10 | **High** | `src/wisp/parser.rs:346–363,389–405,456–459`; `src/wisp/interp.rs:187–188,400–418,557–564` | Deep syntax or a long flat expression, without deep function calls | Native parser/evaluator/AST cleanup can exhaust the kernel stack | **S:** recursive paths and deep boxed trees are unbounded; call-depth check covers only function calls. |
| AUD-11 | **High** | `src/xhci.rs:1427–1459,1492–1516` | Eight-byte boot reports arrive on an endpoint advertising a larger maximum packet size | Valid keystrokes are silently discarded; USB-only interaction fails | **S+M:** code 13 bypasses the report callback; model covers packet sizes 8, 16, and 64. |
| AUD-12 | **Medium** | `src/save_format.rs:291–309` | Initial format fails after writing the superblock but before clearing slots | Next boot accepts an incompletely initialized store; stale slots/capacity are exposed | **S+M:** a second invocation performed no repair and accepted 64 uncleared occupied headers. |
| AUD-13 | **Medium** | `src/wisp/lexer.rs:168–175`; `src/fs.rs:66` | Valid UTF-8 non-ASCII characters occur inside a string literal | Silent corruption of literal values, byte lengths, and persistence keys | **S+M:** `é` becomes `Ã©`; UTF-8 length changes from 2 to 4 bytes. |

### AUD-01 — Boot formats an arbitrary discovered disk

**Verified behavior.** `bring_up_save_disk` is called before the shell (`src/main.rs:493`). AHCI discovery takes the discovered controller and first available ATA port; the flow does not bind that disk to an explicitly selected save volume. After IDENTIFY and the self-test, boot unconditionally calls `ensure_formatted` (`src/main.rs:754`). If the first nine bytes are not `WISPSAVE1`, `ensure_formatted` writes the superblock and clears all 64 eight-sector slots (`src/save_format.rs:299–309`).

A readable, writable foreign disk of at least 513 sectors satisfies this branch. Its previous partitioning, contents, or relationship to the boot device do not prevent formatting. The modeled write range is exactly 513 × 512 = **262,656 bytes**. An intact WISP magic with an unsupported version is correctly rejected at lines 293–294; that narrower guard does not protect foreign disks or disks with damaged magic.

**Affected users/data.** Physical boots with an existing SATA disk, or VM launches attaching a non-disposable disk as the discovered AHCI device. No shell command or malicious script is necessary. Booting from a separate USB device is not a disk-ownership check.

**Implementation guide**

1. Remove automatic formatting from normal boot. Split read-only `probe`/`mount` from an explicit provisioning operation. Unknown or corrupt media must remain untouched and persistence must stay unavailable.
2. Require provisioning to select a specific device and save region. Display stable device identity, capacity, and the exact destructive range; obtain confirmation before any write. A WISP-owned partition is preferable to assuming ownership of an entire disk.
3. Pass a validated region/capability to storage operations, with checked relative-to-physical LBA translation. Reject operations outside it. Do not merely add a warning while retaining first-disk selection.
4. Remove the unrelated destructive boot self-test from production initialization as described in AUD-03. A safe mount path must issue zero writes, not just skip formatting.
5. Add a disposable multi-disk regression: foreign partitioned disk first, valid WISP disk second, then reversed. Hash every sector before/after boot and require all unselected disks to be byte-identical. Include damaged magic and unsupported versions.

### AUD-02 — Timeout returns while hardware may still own the command

**Verified behavior.** `issue_ata_command` prepares slot 0, sets `PxCI`, and polls for its bit to clear. On timeout it returns directly at `src/ahci.rs:617–618`. It neither proves DMA has stopped nor records an unusable-port state. The same `Hba` remains available through shell and WISP save/load calls. `write_sector` overwrites its shared data buffer at lines 772–773, and the helper rewrites its command table/header before issuing another command.

**Concrete failure sequence.** Command A is still active when its timeout expires. The caller reports the error. Command B then changes A's still-owned descriptors or data. Depending on controller progress, a late completion can be confused with B, or a pending write can consume B's payload. The initial boot read at `src/main.rs:719–736` also logs failure and continues into another command chain.

The invalid ownership transition is source-proven; the precise corrupted sector and timing require hardware/emulator injection. Intel's AHCI recovery rules require explicit port recovery after fatal command errors, not treating a timeout as ownership transfer. See [AHCI 1.3.1, §§6.2.2–6.2.2.1](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/serial-ata-ahci-spec-rev1-3-1.pdf).

**Implementation guide**

1. Represent port state explicitly: idle, in-flight, recovering, permanently failed. Make submission require exclusive mutable access; a returned timeout must not imply idle.
2. Before reusing any descriptor or data buffer, stop/recover the port using the applicable AHCI sequence, verify command-engine quiescence, inspect/clear relevant error status, and reset the link when required. Bound recovery with a clock that remains usable during the failure.
3. If quiescence cannot be established, permanently disable that port and retain/quarantine its DMA memory. Do not free it, reuse it, or retry writes blindly. A timed-out write has an indeterminate outcome.
4. Propagate the failed/offline state through `ShellHost` and shell commands. Later requests must fail immediately without modifying DMA memory or MMIO submission state.
5. Delay completion beyond the timeout in a controller test double. Submit a second request and assert that the old command table and buffer remain unchanged until verified quiescence; test fatal status and failed reset as separate branches.

### AUD-03 — The “non-destructive” self-test skips restoration on failure

**Verified behavior.** `Hba::self_test` saves the last sector, writes a test pattern, and only then schedules restoration. The `?` on readback at `src/ahci.rs:839`, the mismatch return at lines 840–842, and an uncertain failure of the pattern write itself can all exit before the original sector is restored. The original exists only on the current stack. A power cut between write and restore has the same permanence problem even when software cleanup is correct.

The independent model injected a readback error after the pattern write; the sector remained changed. This is a defect in the error path, not merely a hypothetical disk selection issue. It also runs on a recognized WISP disk, before format/mount establishes any scratch-space ownership (`src/main.rs:736–754`). Being outside WISP's first 513 sectors does not make an arbitrary disk's last sector free.

**Implementation guide**

1. Remove write/read/restore testing from default boot. Production discovery should use non-destructive identification/read checks.
2. Keep destructive driver validation in explicit test mode, on a disposable disk or a scratch region reserved by the provisioned format. Never infer scratch ownership from “last LBA.”
3. For test-mode restoration, track whether a write was attempted; after safely quiescing DMA, attempt restoration on every recoverable exit. Preserve both the primary error and any restoration error. This does not make the operation power-fail safe.
4. Do not describe write/read/restore on valuable media as non-destructive. Durable backup or sacrificial scratch ownership is necessary to survive interrupted execution.
5. Inject write error, read error, mismatch, restore failure, and reset/power interruption at each boundary. Require ordinary boot to produce no self-test writes; report test-mode restoration failures distinctly.

### AUD-04 — Save success does not establish durable storage

**Verified behavior.** `write_value_into_slot` writes sectors and returns their completion result. `write_sector` uses ordinary WRITE DMA EXT, and IDENTIFY only validates addressing/sector size and obtains capacity. There is no flush command, FUA command, or verified write-cache disable before shell output says `saved` (`src/shell.rs:433–435`). Immediate readback can be satisfied by the same volatile device cache and does not establish persistence.

**Trigger and impact.** On a device with volatile write-back caching, power can disappear after all observed command completions but before media persistence. The save may be wholly lost or partly persisted despite success being reported. Storage completion versus nonvolatile persistence is explicitly distinguished in the [Linux kernel's write-back cache documentation](https://docs.kernel.org/block/writeback_cache_control.html). This finding does not claim that every supported disk has a volatile cache or that every reboot loses data.

`docs/SAVE_FORMAT.md:162–170` already acknowledges that an interrupted in-place slot update is not transactional. That accepted limitation is **not** separately reported as a newly discovered defect. This finding concerns the additional absence of a durability boundary even after a supposedly completed save. A flush fixes durability acknowledgment, not overwrite atomicity.

**Implementation guide**

1. Decode the relevant IDENTIFY cache/flush capabilities and expose a checked `flush_cache` operation in `Hba`. Use an appropriate supported ATA flush command and propagate failures.
2. Make a successful save mean that its sector writes **and final durability operation** completed. Do not flush after every single sector unnecessarily; place barriers at the logical commit boundaries.
3. Use the same durability primitive for explicit provisioning and any restoration that claims persistent completion. If durability cannot be established, return an explicit error or expose a deliberately weaker API contract rather than reporting success.
4. Keep crash atomicity a separate decision. If old-or-new recovery is required, introduce generation/checksummed copy-on-write slots with ordered data and commit barriers; do not claim that flushing alone provides this.
5. Test with a backend that acknowledges writes into volatile memory and discards that memory on simulated power loss. Verify persistence after success, failure propagation from flush, and absence of false success on unsupported configurations.

### AUD-05 — Interrupt routes become live before EOI ownership changes

**Verified behavior.** Boot enables interrupts before APIC bring-up. `route_isa_irq` constructs an unmasked redirection entry (`src/ioapic.rs:219–236`), and boot enables IRQ1 and IRQ0 at `src/main.rs:585–589`. Only at line 601 does it enter the interrupt-disabled section and change ownership. During that interval, `idt::end_of_interrupt` still selects the PIC because `lapic::is_active()` is false (`src/idt.rs:550–555`).

A delivered I/O APIC interrupt in that window is acknowledged at the wrong controller. The LAPIC can retain an in-service interrupt and block subsequent delivery at the affected priority. Timer-dependent waits, USB polling cadence, keyboard interaction, and storage initialization can consequently stop progressing. Additionally, if IRQ1 routing succeeds but IRQ0 routing fails, the fallback returns without undoing the live IRQ1 route (`src/main.rs:585–591`).

The ordering and missing rollback are verified. Whether a given boot hits the interrupt window depends on platform routing and interrupt timing; no failure frequency was measured.

**Implementation guide**

1. Separate route preparation from activation. Validate GSI coverage and program all staged redirection entries **masked** while calibration and other fallible preparation run.
2. Commit the controller handover in one interrupt-disabled section: quiesce the old delivery path, change the EOI ownership state, and activate the prepared routes before restoring interrupts. Define how any already in-service interrupt is handled; avoid a period with two live paths and one incorrect global owner.
3. Make every pre-commit failure roll back all staged routes and restore the original controller configuration. Do not leave IRQ1 unmasked after a later IRQ0 setup error.
4. Inject IRQ0/IRQ1 after each route/state write in a test double or instrumented emulator. Assert that the accepting controller receives the EOI and that tick counts keep increasing after success and every fallback branch.

### AUD-06 — Event payload is read before the ownership bit

**Verified behavior.** `next_event` reads DWORDs 0, 1, and 2 before DWORD 3 and its cycle bit (`src/xhci.rs:731–740`). Consider an initially empty entry: the CPU reads its old payload, the controller publishes a complete new event, and the CPU then reads the new matching cycle bit. The driver accepts its cached **old payload with new control fields** and advances ERDP. The compiler fence at line 746 is too late to undo the reads.

The bounded interleaving model demonstrated exactly this mixed snapshot, with accepted completion code 0 where the new event's completion code was 1. The interleaving does not require hardware to write a TRB in pieces: even a complete publication between the CPU reads suffices. The ownership role of the cycle bit is specified in [Intel xHCI 1.2, §4.9.4](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf).

**Affected callers.** Command/control completion waits and steady-state keyboard polling. A valid completion can be discarded, associated incorrectly, or treated as failure; enumeration/input may fail intermittently.

**Implementation guide**

1. Read the control/cycle DWORD first. Return without consuming the entry when its cycle does not match the consumer state.
2. After ownership is observed, enforce the target platform's DMA-read ordering and compiler ordering, then read the remaining fields. Keep DMA coherence assumptions explicit; volatile accesses are not a substitute for a publication protocol.
3. Snapshot all required fields before advancing/publishing ERDP. Keep command/endpoint identity checks separate from ownership validation.
4. Add a controllable ring producer that publishes before, during, and after every consumer read, including wraparound. Require that a consumer returns either no event or one internally consistent event, never mixed generations.

### AUD-07 — Application ingestion can exhaust the heap before the shell exists

**Verified behavior.** `read_file_to_string` trusts `FileInfo.file_size` as an allocation size and immediately uses `vec![0; size]` (`src/fs.rs:49–52`). `snapshot_apps` then retains every successfully read source (`src/fs.rs:94–100`). Neither a per-file nor an aggregate budget exists. This allocation uses the fixed 8 MiB global heap (`src/allocator.rs:53–58`), already shared with kernel data structures.

**Trigger and impact.** A `.wisp` file larger than the entire heap necessarily cannot fit; multiple smaller sources can produce the same condition. Boot tries to ingest them without the user issuing `run`. `Result` handling around the read does not catch infallible allocation failure. The panic path ends in the kernel's halt loop (`src/panic.rs:159–166`), rather than skipping a bad application. The exact heap availability at each boot stage was not measured.

**Implementation guide**

1. Define a per-source byte limit, a total snapshot budget, and an application-count/filename budget. Reserve explicit headroom for later PMM, parser, interpreter, and device initialization; base admission on aggregate committed memory, not file count alone.
2. Check the advertised length before allocation, then use fallible reservation with controlled conversion to a read error. Bound actual bytes read as well. Reject premature EOF when the advertised file content is required, rather than silently treating a truncated program as complete.
3. Log the rejected filename and reason while continuing boot with other valid applications. Avoid unbounded preliminary filename collection and account for vector/string overhead in the aggregate budget.
4. Avoid the redundant complete source clone in `src/shell.rs:514–515` where ownership can be reorganized safely; it otherwise doubles source residency during execution. This is a concrete secondary contributor, not the root admission-control fix.
5. Test one oversized file, many small files exceeding the aggregate budget, allocation failure, and a valid file following a rejected one. Require a responsive shell with deterministic diagnostics and preserved heap headroom.

### AUD-08 — Depth-limited traversal still expands shared graphs exponentially

**Verified behavior.** Lists share `Rc` storage, so copying a list handle is cheap. Serialization and display nevertheless expand every occurrence as if it were a tree. Serialization checks `VALUE_MAX_LEN` only after the recursive expansion is complete (`src/save_format.rs:350–353`); display has no output cap. `builtin_print` additionally copies each complete formatted value into another output string (`src/wisp/builtins.rs:160`).

A valid example is:

```wisp
let v = [0];
let i = 0;
while i < 22 {
    v = [v, v];
    i = i + 1;
}
print(v);
# In a separate run, save("wide", v) exercises the serialization path
# when a save disk is available. str(v) does not require any disk.
```

This constructs just 23 distinct lists. With `n` doublings, encoded length is `19 × 2^n − 5` bytes and display length is `7 × 2^n − 4` bytes. At `n = 22`, these are **79,691,771** and **29,360,124** bytes respectively. The nesting is well below 64. Small-depth expansions were executed to validate both recurrences; the large outputs were calculated, not allocated.

Structural equality has a related redundant-work path: two separately constructed but structurally identical shared graphs repeatedly compare the same list pairs (`src/wisp/value.rs:124–128`). Identity short-circuiting helps only when the two particular handles are identical. None of these internal traversals polls the host's stop request, so Esc is not a reliable escape from the expensive operation.

**Implementation guide**

1. Make serialization accept a remaining-byte/work budget. Check capacity before every tag, length, scalar, and string append and before descending; return a WISP error before exceeding 4,055 output bytes. Prefer a bounded/fallible buffer.
2. Implement bounded display separately from serialization: stream bounded chunks for printing and return an error or explicit truncation for `str`. Track visited identities for cycles and enforce a total output/work budget even for acyclic shared graphs. Avoid the second full-output copy.
3. For equality, memoize compared list-identity pairs and define cycle behavior. Add a total traversal budget and cooperative cancellation so breadth cannot bypass a depth guard.
4. Ensure all allocation failure paths return a script-level error without consuming emergency kernel reporting memory. Do not merely raise the heap or depth limit.
5. Add the example above, very wide ordinary lists, two distinct shared graphs, and self-cycles as regressions. Require bounded work/output, prompt return or cancellation, and stable heap usage after rejected operations.

### AUD-09 — Strong-reference cycles leak across scopes and runs

**Verified behavior.** `Value::List` uses `Rc<RefCell<Vec<Value>>>`. Both `push` and indexed assignment can insert a reference to the target list, directly or indirectly. Dropping a scope or interpreter only drops external handles. A self-owned list retains a nonzero strong count, so its vector and reachable graph are not freed. There is no tracing collector, per-run arena, or tracked-cycle cleanup.

For example:

```wisp
let i = 0;
while i < 10000 {
    let a = [];
    push(a, a);
    i = i + 1;
}
```

Every loop iteration loses its local binding but retains a self-reference. Each run adds permanently retained objects to the same kernel heap. Depth-limited display/equality is unrelated to reclamation. The language documentation explicitly allows cycles (`docs/WISP_LANGUAGE.md:138–145`) but does not provide a memory-reclamation mechanism. The reference-count consequence follows the behavior described in [The Rust Programming Language, §15.6](https://doc.rust-lang.org/book/ch15-06-reference-cycles.html).

**Implementation guide**

1. Choose an explicit lifetime model. To preserve the currently documented cyclic-list semantics, use interpreter-owned list identities with a tracing collector, or an arena whose entire object graph can be reclaimed at run termination. Add collection/budgets during long-running scripts as well.
2. A smaller but behavior-changing alternative is to reject cycle-forming mutations. Before `push` or indexed replacement, iteratively check whether the inserted value can already reach the destination list; reject without modifying the graph. Cover both mutation entry points and document the new rule.
3. Do not replace all child links with `Weak`: that changes ordinary list ownership semantics. Do not merely clear globals on exit: cycles already lost inside earlier block scopes are no longer reachable from globals.
4. Test direct cycles, indirect cycles, alias-heavy acyclic graphs, errors, Esc cancellation, and repeated runs. Record allocator usage before and after each run and require no cumulative retained application graph.

### AUD-10 — Parser, expression evaluation, cloning, and cleanup have no structural depth budget

**Verified behavior.** The 256-call guard in `Interp::call_function` only counts WISP function invocations. It does not constrain nested parentheses/unary syntax, recursive block parsing, or expression depth. `parse_term` builds a left-deep boxed tree even for a flat chain (`src/wisp/parser.rs:346–363`). Evaluation recursively walks it. Function registration clones the entire AST (`src/wisp/interp.rs:187–188`), and owned boxed trees also need recursive destruction unless redesigned.

**Trigger and impact.** A source containing sufficiently many parentheses or a long `1 + 1 + ...` chain can exhaust the native stack without making a single WISP function call. The mainline kernel stack is 256 KiB (`src/gdt.rs:94`). A guard-page fault may make the failure visible but does not recover to the shell. The unbounded recursive paths are verified; the smallest crashing input for the actual optimized binary remains unmeasured.

**Implementation guide**

1. Apply independent limits for source bytes, tokens, AST nodes, and syntactic/AST depth before constructing an unsafe-sized structure. Track depth for binary/postfix chains as well as explicit parentheses; limiting only parser call nesting misses the flat-chain case.
2. Use explicit work stacks or an arena/index-based AST where practical so parsing, evaluation, cloning, and destruction do not recursively consume the kernel stack. Avoid cloning entire function subtrees when stable shared program ownership suffices.
3. If recursive execution remains, use a shared execution-depth/work budget that accounts for expression and block traversal combined with function calls. Choose the budget from measured release-build stack consumption, not the number 256 alone.
4. Ensure rejection and cleanup are themselves bounded. A parser that rejects a deep tree but recursively drops an already-built oversized prefix can still fail during error handling.
5. Test long flat arithmetic, unary chains, nested blocks/parentheses, deep function bodies, and parse errors near the end of those inputs. Require a WISP error and a live prompt; measure stack high-water usage on the actual release binary.

### AUD-11 — Valid short USB keyboard reports are discarded

**Verified behavior.** The interrupt transfer length is taken from `wMaxPacketSize` (`src/xhci.rs:1432,1458`). The callback is invoked only for completion code 1. Completion code 13 is explicitly ignored before rearming (`src/xhci.rs:1492–1516`).

The HID boot keyboard input report is eight bytes ([USB HID 1.11, Appendix B.1, printed page 60](https://www.usb.org/sites/default/files/documents/hid1_11.pdf)). Endpoint maximum packet size is a transport limit, not a requirement that every report fill it. With maximum packet size 16 or 64 and an eight-byte report, the queued larger request completes short. xHCI identifies that condition and reports residual bytes; the current branch silently drops the valid report. See [xHCI 1.2, §4.10.1.1.2](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf).

**Affected users.** USB boot keyboards with this descriptor/report combination. Enumeration may announce “ready” while every keystroke, including Esc, is ignored. The Python branch model verified the dispatch decision; no physical keyboard model was exercised.

**Implementation guide**

1. Keep endpoint maximum packet size in endpoint configuration, but schedule the intended boot-report length: eight bytes. Do not conflate the two fields.
2. Handle successful and short completions according to the bytes actually transferred. Validate residual length before subtracting and require the supported report's full eight bytes before copying/decoding; do not decode stale bytes from genuinely incomplete reports.
3. Correlate the event to the outstanding keyboard transfer, including endpoint/TRB identity, before clearing `armed`. Handle actual endpoint failures separately from valid short transfers.
4. Add descriptor/report combinations with maximum packet sizes 8, 16, and 64, plus deliberately truncated reports. Require ordinary keys and Esc to be delivered once and malformed reports to be rejected visibly.

### AUD-12 — Format success is published before initialization completes

**Verified behavior.** `ensure_formatted` writes a valid superblock at `src/save_format.rs:304` and clears slots afterward. If a slot write fails, it returns an error, but the valid header remains. On the next boot, matching magic/version immediately returns success at lines 291–296 without checking that initialization completed.

In the bounded model, the initial disk region contained nonzero data, the first slot write failed, and only the superblock persisted. The next invocation returned success without another write; all 64 slot headers still appeared occupied. Real outcomes depend on the old slot bytes: reduced capacity, stale records, or load errors are possible. This is rated Medium because it requires interrupted initial provisioning; AUD-01 separately covers unauthorized formatting itself.

**Implementation guide**

1. After removing automatic provisioning, define a durable initialization state. Keep the new store invalid/in-progress while initializing its data region; a mount must not silently resume destructive provisioning.
2. Clear/initialize the selected slots, make that data durable, then publish the committed superblock and make the commit durable. For reprovisioning, durably invalidate the previous committed state before altering data.
3. Validate the complete supported header, including stored slot count/size and any integrity/state fields introduced by the format revision. Do not accept magic/version alone as proof that initialization finished.
4. Inject failure after every initialization write and barrier. A reboot must either reject an incomplete store without writes or mount a fully initialized store; stale occupied slots must not be accepted as successful fresh formatting.

### AUD-13 — The lexer silently changes valid UTF-8 string contents

**Verified behavior.** The filesystem explicitly validates and returns UTF-8 (`src/fs.rs:66`), but the lexer's ordinary string branch casts each UTF-8 byte to a Unicode scalar with `s.push(b as char)` (`src/wisp/lexer.rs:174`). This is byte reinterpretation, not UTF-8 decoding.

For `é`, input bytes `C3 A9` become the two characters `Ã` and `©`; the resulting UTF-8 bytes occupy four bytes instead of two. This affects the actual `Value::Str`, not merely the ASCII framebuffer's rendering. Literal comparisons, `len`, and save/load keys or values can therefore disagree with the intended source text. The conversion was reproduced with the exact byte-to-code-point mapping in a Python model.

**Implementation guide**

1. Preserve valid UTF-8 spans between escape/delimiter bytes instead of casting each byte to `char`, or scan by Unicode scalar with correctly maintained byte offsets. Continue handling the existing ASCII escapes explicitly.
2. If the language intentionally disallows non-ASCII literals, reject them with a clear lexical error instead of accepting altered values. Update the filesystem/language contract consistently; silent recoding is not an acceptable restriction mechanism.
3. Keep identifier policy separate from string-literal policy. Supporting UTF-8 literal contents does not require adding Unicode identifiers or framebuffer fonts.
4. Test byte-for-byte values for accented characters, CJK, emoji, and mixed escapes. Verify persistence round trips and the documented byte-length semantics independently of how the console renders the text.

## Verification Gaps

The following checks would materially change confidence, severity, or remediation correctness. They are not additional confirmed defects.

1. **Actual build and baseline execution.** Build this exact snapshot with a recorded nightly version and run the four discovered QEMU/OVMF harnesses against disposable images. No Rust or boot test ran here. This establishes whether the reviewed paths execute in the optimized binary and provides a regression baseline; syntax-only passes cannot do that.
2. **Storage ownership, timeout, and durability injection — AUD-01–04, AUD-12.** Trace every write with multiple attached disks; inject delayed DMA, fatal status, failed recovery, readback errors, volatile-cache loss, and interruption at each format/commit boundary. Check whole-image hashes, not just loaded key values. These tests determine actual controller consequences and whether remediation preserves unrelated data.
3. **Interrupt handover timing — AUD-05.** Inject interrupts between each I/O APIC route write and the ownership-state change; force failure of the second route after the first succeeds. Record LAPIC in-service state, EOI destination, and continued clock progress. Ordinary successful boots do not rule out this window.
4. **DMA publication and report-size variation — AUD-06, AUD-11.** Exercise controllable event publication between consumer loads, event-ring wraparound, and keyboards whose endpoint size exceeds their eight-byte report. Stock QEMU keyboard success alone cannot validate either boundary.
5. **Actual interpreter resource containment — AUD-07–10.** Measure allocator usage and native-stack high-water marks for oversized snapshots, shared DAGs, cyclic lists, deep syntax, and repeated run/error/cancel cycles. The exact resource-exhaustion thresholds and optimized-code crash behavior were not executed here. Repeat after fixes to prove rejection and cleanup remain bounded.
6. **Possible firmware-memory overlap, not a confirmed finding.** `src/pmm.rs:89–129` makes usable memory available before the new CR3/stack handover in `src/main.rs:175–284`. Capture the firmware CR3 tree, current stack, descriptor-table storage, and final memory-map types; verify none of their still-live frames can be allocated by the PMM during that interval. Without those platform-specific addresses/types, this remains an ownership concern rather than a proven overlap.
7. **UTF-8 through the real language pipeline — AUD-13.** Run lexer/value/persistence assertions on the actual Rust implementation. The byte-mapping fault is deterministic, but a framebuffer screenshot cannot verify the corrected in-memory value because console rendering independently substitutes unsupported characters.
