# Changes

This is an engineering log, not a dated release changelog -- entries are
ordered by build sequence (each milestone depends on the ones above it),
not by calendar date, since several of these were reconstructed together
from rationale that had been scattered across source comments (each of
which points back here). Where a comment elsewhere in the repo says "see
CHANGES.md," this is the file it means.

## Boot Services-resident UEFI application

Chose to never call `exit_boot_services()` and instead live entirely
within the pre-exit-boot-services epoch for the program's whole
lifetime. This trades away the ability to become a "real" persistent OS
kernel in exchange for keeping every UEFI convenience alive throughout:
memory allocation, keyboard input via the Simple Text Input protocol,
and FAT32 file access via the Simple File System protocol. For a project
whose whole point is a hand-built shell and language running on bare
firmware, staying resident in Boot Services was the pragmatic choice --
it's what makes `fs.rs` and `shell.rs`'s keyboard handling possible
without writing our own drivers.

## Direct-framebuffer console instead of Simple Text Output

`gfx.rs` writes glyphs straight into the Graphics Output Protocol's
linear framebuffer using an embedded 8x16 bitmap font (`font.rs`,
rasterized offline from DejaVu Sans Mono), rather than routing through
UEFI's Simple Text Output protocol. This is what gives the shell its own
color palette (`ACCENT`/`ERROR`/`FOREGROUND` in `gfx.rs`) and consistent
behavior across firmware implementations, instead of inheriting whatever
Simple Text Output happens to support on a given machine.

## `wcslen` linker shim (`src/libc_shims.rs`)

At `opt-level = "s"` (and worse with LTO on, both set in `Cargo.toml`'s
release profile), LLVM's loop-idiom-recognition pass rewrites a
hand-written "scan forward until a zero code unit" loop -- exactly what
`uefi::CStr16` and several protocols in the `uefi` crate use internally
to find a UCS-2 string's length -- into a call to the platform's
`wcslen`. That's a correct, desirable optimization on a hosted target
where libc provides `wcslen`. This is a freestanding, no-libc UEFI
binary, so the call was left dangling and the linker failed with
`undefined symbol: wcslen`. This is not a bug in our code or in the
`uefi` crate; it's an inherent gap in bare-metal `no_std` targets that
don't disable this specific optimization. Fixed by providing our own
trivial `wcslen` rather than degrading codegen quality by turning
optimizations off wholesale -- the standard remedy for this class of
problem in bare-metal Rust.

## `uefi` crate hard-pinned to `=0.30.0`

This crate's API has shifted across releases (including the later
`uefi::boot` free-function API that supersedes some of what
`system_table_boot()` does in `main.rs`). Every call in this project was
written and verified against the exact source of 0.30.0. `Cargo.toml`
pins it with `=` specifically so a routine `cargo update` can't silently
move the project onto an API surface it wasn't written against.

## WISP language v1

Designed and implemented a small tree-walking scripting language
(`src/wisp/`: `lexer` -> `parser` -> `ast` -> `interp`, `value` for the
runtime representation, `builtins` for the fixed built-in-function set)
so that plain-text programs can be dropped onto the boot volume and run
with no compiler and no OS involved. Deliberate scope cuts, all
documented in `docs/WISP_LANGUAGE.md`'s "What's not in v1": no closures
or nested functions (a single global scope plus a stack of block-local
frames is enough for small scripts and much simpler to get right), no
floats (one 64-bit signed `int` type; see the WISP-AI fixed-point
pattern below for how far that goes), no maps/structs (parallel lists
are the documented workaround), no file I/O from within a script. Two
depth guards exist specifically so a buggy or malicious script can
degrade gracefully instead of taking the firmware down with it: a
256-deep call-recursion guard (`interp.rs`'s `MAX_CALL_DEPTH`, since
every WISP call is backed by a real Rust stack frame in a tree-walker)
and a 64-deep structural-recursion guard on list equality/display
(`value.rs`'s `MAX_VALUE_DEPTH`, for the edge case of a
self-referential list like `let a = []; push(a, a);`).

## WISP-AI: trained models as WISP forward-pass code (`docs/WISP_AI.md`)

Added a pattern -- not a language extension, just plain WISP list
literals and integer arithmetic -- for running small trained neural
networks' forward pass inside the interpreter: quantize every weight to
a fixed-point integer (`round(weight * SCALE)`), then re-verify accuracy
using the *exact* integer arithmetic WISP will actually run before
shipping any weights. That re-verification step caught a real bug during
development: Python's `//` is floor division, but WISP's `/` compiles to
Rust's `i64::checked_div`, which truncates toward zero -- the two
disagree on negative operands, so a naive Python reference
implementation of `fixed_mul` using `//` would report accuracy for
rounding behavior WISP doesn't actually have.
`tools/train_tiny_nn.py`'s `_trunc_div` helper exists specifically to
close that gap.

## `tools/train_tiny_nn.py`

An offline, dev-only Python tool (not part of the shipped firmware --
WISP has no training/backprop machinery, on purpose) that derives the
weights baked into `xor_brain.wisp` (an analytic closed-form ReLU
construction, exact integers, no training involved) and `tamer.wisp` (a
genuinely trained 2-input/4-hidden-ReLU/1-output network, full-batch
Adam with hand-rolled analytic backprop -- no autodiff library). Kept in
the repo purely for reproducibility: anyone can re-run it and regenerate
the exact constants transcribed into the `.wisp` source, rather than
those numbers being unexplained magic. Both models' final accuracy is
asserted at 100% -- first in float, then again using the same
fixed-point integer math WISP will run -- before the script prints
anything meant to be copied into a `.wisp` file.

## The `esp/APPS/*.wisp` demo programs

`hello.wisp` (simplest possible script), `guess.wisp` (the worked
example in `docs/WISP_LANGUAGE.md` itself), `oracle.wisp` (a shuffle-bag
fortune teller demonstrating the list swap-to-last-then-`pop()` removal
pattern), `adventure.wisp` (a five-room text adventure -- the
non-trivial demonstration that WISP's "no maps, no string indexing"
constraints still support real program state and whole-line command
parsing), `xor_brain.wisp` and `tamer.wisp` (the WISP-AI pattern's two
worked examples, in increasing order of realism). Every numeric constant
in `xor_brain.wisp` and `tamer.wisp` was transcribed directly from a
`tools/train_tiny_nn.py` run, not hand-derived, so the weights in the
repo and the weights the training script prints stay identical by
construction.

## `tests/acceptance_test.py`

An end-to-end acceptance test that drives a real `qemu-system-x86_64
-bios <OVMF> -drive file=fat:esp,if=none,readonly=on -device
usb-storage` instance over QEMU's HMP monitor socket
(`sendkey`/`screendump`), rather than testing the WISP interpreter in
isolation on the host. It exercises boot-to-banner, every shell command,
backspace-based line editing, and all six WISP programs end-to-end
(`hello` for basic `input()`, `oracle` for lists/`rand`, `tamer` for real
trained-model inference against a live keyboard prompt, `adventure` for
multi-room state and every gating branch, and `xor_brain` for the full
truth table) -- proving the whole stack (GOP console, FAT32 reads,
keyboard input, and the interpreter) works together on real (emulated)
firmware, not just against a native test harness. Waits for the shell's
own banner *text*, confirmed via OCR, to appear rather than sleeping a
fixed duration or trusting pixel presence alone -- see "vvfat read-write
corruption" below for why the OCR check specifically exists, and note
that the drive is mounted read-only via `usb-storage` rather than the
`fat:rw:` this section originally described; that also changed as part
of the same fix.

## OVMF firmware path fallback

The project's reference invocation names a single monolithic
`/usr/share/OVMF/OVMF_CODE.fd` (code + variable store in one file, as
some distros ship it). The `ovmf` package on the distro this project was
developed against instead ships a split code/vars pair (meant for
`-pflash`) under different filenames, plus a separate monolithic image
at `/usr/share/ovmf/OVMF.fd` that's what actually works with `-bios`.
`run.sh` and `tests/acceptance_test.py` both try the spec'd path first
and fall back to the real one, so the project runs unmodified wherever
`OVMF_CODE.fd` exists and still works in the sandbox it was actually
built in.

## USB installer scripts (`scripts/`, `USB_INSTALL.md`)

Added one installer per host OS (Linux/`parted`+`mkfs.vfat`,
macOS/`diskutil`, Windows/PowerShell `Storage` module), all following
the same "measure twice, cut once" safety model: a read-only `list`
mode; `install` refuses non-removable/non-USB media by default; the
disk backing the machine's own running system can never be selected,
with no override flag for that specific check; and two separate typed
confirmations (the exact device identifier, then the literal word
`ERASE`) before anything destructive happens. Development happened
inside a Linux-only sandbox, so only the Linux script's partitioning and
formatting steps were exercised end-to-end (against a loopback disk
image -- the sandbox kernel has no `vfat` driver, so the final
mount-and-copy step couldn't be verified there, though it's standard and
well-trodden). The macOS and Windows scripts were written carefully
against each platform's documented behavior and reviewed, but were never
run against real hardware or a real install of either OS -- treat them
with correspondingly more caution on first use. See `USB_INSTALL.md` for
the full safety-check breakdown and the macOS partition-type-GUID
caveat.

## vvfat read-write corruption: the wrong-shell bug, its real cause, and the fix

This is the longest entry in this file because it was the most expensive
mistake made building this project, and the user's standing instruction
is that defects get written up honestly, not smoothed over. Keeping the
full account here -- including the wrong turn -- is the point.

**The symptom.** After `adventure.wisp` and `xor_brain.wisp` test steps
were added to `tests/acceptance_test.py`, re-running the suite produced
screenshots of OVMF's own built-in "EDK II UEFI Interactive Shell"
(`Shell>` prompt, "'x' is not recognized as an internal or external
command...") instead of this project's shell -- every scripted keystroke
after that point was typed into the wrong program and failed. The
previous run, against the same `./esp` directory, had booted correctly.

**The wrong hypothesis.** `-bios <single monolithic OVMF image>` gives
OVMF no separate writable pflash for `OVMF_VARS`, so this OVMF build
persists its UEFI variable store by writing a file named `NvVars`
directly onto the first FAT filesystem it can reach -- i.e. straight
into `./esp`, alongside `BOOTX64.EFI` and `APPS/`. That looked like a
complete explanation: a stale cached boot-manager entry, from a since-
regenerated vvfat layout, causing OVMF's Boot Device Selection to give up
on the removable-media fallback path. The fix seemed obvious -- delete
`esp/NvVars` before every boot -- and it was implemented in both
`run.sh` and `tests/acceptance_test.py`. It did not work: running the
suite twice back to back with that fix in place, the second run still
booted into OVMF's own shell.

**The real cause.** A byte-for-byte comparison of `esp/EFI/BOOT/
BOOTX64.EFI` (as it sat on disk after two automated test runs) against a
freshly built copy of the same binary showed **47738 of 90624 bytes
different** -- identical file size, but well over half the content
diverging from a specific offset onward. Several `esp/APPS/*.wisp`
files had their modification times bumped by the same event, though
(this time) with unchanged content -- confirmed by re-diffing them
against their known-source text. This was never an NVRAM problem. QEMU's
vvfat driver, in read-write mode (`file=fat:rw:<dir>`), had rewritten
the project's own boot executable, wrongly, directly on the host
filesystem, between one QEMU invocation and the next. QEMU's own
documentation describes vvfat read/write support as available "only for
(beta) testing purposes" -- this is a known, acknowledged sharp edge in
the driver, not a misconfiguration on this project's part (see
<https://gitlab.com/qemu-project/qemu/-/issues/272> and the Debian/
Launchpad/Red Hat bug trackers, all documenting vvfat read-write
instability independently). The NvVars deletion fix wasn't wrong, exactly
-- OVMF really does write that file, and deleting stale state before a
boot is reasonable hygiene -- it just wasn't the disease. The disease was
using a driver mode QEMU itself doesn't stand behind, for something that
mattered.

**The fix.** Mount the ESP read-only. Nothing this project boots ever
needed write access to begin with -- `src/fs.rs` only ever opens files
`FileMode::Read`; `ls` and `run` are the only two shell commands that
touch the filesystem, and neither writes. The only writer was ever
OVMF's own NVRAM persistence, which this project doesn't want anyway
(see "OVMF firmware path fallback" above and USB_INSTALL.md's "Booting
the stick": the whole boot strategy is the UEFI removable-media fallback
path with no cached boot-manager entry required). Getting there took one
extra step: a bare `-drive format=raw,file=fat:esp` (read-only, no `rw:`)
attaches through QEMU's default IDE hard-disk device model, and `ide-hd`
unconditionally refuses a read-only backing node ("Block node is
read-only", even with `readonly=on` set explicitly on the `-drive`) --
disk device models expect to be writable; only device models meant for
read-only media accept one. The fix attaches the volume as a USB
mass-storage device instead
(`-drive file=fat:esp,if=none,id=stick,readonly=on -device
usb-storage,drive=stick`), which QEMU does allow read-only. That isn't a
workaround bolted on for its own sake -- it makes the emulated boot path
a closer match for how this project is actually meant to be used, booted
from a real USB stick (`USB_INSTALL.md`), and UEFI's removable-media
boot fallback applies to a USB mass-storage device exactly the same way
it does to a plain disk. Both `run.sh` and `tests/acceptance_test.py`
were updated identically; the now-unnecessary-but-harmless `NvVars`
deletion was kept in both as a defensive no-op (in case an old `esp/`
directory from before this fix is reused), with its comment corrected to
say so.

**Hardening the test itself.** The original `wait_for_boot()` judged
"booted" by polling for any non-black pixels in the banner's screen
region. That check could not tell this project's shell apart from
OVMF's own built-in shell -- both draw a banner starting at (0, 0) --
which is exactly how the corrupted-boot runs above went unnoticed until
screenshots were inspected by eye. `wait_for_boot()` now OCRs (via the
`tesseract` CLI, shelled out to directly -- no new Python dependency)
the first non-black frame and requires the literal text "Boot Services"
(a substring of `shell.rs`'s `BANNER` constant, "Minimal UEFI Shell --
resident in Boot Services") before declaring boot successful; seeing
"Interactive Shell" (OVMF's own banner) instead is now an immediate hard
failure with a message pointing back to this entry, not something the
test silently continues past.

**Verification.** After the fix: the full acceptance suite was run four
times consecutively (`tests/out_c`, `out_d`, `out_e`, and the default
`tests/out`), plus a separate direct smoke test of `run.sh` itself (not
just the Python harness's own independent QEMU invocation, since the two
build their `-drive`/`-device` arguments separately and either could
have been fixed while the other was missed). Every run's OCR-confirmed
banner read correctly, and after every single run, `md5sum` of
`BOOTX64.EFI` and all six `esp/APPS/*.wisp` files matched a byte-for-byte
pristine rebuild -- zero drift, across five independent boots, where two
had been enough to corrupt data under the old configuration. `esp/NvVars`
did not reappear after any of them, confirming the read-only mount holds
and OVMF degrades to a volatile, in-memory variable store without
erroring when it can't persist one.

## Zero networking, as a hard constraint rather than a default

Every OVMF/QEMU invocation in this repo (`run.sh`,
`tests/acceptance_test.py`) passes `-net none`, and no dependency in
`Cargo.toml` is a networking crate. This isn't "networking happens to be
turned off" -- the firmware binary never links against or calls any
networking protocol at all, so there is no code path that could
accidentally enable one. This was a foundational project requirement
from the start, not a hardening pass applied afterward.

## Leaving Boot Services: our own GDT, IDT, PIC/PIT timer, and keyboard driver

The project was, architecturally, "UEFI wearing an OS costume": every
memory allocation, every keystroke, and every file read went through
Boot Services for the program's entire life (see "Boot Services-resident
UEFI application" above), with no real interrupts, no timer we owned,
and no memory manager we owned. This phase pushes toward an actual
kernel: `exit_boot_services()` is now called, and everything Boot
Services used to provide is replaced with our own hand-written `no_std`
Rust -- explicitly not Linux-derived in any way (no borrowed drivers, no
Linux boot protocol, nothing Linux-sourced), in the same fully-custom
spirit as WISP itself.

**New modules.** `src/gdt.rs` builds a Global Descriptor Table (null +
kernel code + kernel data + a TSS descriptor) and a Task State Segment
whose only job is giving the double-fault handler (vector 8) a dedicated
16 KiB stack via IST1 -- without it, a stack-overflow-triggered fault
can fail to push its own exception frame and silently triple-fault
straight to a hard reset, which is exactly the failure mode this phase
was designed to make impossible to hit blind. `src/idt.rs` builds a
full 256-entry Interrupt Descriptor Table -- every vector gets a real
gate, not just the ones actively used, so an unexpected fault produces a
debuggable "unhandled exception #N" report instead of a second, harder
fault on top of whatever went wrong first. Entry points are
`#[unsafe(naked)]` + `naked_asm!` trampolines (stable since Rust 1.88),
not the nightly-only `x86-interrupt` ABI, generated via three macros for
the two stub shapes: fault stubs (never return, end in `ud2` as a
belt-and-brace) and IRQ stubs (full register restore + `iretq`).
`src/pic.rs` remaps the legacy 8259 PIC to vectors 0x20-0x2F and masks
everything except IRQ0/IRQ1 -- not APIC/IOAPIC, which would need
ACPI/MADT parsing this phase didn't scope for no benefit on a single
core. `src/pit.rs` programs the 8253/8254 timer for a 100 Hz tick.
`src/keyboard.rs` is a from-scratch PS/2 scancode-set-1 driver (shift
tracking, extended-prefix handling, break-code filtering) feeding a
ring buffer. `src/pmm.rs` is a bitmap physical frame allocator over the
memory map `exit_boot_services` hands back, with a boot-time
alloc/write/free/realloc self-test (no consumer yet -- paging is
explicitly future work -- but exercised so it isn't untested dead code).
`src/panic.rs` and `src/debugcon.rs` (a raw port-0xE9 "debugcon" writer,
output-only, no networking implication) existed before this phase but
became load-bearing here: with no debugger available, they're the only
way to see what a bad GDT/IDT/PIC bring-up is doing.

**Boot sequence.** GDT/IDT *data* is built before `exit_boot_services`
(pure memory writes, no CPU state touched, safe at any point); actually
*loading* it (`lgdt`/`lidt`/`ltr`/CS-reload) and enabling interrupts
happens strictly after, since touching live descriptor-table state while
firmware code might still be running against its own is undefined
territory. `fs::snapshot_apps` (below) is the last thing that touches
`BootServices`. Filesystem access no longer exists live after boot:
`fs::snapshot_apps(bs, image)` reads every `APPS/*.wisp` file into an
owned `Vec<(String, String)>` while Boot Services is still alive, and
`shell.rs`'s `ls`/`run` serve from that in-memory snapshot afterward. A
real block-device + FAT32-over-raw-sectors driver for live post-exit
disk access is separate, explicitly out-of-scope future work, same as
paging, APIC/multicore, and USB HID keyboard support.

This was built and verified bottom-up rather than as one untested leap
(allocator/debugcon/panic first, then GDT/IDT with every vector wired to
the generic fault handler, then PIC/PIT/`sti`, then the keyboard driver,
then rewiring `shell.rs`/`fs.rs`, then the frame allocator), matching
how the rest of this project has been worked, and it surfaced three
serious, non-obvious bugs along the way.

### Bug: `lateout` vs `out` register aliasing corrupted the CS reload, triple-faulting silently

`gdt::load()`'s CS-reload sequence (64-bit mode has no direct `mov cs,
imm`; it has to be done via a far return: push a selector, push a
target offset computed with `lea reg, [rip + label]`, then `retfq`) was
first written using `lateout(reg) _` for the temporary register holding
the computed offset. `lateout` tells the compiler this register's value
is only needed *after* every input operand has been read, which permits
the compiler to reuse an `in(reg)` operand's register for it if it
believes that input's last use comes first -- but the compiler does not
parse the asm template text to check that belief against what the
assembly actually does. Here it aliased the temporary with the selector
input register, so by the time `retfq` executed, the pushed "selector"
value had already been overwritten by the computed offset. Result: a
garbage CS selector loaded via `retfq`, and a triple fault with zero
debug output the instant the CPU tried to execute anything through it --
not even a debugcon line got out, because the fault happened before any
of this kernel's own fault-reporting machinery was live. Root-caused via
`objdump` disassembly of the release binary, comparing the actual
register allocation against the intended one. Fixed by using `out(reg)
_` instead, which makes no such reuse assumption; the fix and the full
reasoning are left in place as a comment at the call site, since this is
exactly the kind of asm-operand subtlety that's easy to reintroduce by
accident in future naked-asm code.

### Bug: Microsoft x64 calling convention, not SysV -- IRQ handlers read garbage arguments

`x86_64-unknown-uefi`'s `extern "C"` ABI is the Microsoft x64 calling
convention, not SysV/Linux/Mac's, because UEFI itself is specified
against MS ABI. The first version of every ISR trampoline in `idt.rs`
loaded the frame pointer into RDI before calling the Rust-side handler
(`fault_handler_entry`/`irq_handler_entry`) -- correct for SysV, wrong
here: MS x64 passes the first integer/pointer argument in RCX. The
handlers read their "frame" pointer out of whatever garbage happened to
already be in RCX, producing a `frame` pointing at an essentially random
low address; the instant a real interrupt arrived (PIT or keyboard),
this triple-faulted with the CPU state having been entirely correct up
to that point -- the bug was purely in how the argument was handed
across the call boundary. Confirmed by disassembling the release binary
and comparing `mov %rsp,%rdi` in the stub against the compiled body of
`irq_handler_entry`, which read `frame.vector` via `mov 0x78(%rcx),
%rsi` -- RCX, not RDI. Fixed by changing every stub to `mov rcx, rsp`.
This surfaced a second, related requirement at the same time: MS x64
also obligates the *caller* to reserve 32 bytes of "shadow space" on the
stack immediately below the return address before every call (space the
callee is entitled to spill its register arguments into), on top of the
usual 16-byte stack alignment. The stubs' own 15-register save (120
bytes -- not a multiple of 16) throws off the 16-alignment the CPU
otherwise guarantees at ISR entry, so `sub rsp, 0x20` / `add rsp, 0x20`
bracketing the `call` now does double duty: it restores 16-alignment
*and* provides the required shadow space in the same 32 bytes (the
frame pointer is captured into RCX *before* the `sub`, so the reserved
region never overlaps the frame Rust reads). The full account is also
documented at length in `idt.rs`'s own module doc comment, since every
future vector added to this file inherits this exact requirement.

### Bug: `fs::snapshot_apps` failed every file read with `ACCESS_DENIED`

The first version of `snapshot_apps` opened `bs.get_image_file_system`
once to list `APPS/`'s entries, then, per file, called
`read_file_to_string` -- which opens its *own* fresh
`SimpleFileSystem`/`Directory` handle -- while the outer listing's
`fs`/`root`/`apps_dir` handles were still alive. The UEFI Simple File
System protocol instance refuses a second concurrent open while an
earlier one's handles are outstanding, failing every read with
`ACCESS_DENIED`. Fixed by splitting the walk into two phases:
`list_wisp_filenames` collects every `*.wisp` name and, by construction
(every listing handle is local to that function), drops all of them
before returning; `snapshot_apps` then reads each file only after that
function has returned, so there is never more than one open of the
protocol alive at once.

### Bug: a missing compiler fence made the keyboard-driven shell hang forever, keystrokes queued but never delivered

The most serious and least obvious bug of this phase. After PIC/PIT/
keyboard bring-up, the shell reached a live prompt with no crashes, no
unexpected-interrupt spam, and a clean PMM self-test -- but typing
anything did nothing. Debugcon logging showed the keyboard ISR
(`keyboard::on_irq`) correctly decoding every scancode and pushing every
byte into its ring buffer (queue length correctly incrementing 1, 2, 3,
4, 5 across five keystrokes), while `keyboard::wait_for_key()`'s
`QUEUE.with(|q| q.pop())` check -- executed between every keystroke,
with room to spare even at a deliberately slowed one-key-per-0.4-seconds
typing cadence used specifically to rule out timing as the cause -- kept
finding the queue empty and looping back to `hlt`, forever. The queue
was being filled correctly and never successfully drained; the shell
was permanently parked in the very first `read_line()` call of its main
loop.

The root cause was not a race condition (there was no interrupt landing
in an unsafe window) but a memory-model bug: `sync::without_interrupts`
-- the function every `Locked<T>::with` call goes through -- disables
interrupts with a `cli` instruction marked `options(nomem)` (correctly:
`cli` itself touches no memory) and re-enables with `sti`, similarly
marked. Nothing in that function, or in `keyboard::wait_for_key`'s own
raw `cli` / `sti-hlt` sequence, told the Rust compiler that memory
protected by this critical section could be modified by code the
compiler's own data-flow analysis can't see -- an interrupt handler,
running asynchronously on the same CPU. Under this project's release
profile (`opt-level = "s"`, `lto = true`), LLVM is aggressive enough to
notice that nothing in `wait_for_key`'s own visible control flow writes
to the keyboard ring buffer, and legally concluded it could treat the
"is the queue empty" check as loop-invariant -- hoisting or caching it
across iterations of the `cli` / check / `sti-hlt` loop, so the real,
asynchronous write from `on_irq` was never observed after the first
(correctly empty) check. This explained an earlier, confusing data
point exactly: a prior debugging session that happened to have a
loop-iteration-counting debug print active inside `wait_for_key`'s loop
saw correct behavior, purely because that print was an opaque function
call the optimizer couldn't see through, which incidentally defeated the
hoist -- not because it fixed anything. It also explained why
`pit::sleep_ms`, which uses the identical `cli` / check / `sti-hlt`
idiom, never exhibited this bug even once across the whole session: it
checks an `AtomicU64` tick counter, and even a `Relaxed`-ordered atomic
load cannot be hoisted or eliminated the way a plain field read can,
since the compiler must treat every atomic operation as potentially
observing external modification.

Fixed at the structural layer, not by patching `keyboard.rs` alone:
`sync::without_interrupts` now brackets the wrapped closure with
`core::sync::atomic::compiler_fence(Ordering::SeqCst)` immediately after
disabling interrupts and immediately before re-enabling them, so every
current and future `Locked<T>` user is protected uniformly rather than
relying on each caller to remember this. `keyboard::wait_for_key` also
got matching fences directly around its own raw `cli`/`sti`/`hlt`
sequence, for defense in depth, since it doesn't go through
`without_interrupts` for the outer disable/enable pair. This is a
compiler fence, not a CPU memory-ordering fence -- there is no other CPU
to synchronize against on this single-core kernel, no cache-coherency
problem to solve -- it exists purely to forbid the *optimizer* from
reordering or eliminating memory operations across a point in program
order where an interrupt handler can genuinely intervene. Verified fixed
by re-running the exact slow-cadence "help" typing test that had
previously reproduced the bug: every keystroke now shows the ring
buffer's length staying at 1 (pushed and immediately popped) instead of
climbing to 5, `wait_for_key` returns for every byte, and the full
`help` command output renders and OCR-verifies correctly on screen.

### Bug (lower severity): the fault screen reported a fabricated RSP/SS for most exceptions

While verifying the fault-reporting path with a deliberately triggered
`int3` (see "Fault-test hooks" below), the captured fault screen showed
an RSP/SS pair read directly from `InterruptFrame`'s `rsp`/`ss` fields.
Those fields are only genuinely hardware-pushed data when the CPU
actually switches stacks to take the fault -- which happens for vector 8
(#DF) only, because its IST1 entry unconditionally forces a stack switch
(SDM Vol 3, 6.14.5). Every other vector runs with IST=0, and since this
kernel has no user mode yet (CPL is always 0, so a same-privilege fault
never triggers a stack switch either), the CPU pushes only
RIP/CS/RFLAGS -- three fields, not five. Reading `frame.rsp`/`frame.ss`
directly for those vectors doesn't crash (the fields just overlay
whatever pre-existing stack bytes happen to sit at those offsets), but
it silently misreports state on the one screen meant for debugging a
live fault. Fixed with a small `true_rsp_ss(frame)` helper in `idt.rs`:
for vector 8 it returns the real pushed values; for everything else it
derives the true pre-fault RSP directly (one qword past where RFLAGS was
pushed, which is exactly where RSP pointed immediately before the fault
in the no-stack-switch case) and reports `gdt::KERNEL_DATA_SELECTOR` for
SS, since this kernel loads it once at boot and never changes it. Low
severity (it never affected control flow, only a diagnostic display),
but left unfixed it would have misled whoever next used this fault
screen to debug a real problem -- exactly the "missing link" this
project's own testing standard exists to catch before it ships.

### Fault-test hooks: proving the fault path catches a real exception, not just "nothing crashed yet"

Every verification up to this point proved the *absence* of a crash
during normal operation, which is not the same as proving the
GDT/IDT/fault-screen path actually works when a real CPU exception
occurs -- a kernel with an entirely absent or broken fault handler would
pass every one of those checks just as cleanly, right up until the
first genuine fault, which would then triple-fault silently with zero
diagnostic output. `Cargo.toml` gained a `fault-test-hooks` feature
(off by default, never enabled in the normal release binary) that adds
one hidden shell command, `__fault_test_int3` -- not listed in `help`,
not a name anyone would type by accident -- which issues a real `int3`
breakpoint instruction. `tests/fault_test.py` builds a binary with this
feature enabled into a fully isolated target directory and ESP
(`tests/out_fault/target`, `tests/out_fault/esp` -- it never touches the
project's real `./esp` or `./target`, so a feature-enabled binary can
never end up where `run.sh` or the ordinary acceptance test would pick it
up), boots it, types the hidden command, and asserts on the
`-debugcon` log that a `[CPU EXCEPTION] #3 Breakpoint` line was reported
*and* that this project's own boot markers (`[pmm] self-test OK`,
`[boot] interrupts enabled`) each appear exactly once -- proving the
machine didn't silently reset and reboot, which would otherwise produce
a second copy of both and could easily be mistaken for a clean run by
anyone not specifically checking for it. Verified passing, twice, after
the RSP/SS fix above (re-run specifically to confirm the fix didn't
regress the fault path itself).

### Hardening `tests/acceptance_test.py` for real content verification, not just "the screendump succeeded"

Before this phase, `acceptance_test.py` only ever asserted on the
*initial* boot banner's OCR text; every other step took a screendump and
declared success as long as the `screendump` monitor command didn't
error, with the resulting PNG saved for a human to look at later but
never actually asserted against. This was a real, serious gap, not a
hypothetical one: partway through this phase's own keyboard-driver
debugging, a fully non-functional interactive shell (keystrokes received
and decoded, but never delivered to the shell loop -- see the compiler
fence bug above) passed all 26 of the suite's steps, because every
screenshot happened to look identical to the pristine initial boot
screen and nothing checked otherwise. `assert_ocr()` (in
`acceptance_test.py`) now OCRs every step's screendump and asserts the
expected text actually appears -- the `help` command's full command
list, `echo`'s echoed text, `ls`/`ls APPS`'s exact filenames, the
backspace test's proof that deleted characters are genuinely gone from
the line buffer (not just visually erased), `clear`'s proof that
earlier text is actually wiped, each WISP demo's real output (`hello`'s
greeting by name, `oracle`'s prompt and quit lines, `tamer`'s
Erratic/Predictable classifications, `adventure`'s full room-by-room
text through every gating branch to `*** YOU WIN ***`, `xor_brain`'s
truth-table rows), and a new `uptime` step read twice with a real delay
between reads, asserting the tick count strictly increased -- a
positive, specific proof the PIT/IDT timer interrupt is actually firing
live, not just "the prompt appeared so it must be working." Getting
these assertions right took its own round of fixing: `wait_for_boot`'s
banner check still required the substring "Boot Services", left over
from before this phase's rename of the shell banner (now "Minimal
Custom Kernel Shell -- own GDT/IDT/PIC/PIT"), and would have hard-failed
every run; it now checks for "Custom Kernel" instead, still an
unambiguous discriminator against OVMF's own built-in shell banner. Two
of the new assertions were themselves initially wrong in ways worth
recording: the `ls`/`abcXX` checks first asserted exact substrings that
tesseract's OCR reliably misreads in this font (a lone lowercase "ls"
misread as "1s"; "abcXX" with its lowercase "c" misread as "E") and were
loosened to substrings robust to those specific, confirmed
misreadings; and the `uptime` regex first matched the *first* "N ticks"
occurrence on screen rather than the last, silently reading a stale
value still scrolled into view from an earlier command instead of the
freshest reading -- both are the kind of test-harness bug that produces
a false failure indistinguishable from a real one without opening the
actual screenshot to check, which is exactly what was done to confirm
each was a harness bug and not a kernel bug before fixing it.

**Verification.** The full acceptance suite, with all of the above
assertions active, was run five times consecutively after every fix
landed (three back-to-back clean passes are archived under
`tests/out_full7`/`out_full8` and two earlier ones since discarded to
save disk); every step's OCR content was checked, not just that its
screendump succeeded. `tests/fault_test.py` was run twice, confirming
the fault path both works and correctly reports state after the RSP/SS
fix. All temporary debugcon instrumentation added to `keyboard.rs` and
`shell.rs` while diagnosing the compiler-fence bug was removed once the
fix was confirmed, matching the already-clean state of `gdt.rs`,
`idt.rs`, and `main.rs`.

## Phase 2, Part A: our own page tables, guard pages, and a dedicated mainline stack

Phase 1 left this kernel running under whatever flat, identity-mapped
page tables UEFI's firmware built -- the CPU was already in long mode
(which *requires* paging to be on), but `CR3` still pointed at tables
this kernel never inspected and did not own. No protection existed: a
bad pointer write from a kernel bug, or a WISP interpreter bug, could
silently corrupt arbitrary physical memory instead of faulting cleanly.
`pmm.rs`'s frame allocator had existed since Phase 1 with zero real
consumers, waiting for exactly this. This phase builds a brand new,
hand-written 4-level page table structure, switches `CR3` to point at
it, adds real guard pages below both stacks this kernel owns, and moves
the shell/WISP interpreter off UEFI's original loader-provided stack
onto a dedicated one of its own.

**Architecture.** `src/paging.rs` (new) builds one flat, 4-KiB-page-only
identity map (virt == phys everywhere, matching what was already true
under firmware's tables) -- no huge pages, no higher-half kernel (the
`x86_64-unknown-uefi` target has no linker script and no fixed load
address, so a fixed high virtual base is real, separate future work).
Every region the post-`exit_boot_services` UEFI memory map reports gets
mapped, not just `pmm.rs`'s "usable" subset -- critically including
`LOADER_CODE`/`LOADER_DATA` (this kernel's own running image, which
`pmm` deliberately excludes from its free pool but which still must stay
mapped, or the very next instruction fetch after the `CR3` switch faults
into the kernel's own code). Regions typed `MMIO`/`MMIO_PORT_SPACE`, plus
the GOP framebuffer (passed in as an explicit extra range, since
firmware does not reliably describe it as its own memory-map entry), get
the cache-disable bit (`PCD`) set in addition to present/writable --
functionally invisible under QEMU, load-bearing on real hardware, and
needed again in Part B for the AHCI controller's MMIO registers. A
runtime `paging::map()` is exposed for exactly that future consumer.
Guard pages are built via a new `GuardedStack<N>` composite struct
(`#[repr(C, align(4096))] { guard: [u8; 4096], stack: [u8; N] }`) rather
than assuming "the page below this address happens to be unmapped" --
the guard page is part of the same object, guaranteed adjacent, which a
bare `#[repr(align(16))]` array (the original `DoubleFaultStack`) could
never guarantee. `paging::init`'s per-page walker skips any page falling
inside a caller-supplied guard range.

**Boot sequence, and risk staging.** The four riskiest changes in this
kernel's history so far landed as four separate, independently-verified
steps, never combined: (1) build the new tables via `paging::init`
without switching to them -- old firmware tables stay live throughout,
zero risk; (2) `paging::load` -- the actual `mov cr3` -- fully
permissive (every mapping RWX, no guard pages yet), so a regression here
can only ever mean "the mapping itself is wrong," never "the mapping is
right but a new restriction rejected something that needed access"; (3)
guard-page the double-fault stack only (lowest risk -- nothing runs on
IST1 except during an actual `#DF`); (4) take ownership of a dedicated,
guard-paged mainline stack via a live, one-way RSP switch. The final
boot order: `pmm::init` -> `pmm::self_test` -> `paging::init` ->
`gdt::load`/`idt::load` (our own fault handler is the live safety net
*before* the risky switch, not after) -> `paging::load` -> the mainline-
stack switch -> `pic::init`/`pit::init` -> `sti` -> `shell.run`. Each
step was re-verified against the full, unmodified acceptance suite
multiple consecutive times before the next step began -- see
"Verification" below.

**The mainline-stack switch.** Until this phase, the shell and every
WISP script ran on whatever stack UEFI's own PE loader happened to hand
this image -- never explicitly owned, sized, or protected. `main.rs` now
boxes the live `Console` and the in-memory `APPS` snapshot (moving both
off the stack and onto the heap, so nothing this kernel still needs is
left referencing the stack about to be abandoned), then hands a single
pointer to `switch_stack_and_continue` -- a `#[unsafe(naked)]`
trampoline that loads the new stack's top into RSP and `call`s forward
into `continue_boot_on_new_stack`, reusing `idt.rs`'s already-proven
"reserve 32-byte MS x64 shadow space, then `call`" pattern rather than
inventing a new `jmp`-based one. Ordinary Rust code cannot do this
safely: any statement between "change RSP" and "start using the new
stack" would run with a stack pointer the compiler doesn't know changed.
`continue_boot_on_new_stack` self-verifies immediately by reading RSP
back and logging it to debugcon before doing anything else, then runs
the rest of boot (PIC/PIT bring-up, enabling interrupts, handing off to
the shell) entirely on the new stack.

**Fault-test coverage (`tests/fault_test.py`, extended to four
scenarios).** Beyond the existing `int3` breakpoint case,
`shell.rs` gained three more hidden, `fault-test-hooks`-gated commands:
`__fault_test_pagefault` (a volatile write to a deliberately unmapped,
canonical address 16 TiB up, nowhere near this kernel's actual few-GiB
memory map) proves the new tables actually enforce "unmapped means
unmapped," not just "whatever firmware happened to leave unmapped."
`__fault_test_guard_overflow` and `__fault_test_stack_overflow` are both
`#[unsafe(naked)]` downward-walking loops (deliberately not Rust
recursion -- at this project's `opt-level = "s"`, `lto = true` profile,
naive unbounded recursion is exactly the shape LLVM's tail-call/loop-
conversion optimizations target, which could silently turn the test into
a non-growing infinite loop that never faults) that probe off the bottom
of the mainline stack into its guard page. They differ in one detail
that turned out to matter a great deal (see the postmortem below):
`__fault_test_guard_overflow` probes through a scratch register,
leaving RSP untouched, while `__fault_test_stack_overflow` walks RSP
itself, simulating a genuine runaway-recursion stack overflow.
`paging::init` now also logs every guard range's exact bounds to
debugcon at boot, so the Python test can assert `cr2` against
kernel-reported ground truth instead of a guessed or hardcoded address.

### Postmortem: the guard-overflow test's first version produced `#8 Double Fault`, not `#14 Page Fault` -- and that turned out to be a second, real, valuable test

The first version of `__fault_test_guard_overflow` walked RSP itself
downward (`sub rsp, 4096` in a loop, then wrote to `[rsp]`). Run against
the real kernel, it produced `#8 Double Fault`, not the expected `#14
Page Fault`. This was not a bug in the guard page -- it's correct CPU
behavior: by the time the probing write actually touched the guard page,
RSP itself already pointed inside it, so the CPU's own exception-
delivery frame push for the resulting `#PF` (which happens at the
*current* RSP) also faulted, escalating to `#DF` per the documented
contributory-exception combinations (Intel SDM Vol 3, 6.15). That's
real, and it's actually the exact disaster scenario `gdt.rs`'s own
module doc comment describes as the reason `DOUBLE_FAULT_STACK`/IST1
exist at all -- but it's a *different* thing than "prove the guard
mapping itself is genuinely absent," which was this specific test's
purpose, and conflating the two would have meant a broken guard mapping
and a broken IST1 stack could each mask the other's failure.

The fix: split into two tests. `__fault_test_guard_overflow` now probes
through a scratch register (RAX), leaving RSP valid throughout, so the
CPU can push its `#PF` exception frame normally and the test gets a
clean, isolated proof (`#14`, with `cr2` landing exactly inside the
mainline stack's guard range as logged by the kernel itself). A new,
second test, `__fault_test_stack_overflow`, keeps the original
RSP-walking version specifically *because* it produces the cascading
`#DF` -- this is real, valuable coverage that did not exist before:
`DOUBLE_FAULT_STACK`/IST1 had been wired up since the "Leaving Boot
Services" phase but never actually exercised by a real fault until now.
Confirmed: `tests/fault_test.py`'s fourth scenario boots, triggers a
genuine runaway "stack overflow," and confirms the resulting `#DF` is
caught and reported (IST1 correctly forced the stack switch needed to
push its own exception frame) rather than the machine silently
triple-faulting into a hard reset with zero diagnostic output -- which
is exactly what would happen without a working guard page and IST1
working together correctly.

### Postmortem: two apparent Step 4 test failures, both tesseract misreads, neither a kernel bug

Immediately after landing the mainline-stack switch (the single riskiest
step in this phase), repeated acceptance-suite runs surfaced two
different-looking failures in the `uptime` step, at roughly a 1-in-9
rate: once, `second_ticks <= first_ticks` (`1931 -> 1931`, "the tick
count did not increase"); once, an `IndexError` because the tick-count
regex found zero matches at all despite `assert_ocr`'s looser "ticks"
substring check passing. Both looked, on the surface, exactly like the
kind of real regression this phase was most worried about introducing
(interrupts silently not being delivered correctly from the new stack).

Rather than assume either conclusion, both failing runs' actual saved
screenshots were opened directly and inspected. Both showed a perfectly
correct, cleanly rendered line: `up 20.200s (2020 ticks)`. Running
`tesseract` directly against the saved PNG confirmed the actual OCR
output: `up 20.2008 {2020 ticks)` -- tesseract had misread this bitmap
font's `(` as `{` in this exact position, consistently, every retry
against the same unchanged pixels within a run. That's the same class of
bug already on record in this file (the "ls" vs "1s" and lowercase-`c`-
vs-`E` misreadings, and the `xor_brain` truth-table digit misreadings)
-- a confirmed, specific OCR ambiguity, not a kernel defect. Conclusive,
independent confirmation: `tests/fault_test.py`'s `int3` scenario, and
every other acceptance step exercising the new stack extensively
(keyboard input, four different WISP scripts including recursive/looping
ones, `pic`/`pit` bring-up), passed cleanly throughout the exact same
run that hit this failure -- a real interrupt-delivery bug would not be
this selective.

The fix, `tests/acceptance_test.py`'s new `read_uptime_ticks()` helper:
the tick-extraction regex now accepts either `(`/`)` or `{`/`}` as the
same unambiguous on-screen glyph (`[({](\d+)\s*ticks[)}]`), plus a
bounded retry against fresh screendumps of the same, unchanged screen as
defense in depth against any other still-unknown transient misread
class. This is the same *kind* of fix as every prior OCR-ambiguity
postmortem in this file: loosen what OCR is allowed to read as a match
for a glyph that's already confirmed, by direct pixel inspection, to be
rendering correctly -- never loosen what the kernel is actually required
to prove. Verified: 12 consecutive clean acceptance-suite runs after the
fix landed, with zero recurrences.

### NX / `CR0.WP` permission tightening: deliberately deferred, not forgotten

The Phase 2 plan scoped `EFER.NXE` + per-page NX bits and `CR0.WP`
enforcement as an explicit stretch goal, landed only after the base
identity map and both guard pages were independently proven solid --
never bundled into the same untested step as the riskiest changes above.
That base work (own tables, real guard pages, a dedicated protected
mainline stack) is what actually matters most for this phase's goal and
is now fully shipped and verified; NX/`CR0.WP` remains unimplemented,
explicitly, as real follow-up work rather than a silently dropped item.
Every mapping this kernel builds today is still present+writable (RWX in
practice, since NX is never set): a bug that jumps into data or writes
to code is not yet caught by hardware, only bugs that touch genuinely
unmapped memory are. This is a real, known gap, not an oversight.

**Verification.** Every one of the four Build Order steps (table build,
`CR3` switch, double-fault guard page, mainline-stack switch) was
verified against the full, unmodified `tests/acceptance_test.py` (all 26
original steps plus the new "paging + mainline stack active" debug-log
step) multiple consecutive times before the next step began -- the `CR3`
switch and the mainline-stack switch, the two highest-risk steps, each
got three consecutive clean runs on their own before any further work
proceeded. After the OCR-flake fix above, twelve further consecutive
clean acceptance-suite runs confirmed zero recurrence. `tests/
fault_test.py`'s four scenarios (`int3`, deliberate page fault, isolated
guard-page probe, real stack overflow escalating to `#DF`) all pass,
each asserting the exact expected exception vector and, where
meaningful, `cr2` against kernel-reported ground truth rather than a
guessed address. `paging::init`'s and `gdt.rs`'s new guard-range
debugcon logging make every one of these assertions traceable to a
specific, reported physical address range rather than an assumption
about link-time layout.

## Phase 2, Part B: AHCI persistent storage

Until this phase, nothing a WISP program did could survive a reboot.
`fs::snapshot_apps` reads `APPS/*.wisp` into RAM once, before
`exit_boot_services`, and that was the only filesystem access this
kernel would ever perform -- the boot medium itself stays read-only,
deliberately, forever (see the vvfat-corruption postmortem above). This
phase gives the kernel a second, entirely separate disk, attached via a
hand-written AHCI driver, purely for save data: a genuine, from-scratch
storage stack -- PCI enumeration, HBA/port bring-up, raw sector
read/write, a hand-designed on-disk key/value format, and `save()`/
`load()` builtins in WISP itself -- with the boot-USB `-drive`/`-device
usb-storage` pair in `run.sh`/`tests/acceptance_test.py` staying
byte-for-byte unchanged throughout. Zero networking, as ever, was never
on the table; this is local, additive storage only.

**Why AHCI, not USB mass storage or UEFI variables.** Three transports
were weighed before implementation started. A full USB mass-storage
stack (a UHCI/EHCI/xHCI host controller driver plus bulk-only transport
plus SCSI) is real engineering effort on the order of everything built
so far combined, for a payoff no bigger than a second disk gives more
simply. UEFI variable-store persistence is low-risk but caps out at a
small NVRAM-sized key/value store -- not real storage, and this kernel
already left Boot Services (and with it, any live UEFI protocol calls)
behind in an earlier phase. AHCI -- a second, separate virtual disk,
QEMU's `ich9-ahci` plus an `ide-hd` -- won: it maps directly onto "a
real machine with a SATA/NVMe drive alongside the boot stick," it is
architecturally simple enough for one driver to own end to end (single-
outstanding-command, no NCQ, no port multipliers), and it gave this
project its first-ever write path to genuinely persistent storage.

**`src/pci.rs` (new).** Legacy `0xCF8`/`0xCFC` config-space I/O, a flat
bus-0 enumeration (device 0-31 x function 0-7, multi-function probed
only when function 0's header-type bit 7 says so -- no PCI-to-PCI
bridge recursion, the same "no APIC, single core" scope reduction this
project already applies elsewhere). `find_ahci_controller` matches by
classification (`class:subclass:prog_if == 01:06:01`), never a
hardcoded vendor:device ID, so this recognizes any spec-conformant AHCI
controller, not just QEMU's. `dump_devices` logs every discovered
device to debugcon (`bus:device.function`, `vendor:device`,
`class:subclass:prog_if`, header type) -- the same "boot diagnostics
first" discipline `paging::init`'s guard-range logging already
established, and what both the interactive verification and the new
`tests/ahci_test.py` grep to confirm the controller was actually found.

**`src/ahci.rs` (new, the largest and most involved driver in this
project so far).** `init()` finds the controller via `pci.rs`, sets Bus
Master + Memory Space Enable on it unconditionally (never assumes
firmware already did), reads BAR5 (ABAR -- architecturally a 32-bit,
non-prefetchable memory BAR for AHCI specifically, so no 64-bit BAR-pair
handling is needed), and maps it via Part A's `paging::map` with
`PageFlags::Mmio` (cache-disable set -- functionally invisible under
QEMU, load-bearing on real hardware). A full HBA reset (`GHC.HR`, AHCI
1.3.1 SS10.4.3) puts the controller in a known state regardless of what
firmware or a previous boot left behind, and re-confirms `GHC.AE` stays
set afterward (a reset clears it as a documented side effect).
`bring_up_first_available_port` walks every bit set in `PI` (Ports
Implemented), spins up each port, polls `PxSSTS.DET` for a device
present with Phy communication established, allocates and zeroes that
port's Command List / FIS Receive area / Command Table / a scratch DMA
data buffer (one `pmm::alloc_frame()` page each, physically addressed
since this kernel's whole memory model is virt == phys), wires command
header 0's Command Table Base Address, and starts the command engine
(`FRE` before `ST`, per spec ordering). A port whose signature isn't
plain SATA (`PxSIG != SIG_ATA` -- an ATAPI drive, a port multiplier) is
skipped, not treated as fatal, and the next implemented port is tried.
`issue_ata_command` hand-builds a 20-byte Register H2D FIS and a single
PRDT entry, rings the doorbell, and polls `PxCI`/checks
`PxTFD.STS.ERR` -- single-outstanding-command, polling-only completion,
deliberately no NCQ, matching the same risk-reduction philosophy as
"no APIC." IDENTIFY DEVICE (`0xEC`), READ DMA EXT (`0x25`), and WRITE
DMA EXT (`0x35`) -- all LBA48 -- are the only three ATA commands this
driver speaks. `Hba::identify` validates word 83 (LBA48 support) and
word 106 (512-byte logical sectors) against ATA8-ACS's own `0b01`
validity convention before trusting either, and extracts the exact
48-bit total sector count from words 100-103. `Hba::self_test`
exercises `write_sector`/`read_sector` against the disk's own *last*
LBA on every boot (never LBA 0, reserved for `save_format.rs`'s
superblock) -- read-remember, write a pattern, read back and verify,
then restore the original contents and verify the restore too, the same
"prove it, every boot" standard `pmm::self_test` already set for the
frame allocator, and non-destructive against a real save disk that
already holds real data.

**Bounded timeouts, everywhere, on purpose.** Every AHCI wait loop
(`poll_until`) takes a real deadline computed from `pit::ticks()`, never
an unbounded spin -- "no AHCI storage present" (no controller, no
implemented ports, no device on a given port) is explicitly a
first-class, non-fatal, boot-continues outcome per the Phase 2 plan, not
an afterthought, and a hardware condition that never resolves must
produce a reported timeout, not a silent hang. This bounded-timeout
design is also exactly what the postmortem below is about.

**`src/save_format.rs` (new): a hand-designed key/value store, not a
filesystem.** Writable FAT32 needs synchronized dual FAT tables,
cluster-chain allocation, and long-filename directory entries -- real
effort with no user-visible payoff, since nothing needs a hierarchical
filesystem and `APPS/*.wisp` is still served from the read-only RAM
snapshot this format never touches. LBA 0 is a superblock (magic
`"WISPSAVE1"`, format version, slot count, slot size); LBA 1 onward is
64 fixed 4 KiB slots (in-use flag, a 32-byte zero-padded key, a `u32`
value length, an FNV-1a/32 checksum, then the value bytes) -- `save`/
`load` do a plain linear scan, adequate at 64 slots, no index needed.
Values use a minimal type-tagged binary encoding (one tag byte then a
type-specific payload) for `Nil`/`Bool`/`Int`/`Str` directly, and `List`
recursively, bounded by `wisp::value::MAX_VALUE_DEPTH` (widened from
private to `pub(crate)` so this module reuses the interpreter's own
existing recursion guard rather than inventing a second one). `save`
returns an explicit error -- never silently truncates -- if a key is
empty/too long or a value's encoded form exceeds what a slot can hold; a
decoded list's claimed element count is rejected outright if it's larger
than physically possible for the remaining bytes, before ever looping
over it, so corrupt on-disk data can't be walked as if it were trusted
input. The full byte-level spec -- every field's exact offset, a
worked example with real, hardware-verified bytes, and what this
format deliberately doesn't attempt (wear leveling, transactions,
namespacing) -- is `docs/SAVE_FORMAT.md`, matching `docs/
WISP_LANGUAGE.md`'s standard for the language itself.

**WISP integration.** `wisp::interp::Host` gained
`save(&mut self, key: &str, value: &Value) -> Result<(), &'static str>`
and `load(&mut self, key: &str) -> Result<Option<Value>, &'static str>`,
implemented by `shell.rs`'s `ShellHost` by delegating straight to
`save_format.rs`. `builtins.rs` wires up matching `save(key, value)`/
`load(key)` WISP builtins; `load` returns WISP `nil` for "never saved"
-- a first-class outcome, not an error, matching the language's existing
"no separate undefined type" design (a key saved with the value `nil`
itself is indistinguishable from never-saved -- a documented,
deliberate simplification). The shell also grew its own, WISP-
independent `save <key> <value>`/`load <key>` commands (mirroring
`echo`'s "rest of the line, verbatim" shape) specifically so a driver
bug and an interpreter-plumbing bug can never be confused for each
other -- exactly the second verification surface the Phase 2 plan called
for, and what caught real issues faster during development than routing
every check through a WISP script would have.

`main.rs`'s new `bring_up_save_disk` chains `ahci::init` ->
`identify` -> a diagnostic `read_sector(0)` + hex dump ->
`self_test` -> `save_format::ensure_formatted`, short-circuiting to
`None` (persistent storage unavailable, boot continues) the moment any
step fails, and only handing the live `Hba` to `Shell::new` once every
step has actually succeeded. `esp/APPS/counter.wisp` (new) is the
project's own end-to-end proof and payoff: `load("boot_count")`,
branch on `nil` (first run) vs. a real prior value ("Welcome back."),
increment, save, print a correctly pluralized "You have run this N
time(s)." -- the simplest possible demonstration that state written on
one boot is read back, correctly, on a completely different boot.

### Postmortem: a genuine hang bug, caught only because this project insists on testing its own documented failure paths

The Phase 2 plan explicitly requires exercising "no AHCI storage
present" as a first-class scenario, not just the happy path. Testing it
-- an AHCI controller attached with no device wired to any of its ports
-- hung the boot forever instead of reporting the clean, bounded
timeout `ahci.rs`'s own `poll_until` was written to guarantee.

Root cause: `ahci::init()` was being called from `main.rs`'s
`continue_boot_on_new_stack()` *before* `sti` ran. Every AHCI wait loop
computes its deadline as `pit::ticks() + N`, and `pit::ticks()` only
ever advances inside the PIT's own interrupt handler -- with interrupts
still masked, `pit::ticks()` was frozen at `0` for the entire AHCI
bring-up sequence, so `deadline` never arrived and never could. On real
hardware with a device on every port this would have stayed invisible
indefinitely, since every `poll_until` call would resolve `true` almost
immediately regardless of whether the clock was ticking -- it took the
specific, deliberately-required "nothing responds" scenario to surface
a bug that had been sitting in the boot sequence the whole time. This is
the second real, previously-undiscovered kernel bug this project has
caught purely through its own "test every documented failure path, not
just the happy one" discipline -- the first being the guard-overflow-
vs-double-fault distinction from Part A above.

The fix: move `sti` (and the "Interrupts, timer, and keyboard online."
banner) to immediately after `pic::init()`/`pit::init()`, strictly
before any PCI/AHCI work runs -- interrupts are live, and `pit::ticks()`
is advancing, for the entire rest of boot from that point on. Verified:
re-running the exact same "controller present, no device on any port"
scenario afterward now shows every implemented port (all 6, on QEMU's
`ich9-ahci`) correctly timing out and logging "no device detected,
skipping" within its bounded deadline, then a clean "persistent storage
unavailable" report and a normal boot to a working shell prompt --
confirmed both via debugcon log inspection and a direct screenshot
view, not OCR alone. The "no AHCI controller at all" scenario was
re-verified unaffected by the reordering, and this exact scenario --
"an AHCI controller is present but no drive is wired to any of its
ports" -- is now permanent, standing regression coverage as
`tests/ahci_test.py`'s `controller_no_disk` case, specifically called
out in that test's own doc comment as a guard against this bug ever
coming back silently.

**Verification.** Every Build Order step (PCI enumeration; ABAR
mapping and HBA generic registers; HBA reset, port bring-up, and the
"no device present" soft-fail path; IDENTIFY DEVICE against a known 64
MiB image; a single-sector read verified byte-for-byte against a
host-written pattern; a single-sector write + readback + restore, this
project's first-ever write to real persistent storage; `save_format.rs`
on top of the shell's own `save`/`load` commands; the WISP `save()`/
`load()` builtins; `counter.wisp`) was independently verified before
the next began, matching this project's standing "never trust a single
clean run" discipline. Two new, permanent automated suites now cover
this whole stack:

- `tests/ahci_test.py` (new, isolated build/ESP/target-dir, matching
  `tests/fault_test.py`'s pattern): three independent scenarios, each a
  fresh boot -- no AHCI device at all (the PCI-level soft fail); an AHCI
  controller with no drive on any port (the exact scenario that
  surfaced the sti-ordering bug above, now permanent regression
  coverage); and a full-stack run against a real 64 MiB disk pre-seeded
  with a known, non-trivial byte pattern, asserting PCI enumeration,
  ABAR/HBA registers, port bring-up and signature, IDENTIFY's exact
  sector count, a byte-for-byte raw-read match against the seeded
  pattern (reconstructed from `ahci::debug_dump_sector`'s hex-dump log
  lines), the write/read-back/restore self-test, `save_format`
  formatting, a live shell `save`/`load` round trip, and -- read
  directly from the disk image on the host after a clean shutdown, not
  just trusted from the driver's own claim -- the exact on-disk
  superblock and slot bytes `save_format.rs`'s own module doc comment
  specifies. Run three times consecutively during this phase, clean
  every time, with an identical FNV-1a/32 checksum each run (proof of
  determinism, not just "it passed").
- `tests/acceptance_test.py`, extended with a genuine two-boot
  persistence scenario: run `counter` against a fresh `save.img` inside
  the existing, single long-lived acceptance VM (asserting the
  first-run branch and a saved count of 1), cleanly shut that VM down,
  then boot a **second, entirely separate** QEMU process against the
  exact same, never-recreated `save.img` file and run `counter` again
  (asserting "Welcome back." and a persisted count of 2) -- concrete,
  specific proof of real on-disk persistence across a genuine reboot,
  not RAM state that happened to survive within one process. Verified
  visually as well as via OCR (the second boot's screenshot was opened
  directly and confirmed to show the correct persisted count). Run
  three consecutive full clean passes (all 30 steps, including the new
  two-boot counter check) with zero regressions to any of the original
  steps.
- `tests/fault_test.py`'s existing four scenarios were re-run twice
  during this phase (once after `counter.wisp` landed, once after the
  `tests/ahci_test.py`/`tests/acceptance_test.py` changes that share its
  `Monitor`/`wait_for_boot` imports) with zero regressions -- this
  phase's changes to the boot-time interrupt-enable ordering are
  exactly the kind of change that could plausibly have affected fault
  delivery, so re-confirming this suite specifically (not just
  AHCI-focused tests) mattered here.

## Full source-tree reconstruction from the project's stored knowledge base, and two latent bugs it surfaced

The entire working tree (every kernel module, the WISP interpreter, all
docs, all three test suites, the install scripts, and every demo
`.wisp` app) was rebuilt from scratch in a fresh environment, sourced
file-by-file from this project's own knowledge base rather than from an
existing checkout. Rebuilding from that stored state -- rather than
copying a known-good working directory -- is exactly the kind of full,
independent replay that surfaces drift between what was last *saved*
and what was last *actually running*, and it caught two real bugs this
time:

**1. `gfx::Console` was missing `raw_parts()` entirely -- a build
break.** `main.rs` calls `console.raw_parts()` at two points (registering
the raw framebuffer with `panic::register` for the fault-screen path, and
again after `exit_boot_services` to build `paging::init`'s `extra_ranges`
for the GOP framebuffer's MMIO identity mapping), and `paging.rs`'s own
module doc comment references `Console::raw_parts` as an established,
already-existing method. The stored `gfx.rs` had no such method on
`Console` at all -- a plain `cargo build` failed immediately with
`E0599: no method named 'raw_parts' found`. The stored `gfx.rs` was, in
other words, a version that predates a later change to `main.rs`/
`paging.rs` that was never re-saved back to this project's knowledge
base afterward. Fixed by adding the accessor exactly where the two call
sites need it: a `&self` method returning
`(fb, fb_len, width, height, stride, bgr)` in precisely the tuple order
`panic::register`'s parameter list expects and `main.rs` destructures
into. Verified by a clean release build, a clean `fault-test-hooks`
feature build, and (transitively, since nothing else exercises the
framebuffer/paging path more thoroughly) all four `tests/fault_test.py`
scenarios and the full `tests/acceptance_test.py` run passing end to
end, including its OCR verification of real on-screen text -- proof the
newly-added accessor hands back the exact bytes the console actually
draws through, not merely bytes that satisfy the type checker.

**2. `tests/fault_test.py` was missing the `acceptance_test.OUT_DIR`
monkey-patch that `tests/ahci_test.py` already carries -- a latent,
order-dependent test bug.** `Monitor.screendump()` and `wait_for_boot()`
(both defined in `acceptance_test.py`) resolve `OUT_DIR` as a free
variable in *that module's* namespace, not the caller's -- Python
closes over the defining module, not the call site. `tests/ahci_test.py`
already patches `acceptance_test.OUT_DIR = scenario_dir` before creating
each scenario's `Monitor`, with a comment explaining exactly why; `tests/
fault_test.py` never did. On a repository where `tests/out/` (`accep
tance_test.py`'s own default `OUT_DIR`) already exists -- true of every
development machine that had ever run the acceptance suite even once --
this was invisible: `screendump()` would silently write each fault
scenario's screenshot into `tests/out/` instead of its own `tests/
out_fault/<scenario>/` directory, misfiled but not missing, so nothing
ever failed loudly enough to get noticed. On this fresh reconstruction,
with no prior `tests/out/` on disk, QEMU's monitor `screendump` command
failed silently against the nonexistent directory and the very next
line -- `Image.open(ppm_path)` -- raised `FileNotFoundError` instead,
which is what actually surfaced this. Fixed by adding the identical
monkey-patch (and the identical explanatory comment) `tests/ahci_test.py`
already established, immediately before each scenario's `Monitor` is
constructed. Verified: all four scenarios (`int3`, `pagefault`,
`guard_overflow`, `stack_overflow`) now pass, each with its screenshot
correctly landing under its own `tests/out_fault/<scenario>/` directory.

**Full verification after both fixes:** a clean `cargo +nightly build
--release --target x86_64-unknown-uefi` and an isolated
`--features fault-test-hooks` build, both with zero warnings; all four
`tests/fault_test.py` scenarios (breakpoint, deliberate page fault with
`cr2` matching the exact target address, guard-page overflow with `cr2`
falling inside the mainline stack's logged guard range, and a real stack
overflow correctly escalating to `#8` Double Fault via IST1); all three
`tests/ahci_test.py` scenarios (no controller, controller with no
device on any of the six implemented ports, and the full PCI -> ABAR ->
port -> IDENTIFY -> raw sector read/write -> `save_format` stack against
a real 64 MiB disk, including a byte-for-byte match against a seeded
pattern and an on-disk checksum -- `0x140fd6b3` for `save("greeting",
"hello world")` -- that matches `docs/SAVE_FORMAT.md`'s own worked
example exactly); and the complete 30-step `tests/acceptance_test.py`
run, including the two-boot cross-reboot persistence proof for
`counter.wisp` (count saved as 1 on a fresh disk, a second, wholly
separate QEMU process booted against that same disk image reading it
back as 2). Several of the resulting screenshots -- the boot banner, the
second boot's "Welcome back. You have run this 2 times." line, the full
`adventure.wisp` walkthrough including its dark-room and locked-door
gating, the `tamer.wisp` neural net correctly classifying a mismatched
`(8, 2)` stat pair as "Erratic," and `xor_brain.wisp`'s full four-row
truth table -- were also opened and read directly, not trusted from OCR
alone, matching this project's standing "OCR is a convenience, pixels
are the ground truth" discipline from the xor_brain and uptime-ticks
postmortems earlier in this log.

## WISP v2, Part 1: pixel/graphics primitives

The most prominent scope-cut called out in `docs/WISP_LANGUAGE.md`'s
"What's not in v1" ("No pixel/graphics/sprite drawing API (planned v2)")
is closed: seven new builtins -- `screen_width`, `screen_height`, `rgb`,
`plot`, `rect`, `line`, `fill_screen` -- let a WISP script draw directly
into the same GOP framebuffer `gfx.rs`'s text console has always
rendered into. Full spec in `docs/WISP_LANGUAGE.md`'s new "Graphics"
section and, in depth, the new `docs/WISP_GRAPHICS.md`; two new demo
programs, `esp/APPS/bounce.wisp` (a short automatic bouncing-square
animation) and `esp/APPS/canvas.wisp` (an interactive tool exercising
every one of the seven builtins), ship alongside the existing ones.

**Where the new code lives.** `wisp/` stayed exactly as decoupled from
the rest of the kernel as it already was: `wisp::interp::Host` gained six
new methods (`screen_width`/`screen_height`/`plot`/`draw_rect`/
`draw_line`/`fill_screen`), all scalar primitives -- no `gfx::Color`, no
`uefi` types -- matching the trait's existing "everything crosses this
one boundary as a plain value" discipline. `wisp/builtins.rs` gained the
seven builtin functions plus their argument validation, `gfx.rs` gained
the actual bounds-checked pixel-writing primitives (`plot_pixel`,
`draw_rect_filled`, `draw_line`, `fill_screen`, plus `width_px`/
`height_px` accessors and `Color::from_rgb24`), and `shell.rs`'s
`ShellHost` bridges the two exactly as it already did for `save`/`load`.
No changes were needed anywhere in `wisp/lexer.rs`, `wisp/parser.rs`, or
`wisp/ast.rs` -- WISP function calls were already fully generic
(`Expr::Call { name, args }`, resolved by name at runtime in
`Interp::call_named`), so adding new builtins never touches syntax at
all, only `is_builtin`/`call`'s dispatch table.

**Color is a packed `int`, not a new `Value` variant.** `rgb(r, g, b)`
packs three `0..=255` channels into a single `0xRRGGBB` integer -- WISP
has no bitwise operators to build one by hand, so `rgb()` (or a raw
literal, e.g. `16711680` for pure red) is the only way to get one at the
language level. This fits inside the existing five-value-type language
(`docs/WISP_LANGUAGE.md`'s "Values" section is unchanged) rather than
growing a sixth type for one feature. The packed value crosses the
`Host` boundary as a plain `u32` and is only ever turned into a real
`gfx::Color` on the native side (`Color::from_rgb24`, in `ShellHost`'s
impl) -- `Console::encode`'s existing BGR-vs-RGB hardware-layout
translation, previously only exercised by the four hardcoded text-console
colors, now also handles every color a WISP script constructs at
runtime, unmodified.

**Bounds safety: `debug_assert!` doesn't protect a release build.**
Every coordinate reaching these primitives is untrusted, script-supplied
input -- unlike the pre-existing `put_pixel`, whose callers (`draw_glyph`,
`scroll_up_one_row`) can only ever pass in-range coordinates derived from
the console's own character grid, and which therefore only
`debug_assert!`s its bounds. This project's `[profile.release]`
(`Cargo.toml`) sets neither `debug-assertions` nor `overflow-checks`, so
it gets the standard release defaults for both: `debug_assert!` compiles
to nothing at all, and integer overflow silently wraps instead of
panicking. A script-controlled out-of-bounds pixel write under those
defaults would be a real out-of-bounds pointer write, not a caught
assertion. Every new drawing primitive does a real, always-compiled `if`
bounds check instead (`plot_pixel`'s `x < 0 || y < 0` / `x >= width ||
y >= height`, `draw_rect_filled`'s saturating-arithmetic clip), and none
of them uses plain `.abs()` on a script-supplied `i64` --
`i64::MIN.abs()` cannot be represented and, under this release profile,
silently wraps back to a *negative* result instead of panicking, which
would defeat a magnitude check rather than merely skip it.
`.unsigned_abs()` (correct for every possible `i64` input, including
`i64::MIN`) is used throughout instead.

**Why `line()` alone validates coordinate magnitude.** `plot()` touches
exactly one pixel regardless of how far off-screen `(x, y)` is, and
`rect()` clips its extent to the screen *before* it starts filling, so
both are safe -- and fast -- for any input. Bresenham's line algorithm has
no equivalent shortcut: its cost is `max(|x2-x1|, |y2-y1|)`, a property of
the endpoints themselves, before any clipping against the screen even
happens. `builtins::builtin_line` rejects any coordinate farther than
200,000 from the origin with a clear runtime error before the interpreter
ever reaches `Console::draw_line` (never a screen this project targets
comes anywhere near that far across, so this is essentially always a
script bug, not a legitimate value); `Console::draw_line` also enforces
its own, much larger backstop (500,000 steps) independently, so the walk
itself can never run unbounded even if some future caller reaches it
without going through that validation. No floating point was introduced
anywhere in the kernel to make this safe -- the algorithm stays
integer-only throughout, deliberately: this project has never exercised
FPU/SSE exception handling end to end the way `idt.rs`'s other fault
paths are proven by `tests/fault_test.py`, so borrowing floating point as
a hidden implementation detail here, even one never surfaced to a
script, would have been jumping ahead of infrastructure this project
hasn't actually built or verified yet.

**A real bug this surfaced: `fill_screen()` erases previously-printed
text.** An early draft of `canvas.wisp` printed its three-line startup
banner and *then* called `fill_screen()` to paint a background before
entering its main loop. `tests/acceptance_test.py`'s new step 15a caught
this immediately: OCR of the post-boot screenshot found only the bare
`canvas>` prompt, none of the banner text that should have preceded it.
Root cause: text and graphics draw into the exact same framebuffer (there
is no separate layer, no compositing), so `fill_screen()` painted over
every pixel of the banner the instant it ran, regardless of the fact that
the banner had been "already printed" moments earlier in program order --
"already drawn" carries no special protection once something else draws
over the same pixels. Fixed by reordering `canvas.wisp` to fill the
background *before* printing the banner, not after; `docs/WISP_GRAPHICS.md`
now documents the general rule this illustrates ("draw your background
before printing text you want to stay visible, never after") as a named
section, not just a code comment, specifically so the next script author
(human or LLM) hits the documentation before hitting the bug.

**Verification.** A clean `cargo +nightly build --release --target
x86_64-unknown-uefi`, an isolated `--features fault-test-hooks` build,
and `cargo +nightly clippy --release --target x86_64-unknown-uefi -- -W
clippy::all`, all with zero warnings anywhere in the new code (every
warning clippy did report was pre-existing, in unrelated code, confirmed
by cross-checking line numbers against the pre-change file). All four
`tests/fault_test.py` scenarios and all three `tests/ahci_test.py`
scenarios still pass unmodified -- no regression from this feature
touching `gfx.rs`/`shell.rs`. `tests/acceptance_test.py` gained two new
steps that boot the real kernel and drive the real interpreter over the
QEMU monitor exactly like every other step in that file: step 14 runs
`bounce.wisp` and asserts its two, deliberately far-apart colors
(background and ball) are actually present in a mid-flight screenshot,
then waits for its "finished" message; step 15 runs `canvas.wisp`
interactively (`plot`, `rect`, `line`, each followed by a real, sampled
pixel-color assertion at the exact coordinates this test chose -- not
just OCR'd text) and deliberately triggers `rect()`'s negative-size
error, confirming both the clean error message and that the shell prompt
is fully responsive again afterward. `assert_colors_present` and
`assert_pixel_near` (new helpers in that file) are the first place this
project's acceptance suite checks actual on-screen pixel colors rather
than only OCR text or the debug log -- a strictly stronger proof for a
feature whose entire purpose is "the right-colored pixels end up in the
right place." Every new screenshot was also opened and visually
confirmed by direct inspection (a crisp orange square on navy for
`bounce.wisp`; a correctly-positioned white dot, green rectangle, and
yellow vertical line, plus the red error line and the shell prompt
underneath it, all simultaneously legible for `canvas.wisp`), matching
this project's standing "OCR/pixel-assertions are a convenience, the
actual image is the ground truth" discipline.


## Full source-tree reconstruction (second time), and what it confirmed

Before any of the work below started, the entire working tree was rebuilt
from scratch in a fresh environment, file by file, out of this project's
own stored knowledge base -- the same independent replay that caught two
real bugs the last time it was done (see the previous entry). This time
it caught none: every file rebuilt cleanly, `cargo +nightly build
--release --target x86_64-unknown-uefi` produced a zero-warning binary on
the first attempt, and all three existing suites passed unmodified
(`tests/acceptance_test.py`'s full 30 steps including the two-boot
cross-reboot persistence proof, `tests/fault_test.py`'s four scenarios,
`tests/ahci_test.py`'s three).

One detail is worth recording as evidence rather than just asserted:
`tests/ahci_test.py`'s `full_stack` scenario reported the on-disk FNV-1a/32
checksum for `save("greeting", "hello world")` as `0x140fd6b3` -- byte for
byte the same value `docs/SAVE_FORMAT.md`'s worked example documents and
the previous reconstruction recorded. A save format reassembled from
stored source, running on a freshly built kernel, producing the exact
documented bytes is a much stronger statement about the reconstruction's
fidelity than "it compiled."

## Phase 3: real hardware -- ACPI, the APIC pair, and a USB keyboard

Every interrupt this kernel had ever taken came through the legacy 8259
PIC, and every keystroke came from a PS/2 controller on ISA IRQ1.
`pic.rs`'s own module doc comment justified that: "APIC needs ACPI/MADT
table parsing and MMIO register access that this phase never scoped, for
no benefit on a single core." That reasoning was correct about QEMU and
wrong about the world. A machine built in roughly the last fifteen years
may have no PS/2 controller at all, and its keyboard -- built-in or not
-- is a USB HID device behind an xHCI host controller. Booting this
project's USB stick on such a machine would produce a perfect shell
prompt that could never accept a single keystroke.

This phase closes that gap, and it has a strict dependency order baked
into it: reaching *any* PCI device's interrupts needs a real interrupt
controller, which needs to know where that controller is, which needs the
firmware's own ACPI tables. So: ACPI first, then the Local APIC and I/O
APIC, then xHCI and USB HID. Each landed as its own independently
verified step, for the same reason Phase 2's `CR3` switch did -- and the
first of those steps surfaced a hang that would have been extremely
unpleasant to diagnose if three changes had landed at once.

### `src/acpi.rs`: the firmware's own interrupt topology

RSDP -> XSDT (RSDT fallback) -> MADT, parsed from scratch against ACPI
6.x. No `acpi` crate, no borrowed code.

The RSDP is not at a fixed address; it is reached through the UEFI system
table's configuration-table array, which exists only while a
`SystemTable` handle does. So `acpi::find_rsdp` is called from `main.rs`
*before* `exit_boot_services` consumes that handle, and only the
resulting physical address crosses the boundary -- the same pattern
`gfx::Console`'s framebuffer pointer and `fs::snapshot_apps`'s owned
`Vec` already use. The tables themselves live in memory the firmware
types `ACPI_RECLAIM`/`ACPI_NVS`, which `pmm.rs` deliberately never hands
out and `paging.rs` identity-maps along with every other memory-map
region, so the captured address stays valid and readable afterward.

Every table is checksum-validated (the 8-bit sum of its bytes must be
zero) before anything in it is trusted. Firmware bugs here are rare but
real, and a bad Local APIC address read out of an unvalidated table would
fault somewhere with no useful diagnostic, long after the actual mistake.
Every multi-byte field is assembled from individual bytes rather than
read through a `#[repr(packed)]` pointer: ACPI tables are byte-packed
with no alignment guarantees whatsoever (a MADT's variable-length entry
list routinely leaves a 4-byte field on an odd address), and an unaligned
load through a typed pointer is undefined behavior in Rust even though it
happens to work on x86.

**The single most load-bearing thing in the whole module** is Interrupt
Source Override handling. ACPI numbers every interrupt input in the
machine in one flat Global System Interrupt space, and a legacy ISA IRQ
number is *not* generally its GSI. On essentially every PC -- QEMU
included, as this kernel's own boot log now shows -- the timer's ISA IRQ0
is wired to GSI 2, reported as a MADT type-2 override. A kernel that
assumed `gsi == irq` would program I/O APIC redirection entry 0 instead
of entry 2 and never see a timer tick, with nothing anywhere reporting an
error. `AcpiInfo::resolve_isa_irq` exists so that translation cannot be
skipped, and every routing call goes through it.

Deliberately out of scope, and stated as such rather than left implied:
**no AML interpreter.** This module reads fixed-format tables only, so it
cannot evaluate `_PRT` (PCI interrupt routing), `_CRS`, or any other
method-backed object. That is not a footnote -- it is the direct cause of
`xhci.rs` being a polled driver, below. Also out of scope: SRAT/SLIT/NUMA,
multiprocessor startup (other CPUs are counted and logged, purely so the
boot log is honest about them, and never started), and MADT type-4 Local
APIC NMI entries.

### `src/lapic.rs`: the per-CPU half

xAPIC (MMIO) only, not x2APIC: every machine supporting x2APIC also
supports xAPIC, xAPIC is the mode firmware leaves the BSP in, and
x2APIC's benefits (larger APIC ID space, faster IPIs) are both
meaningless on one core with no IPIs. No Interrupt Command Register
writes, no AP startup.

`init` sets the `IA32_APIC_BASE` MSR's global enable rather than assuming
firmware did, zeroes the Task Priority Register (a nonzero TPR blocks
interrupts in a way that looks *exactly* like wrong I/O APIC routing from
every other vantage point), masks the LVT entries this kernel does not
use, and finally software-enables the APIC through the Spurious Interrupt
Vector Register.

The spurious vector is 0xFF, and `idt.rs` deliberately does **not**
acknowledge it. A spurious interrupt means the LAPIC never actually
accepted one -- it raised the vector precisely because the request went
away first -- so an EOI there would retire whatever genuine interrupt *is*
currently in service instead. The same trap as the 8259's spurious IRQ7,
one level up.

#### Bug: masking LINT0 silently killed the timer and hung the boot

The first version of `lapic::init` did the obvious, tidy thing: mask
every LVT entry this kernel does not use, including LINT0. The result was
a boot that stopped dead midway through APIC bring-up with no fault, no
error, and nothing further in the debug log after `[lapic] base=...`.

Root cause: enabling the Local APIC changes, immediately and invisibly,
how the legacy 8259's interrupts reach the CPU. While the LAPIC is
software-disabled, the CPU takes the PIC's `INTR` assertion directly; the
moment it is enabled, that same assertion must arrive through the LAPIC's
LINT0 pin in ExtINT delivery mode -- which is exactly how firmware leaves
it, and exactly what a "mask everything we don't use" sweep destroys. So
masking LINT0 cut off the PIT's IRQ0; `calibrate_timer`, which measures
the LAPIC's free-running counter against `pit::ticks()`, then spun
forever against a tick count that could no longer advance; and the shell
was never reached.

Note the shape of that failure: a **hang, not a crash**. Every
fault-reporting mechanism this project built in Phase 1 and Phase 2 --
the IDT's 256 real gates, the fault screen, the double-fault stack -- had
nothing to report, because nothing faulted. The only diagnostic was the
absence of the next expected debugcon line, which is why the boot log is
littered with them.

The fix is ordering, not masking: LINT0 is programmed as ExtINT and left
*live* through `init` on any machine that reports having an 8259
(`pcat_compat`), and masked only in `enter_apic_mode`, at the same moment
the 8259 itself is masked and its pass-through stops being wanted. For
the entire window between enabling the LAPIC and having real I/O APIC
routes, LINT0 is the only way a timer tick or a keystroke reaches the CPU
at all. On a machine reporting no 8259, there is nothing to pass through
and LINT0 is masked from the start.

#### Timer calibration, and why it is allowed to fail

The LAPIC timer's count rate is not an architectural constant the way the
PIT's 1.193182 MHz input is -- it has to be measured. `calibrate_timer`
runs the LAPIC's counter against the PIT's already-live 100 Hz tick for
100 ms (starting on a tick boundary, because without that up to a full
10 ms of a 100 ms window is unaccounted for -- a 10% error), then divides.
On QEMU this reports 62553 counts/ms after the divide-by-16, which is
exactly QEMU's 1 GHz APIC bus clock.

It returns `Err` rather than a number if the measurement is implausible,
and that is not defensive decoration: under some hypervisors the LAPIC
timer's count does not advance until armed in a particular way, and a
calibration that silently returned "0 counts per millisecond" would arm a
periodic timer with an initial count of zero -- which the SDM defines as
*stopping* the timer. That failure mode is another silent hang, so it is
detected and reported, and the caller simply keeps the PIT as the clock.

### `src/ioapic.rs`: the external half

An I/O APIC is a pin-to-message translator and nothing else: each
redirection table entry says "when input pin *n* is asserted, send an
interrupt message with this vector to the Local APIC with this ID."

Three details are load-bearing:

- **Every entry is masked before anything is routed.** Firmware has no
  obligation to hand the controller over quiescent, and an entry left
  live from the firmware's own use would deliver a vector this kernel
  never configured the instant the 8259 stops shadowing it.
- **Redirection entries are written high half first.** The low half holds
  the mask bit, so writing it last means the entry can never be briefly
  live with a stale destination field.
- **Entry counts are read from each controller's own version register**,
  not assumed -- the MADT reports only where a controller's GSI range
  starts, never how long it is, so "which controller serves this GSI" is
  otherwise unanswerable.

MPS INTI flags ("conforms to bus specification") are resolved to the ISA
defaults (active high, edge triggered) before being written, because the
I/O APIC register has no "conforms" encoding: every bit has to be a
concrete choice by the time it is written.

### `src/idt.rs`: one place that decides where an EOI goes

This kernel now has two interrupt controllers over its lifetime, and
exactly one of them owns any given interrupt. Sending an EOI to the wrong
one is not a loud failure: the controller that really had the interrupt
in service never retires it, and every equal-or-lower-priority interrupt
after that is blocked forever -- presenting as "the keyboard stopped
working" with nothing in any log. So every acknowledgement now routes
through a single `idt::end_of_interrupt` keyed off `lapic::is_active()`,
which makes the mistake structurally impossible rather than something
each call site has to remember.

One behavioral change worth naming: an *unexpected* vector at or above
0x30 is now EOI'd when the APIC is live, where before it was only logged.
Before the APIC existed, nothing could have delivered such a vector and
there was nothing to acknowledge; now the LAPIC really did accept it and
has that priority level in service, so failing to acknowledge it would
block every lower-priority interrupt -- including the keyboard -- from
then on.

### Build order, and two orderings that are not cosmetic

`main.rs`'s `bring_up_apic` runs: enable LAPIC -> calibrate the timer ->
map and quiesce the I/O APIC -> route IRQ1 and IRQ0 -> switch to APIC
mode -> arm the LAPIC timer -> mask the now-redundant PIT line. Two of
those orderings are load-bearing, and both are the same kind of mistake
this project already made once in `ahci.rs` (the sti-ordering bug):

1. **Calibration happens before the switch, not after.** It measures
   against `pit::ticks()`, which only advances while something is
   actually delivering IRQ0. Calibrating after the 8259 is masked but
   before the LAPIC timer is armed would spin forever against a frozen
   counter.
2. **The LAPIC timer is armed before the PIT's line is masked**, not
   after. `start_timer` can still fail (a computed period that doesn't
   fit the 32-bit count register), and masking first would leave the
   kernel with no clock at all -- which does not degrade gracefully,
   because every bounded-timeout loop in `ahci.rs` and `xhci.rs` polls
   `pit::ticks()` against a deadline. The cost of this ordering is a few
   microseconds in which both timers are live and a handful of ticks are
   double-counted; the cost of the other ordering is a dead machine.

Every failure along the chain leaves the machine on the legacy 8259 it is
already running on, and says so. That is not a courtesy: the 8259 is
*working* at the moment `bring_up_apic` is called, so anything that goes
wrong must leave it working rather than strand the kernel between two
controllers with neither delivering.

### `src/xhci.rs`: a USB host controller, from scratch

The largest single driver in this project: PCI discovery by
classification (class 0C:03:30, never a hardcoded vendor:device ID),
64-bit BAR handling, controller reset, the Device Context Base Address
Array, command ring, event ring with its segment table, root-port power
and reset, Enable Slot, Address Device, control transfers, descriptor
parsing, Configure Endpoint, SET_PROTOCOL, and an interrupt-IN transfer
ring.

**Polled, not interrupt-driven -- and this is a decision with a reason,
not a shortcut.** A legacy PCI INTx interrupt's routing lives in ACPI's
`_PRT` object, which is AML, and this kernel has no AML interpreter (see
`acpi.rs` above) -- so that path is genuinely unavailable, not merely
unimplemented. MSI/MSI-X would sidestep AML entirely and is the right
long-term answer, but it is separate, independently testable work, and
bundling it into the same step as bringing up xHCI at all would mean a
failure could be either with no way to tell which. The event ring is
instead drained from `keyboard::wait_for_key`'s idle loop, which already
wakes on every timer tick, so a report is picked up within 10 ms of the
controller producing it -- below anything a person can perceive while
typing -- and the poll itself is a handful of MMIO reads on a ring that
is almost always empty.

Other deliberate scope cuts: one device, one interface, one endpoint (the
first port whose descriptors identify a HID boot-protocol keyboard; a
mouse or a mass-storage device is identified, logged, and left alone); no
hubs, so root ports only; no USB3 streams, no LTM, no bandwidth
negotiation; no hot-plug (Port Status Change Events are consumed so the
ring keeps draining, and logged).

Several implementation details that are easy to get wrong and silent when
wrong, all recorded in the source at the point they matter:

- **The cycle bit is the entire ring-ownership protocol.** There is no
  count and no head pointer; a TRB whose cycle bit differs from the
  consumer's cycle state is simply one the producer has not written since
  the last wrap. Every TRB this driver produces therefore writes dwords
  0-2 first and the cycle-carrying dword 3 last, the same
  "ownership-transferring write goes last" discipline as `ioapic.rs`'s
  redirection entries.
- **Context size is 32 or 64 bytes depending on `HCCPARAMS1.CSZ`,** and
  the difference is invisible: a 64-byte-context controller handed
  32-byte-spaced contexts reads the slot context where the endpoint
  context should be and rejects the command with a bare Parameter Error.
  Every context access goes through one scaling helper so it cannot be
  forgotten at a call site.
- **A Transfer Event reports the residual untransferred byte count,** not
  the transferred count. Reading it as the latter produces
  plausible-looking but wrong short-read handling.
- **`PORTSC` is full of write-1-to-clear bits, including `PED`.** A naive
  read-modify-write disables the port outright and throws away change
  flags nothing has looked at yet.
- **Endpoint 0's max packet size is a guess until the device says
  otherwise.** For Full speed, 8/16/32/64 are all legal. This driver
  reads the first 8 bytes of the device descriptor (the one length every
  legal value can deliver in a single packet -- which is precisely why
  USB puts that field at offset 7), and if the real value differs,
  corrects endpoint 0 with an Evaluate Context command before reading
  anything longer.
- **Every wait loop is deadline-bounded** against `pit::ticks()`, same
  discipline as `ahci.rs`, for the same reason: a wedged or absent
  controller must produce a reported timeout, never a silent hang.
- **The controller is reset unconditionally**, not trusted in whatever
  state firmware left it. UEFI's own USB stack has almost certainly been
  driving it moments earlier, with rings and contexts at addresses this
  kernel knows nothing about, and a controller still holding those
  pointers will happily DMA into memory `pmm` has since handed out.
- **Nothing this driver allocates is ever freed.** The controller is live
  for the rest of the program's life; returning a frame to `pmm` while
  the hardware still holds its address is exactly the kind of
  use-after-free that presents as random memory corruption hours later.

#### Bug: a 16-bit MMIO read of HCIVERSION returned zero

`HCIVERSION` is a 16-bit field at capability-register offset 0x02, and
reading it as a `u16` returned 0 from QEMU's own xHCI model -- while the
8-bit `CAPLENGTH` read at offset 0x00 worked fine. A sub-dword MMIO
access that particular register window does not decode.

Low severity: nothing depends on the version number, so the only symptom
was a boot log that said `version=0x0000` forever, as if that were what
the controller actually reported. But that is exactly the kind of quietly
wrong diagnostic this project's own standard says must not ship -- the
next person debugging an xHCI problem would have been reading a
fabricated value. Fixed by reading `CAPLENGTH` and `HCIVERSION` as the
single dword they share, which is both what the spec says these registers
are (xHCI 1.2, 5.3) and what works everywhere.

### `src/usb_hid.rs`: decoding, and one input stream

A general USB HID device describes its own report format in a HID report
descriptor -- a small stack language a host must parse to know which bits
mean which key. This kernel does not have that parser, and does not need
one: the HID specification anticipates exactly this case. A device whose
interface declares subclass 1 ("boot interface") and protocol 1
("keyboard") must also support a fixed, parser-free eight-byte report
format (HID 1.11 Appendix B), defined specifically so a system with no
HID parser -- historically a BIOS, here this kernel -- can still read a
keyboard. `xhci.rs` asks for it explicitly with SET_PROTOCOL; a device
that stalls that request is already in boot protocol and is used anyway,
rather than rejected over an optional request.

Two decisions shape the rest:

- **Press edges, not held keys.** A boot report is a *state* ("these keys
  are down right now"), not an event; a held key appears in report after
  report unchanged. Each report is diffed against the previous one and a
  byte is emitted only for a usage that was absent before and is present
  now. Without that, holding a key for a tenth of a second would type it
  ten times. HID's ErrorRollOver and friends (usages 1-3) are skipped
  rather than decoded -- they mean "more keys are down than I can
  represent," and typing garbage exactly when someone types fast would be
  a memorable bug.
- **The same queue as PS/2.** USB bytes are pushed into
  `keyboard.rs`'s existing ring buffer via a new `keyboard::push_decoded`,
  not a second queue. `keyboard::wait_for_key` stays the single blocking
  entry point every reader already goes through, two attached keyboards
  simply interleave into one stream instead of one being silently
  ignored, and -- most importantly -- nothing above that layer has to
  learn that USB exists at all. `shell.rs` and WISP's `input()` are
  completely unchanged by this phase.

`usb_hid::poll` is called from `wait_for_key`'s loop *before* its `cli`,
deliberately: the poll issues real MMIO reads and can walk a whole event
ring, which is more work than belongs inside an interrupt-disabled window
when nothing requires it to be there, and nothing is lost by a timer tick
landing mid-poll.

One debugcon line, `[usb_hid] first keystroke received over USB`, is
emitted on the first USB keystroke of a boot and never again. It exists
because `tests/usb_test.py` needs to prove a keystroke arrived through
the USB path *specifically* -- QEMU's `pc` machine always has an i8042
too, so a character appearing on screen proves only that some keyboard
worked. Only the first, because logging every keystroke would write
everything anyone types into the debug log, which this project has no
reason to do.

### The banner stopped claiming hardware it no longer uses

`shell.rs`'s startup banner read "Minimal Custom Kernel Shell -- own
GDT/IDT/PIC/PIT". That was exactly true when written and became a lie the
moment this kernel started bringing up the APIC: on a machine with usable
ACPI tables the 8259 is fully masked and the PIT's line is masked at the
I/O APIC, so the banner was naming hardware sitting there doing nothing.
It also cannot simply be updated to say "APIC" instead, because the
legacy path is still a real, supported fallback -- which controller is
live genuinely varies per boot. The banner is now generic ("hand-built
from firmware up"), and `main.rs` prints specific `ACPI:`/`APIC:`/`USB:`
lines just above it that can be honest about what actually happened this
boot. "Custom Kernel" stays in the text because
`tests/acceptance_test.py`'s `wait_for_boot` uses that substring as its
discriminator against OVMF's own built-in shell banner.

### `tests/usb_test.py`: four scenarios

Isolated build/ESP/target directories, matching `fault_test.py` and
`ahci_test.py`; four independent boots:

1. **`apic`** -- ordinary configuration, no USB. Asserts the MADT was
   parsed, that the ISA IRQ0 -> GSI override was *honored* (it reads the
   GSI number out of the kernel's own parse log and then requires the
   routing line to name that same GSI -- asserting the routing, not just
   that an override was seen, is what makes this a real check), that the
   LAPIC timer calibrated and armed, that the 8259 was masked, and that
   the shell is still fully interactive with an advancing clock
   afterward. Also asserts nothing arrived on a vector this kernel never
   routed.
2. **`no_xhci`** -- no controller present: the soft-fail path reports
   cleanly and PS/2 still works.
3. **`xhci_no_device`** -- controller with nothing attached: every port
   probe completes within its bounded timeout and boot finishes. This is
   the direct analogue of `ahci_test.py`'s `controller_no_disk` scenario
   and exists for the same reason -- it is the configuration a hang would
   hide in.
4. **`usb_keyboard`** -- controller plus a `usb-kbd`: the full
   enumeration chain, and a real keystroke proven to have arrived over
   USB via the marker described above.

#### Postmortem: a test assertion that was not sound, and was removed rather than loosened

The `apic` scenario originally asserted a tick-rate bound: "a 100 Hz clock
should advance about 150 ticks across this 1.5 s sleep, so anything far
above that means two timer sources are both incrementing the counter."
It failed immediately, reporting 518 ticks.

The kernel was fine. The assertion was wrong. The real elapsed guest time
between two readings is the 1.5 s sleep *plus* six monitor `sendkey`
round trips *plus* a screendump *plus* a tesseract OCR pass, and that
total varies with host load by more than a factor of two -- 518 ticks is
5.18 s, which is entirely plausible for that sequence on a loaded
machine. Comparing a guest tick count against an unmeasured host-side
duration is simply not a sound test.

The tempting fix -- widen the bound until it stops failing -- would have
left an assertion that no longer caught anything, since a genuine 2x
double-count would sit inside any bound loose enough to absorb the
timing noise. So the timing check was removed entirely and replaced with
an exact, deterministic one: the kernel logs the masking of the
now-redundant PIT line, and logs a specific warning if it could not, so
the absence of that warning together with the presence of the masking
line *is* the proof that the tick counter has one incrementer rather than
two. Property checked, not inferred.

Cross-checked independently while diagnosing this: the full acceptance
suite's own two `uptime` readings advance by 244 ticks before this phase
and 245 after it -- the same clock rate, through an entirely different
interrupt path. That is the real evidence the LAPIC timer runs at the
rate it claims.

#### Postmortem: the lowercase-`c` OCR misread, again

The `no_xhci` scenario failed with `no_xhci_ok` OCR'd as `no_xhei_ok` --
the same tesseract misread of this bitmap font's lowercase `c` as `e`
already on record in this file twice (the `ls`/`1s` and `abcXX`/`abEXX`
cases). Every marker string `tests/usb_test.py` types is now deliberately
free of the letter `c`. Choosing strings the OCR pass cannot garble is a
better fix than loosening the assertion, because it keeps the check
exact -- the same reasoning as the `(`/`{` fix in `read_uptime_ticks`,
arrived at from the opposite direction.

### Still deferred, explicitly

- **MSI/MSI-X for xHCI**, which would make the USB driver
  interrupt-driven and remove the 10 ms polling latency. Real follow-up
  work, deliberately not bundled here.
- **An AML interpreter**, and with it `_PRT`-based legacy PCI interrupt
  routing. Large, separate, and not needed for anything this kernel does.
- **USB hubs and hot-plug.** A keyboard behind an external hub is not
  reachable, and unplugging one mid-session is noticed in the log and
  nowhere else.
- **NX and `CR0.WP`**, still unimplemented since Phase 2, Part A flagged
  them. Every mapping this kernel builds is present+writable, so a bug
  that jumps into data or writes to code is still not caught by hardware.
- **Multicore.** `acpi.rs` counts the other CPUs so the boot log is
  honest about them; nothing is ever started on one.

### Verification

- Clean `cargo +nightly build --release --target x86_64-unknown-uefi`
  with **zero warnings** in this project's own code at every step, and an
  isolated `--features fault-test-hooks` build likewise.
- `tests/usb_test.py`: all four scenarios pass. The `usb_keyboard`
  scenario's screenshot was also opened and read directly, not trusted
  from OCR alone, confirming the boot sequence on screen reads
  `ACPI: 1 CPU(s), 1 I/O APIC(s), LAPIC at 0xfee00000` /
  `APIC: interrupts now routed through the I/O APIC; clock = LAPIC timer.` /
  `USB HID keyboard ready.` above a live prompt with the typed text
  echoed back -- matching this project's standing "OCR is a convenience,
  pixels are the ground truth" discipline.
- `tests/acceptance_test.py`: the full suite, run under the new APIC
  interrupt path, all steps passing including the two-boot cross-reboot
  persistence proof -- the strongest available statement that moving
  every interrupt in the machine onto different hardware changed nothing
  observable above the driver layer.
- `tests/fault_test.py` and `tests/ahci_test.py`: re-run unmodified after
  the phase landed. Both matter specifically here rather than as a
  formality: this phase edited `idt.rs`'s IRQ dispatch (the file
  `fault_test.py` exists to cover) and changed where `sti` sits relative
  to device bring-up (the exact ordering `ahci_test.py`'s
  `controller_no_disk` scenario was written to guard).
- The first APIC bring-up attempt hung, and is written up in full above
  rather than quietly fixed -- as is the unsound test assertion, which is
  arguably the more instructive of the two.

### `run.sh`: the development boot now has an xHCI controller

Up to this phase, `run.sh` gave the guest one USB controller -- the UHCI
that `-usb` implies, carrying the boot stick -- so the whole xHCI driver
was reachable only from `tests/usb_test.py`. Code that only ever runs
under its own test is code nobody is really living with. `run.sh` now
attaches `-device qemu-xhci,id=xhci` on every run, additively: the boot
stick's `-drive`/`-device usb-storage` pair is unchanged, and the new
controller comes up with nothing plugged into it.

An empty controller is deliberately the default rather than a wasted
device. It drives `src/xhci.rs` all the way through reset, ring setup and
the root-port scan, and ends at `[xhci] no USB HID boot keyboard found on
any root port` -- which is the honest steady state of a real machine
whose keyboard is PS/2, or is behind a hub we do not yet walk. It is the
same state `tests/usb_test.py`'s `xhci_no_device` scenario asserts.

Attaching the keyboard is opt-in (`UEFI_SHELL_USB_KBD=1 ./run.sh`) for a
reason worth recording: with a `usb-kbd` present, QEMU delivers host
keystrokes and monitor `sendkey` to it rather than to the emulated PS/2
controller. Making it the default would have silently swapped which of
the two input drivers every future interactive session was exercising,
while looking like no change at all. Both configurations are covered by
`tests/usb_test.py`; the switch only makes the choice visible.

Both branches were verified by running `run.sh` itself rather than by
inspection. Default: `[xhci] controller reset and running` followed by
`[xhci] no USB HID boot keyboard found on any root port`. With
`UEFI_SHELL_USB_KBD=1`: `port 5: device connected`, `port 5: slot 1
addressed`, `slot 1: HID boot keyboard on interface 0 endpoint 1
(wMaxPacketSize=8 bInterval=7)`, `USB HID boot keyboard ready`,
`[usb_hid] USB keyboard installed as an input source`.

### Documentation caught up with the machine, and the last build warning

Three things that had quietly drifted out of true, fixed together
because shipping accurate code behind an inaccurate README is its own
kind of defect.

`README.md` still opened with "This project never calls
`exit_boot_services()`. It lives entirely within the pre-exit-boot-
services epoch" -- a sentence that stopped being true several phases
ago, and one a reader would have taken as the project's central design
statement. It also listed six demo programs when there are nine, and one
test suite when there are four. It has been rewritten around what the
kernel actually does, including an explicit boot-order walkthrough, and
a "known rough edges" list that now names the real limits out loud: no
hubs or hot-plug on USB, xHCI only, AHCI only for persistence, boot
processor only, polled rather than MSI, no AML.

`USB_INSTALL.md` gained a "What the machine has to provide" section. The
phase that just landed is the one that makes "boot this on a real PC" a
credible claim, and a credible claim needs its boundaries stated with
the same specificity: which interrupt controllers, which USB controller,
which storage controller, what happens when each is absent, and the
practical consequence for someone standing at a machine that came up
without a working keyboard (move it out of the hub or dock, and
power-cycle rather than hot-plug). The no-networking section was also
extended, since USB is exactly the bus by which networking could arrive
as an accident: `src/xhci.rs` configures a device only after matching
the HID boot-keyboard class triple, so a USB network adapter is a device
it declines to configure, and adding a class driver for one would be a
decision made out loud.

`shell.rs`'s `help` text described `uptime` as "(PIT ticks)". Since the
APIC switch the ticks come from the LAPIC timer; the PIT's line is
masked at the I/O APIC. Now "(timer ticks)", which is true on both the
APIC path and the 8259 fallback. The same drift, in the same file, as
the banner correction recorded earlier in this entry -- worth noting
that small strings go stale quietly and in more than one place.

Finally, `cargo build` had begun emitting `cargo::non_kebab_case_bins`
for the `[[bin]] name = "BOOTX64"`. The name is deliberate: it matches
the UEFI removable-media fallback filename, so cargo's artifact is
already named what `build.sh`, the three installers and all four test
suites copy. Rather than rename it (six files would have had to agree
again, for no functional gain) or blanket-allow lints, the one lint is
allowed in `Cargo.toml` with the reasoning written next to the name it
is about. Builds are back to genuinely zero warnings, which is the only
state in which a warning still means something.

## v1.0.0: architecture hardening, and the first release archive

The first tagged release. Each change below is architectural -- it removes
a class of failure rather than one instance of it -- and each is proven by
a test that fails without it. All four suites pass (acceptance, fault,
AHCI, USB/APIC), the build has zero warnings, and the binary was built
with `rustc 1.101.0-nightly (822182db4 2026-10-13)` (dist
`nightly-2026-10-14`). Still zero networking: nothing here adds a device
class, a protocol, or a port.

### One timebase: `clock.rs`

The tick counter lived in `pit.rs`, whose doc comment still called the PIT
"the kernel's only clock" -- untrue since Phase 3, when the Local APIC
timer took over vector 0x20 and the PIT's line was masked. `ahci.rs`,
`xhci.rs`, `lapic.rs` and the shell all named `pit::ticks()` even on boots
where the PIT was silent, and `ahci.rs`/`xhci.rs` each carried a private,
byte-identical `poll_until`. Now `clock.rs` owns the counter, `TICK_HZ`,
`uptime_ms`, an interruptible `sleep_ms_unless`, and the single shared
`poll_until`; `pit.rs` is purely a hardware driver, and `main.rs` arms the
LAPIC timer at `clock::TICK_HZ` rather than a duplicated literal `100`.

`poll_until` checks RFLAGS.IF before it waits and panics with a precise
message if interrupts are off. That is the exact shape of the
sti-ordering hang recorded in the Phase 2 Part B entry: a tick deadline
can never arrive with interrupts disabled. It used to be a silent freeze
that only a dedicated test scenario could find; it is now a named panic
on the fault screen, whatever future call site reintroduces it. The
xHCI `wait_for_event` loop, which had its own unguarded deadline loop,
now goes through `poll_until` too. LAPIC calibration's two waits are on
the tick counter itself, so they cannot use a tick deadline; they are
bounded by the TSC instead (2^36 cycles) and return an error -- leaving
the PIT as the clock -- rather than spinning forever if the reference
tick never moves.

### Heap: address-ordered, coalescing, self-tested

The free list was unordered and never merged neighbours (the textbook
teaching allocator, documented as an accepted trade-off). Every `run`
builds and drops an AST, scope maps and lists of many sizes, so a long
session fragments the heap into small blocks that cannot satisfy a large
allocation despite plenty of free memory. The list is now kept sorted by
address and every free merges with adjacent free blocks on both sides.
Two latent issues went with the rewrite: an over-aligned allocation used
to *leak* the alignment gap in front of it (now returned to the free
list, with a too-small gap avoided by stepping one alignment unit
further), and a double free silently cross-linked the list (now an
immediate panic naming the address, detected for free by the ordering).
`allocator::self_test` runs every boot: 48 allocations of varied sizes
and alignments (1 to 128), an overlap check by pattern, frees in a
deliberately fragmenting order, and a requirement that the heap return
to the identical shape -- same free bytes, same block count.

### W^X memory protection

`paging.rs` used to map every page read/write/execute. It now maps the
running image from its own PE/COFF section table (located through the
LoadedImage protocol before `exit_boot_services`): `.text` read-only and
executable, `.rdata`/headers/`.reloc` read-only, `.data` (which holds
`.bss`, the heap and both kernel stacks) read/write and non-executable.
Every other page -- RAM, page tables, DMA frames, MMIO, firmware tables --
is non-executable. `paging::load` enables `IA32_EFER.NXE` before the `CR3`
switch (bit 63 is a reserved-bit fault without it) and sets `CR0.WP`
after it, so ring 0 honours read-only pages too. NX support is detected
by CPUID rather than assumed. Each header field is validated (MZ/PE
signatures, PE32+, page-sized section alignment, page-aligned in-bounds
sections, at least one code section); any failure degrades to mapping
the image RWX with a logged warning rather than faulting on the first
instruction fetch. Guard pages inside `.data` stay unmapped. The runtime
`paging::map` now `invlpg`s every page it touches (previously a
re-mapped page could keep a stale TLB entry) and rejects non-identity
requests explicitly instead of silently ignoring `phys`.

The fault screen decodes #PF error codes ("page fault: write, protection
violation", "instruction fetch", "page not present"), from static
strings only -- no allocation on the fault path. Two new fault-test
hooks prove both halves: `__fault_test_wp` writes to a constant in
`.rdata` (#14 with P|W, `cr2` equal to the logged probe address), and
`__fault_test_nx` calls into a `ret`-filled `static mut` buffer (#14 with
P|I/D). If either protection were missing the hook would complete and
log "NOT enforced", which fails the test explicitly instead of hanging.

### Esc stops a running WISP program

`docs/WISP_LANGUAGE.md` used to concede that `while true { }` meant a
hardware reset and named a kill-key as the natural follow-up. Esc now
raises an atomic stop request from the PS/2 ISR and the USB HID decoder
(and is queued as byte 0x1B so a blocked `input()` wakes). The
interpreter checks it before every `while` iteration and every user
function call -- the only two ways a WISP program can run unboundedly --
and `input()`/`sleep_ms()` check it after returning. The host's
`sleep_ms` wakes every tick to look, so a long sleep stops within 10 ms.
The USB keyboard has no interrupt, so `stop_requested` polls the xHCI
event ring at most once per tick: often enough to notice Esc promptly,
rare enough that a tight loop is not slowed by MMIO. The program ends
with an ordinary `wisp: line N: stopped (Esc pressed)` error at the loop
or call it was in. The stop flag and key queue are cleared when a
program starts and when it ends, so a stale Esc never kills the next
program and keys typed into a stopped program never reach the prompt; at
the prompt itself Esc discards the half-typed line. `Stmt::While` gained
a `line` (the keyword's), because `while true` has no line of its own to
report. Stops are logged to debugcon as the fact and the line only --
never message text, which can quote user input.

### Language, shell, apps

- `time_ms()` builtin: milliseconds since boot from `clock.rs`.
- `sleep_ms(0)` now returns immediately (it used to sleep one tick).
- Shell: `version` (kernel version, NX/WP status, clock source, "no
  networking"), `mem` (heap used/free/fragment count/largest block,
  physical frames free -- counted incrementally by `pmm`), banner names
  the version from `CARGO_PKG_VERSION` and mentions Esc.
- New apps: `stopwatch.wisp` (laps via `time_ms()`, ends only on Esc) and
  `primes.wisp` (a pure compute loop, ends only on Esc).
- The package is now `wisp-os` 1.0.0 in `Cargo.toml`; the `[[bin]]` name
  stays `BOOTX64`.

### Deterministic test synchronization

`tests/fault_test.py` waited a fixed 4 seconds for boot and then typed.
Rebuilding the baseline with three suites running at once, the
guard-overflow scenario typed into a VM that had not finished booting and
failed with no exception in the log. The shell now writes `[shell] ready`
to debugcon exactly once, right before its first prompt accepts input;
`acceptance_test.wait_for_debug_marker` polls for it, and the fault suite
waits for the fault report itself instead of a second fixed sleep. The
acceptance suite also asserts the new boot proofs (heap self-test, image
sections, `nx=on wp=on`, `[shell] ready`), `version`, `mem`, Esc stopping
`primes` and `stopwatch`, and Esc clearing the prompt line; the USB suite
stops `primes` with Esc over USB.

### Stale comments corrected

`gfx.rs` still said the framebuffer stays valid because "this project
never" calls `exit_boot_services`; `pmm.rs` said it had no consumer and
that "no paging is active yet"; `main.rs` said the `CR3` switch lands
"fully permissive" and that no paging was added; `ahci.rs` and `pci.rs`
said the kernel has no ACPI parser. All describe an earlier phase and now
describe this one. A `pmm::stats` addition had briefly separated
`alloc_frame`'s doc comment from its function; caught in review.

### Release packaging

`scripts/make_release.sh` builds (or `--skip-build`) and writes
`dist/wisp-os-v<version>.tar.gz` and `.zip`: the full source, docs,
apps, tests and tools, the prebuilt `esp/EFI/BOOT/BOOTX64.EFI`, a
per-file `SHA256SUMS` inside, and `dist/SHA256SUMS` for the archives.
Entries are sorted, owners zeroed and timestamps pinned to
`SOURCE_DATE_EPOCH` (default: newest packaged file), so an unchanged tree
repackages byte-for-byte; zip is written with Python's standard library
so no `zip` binary is needed, and keeps executable bits. The toolchain
stays on the `nightly` channel, with the exact verified nightly recorded
in the README as the fallback pin.

### Reproducible binary

Rebuilding from the extracted release archive produced a `BOOTX64.EFI`
that differed from the shipped one in 12-20 bytes: the COFF timestamp and
the CodeView debug-directory timestamp and GUID. The code was identical;
the metadata was wall-clock- and path-dependent (the GUID derives from
the PDB, which records absolute build paths). `.cargo/config.toml` now
passes `/Brepro` (content-hash timestamp) and `/DEBUG:NONE` (no debug
directory -- the PDB was never shipped or used). Two builds of the same
tree in different directories are now byte-identical, so anyone can
check the prebuilt binary in a release against their own build. All four
suites were re-run against the binary built this way.

## Audit remediation (CODEBASE_AUDIT.md, AUD-01 to AUD-13)

An external audit of the v1.0.0 snapshot reported 13 findings. All are
addressed below. The QEMU suites were updated to match but could not be
run in this environment (no QEMU); everything marked *tested* was checked
by the new host-side suite, `tests/host` (`cargo test --release`, 28
tests), which compiles `src/wisp` and `src/save_format.rs` unmodified
against an in-memory disk with a volatile write cache.

### Storage: boot never writes; explicit, ordered, durable provisioning

- **AUD-01.** Boot no longer formats a disk. `save_format::probe` reads
  LBA 0 and mounts only a complete, valid superblock (magic, version,
  slot count, slot size); blank, foreign and unsupported disks are left
  untouched and reported. The new shell command `format` shows the AHCI
  port, model and serial (decoded from IDENTIFY), size, and the exact
  destructive range, and proceeds only on typed `ERASE`. Every sector
  access goes through `save_format::region_lba`, which rejects anything
  outside LBA 0-512. *Tested:* probe issues no writes; provisioning
  touches nothing outside the region. New QEMU scenario
  `foreign_disk_untouched` hashes the whole image before and after boot.
- **AUD-02.** `Hba::issue` now owns the whole command lifecycle. On a
  timeout or task-file error it runs AHCI 1.3.1 6.2.2.1 recovery: clear
  `PxCMD.ST`, wait for `PxCMD.CR` to drop and verify `PxCI` is clear
  (only then has the HBA released the command table and data buffer),
  clear `PxSERR`/`PxIS`, COMRESET if the device is still BSY/DRQ, and
  restart. If quiescence cannot be proven the port is marked failed:
  every later request fails before touching MMIO or DMA memory, and the
  frames stay allocated.
- **AUD-03.** The write/read-back/restore boot self-test is gone. The
  shell `save`/`load` round trip on a disposable test image covers the
  write path in `tests/ahci_test.py`, which now also checks the disk's
  last sector is unchanged.
- **AUD-04.** `Hba::flush_cache` issues FLUSH CACHE EXT (or FLUSH CACHE),
  chosen from IDENTIFY words 82/83/85/87. `save` succeeds only after the
  slot write *and* the flush; a device with a write cache and no flush
  command makes `save` fail rather than report unbacked success.
  *Tested:* a successful save survives simulated power loss; a failed
  flush is reported.
- **AUD-12.** `provision` invalidates LBA 0 and flushes, clears the slots
  and flushes, then publishes the superblock and flushes. *Tested:* a
  failure injected at every write, followed by power loss, never yields
  a disk `probe` accepts unless nothing durable changed (and a mutation
  that restores the old superblock-first order fails the test).

### Interrupts and USB

- **AUD-05.** `ioapic::stage_isa_irq` programs a redirection entry fully
  but masked. `main.rs` stages IRQ1 and IRQ0 (re-masking IRQ1 if IRQ0
  fails), then in one interrupts-disabled section masks the 8259, moves
  EOI ownership to the LAPIC, unmasks both routes, and drains any PS/2
  byte that arrived during the switch (IRQ1 is edge-triggered; an
  unread byte would otherwise suppress every later edge).
- **AUD-06.** `next_event` reads the cycle-bit dword first and the
  payload only after ownership is established.
- **AUD-11.** Keyboard transfers request the 8-byte boot report, not
  `wMaxPacketSize`. Success and Short Packet completions are both
  decoded when the transferred length (request minus residual) is a full
  report, and only for the event matching the outstanding TRB, slot and
  endpoint.

### Memory

- **AUD-07.** `fs::snapshot_apps` checks each file's advertised size
  against a per-file (256 KiB) and remaining aggregate (1 MiB) budget
  before allocating, allocates fallibly, rejects truncated reads, caps
  file count (64) and name length, and logs every skipped file by name.
  `run` borrows the source instead of cloning it.
- **AUD-08.** Serialization checks the 4055-byte slot budget before every
  append. Display stops at 16 KiB with `...(truncated)`; `str()` of a
  longer value is an error; `print` streams each argument instead of
  concatenating them. Equality memoizes list-identity pairs (linear on
  shared graphs; bisimulation for cycles) under a 100,000-step budget.
  *Tested:* the audit's 23-list graph prints, fails `str()` and `save()`
  cleanly, and leaks nothing; two 41-list graphs compare instantly.
- **AUD-09.** Lists are now `Rc<ListCell>`. The interpreter records a
  weak handle to every list a run creates (and to lists `load` returns)
  and empties all of them when the run ends, which breaks every cycle.
  `ListCell`'s `Drop` is iterative, so a 200,000-deep chain frees without
  recursion. Strings are capped at 256 KiB and lists at 32,768
  elements, both allocated fallibly, and the shell stops a script with
  "out of memory" when free heap falls below a 1.5 MiB reserve (a new
  O(1) `allocator::free_bytes` counter, checked by the heap self-test).
  *Tested:* repeated runs creating 10,000 cyclic lists return to the
  same live-byte count.
- **AUD-13.** The lexer decodes non-ASCII characters inside string
  literals as UTF-8 instead of casting bytes to `char`. *Tested:* `é`,
  CJK and emoji keep their bytes and lengths through `len` and `save`.

### Native stack depth (AUD-10)

Measured, not assumed. Frame sizes were read from the UEFI-target
assembly (`cargo rustc --release -- --emit=asm`, prologue `push`/`sub
rsp`) and cross-checked by running scripts on bounded host threads:

- Before: `eval_expr` and `exec_stmt` were 480 bytes each, `parse_stmt`
  1088, and every parser precedence level about 300. One nested WISP
  call cost ~1.9 KiB, so the documented 256-call limit needed ~470 KiB
  against a **256 KiB** stack; one parenthesis level cost ~2 KiB.
- The recursive match arms are now separate never-inlined functions
  (`eval_expr` 160 bytes) and the parser uses precedence climbing (one
  frame per sub-expression instead of six): a parenthesis level is now
  ~1 KiB.
- New limits: `parser::MAX_SYNTAX_DEPTH` = 128 (blocks, sub-expressions,
  unary operators, indexing and each operator in a chain all count, so
  a flat `1 + 1 + ...` is bounded too), and `interp::MAX_NESTING` = 1024
  units shared by statements, expressions and calls. The costliest unit
  is ~750 bytes, so the worst case is ~770 KiB.
- The guard-paged mainline stack is now 1 MiB (zero-fill; the binary
  size is unchanged).
- Function bodies are shared (`Rc<FuncDecl>`) instead of deep-cloned.

*Tested* on a 640 KiB host stack: 250-deep recursion succeeds;
runaway recursion, 50 nested `while` bodies per call, 50-level
expressions per call, 20,000 parentheses, 50,000-term chains and 5,000
nested blocks all end in a WISP error.

### Not addressed

- The save store still owns the whole disk's first 513 sectors rather
  than a WISP partition; `format` makes that explicit and confirmed.
- Slot updates are still not atomic (documented in SAVE_FORMAT.md);
  flushing makes them durable, not crash-atomic.
- The audit's "possible firmware-memory overlap" (verification gap 6) was
  not investigated.
- `SHA256SUMS` in the repository root describes the v1.0.0 release and
  is regenerated by `scripts/make_release.sh`.

## Firmware memory is not free until firmware is gone (audit verification gap 6)

The audit's "possible firmware-memory overlap" was left uninvestigated.
It was investigated here, and it is real: `pmm::init` freed all
`BOOT_SERVICES_CODE`/`_DATA` at once, and `paging::init` then allocated
and zeroed thousands of page-table frames from that pool while the CPU was
still translating through firmware's page tables, faulting through
firmware's IDT and running on firmware's stack.

Measured, not assumed (a temporary diagnostic walked the live `CR3`
hierarchy right after `exit_boot_services` and classified every frame by
memory-map type), under QEMU/OVMF with 256 MiB:

- firmware's live page tables: 1029 frames, **all** `BOOT_SERVICES_DATA`
  (7 frames with `-cpu max`, which has 1 GiB pages -- also all
  `BOOT_SERVICES_DATA`);
- the live stack pointer and the live IDT: `BOOT_SERVICES_DATA`;
- `paging::init` allocates ~6,300 frames, first-fit from the bottom of
  RAM (most of them page tables for a 12 GiB `RESERVED` region OVMF
  reports at 0xfd00000000).

It never fired only because OVMF places those structures near the top of
RAM and the allocator is first-fit from the bottom. Nothing guarantees
that layout. On firmware that puts them low, `paging::init` zeroes a live
page table, the next access through it faults into an IDT that may be
gone too, and the machine resets with nothing on screen.

Fixed by construction rather than by layout:

- `pmm::init` frees only `CONVENTIONAL` memory and *withholds*
  boot-services memory. `pmm::release_boot_services_memory` frees it, and
  `main.rs` calls it as the first thing on the new stack -- the first
  point at which the CPU runs on the kernel's own page tables, GDT/IDT and
  stack. Withheld runs are built from the bitmap, so a frame some
  malformed, overlapping descriptor also calls conventional can never be
  freed twice.
- Second line of defence for firmware that breaks the memory-map rules:
  `pmm::init` walks the live paging hierarchy (`paging::
  for_each_live_table_frame`: 4 or 5 levels, stopping at 1 GiB/2 MiB
  leaves) and pins any frame of it -- and the pages of the live GDT, IDT
  and stack -- that the map calls conventional, with a warning. The stack
  window is clipped to the stack's own memory-map descriptor: the first
  version was not, and flagged a conventional frame directly below
  OVMF's 128 KiB boot stack as "live firmware state" -- a false accusation
  caught on the first boot, and fixed before it shipped.
- 5-level paging: `paging::load` used to install a 4-level PML4
  unconditionally, which faults immediately if firmware has enabled
  `CR4.LA57` (it cannot be turned off in long mode). `paging::init` now
  wraps the PML4 in a PML5 (entry 0 covers every identity-mapped address
  below 2^48) when LA57 is on, and `paging::map` resolves the PML4 through
  it. **Not exercised:** no available OVMF build enables LA57, even with
  `-cpu max`.

Verified: `tests/acceptance_test.py` step 1c parses the new debugcon
lines and requires that boot-services frames were withheld, that every
frame of firmware's live hierarchy was among them, that nothing had to be
pinned, and that exactly the withheld count was released after both the
`CR3` and stack switches. Reverting `pmm::init` to the old behaviour fails
that step with all three diagnostics. All four QEMU suites pass; boots
with `-cpu max` and with 2 GiB were checked by hand.

## Save format version 2: crash-atomic saves

The audit remediation left one storage limitation standing: "slot
updates are still not atomic -- flushing makes them durable, not
crash-atomic." Version 1 overwrote a key's slot in place, sector by
sector, so a power cut in the middle of `save` could leave the slot torn:
the old value gone and the new one incomplete, reported on the next
`load` as a checksum error. For `counter.wisp`, a power cut at the wrong
moment meant the count was lost outright.

Version 2 never overwrites a live value. Each key's value is a *copy*
with a generation number; readers take the highest generation. An update
writes a new copy into a free slot -- value sectors, flush, header sector,
flush (the commit) -- and only then frees the old copy. Under a cache that
may persist any subset of unflushed writes in any order, a crash at any
point leaves the old value or the new one. Details and the argument are in
`docs/SAVE_FORMAT.md` ("Saving: copy-on-write").

- **Whole-copy checksum.** The checksum now covers the header (state, key,
  length, generation) as well as the value: otherwise a bit flip that
  raised a stale copy's generation would silently promote an old value.
- **Stale copies.** A crash after the commit but before the old copy is
  freed leaves a stale copy. Every save and delete frees all stale copies,
  and flushes, before writing anything else, so a later delete cannot let
  an older generation resurface.
- **A save whose cleanup fails still succeeds.** Once the commit flush has
  completed the new value is durable; failing to free the old copy only
  leaves it stale. Reporting an error there would claim a save failed that
  had in fact taken effect.
- **63 keys.** One slot is always kept free for the next copy-on-write
  update. `save(key, nil)` now deletes the key (frees its slot): `load` of
  a missing key already returned `nil`, so the two were never
  distinguishable, and the slot is no longer spent on "nothing".
- **New shell commands** `keys` (each key and its size, plus how many of
  the 63 are in use) and `delete <key>`.
- **Header-only scans.** A slot's header fits in its first sector, so
  `save`/`load` scan 64 sectors instead of reading all 512.
- **Keys may not contain NUL** (the key field is zero-padded, so `"a"` and
  `"a\0"` were the same key). Unreachable from WISP source, rejected
  anyway.
- **Version-1 disks keep working.** They mount and read unchanged (a v1
  copy is generation 0). The first save -- once it is known to have room
  -- upgrades the store by rewriting the superblock's version byte, one
  flushed sector, before any v2 copy exists; a v1.0.0 kernel then reports
  the disk unsupported instead of misreading it. A v1 store that used all
  64 slots cannot take an update (no free slot), and says so -- with
  "delete a key" as the remedy -- rather than falling back to an unsafe
  in-place write. The first draft upgraded the superblock before checking
  for room, so a save that *failed* still locked v1.0.0 out of the disk;
  caught while writing the test for exactly that case, and reordered.

Verified:

- `tests/host/tests/save_format_v2.rs` (20 tests). The core harness runs
  each operation -- small, multi-sector, shrinking and growing updates,
  new keys, deletes, `nil` saves, operations on a store with a stale copy,
  and upgrading saves on v1 stores -- with the device stopped after every
  possible number of writes and flushes, builds the disk image for *every*
  subset of the unflushed writes reaching the media (the multi-sector case
  has seven in flight: all 128 subsets per crash point), and on each image
  checks that the store mounts, the key holds its old or new value (the new
  one if the operation had reported success), no other key changed, no v2
  copy sits on a disk still labelled v1, and that the store recovers -- a
  further save succeeds and leaves no stale copies. Version-1 disks are
  built by an independent encoder written from the spec. The documented
  worked example is checked byte for byte.
- Mutation testing: eight deliberate bugs -- dropping the value barrier,
  freeing the old copy before the commit, picking the lowest generation,
  reclaiming without a flush, upgrading without a flush, upgrading after
  the copy, skipping reclamation in delete, and leaving the generation out
  of the checksum -- each fail at least one test.
- `tests/ahci_test.py` `full_stack` now updates a key, adds, lists and
  deletes another over real (emulated) AHCI, then decodes the image: only
  slot 1 in use, holding generation 2 with a checksum recomputed in
  Python. New scenario `legacy_v1_store` hand-builds a v1.0.0-layout disk,
  boots, reads its values, saves, and checks the upgrade on disk: version
  byte (and nothing else in the superblock) changed, the update written
  copy-on-write at generation 1, the old copy freed, the untouched v1 copy
  byte-identical, nothing outside the region written.

## WISP v2, Part 2: `for` loops, slicing, strings, and list builtins

`docs/WISP_LANGUAGE.md` closed with a list of what v1 deliberately left
out, "the first things to reach for if you outgrow v1". The ones that
cost the most in real programs were the string ones: with no indexing,
no slicing and no splitting, a script could not take a typed line apart,
which is why `adventure.wisp` only understands whole lines. This adds:

- **`for x in list_or_string { ... }`.** A list is iterated as a
  snapshot of its element handles (refcount bumps, no deep copy), so
  growing the list inside the loop cannot make it run forever; a string
  is walked one character at a time without copying it (strings are
  immutable). The loop variable lives in each pass's fresh scope. Esc is
  checked before every pass, like `while`.
- **Slicing** `a[i:j]` (either bound optional) on lists and strings, and
  **string indexing** `s[i]`. Bounds are strict -- `0 <= start <= end <=
  len` or a runtime error, never a silently shortened result, the same
  stance as indexing. Assigning to a slice, or into a string, is a
  precise error.
- **String offsets are bytes**, the unit `len()` has always counted in.
  An offset inside a multi-byte UTF-8 character is an error ("falls
  inside a multi-byte character") rather than half a character; `for`
  and `chars()` iterate by character. `find()` answers in bytes, so its
  result slices correctly.
- **List `+`**, making a new list.
- **Builtins:** strings -- `split` (whitespace, or an explicit separator
  that round-trips through `join`), `join`, `find`, `contains`,
  `replace`, `upper`/`lower` (ASCII only; everything else unchanged),
  `trim`, `starts_with`, `ends_with`, `chars`, `repeat`, `chr`, `ord`;
  lists -- `insert`, `remove`, `sort` (all ints or all strings, in
  place, no allocation), `reverse`, `range`, plus `find`/`contains` on
  lists; numbers and types -- `abs`, `min`, `max`, `type`.
- **`calc.wisp`**, an integer calculator whose recursive-descent
  expression parser is written in WISP. It never ends in a WISP error:
  numbers are capped at 18 digits and every result at 10^18 (so no
  operation can overflow), and parentheses and minus signs together nest
  at most 40 deep (so its recursion stays inside the call limit). The
  first draft bounded only parentheses; 300 minus signs in a row would
  have hit WISP's own recursion limit. Caught while writing its test.

Every resource bound the audit remediation established still holds for
the new code:

- Every string a builtin builds is size-checked against the 256 KiB
  limit *before* allocating and allocated fallibly, like `+`.
- `split`/`chars` build up to 32,768 small strings in one call, more
  than the 1.5 MiB heap reserve covers; they check the host's heap
  reserve every 256 elements, as the interpreter does every loop pass.
- Every list a builtin or slice or `+` creates is registered with the
  session's list registry, so a cycle built from one is still reclaimed
  (AUD-09). `builtins::call` now takes the registry, so a future builtin
  cannot create a list any other way.
- Native stack depth (AUD-10), measured on the UEFI target as before
  (`--emit=asm`, prologue sizes): every existing recursive frame is
  unchanged (`eval_expr` 160 bytes, `exec_stmt` 304, `eval_binary` 288).
  But a nested `for` body costs ~1040 bytes between consecutive nesting
  units, against the ~750 `MAX_NESTING` was sized for -- a recursive
  function with deeply nested `for` loops could have needed more than
  the 1 MiB stack. No test caught this; the measurement did. A `for`
  statement is now charged two units of the budget (~500 bytes each).
  `nested_for_loops_inside_deep_recursion_are_a_clean_error` overflows
  its 640 KiB host stack without the double charge, and ends in a clean
  "stack overflow" WISP error with it.

A review pass for unbounded work in the new builtins, before commit,
found two more:

- `repeat("", n)`: the output-size check bounds the work only for a
  non-empty string. With an empty one it passed (0 bytes) and the loop
  then ran `n` times -- up to 9 * 10^18, with no Esc check: a hung
  kernel. An empty string now returns at once.
- `find`/`contains` on a list: each element comparison may legitimately
  take up to the 100,000-step equality budget, and a list holds up to
  32,768 elements, with no Esc check in between. They now check for Esc
  before each element, as a loop does before each pass.

Both have tests (`repeat_is_bounded`, `searching_a_list_honours_esc`;
the latter fails with its check removed).

New keywords `for` and `in`, and the new builtin names, could break an
existing program; none of the bundled apps or documented examples uses
any of them as an identifier or function name.

## The interactive prompt

Every WISP program used to be written on another computer and copied to
the stick. `wisp` at the shell prompt now opens an interactive session:
each line runs as it is entered, an expression's value is shown
(strings quoted, so `"1"` and `1` differ), and the session keeps every
global, function and list it builds until `exit`.

- The interpreter was split into a `Session` (globals, functions, list
  registry) and an `Interp` (one run against a session and a host), so
  a session can outlive one run while the shell lends it the console,
  keyboard and save disk afresh for each line.
- `Session::run_interactive` differs from a program run exactly where a
  prompt needs it to: `let` may re-declare a global, `fn` may redefine a
  function from an *earlier* line (within one line, both are still
  errors), and a trailing bare expression's value is returned. A line
  whose functions fail validation registers none of them.
- `wisp::parse_interactive` accepts a line missing only its final `;`.
- A line with more `(`/`[`/`{` than closers continues at a `...` prompt
  (`wisp::open_brackets`, which ignores brackets in strings and
  comments); Esc there discards the unfinished input.
- Esc stops a running line exactly as it stops a program; the session
  continues. `vars` lists globals and functions.
- A long session can hold most of the heap; once free heap is below the
  script reserve the prompt says so and names the remedy (`exit`).

Verified: `tests/host/tests/language.rs` (41 tests) covers every new
construct and builtin, including each error message, the size limits,
cycle reclamation for every list-creating path (allocator-counted), the
heap-reserve checks inside `split`/`chars`, interactive sessions, the
documented examples, and `calc.wisp` driven through scripted sessions
on a 640 KiB stack. Nine deliberate bugs -- an unregistered list from
each of `split`, `range`, slicing and `+`; no heap check in
`split`/`chars`; `for` iterating the live list; registering functions
before validating them; no Esc check in `for`; string indexing without
the character-boundary check -- each fail at least one test.
`tests/acceptance_test.py` steps 20-21 run `calc.wisp` and an
interactive session (a function typed over three lines, an error, Esc
stopping `while true { }`, `vars`, `exit`) on the emulated machine. Two
of the first assertions failed on OCR, not on the kernel -- tesseract
reads `= 11` as `- 1` and the red `(Esc pressed)` as `{Ese pressed` --
and were changed to text it reads reliably; the Esc stop is proven by
the kernel's debugcon line instead.
