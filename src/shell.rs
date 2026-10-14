//! The interactive command shell: prompt, line editing, and command
//! dispatch for `help`, `clear`, `echo`, `ls`, `run`, `uptime`, `mem`,
//! `version`, `save`, and `load`.
//!
//! Runs entirely on our own drivers (`keyboard.rs` for input, `clock.rs` for
//! timing) -- there is no Boot Services left to lean on by the time this
//! runs; see `main.rs`'s boot sequence and `fs::snapshot_apps` for how
//! `APPS/*.wisp` made it into memory before Boot Services went away.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use core::fmt::Write as _;

use crate::ahci;
use crate::allocator;
use crate::lapic;
use crate::paging;
use crate::pmm;
use crate::gfx::{self, Console};
use crate::keyboard;
use crate::usb_hid;
use crate::clock;
use crate::save_format;
use crate::wisp;
use crate::wisp::value::Value;

/// Target address for `__fault_test_pagefault`: canonical (bits 63:48 all
/// equal to bit 47, so this takes `#PF`, not `#GP`) but 16 TiB up --
/// nowhere near this kernel's actual physical memory map, which
/// `paging.rs`'s own boot-time debugcon line confirms tops out at a few
/// GiB. See that command's match arm for the full reasoning.
#[cfg(feature = "fault-test-hooks")]
const FAULT_TEST_UNMAPPED_ADDR: u64 = 0x0000_1000_0000_0000;

/// A tight, `#[unsafe(naked)]` downward-walking loop that repeatedly
/// subtracts a full page from a *scratch* pointer (RAX) and touches the new
/// bottom byte through it, used by `__fault_test_guard_overflow` to
/// deliberately walk off the bottom of the mainline stack into its guard
/// page -- while leaving the real RSP completely untouched.
///
/// Deliberately raw asm, not a Rust function calling itself recursively:
/// at this project's actual release profile (`opt-level = "s"`,
/// `lto = true`), naive "recursion with no base case" is exactly the shape
/// LLVM's tail-call/loop-conversion optimizations target -- it could
/// silently turn a recursive version of this test into a non-growing
/// infinite loop that never actually faults, producing a false pass (or a
/// hang) instead of a real proof. A hand-written loop is deterministic and
/// immune to that class of optimization entirely, matching this project's
/// established preference for raw asm over relying on Rust codegen
/// wherever correctness is safety-critical (see `idt.rs`'s RCX-vs-RDI
/// postmortem).
///
/// # Why a scratch pointer, not RSP itself
/// An earlier version of this walked RSP itself downward (`sub rsp, 4096`
/// in a loop). That produced a *cascading* `#8` Double Fault instead of a
/// clean `#14` Page Fault: by the time the probing write actually touched
/// the guard page, RSP itself already pointed inside it, so the CPU's own
/// exception-delivery frame push -- which happens at the *current* RSP --
/// also faulted, escalating to `#DF` per the CPU's documented
/// contributory-exception rules (Intel SDM Vol 3, 6.15). That's real,
/// correct CPU behavior (and exactly the scenario [`fault_test_stack_overflow`]
/// exists to test on purpose, in isolation), but it defeats the point of
/// *this* test: proving the guard mapping itself is genuinely absent,
/// isolated from what happens to RSP separately. Probing through RAX
/// instead keeps RSP valid throughout, so the resulting fault is a clean,
/// single `#PF` with `cr2` reporting exactly the probed address -- the
/// guard-page equivalent of `__fault_test_pagefault`'s direct write.
///
/// Never returns by design -- it either faults (the intended outcome, at
/// which point `idt.rs`'s fault handler takes over and halts) or, if the
/// guard page is somehow broken, keeps walking downward into whatever
/// memory follows it; `tests/fault_test.py`'s own external timeout is what
/// bounds that failure mode, not anything in this function.
///
/// # Safety
/// Must only ever be invoked from `__fault_test_guard_overflow`'s hidden
/// shell command.
#[cfg(feature = "fault-test-hooks")]
#[unsafe(naked)]
unsafe extern "C" fn fault_test_guard_overflow() -> ! {
    core::arch::naked_asm!(
        "mov rax, rsp",
        "2:",
        "sub rax, 4096",
        "mov byte ptr [rax], 0",
        "jmp 2b",
    )
}

/// A tight, `#[unsafe(naked)]` downward-walking loop -- identical in shape
/// to [`fault_test_guard_overflow`], but walking the *real* RSP instead of
/// a scratch pointer -- used by `__fault_test_stack_overflow` to simulate
/// an actual runaway stack overflow (e.g. unbounded recursion) rather than
/// a synthetic probe of the guard mapping alone.
///
/// This is the exact disaster scenario `gdt.rs`'s module doc comment
/// describes as the reason `DOUBLE_FAULT_STACK`/IST1 exist at all: RSP
/// itself walks into the guard page, so the CPU's own attempt to deliver
/// the resulting `#PF` -- which pushes its exception frame at the
/// now-invalid RSP -- also faults, escalating to `#8` Double Fault (Intel
/// SDM Vol 3, 6.15's contributory-exception rules). Without IST1 forcing a
/// stack switch for vector 8, that `#DF`'s own frame push would fail for
/// the exact same reason and silently triple-fault the whole machine into
/// a hard reset, with zero diagnostic output -- until this test, that path
/// had never actually been exercised end to end, only assumed correct from
/// the architecture. `tests/fault_test.py` asserts the resulting `#DF` is
/// caught and reported rather than the machine going silent.
///
/// # Safety
/// Must only ever be invoked from `__fault_test_stack_overflow`'s hidden
/// shell command -- it permanently destroys the mainline stack by design,
/// which is fine: the fault handler this deliberately triggers never
/// returns either way.
#[cfg(feature = "fault-test-hooks")]
#[unsafe(naked)]
unsafe extern "C" fn fault_test_stack_overflow() -> ! {
    core::arch::naked_asm!(
        "2:",
        "sub rsp, 4096",
        "mov byte ptr [rsp], 0",
        "jmp 2b",
    )
}

/// A constant the W^X write-protect test tries to overwrite. Being a plain
/// immutable `static` with no interior mutability, rustc places it in the
/// image's read-only `.rdata` section, which `paging.rs` maps read-only;
/// with `CR0.WP` set even ring 0 must fault on the write.
#[cfg(feature = "fault-test-hooks")]
static WP_PROBE: u64 = 0x5752_4954_4550_524F;

/// A writable buffer pre-filled with `ret` (0xC3) that the W^X
/// execute-disable test calls into. Being `static mut`, it lives in the
/// image's writable `.data` section, which `paging.rs` maps non-executable:
/// the call must fault on the instruction fetch. If NX were not enforced,
/// the `ret` would simply return and the test command would report that
/// instead of faulting -- a clean, observable failure, never a hang.
#[cfg(feature = "fault-test-hooks")]
static mut NX_PROBE: [u8; 16] = [0xC3; 16];

const PROMPT: &str = "> ";

/// The startup banner.
///
/// Deliberately no longer names specific hardware. It used to read "own
/// GDT/IDT/PIC/PIT", which was exactly true when it was written and
/// became a lie the moment this kernel started bringing up the APIC:
/// on a machine with usable ACPI tables the 8259 is fully masked and
/// the PIT's line is masked at the I/O APIC, so a banner claiming them
/// describes hardware that is sitting there doing nothing. And it
/// cannot simply be updated to name the APIC instead, because the
/// legacy path is still a real, supported fallback -- which controller
/// is live genuinely varies per boot, and is reported honestly by the
/// `ACPI:`/`APIC:`/`USB:` lines `main.rs` prints just above this banner,
/// where it can be specific about what actually happened.
///
/// "Custom Kernel" stays in the text: `tests/acceptance_test.py`'s
/// `wait_for_boot` uses that substring as its discriminator against
/// OVMF's own built-in EDK II shell banner, and has since this project
/// left Boot Services (see CHANGES.md).
///
/// The version comes from `Cargo.toml` at compile time, so the banner,
/// `version`, and the release archive's name can never disagree.
const BANNER: &str = concat!(
    "===========================================================\n",
    " WISP OS v", env!("CARGO_PKG_VERSION"), " -- Minimal Custom Kernel Shell\n",
    " hand-built from firmware up; no network stack, by design\n",
    "===========================================================\n",
    "Type 'help' for a list of commands. Esc stops a running program.\n",
    "\n",
);

/// ASCII backspace, as decoded by `keyboard.rs`.
const BACKSPACE: u8 = 0x08;
/// ASCII carriage return, as decoded by `keyboard.rs`.
const ENTER: u8 = 0x0d;

pub struct Shell {
    /// Every `APPS/*.wisp` file, read into memory by `fs::snapshot_apps`
    /// while Boot Services was still alive. `run <name>` and `ls`/`ls APPS`
    /// both serve from this instead of live filesystem access, which no
    /// longer exists once Boot Services is gone -- see `CHANGES.md` for
    /// why a real post-exit block-device driver is out of scope for this
    /// phase.
    apps: alloc::vec::Vec<(String, String)>,
    /// State for the xorshift64 PRNG backing WISP's `rand()` builtin.
    /// Seeded once at startup from the CPU cycle counter (see
    /// `Shell::new`); never re-seeded, so successive `run` invocations of
    /// different scripts don't repeat the same sequence.
    rng_state: u64,
    /// The AHCI save disk, already formatted and ready for
    /// `save_format::save`/`load` -- `None` if `main.rs`'s
    /// `bring_up_save_disk` couldn't get one working this boot (see its
    /// own doc comment for the first-class failure modes that lead here).
    /// Owned here, not dropped after boot, specifically so `save`/`load`
    /// (this module's own commands below, and the WISP builtins that will
    /// route through them in a later Build Order step) can issue commands
    /// at any later point in the shell's lifetime.
    save_disk: Option<ahci::Hba>,
}

impl Shell {
    pub fn new(apps: alloc::vec::Vec<(String, String)>, save_disk: Option<ahci::Hba>) -> Self {
        Self {
            apps,
            rng_state: seed_rng(),
            save_disk,
        }
    }

    /// Draw the startup banner and hand control to the shell loop. Never
    /// returns: idles the CPU between keystrokes via `keyboard::wait_for_key`
    /// (which itself halts the CPU between interrupts) and only exits the
    /// machine through a panic, an unhandled CPU exception, or a firmware
    /// shutdown -- there is no "return to firmware" path once Boot Services
    /// is gone.
    pub fn run(&mut self, console: &mut Console) -> ! {
        console.write_str(BANNER);
        // Boot-complete marker for the test harnesses: emitted exactly once,
        // after the banner is on screen and immediately before the first
        // prompt starts accepting keystrokes. `tests/*.py` wait for this
        // line in the debugcon log instead of sleeping a fixed time and
        // hoping boot finished -- a fixed sleep was observed to lose
        // keystrokes when the host was busy running several VMs at once.
        crate::debugcon::write_str("[shell] ready\n");

        loop {
            self.print_prompt(console);
            let line = read_line(console, LineMode::Prompt);
            console.newline();
            self.execute(line.trim(), console);
        }
    }

    fn print_prompt(&self, console: &mut Console) {
        console.set_fg(gfx::ACCENT);
        console.write_str(PROMPT);
        console.set_fg(gfx::FOREGROUND);
    }

    fn execute(&mut self, line: &str, console: &mut Console) {
        if line.is_empty() {
            return;
        }

        let (cmd, rest) = match line.split_once(' ') {
            Some((c, r)) => (c, r),
            None => (line, ""),
        };

        match cmd {
            "help" => self.cmd_help(console),
            "clear" => console.clear(),
            "echo" => self.cmd_echo(rest, console),
            "ls" => self.cmd_ls(rest, console),
            "run" => self.cmd_run(console, rest),
            "uptime" => self.cmd_uptime(console),
            "version" => self.cmd_version(console),
            "mem" => self.cmd_mem(console),
            "save" => self.cmd_save(rest, console),
            "load" => self.cmd_load(rest, console),
            // Hidden, feature-gated, never listed in `cmd_help`: exists
            // purely so `tests/fault_test.py` can prove the GDT/IDT/
            // fault-screen path built in this phase actually catches and
            // reports a real CPU exception instead of silently resetting
            // the VM. Off in the normal release binary -- see Cargo.toml.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_int3" => {
                console.write_str("triggering a deliberate int3 breakpoint exception...\n");
                // SAFETY: `int3` is the dedicated one-byte breakpoint
                // opcode (0xCC); it's always permitted regardless of the
                // IDT gate's DPL (unlike the general `int n` form, whose
                // gate-DPL-vs-CPL check this bypasses), and vector 3 has a
                // real gate in `idt.rs` pointing at `fault_handler_entry`,
                // which reports the exception (to both the debug port and
                // the framebuffer) and halts -- it never returns, so
                // nothing after this can run, by design.
                unsafe { core::arch::asm!("int3", options(nomem, nostack)) };
            }
            // Proves `paging.rs`'s own page tables actually fault on a
            // genuinely unmapped address, not just "whatever firmware
            // happened to leave unmapped" -- `tests/fault_test.py` asserts
            // both the vector (#14 Page Fault) and `cr2` (the faulting
            // address) against this exact target.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_pagefault" => {
                console.write_str("triggering a deliberate page fault (write to an unmapped address)...\n");
                // SAFETY: none -- this is a deliberate fault, by design.
                // `FAULT_TEST_UNMAPPED_ADDR` is canonical (so this reliably
                // takes `#PF`, not `#GP` from a non-canonical address) but
                // far outside anything `paging::init` ever maps -- this
                // kernel's entire physical memory map tops out at a few
                // GiB (see `paging.rs`'s own boot-time "pages_mapped~="
                // debugcon line), and this target is 16 TiB up, nowhere
                // near any real or reserved region a real memory map could
                // plausibly report. A plain volatile write (not raw asm)
                // is fine here, unlike the guard-overflow test below: there
                // is no loop or recursion for the optimizer to collapse,
                // and a volatile write is never elided.
                unsafe {
                    core::ptr::write_volatile(FAULT_TEST_UNMAPPED_ADDR as *mut u8, 0x42);
                }
            }
            // Proves the *mainline* stack's guard page (Build Order Step 4
            // of the Phase 2 plan) is genuinely unmapped, not just "there
            // happens to be a gap." `tests/fault_test.py` asserts the
            // vector (#14 Page Fault) and that `cr2` falls inside the
            // guard range `paging.rs` itself logged at boot (see
            // `paging::init`'s "[paging] guard page:" debugcon lines) --
            // ground truth from the same run, not a guessed address.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_guard_overflow" => {
                console.write_str("triggering a deliberate mainline-stack guard-page overflow...\n");
                // SAFETY: none -- this permanently destroys the mainline
                // stack by design, which is fine: the fault handler this
                // deliberately triggers never returns either way (see
                // `idt.rs::fault_handler_entry`), so nothing ever tries to
                // unwind back through the frames this walks past.
                unsafe { fault_test_guard_overflow() };
            }
            // Proves the double-fault stack/IST1 mechanism (`gdt.rs`)
            // actually survives a *real* stack overflow -- RSP itself
            // walking into the guard page, not just a synthetic probe --
            // instead of silently triple-faulting the machine. See
            // `fault_test_stack_overflow`'s doc comment for the full
            // reasoning; `tests/fault_test.py` asserts the resulting #8
            // Double Fault is caught and reported.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_stack_overflow" => {
                console.write_str("triggering a deliberate real stack overflow (RSP walks into the guard page)...\n");
                // SAFETY: none -- destroys the mainline stack (including
                // RSP itself) by design; this is exactly the scenario
                // IST1/DOUBLE_FAULT_STACK exist to survive.
                unsafe { fault_test_stack_overflow() };
            }
            // W^X, half one: a ring-0 write to the kernel's own read-only
            // data must fault (#14 with P=1, W=1) now that `.rdata` is
            // mapped read-only and `CR0.WP` is set. The probe's address is
            // logged first so `tests/fault_test.py` can match `cr2` against
            // it exactly.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_wp" => {
                let target = core::ptr::addr_of!(WP_PROBE) as u64;
                let _ = crate::debugcon::write_fmt(format_args!("[fault_test] wp target=0x{target:x}\n"));
                console.write_str("triggering a deliberate write to read-only kernel data...\n");
                // SAFETY: none -- a deliberate write to a read-only page,
                // which must fault and never return.
                unsafe { core::ptr::write_volatile(target as *mut u64, 0) };
                console.set_fg(gfx::ERROR);
                console.write_str("write to read-only data did NOT fault: write protection is not enforced\n");
                console.set_fg(gfx::FOREGROUND);
                crate::debugcon::write_str("[fault_test] wp NOT enforced\n");
            }
            // W^X, half two: executing from a writable data page must fault
            // (#14 with the instruction-fetch bit set) now that every
            // non-code page carries NX.
            #[cfg(feature = "fault-test-hooks")]
            "__fault_test_nx" => {
                let target = core::ptr::addr_of!(NX_PROBE) as *const u8;
                let _ = crate::debugcon::write_fmt(format_args!("[fault_test] nx target=0x{:x}\n", target as u64));
                console.write_str("triggering a deliberate call into a non-executable data page...\n");
                // SAFETY: none -- a deliberate jump into data, which must
                // fault on the instruction fetch. Were NX not enforced, the
                // buffer holds only `ret`, so the call returns harmlessly
                // and the failure is reported below.
                unsafe {
                    let f: extern "C" fn() = core::mem::transmute::<*const u8, extern "C" fn()>(target);
                    f();
                }
                console.set_fg(gfx::ERROR);
                console.write_str("call into data did NOT fault: execute-disable is not enforced\n");
                console.set_fg(gfx::FOREGROUND);
                crate::debugcon::write_str("[fault_test] nx NOT enforced\n");
            }
            _ => {
                console.set_fg(gfx::ERROR);
                let _ = write!(console, "unknown command: {cmd}\n");
                console.set_fg(gfx::FOREGROUND);
            }
        }
    }

    fn cmd_help(&self, console: &mut Console) {
        console.write_str(
            "Available commands:\n\
             \x20\x20help          Show this list of commands\n\
             \x20\x20clear         Clear the screen\n\
             \x20\x20echo <text>   Print <text> back to the screen\n\
             \x20\x20ls [APPS]     List the in-memory APPS snapshot\n\
             \x20\x20run <name>    Run APPS/<name>.wisp (a WISP program)\n\
             \x20\x20uptime        Show elapsed time since boot (timer ticks)\n\
             \x20\x20mem           Show heap and physical memory usage\n\
             \x20\x20version       Show the kernel version and protections\n\
             \x20\x20save <k> <v>  Save <v> under key <k> on the AHCI save disk\n\
             \x20\x20load <k>      Print the value saved under key <k>\n\
             Press Esc to stop a running program.\n",
        );
    }

    fn cmd_echo(&self, rest: &str, console: &mut Console) {
        console.write_str(rest);
        console.newline();
    }

    /// `save <key> <value>`: stores everything after the key as a plain
    /// WISP string (the same "rest of the line, verbatim" shape as
    /// `echo`), the same real, hardware-backed save path a WISP script's
    /// `save()` builtin will route through in a later Build Order step --
    /// a second, WISP-independent verification surface that isolates "the
    /// driver is broken" from "the interpreter plumbing is broken."
    fn cmd_save(&mut self, rest: &str, console: &mut Console) {
        let (key, value_text) = match rest.split_once(' ') {
            Some((k, v)) => (k, v),
            None => (rest, ""),
        };
        if key.is_empty() {
            console.set_fg(gfx::ERROR);
            console.write_str("save: expected 'save <key> <value>'\n");
            console.set_fg(gfx::FOREGROUND);
            return;
        }
        let Some(hba) = self.save_disk.as_ref() else {
            console.set_fg(gfx::ERROR);
            console.write_str("save: no AHCI save disk available this boot\n");
            console.set_fg(gfx::FOREGROUND);
            return;
        };
        let value = Value::Str(Rc::new(value_text.to_string()));
        // SAFETY: `hba` came from `main.rs`'s `bring_up_save_disk`, which
        // already called `save_format::ensure_formatted` on it before
        // handing it to this shell -- `save_format::save`'s own contract.
        match unsafe { save_format::save(hba, key, &value) } {
            Ok(()) => {
                let _ = write!(console, "saved '{key}'\n");
            }
            Err(msg) => {
                console.set_fg(gfx::ERROR);
                let _ = write!(console, "save: {msg}\n");
                console.set_fg(gfx::FOREGROUND);
            }
        }
    }

    /// `load <key>`: prints the value's display string (the same rendering
    /// `print()` uses), or a clear "not found"/error message.
    fn cmd_load(&mut self, rest: &str, console: &mut Console) {
        let key = rest.trim();
        if key.is_empty() {
            console.set_fg(gfx::ERROR);
            console.write_str("load: expected 'load <key>'\n");
            console.set_fg(gfx::FOREGROUND);
            return;
        }
        let Some(hba) = self.save_disk.as_ref() else {
            console.set_fg(gfx::ERROR);
            console.write_str("load: no AHCI save disk available this boot\n");
            console.set_fg(gfx::FOREGROUND);
            return;
        };
        // SAFETY: same reasoning as `cmd_save`.
        match unsafe { save_format::load(hba, key) } {
            Ok(Some(value)) => {
                let _ = write!(console, "{}\n", value.to_display_string());
            }
            Ok(None) => {
                let _ = write!(console, "load: '{key}' not found\n");
            }
            Err(msg) => {
                console.set_fg(gfx::ERROR);
                let _ = write!(console, "load: {msg}\n");
                console.set_fg(gfx::FOREGROUND);
            }
        }
    }

    /// `ls`/`ls APPS` both list the in-memory snapshot; there is no other
    /// directory to list once Boot Services (and with it, live filesystem
    /// access) is gone. Any other argument is reported as not available
    /// rather than silently showing the same snapshot under a misleading
    /// path, so a script or user typing `ls SOMETHINGELSE` gets an honest
    /// answer instead of a confusing one.
    fn cmd_ls(&self, path: &str, console: &mut Console) {
        let normalized = path.trim().trim_matches('/').to_ascii_uppercase();
        if !normalized.is_empty() && normalized != "APPS" {
            console.set_fg(gfx::ERROR);
            let _ = write!(
                console,
                "ls: '{path}': no live filesystem after boot -- only the in-memory APPS snapshot is available (try 'ls' or 'ls APPS')\n"
            );
            console.set_fg(gfx::FOREGROUND);
            return;
        }

        if self.apps.is_empty() {
            console.write_str("(empty)\n");
            return;
        }
        for (name, source) in &self.apps {
            let _ = write!(console, "     {name}  ({} bytes)\n", source.len());
        }
    }

    fn cmd_run(&mut self, console: &mut Console, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            console.set_fg(gfx::ERROR);
            console.write_str("run: expected a program name, e.g. 'run hello'\n");
            console.set_fg(gfx::FOREGROUND);
            return;
        }

        let filename = resolve_program_filename(name);
        let source = match self.apps.iter().find(|(n, _)| n.eq_ignore_ascii_case(&filename)) {
            Some((_, s)) => s.clone(),
            None => {
                console.set_fg(gfx::ERROR);
                let _ = write!(console, "run: '{filename}' not found in the APPS snapshot\n");
                console.set_fg(gfx::FOREGROUND);
                return;
            }
        };

        // A fresh start: an Esc pressed before this program began (at the
        // prompt, or to stop the previous program) must not stop this one.
        keyboard::clear_stop_request();
        let mut host = ShellHost {
            console,
            rng: &mut self.rng_state,
            save_disk: self.save_disk.as_ref(),
            last_usb_poll_tick: u64::MAX,
        };
        let result = wisp::run_source(&source, &mut host);
        // And a clean exit: keys typed into the program (including the Esc
        // that stopped it) never spill into the shell prompt.
        keyboard::clear_stop_request();
        if let Err(e) = result {
            // A stop is logged (just the fact and the line -- never message
            // text, which can quote what the user typed) so the test
            // harness can verify it without OCR'ing a scrolling screen.
            if e.message == wisp::interp::STOPPED_MESSAGE {
                let _ = crate::debugcon::write_fmt(format_args!(
                    "[wisp] {filename} stopped by Esc at line {}\n",
                    e.line
                ));
            }
            console.set_fg(gfx::ERROR);
            let _ = write!(console, "{}\n", e.to_display_string());
            console.set_fg(gfx::FOREGROUND);
        }
    }

    /// `version`: what this build is, and which hardware protections are
    /// live this boot -- the things a bug report needs first.
    fn cmd_version(&self, console: &mut Console) {
        let _ = write!(
            console,
            "WISP OS v{} (x86_64 UEFI, no_std Rust)\n\
             memory protection: W^X, NX {}, kernel write-protect on, guard-paged stacks\n\
             clock: {} at {} Hz\n\
             networking: none (by design)\n",
            env!("CARGO_PKG_VERSION"),
            if paging::nx_enabled() { "on" } else { "unsupported by this CPU" },
            if lapic::timer_active() { "Local APIC timer" } else { "PIT" },
            clock::TICK_HZ,
        );
    }

    /// `mem`: heap occupancy and fragmentation, and physical frames.
    fn cmd_mem(&self, console: &mut Console) {
        let heap = allocator::stats();
        let used = heap.total - heap.free;
        let (free_frames, total_frames) = pmm::stats();
        let frame_kib = pmm::FRAME_BYTES / 1024;
        let _ = write!(
            console,
            "heap:   {} KiB used / {} KiB total, {} KiB free in {} block(s), largest {} KiB\n\
             frames: {} MiB free of {} MiB spanned ({} free 4 KiB frames)\n",
            used / 1024,
            heap.total / 1024,
            heap.free / 1024,
            heap.free_blocks,
            heap.largest_free / 1024,
            free_frames as u64 * frame_kib / 1024,
            total_frames as u64 * frame_kib / 1024,
            free_frames,
        );
    }

    fn cmd_uptime(&self, console: &mut Console) {
        let ms = clock::uptime_ms();
        let _ = write!(
            console,
            "up {}.{:03}s ({} ticks)\n",
            ms / 1000,
            ms % 1000,
            clock::ticks()
        );
    }
}

/// `run <name>` always resolves against the `APPS` snapshot, auto-appending
/// `.wisp` when `name` has no extension of its own, per
/// `docs/WISP_LANGUAGE.md`.
fn resolve_program_filename(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}.wisp")
    }
}

/// Who is reading a line, which decides what Esc does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineMode {
    /// The shell's own `>` prompt: Esc erases everything typed so far on
    /// this line (and is otherwise harmless -- there is nothing to stop).
    Prompt,
    /// A WISP program's `input()`: Esc ends the read immediately so the
    /// program can be stopped (see `builtins::builtin_input`).
    Program,
}

/// Read one line of text from the keyboard with echo and backspace,
/// blocking (idling the CPU between keystrokes via `keyboard::wait_for_key`)
/// until Enter is pressed. Shared by the shell's own prompt and by the WISP
/// `input()` builtin, so a running script's text input behaves identically
/// to the shell's -- except for Esc; see [`LineMode`].
fn read_line(console: &mut Console, mode: LineMode) -> String {
    let mut line = String::new();
    loop {
        let byte = keyboard::wait_for_key();
        match byte {
            ENTER => return line,
            keyboard::ESC => match mode {
                LineMode::Program => return line,
                LineMode::Prompt => {
                    while line.pop().is_some() {
                        console.backspace();
                    }
                    // Nothing is running to stop; don't let this Esc
                    // linger and stop the next program the instant it
                    // starts. (`cmd_run` clears it too -- belt and braces.)
                    keyboard::clear_stop_request();
                }
            },
            BACKSPACE => {
                if line.pop().is_some() {
                    console.backspace();
                }
            }
            0x20..=0x7e => {
                line.push(byte as char);
                console.put_ascii(byte);
            }
            _ => {}
        }
    }
}

/// Seed the `rand()` PRNG from the CPU's cycle counter. Not
/// cryptographically secure -- see `docs/WISP_LANGUAGE.md` -- just varied
/// enough run to run for game mechanics.
fn seed_rng() -> u64 {
    let ticks = unsafe { core::arch::x86_64::_rdtsc() };
    if ticks == 0 {
        // xorshift64 requires a nonzero state; astronomically unlikely in
        // practice, but a fixed fallback keeps `rand()` well-defined
        // regardless.
        0x9E3779B97F4A7C15
    } else {
        ticks
    }
}

fn next_rand_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Bridges the WISP interpreter's `Host` trait to this shell's actual
/// screen, keyboard, timer, and (Phase 2, Part B) AHCI save disk, for the
/// duration of one `run` command. `save_disk` borrows `Shell::save_disk`
/// rather than owning it, since the `Hba` itself must keep living in the
/// `Shell` across every `run` invocation, not just this one.
struct ShellHost<'a> {
    console: &'a mut Console,
    rng: &'a mut u64,
    save_disk: Option<&'a ahci::Hba>,
    /// The tick at which the USB keyboard was last polled for Esc by
    /// [`wisp::interp::Host::stop_requested`]; see that method.
    last_usb_poll_tick: u64,
}

impl wisp::interp::Host for ShellHost<'_> {
    fn print(&mut self, s: &str) {
        self.console.write_str(s);
    }

    fn input(&mut self, prompt: &str) -> String {
        self.console.write_str(prompt);
        let line = read_line(self.console, LineMode::Program);
        self.console.newline();
        line
    }

    fn clear(&mut self) {
        self.console.clear();
    }

    fn sleep_ms(&mut self, ms: u64) {
        // Wakes every tick to check for Esc (polling the USB keyboard, which
        // has no interrupt of its own), so a long `sleep_ms()` stops within
        // 10 ms of the key instead of at the end of the sleep.
        clock::sleep_ms_unless(ms, || {
            usb_hid::poll();
            keyboard::stop_requested()
        });
    }

    fn stop_requested(&mut self) -> bool {
        // The PS/2 keyboard raises the flag from its interrupt handler; the
        // USB keyboard only reports when polled. Poll it at most once per
        // timer tick -- often enough that Esc is noticed within 10 ms, and
        // rare enough that a tight WISP loop (which calls this every
        // iteration) is not slowed by MMIO traffic.
        let now = clock::ticks();
        if now != self.last_usb_poll_tick {
            self.last_usb_poll_tick = now;
            usb_hid::poll();
        }
        keyboard::stop_requested()
    }

    fn time_ms(&mut self) -> u64 {
        clock::uptime_ms()
    }

    fn rand_u64(&mut self) -> u64 {
        next_rand_u64(self.rng)
    }

    fn save(&mut self, key: &str, value: &Value) -> Result<(), &'static str> {
        let hba = self.save_disk.ok_or("no AHCI save disk available this boot")?;
        // SAFETY: `hba` came from `main.rs`'s `bring_up_save_disk`, which
        // already called `save_format::ensure_formatted` on it before
        // handing it to this shell -- `save_format::save`'s own contract.
        unsafe { save_format::save(hba, key, value) }
    }

    fn load(&mut self, key: &str) -> Result<Option<Value>, &'static str> {
        let hba = self.save_disk.ok_or("no AHCI save disk available this boot")?;
        // SAFETY: same reasoning as `save` above.
        unsafe { save_format::load(hba, key) }
    }

    // ---- WISP v2: pixel/graphics primitives ----------------------------
    //
    // Pure delegation to `Console`'s own graphics methods (`gfx.rs`),
    // which do the actual bounds-checked pixel work; the only thing this
    // layer adds is unpacking the WISP-level `0xRRGGBB` `u32` (already
    // validated by `wisp::builtins::expect_color`) into the `gfx::Color`
    // every `Console` drawing method takes.

    fn screen_width(&self) -> usize {
        self.console.width_px()
    }

    fn screen_height(&self) -> usize {
        self.console.height_px()
    }

    fn plot(&mut self, x: i64, y: i64, color: u32) {
        self.console.plot_pixel(x, y, gfx::Color::from_rgb24(color));
    }

    fn draw_rect(&mut self, x: i64, y: i64, w: usize, h: usize, color: u32) {
        self.console
            .draw_rect_filled(x, y, w, h, gfx::Color::from_rgb24(color));
    }

    fn draw_line(&mut self, x1: i64, y1: i64, x2: i64, y2: i64, color: u32) {
        self.console
            .draw_line(x1, y1, x2, y2, gfx::Color::from_rgb24(color));
    }

    fn fill_screen(&mut self, color: u32) {
        self.console.fill_screen(gfx::Color::from_rgb24(color));
    }
}
