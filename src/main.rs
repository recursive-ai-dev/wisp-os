//! WISP OS: a freestanding x86_64 kernel and text shell -- resident in UEFI
//! only long enough to snapshot its filesystem and exit Boot Services, then
//! running on its own hand-written GDT/IDT, W^X page tables, heap and frame
//! allocators, ACPI/APIC interrupt routing (with an 8259/PIT fallback),
//! PS/2 and USB keyboard drivers, and AHCI storage for the rest of its
//! life. No Linux code, no borrowed drivers, no borrowed boot protocol, no
//! network stack -- every piece past `exit_boot_services` is our own
//! `no_std` Rust, in the same spirit as the WISP language it runs.
//!
//! It owns the screen by writing pixels directly into the Graphics Output
//! Protocol framebuffer using an embedded 8x16 bitmap font, rather than
//! going through the UEFI Simple Text Output protocol -- and, once Boot
//! Services is gone, that same raw framebuffer keeps working completely
//! unmodified: `paging.rs` identity-maps it (virt == phys, same as every
//! other mapping this kernel builds) before the switch away from
//! firmware's own page tables, so every pointer/length pair captured here
//! stays valid on both sides of that switch, with no address translation
//! ever required. See `paging.rs` for the own-page-tables/guard-page
//! story and `main.rs`'s boot sequence below for exactly where that switch
//! happens.

#![no_std]
#![no_main]

extern crate alloc;

mod acpi;
mod ahci;
mod allocator;
mod clock;
mod debugcon;
mod fs;
mod font;
mod gdt;
mod gfx;
mod idt;
mod ioapic;
mod keyboard;
mod lapic;
mod libc_shims;
mod panic;
mod paging;
mod pci;
mod pic;
mod pit;
mod pmm;
mod save_format;
mod shell;
mod sync;
mod usb_hid;
mod wisp;
mod xhci;

use core::arch::{asm, naked_asm};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::table::boot::{BootServices, MemoryType};
use uefi::table::system_table_boot;

use gfx::Console;

#[entry]
// The `panic-screen-smoke-test` bring-up feature panics deliberately right
// after the console exists, which makes the rest of this function
// unreachable (and `console` never mutated) in that one build only.
#[cfg_attr(feature = "panic-screen-smoke-test", allow(unreachable_code, unused_mut, unused_variables))]
fn efi_main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    // Must be the very first thing that happens in this binary: nothing
    // before this may allocate, directly or transitively -- including
    // inside `uefi::helpers::init` just below -- because this installs the
    // only `#[global_allocator]` the binary has. See allocator.rs. (The
    // `uefi` crate's own `global_allocator`/`panic_handler` Cargo features
    // are disabled in Cargo.toml specifically so ours are the only ones.)
    unsafe { allocator::init() };

    if let Err(e) = uefi::helpers::init(&mut st) {
        return e.status();
    }

    // We need a `&BootServices` for console setup and the filesystem
    // snapshot below, obtained independently of the owned `st` binding
    // (see the historical note this replaced in git history: this is a
    // fresh handle to the same firmware table, not a second borrow of
    // `st`). Unlike the previous UEFI-resident version of this binary,
    // nothing here also needs `st.stdin()` -- keyboard input now comes
    // from our own `keyboard` driver, so `st` itself is never borrowed at
    // all before it's consumed by `exit_boot_services` below.
    let st_for_bs =
        system_table_boot().expect("system table not installed (uefi::helpers::init not run?)");
    let bs: &BootServices = st_for_bs.boot_services();

    let mut console = match init_console(bs) {
        Ok(console) => console,
        Err(status) => {
            // We have no graphics console to report through, so fall back
            // to the firmware's own text output for this one diagnostic.
            uefi::println!("fatal: could not initialize the GOP framebuffer console");
            bs.stall(5_000_000);
            return status;
        }
    };

    // Register the raw framebuffer with the fault-screen path (panic.rs)
    // so a panic or an unhandled CPU exception (idt.rs) can draw a report
    // directly, independent of whatever state the interactive `Console`
    // was left in. Safe immediately: these are the same raw
    // pointer/length/geometry fields `Console` itself holds, and they stay
    // valid for the life of the program -- across `exit_boot_services`,
    // and across the switch to the kernel's own page tables, which
    // identity-map the framebuffer.
    {
        let (ptr, len, width, height, stride, bgr) = console.raw_parts();
        panic::register(ptr, len, width, height, stride, bgr);
    }

    #[cfg(feature = "panic-screen-smoke-test")]
    panic!("allocator + panic screen smoke test: if you can read this, both work");

    // Build the GDT and IDT contents now, while Boot Services is still
    // alive. This is safe at any point -- it's pure memory writes, no CPU
    // descriptor-table state changes yet -- but *loading* them (`lgdt`/
    // `lidt`/`ltr`/the CS reload) must wait until after
    // `exit_boot_services` returns, so we never touch live descriptor-table
    // state while firmware code might still be running against its own.
    unsafe {
        gdt::init();
        idt::init();
    }

    // Read every `APPS/*.wisp` file into memory now: this is the last use
    // of Boot Services (and of `bs`/`image`) in the whole program. Once
    // `exit_boot_services` returns, the Simple File System protocol this
    // depends on is gone for good -- see `fs::snapshot_apps`'s doc comment
    // and `CHANGES.md` for why a real post-exit block-device driver is
    // separate, out-of-scope future work for this phase.
    let apps = fs::snapshot_apps(bs, image);

    // Where the firmware's PE loader put this image, and how big it is --
    // `paging::init` needs it to map the kernel's own sections under W^X
    // (code read-only+executable, everything else non-executable). Only
    // two integers cross the `exit_boot_services` boundary; `None` (the
    // protocol unavailable, which no conforming firmware does) degrades to
    // mapping the image writable+executable, logged, rather than failing.
    let image_range = image_location(bs, image);

    // Capture the ACPI RSDP's physical address while the UEFI system
    // table -- the only thing that knows where it is -- still exists.
    // Only this one integer crosses the `exit_boot_services` boundary;
    // the tables it points at live in firmware-reserved memory that
    // `pmm.rs` never hands out and `paging.rs` identity-maps along with
    // every other memory-map region, so they stay readable afterward.
    // See `acpi.rs`'s module doc comment. `None` here means this
    // firmware publishes no ACPI tables at all -- a first-class outcome
    // this kernel keeps booting through, on the legacy 8259 path.
    let rsdp_addr = acpi::find_rsdp(&st);

    console.write_str("Exiting UEFI Boot Services...\n");

    // SAFETY: every protocol reference obtained above (`bs`, `st_for_bs`,
    // the GOP handle inside `init_console`) is either about to go out of
    // scope or was already converted to a raw pointer/plain-integer form
    // that outlives Boot Services (`console`'s framebuffer fields,
    // `panic`'s registered copies of them). `apps` is an owned, heap
    // allocated `Vec` on our own allocator, independent of Boot Services.
    // `LOADER_DATA` is the memory type this loaded image's own
    // Boot-Services-issued allocations (there are none we still care
    // about; our heap is a static array, not a Boot Services allocation)
    // would be folded into -- it does not affect what becomes free, only
    // how this call's own bookkeeping allocation is typed.
    let (_runtime_st, mmap) = unsafe { st.exit_boot_services(MemoryType::LOADER_DATA) };

    // From here on, Boot Services no longer exists. Nothing below this
    // line may reference `bs`, `st_for_bs`, or any UEFI protocol.

    // Conventional memory only, for now: firmware's page tables, IDT and
    // the stack this code is running on still live in boot-services
    // memory, which stays withheld until `continue_boot_on_new_stack` has
    // replaced all three (see `pmm.rs`).
    pmm::init(&mmap);
    match pmm::self_test() {
        Ok(()) => {
            console.write_str("Physical frame allocator self-test: OK\n");
            debugcon::write_str("\n[pmm] self-test OK\n");
        }
        Err(msg) => {
            // The self-test only exercises two frames; a failure here means
            // the bitmap is wrong somewhere, which paging and the DMA
            // drivers will hit next. Report it loudly and keep going -- a
            // boot that continues with a visible error is diagnosable; one
            // that stops here shows nothing about what follows.
            console.set_fg(gfx::ERROR);
            let _ = core::fmt::Write::write_fmt(&mut console, format_args!("Physical frame allocator self-test: FAILED: {msg}\n"));
            console.set_fg(gfx::FOREGROUND);
            debugcon::write_str("\n[pmm] self-test FAILED: ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
        }
    }

    // Prove the heap allocator's split/align/coalesce paths on the live
    // heap, every boot, the same standard `pmm::self_test` sets for the
    // frame allocator. A failure is reported loudly but is not fatal:
    // everything allocated so far worked, and refusing to boot would hide
    // the very diagnostic this exists to produce.
    match allocator::self_test() {
        Ok(()) => {
            console.write_str("Heap allocator self-test: OK\n");
            debugcon::write_str("[heap] self-test OK\n");
        }
        Err(msg) => {
            console.set_fg(gfx::ERROR);
            let _ = core::fmt::Write::write_fmt(&mut console, format_args!("Heap allocator self-test: FAILED: {msg}\n"));
            console.set_fg(gfx::FOREGROUND);
            debugcon::write_str("[heap] self-test FAILED: ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
        }
    }

    // Build our own page tables now, still running under whatever tables
    // firmware left active -- this step only ever *allocates and writes*
    // through the still-live old mapping via `pmm::alloc_frame`; it does not
    // change what the CPU is actually using. `extra_ranges` covers the GOP
    // framebuffer (an MMIO window firmware does not reliably describe as
    // its own memory-map entry -- see `paging.rs`'s module doc comment);
    // `console.raw_parts()` is just a read of fields already captured
    // earlier, safe to call again here now that Boot Services is gone.
    let (fb_ptr, fb_len, ..) = console.raw_parts();
    let extra_ranges = [(fb_ptr as u64, fb_len as u64, paging::PageFlags::Mmio)];
    // Build Order Steps 3-4 of the Phase 2 plan: the double-fault stack's
    // guard page landed first (lowest risk -- nothing runs on IST1 during
    // normal operation, only during an actual `#DF`); the mainline stack's
    // guard page lands here too, ahead of the live switch onto that stack
    // below, so the very first thing ever mapped under the new tables
    // already protects the stack about to become "the" stack.
    let guard_ranges = [
        gdt::double_fault_stack_guard_range(),
        gdt::mainline_stack_guard_range(),
    ];
    let root_phys = unsafe { paging::init(&mmap, &extra_ranges, &guard_ranges, image_range) };

    // First point in the entire program that live CPU descriptor-table
    // state changes. Order matters: the IDT's gates name
    // `gdt::KERNEL_CODE_SELECTOR`, which must already be a valid, loaded
    // GDT entry before `idt::load` (and especially before `sti`, below)
    // runs. Loading our own IDT *before* the `CR3` switch below means our
    // own fault handler is already the live safety net if the new mapping
    // turns out to be subtly wrong -- a page fault or general-protection
    // fault from a bad mapping gets caught and reported instead of running
    // under whatever (or however incomplete) a still-firmware-owned IDT
    // would have done with it.
    unsafe {
        gdt::load();
        idt::load();
    }

    // Switch to the page tables built above: identity-mapped, guard pages
    // unmapped, and (since v1.0) W^X -- the image's code read-only and
    // executable, everything else non-executable, with EFER.NXE and CR0.WP
    // enabled by `paging::load` itself. Historically this switch landed
    // fully permissive first and each restriction was added as its own
    // independently-tested step (see CHANGES.md); `tests/fault_test.py`
    // now proves each restriction faults where it should.
    unsafe {
        paging::load(root_phys);
    }

    // Build Order Step 4 of the Phase 2 plan: take ownership of the
    // dedicated, guard-paged mainline stack -- until this point, the shell
    // and every WISP script have run on whatever stack UEFI's own loader
    // happened to hand this image, never explicitly owned or protected.
    //
    // `console` and `apps` are boxed (moved onto the heap) *before* the
    // switch specifically so nothing this kernel still needs is left
    // referencing the stack about to be abandoned -- once the switch
    // happens, any address on the old stack is meaningless. `Box::into_raw`
    // hands the single resulting pointer to the trampoline below, which is
    // the only code in this whole program that ever changes RSP directly
    // outside of an interrupt/exception entry (see `idt.rs`'s stubs for the
    // other case).
    let ctx = Box::into_raw(Box::new(BootContinuation { console, apps, rsdp_addr }));
    let new_rsp = gdt::mainline_stack_top();
    unsafe { switch_stack_and_continue(new_rsp, ctx as u64) }
}

/// Bundles every piece of live state [`continue_boot_on_new_stack`] needs
/// to carry across the mainline-stack switch. Boxed (heap-allocated)
/// specifically so its address is stable regardless of which stack it's
/// referenced from -- see [`switch_stack_and_continue`]'s doc comment.
struct BootContinuation {
    console: Console,
    apps: Vec<(String, String)>,
    /// The ACPI RSDP's physical address, captured before Boot Services
    /// went away (see `acpi::find_rsdp`). `None` if this firmware
    /// publishes no ACPI tables.
    rsdp_addr: Option<u64>,
}

/// One-way trampoline: load the dedicated, guard-paged mainline stack into
/// RSP, then transfer control into [`continue_boot_on_new_stack`] -- see
/// the Phase 2 plan's Build Order Step 4 and `paging.rs`'s module doc
/// comment ("Guard pages") for why this needs to be hand-written asm
/// rather than ordinary Rust code.
///
/// # Why this can't be plain Rust
/// Any ordinary Rust code -- even a single extra statement -- executing
/// between "change RSP" and "start using the new stack" would be running
/// with a stack pointer the compiler doesn't know changed: its own notion
/// of "where are my locals" is still relative to the *old* stack, so no
/// such window may exist at all. A naked function's body is exactly and
/// only the instructions written here -- no compiler-generated prologue,
/// no spilled locals -- which is what makes "change RSP, then immediately
/// transfer control" atomic from the compiler's point of view.
///
/// # Calling convention
/// This target's `extern "C"` is the Microsoft x64 ABI (see `idt.rs`'s
/// module doc comment for why): the first two integer/pointer arguments
/// arrive in RCX, RDX. `new_rsp` (RCX) becomes the new stack pointer, read
/// before it's clobbered; `ctx` (RDX) is then moved into RCX so
/// [`continue_boot_on_new_stack`] receives it as *its* own first argument.
/// `sub rsp, 32` reserves the 32-byte MS x64 shadow space the callee is
/// entitled to spill its register argument into, and a real `call` (not a
/// bare `jmp`) pushes a genuine return address and leaves RSP at exactly
/// the alignment the callee's compiler-generated prologue expects --
/// identical reasoning to every stub in `idt.rs`. `call` is used instead of
/// `jmp`/tail-call specifically to reuse that already-proven pattern rather
/// than hand-deriving the alignment math a `jmp`-based trampoline would
/// need; `continue_boot_on_new_stack` is `-> !`, so the pushed return
/// address is simply never used. `ud2` after the call is the same
/// belt-and-brace `idt.rs` already uses: unreachable in practice, and if
/// ever reached (a future edit that lets `continue_boot_on_new_stack`
/// return), it takes an immediate, obvious `#UD` instead of running off
/// into whatever garbage follows.
///
/// This requires `new_rsp` to already be 16-byte aligned, which
/// [`gdt::mainline_stack_top`] guarantees (a page-aligned base plus a
/// stack size that's a multiple of 16) -- the same alignment `idt.rs`'s
/// hardware-entry stubs rely on the CPU to provide, provided here instead
/// by construction.
///
/// # Safety
/// `new_rsp` must be [`gdt::mainline_stack_top`]'s return value (a valid,
/// guard-paged, currently-unused stack top); `ctx` must be a live
/// `Box<BootContinuation>` pointer obtained via `Box::into_raw`, whose
/// ownership this call permanently transfers to
/// [`continue_boot_on_new_stack`]. Must be called after `paging::load` (the
/// new stack is only mapped once the new page tables are active), and
/// never returns -- there is no code after this call in `efi_main`, and
/// there must never be: anything placed there would be running on an
/// abandoned stack that this function has already jumped away from.
#[unsafe(naked)]
unsafe extern "C" fn switch_stack_and_continue(new_rsp: u64, ctx: u64) -> ! {
    naked_asm!(
        "mov rsp, rcx",  // new_rsp -> RSP: the live switch onto the new stack.
        "mov rcx, rdx",  // ctx -> RCX: becomes the callee's first argument.
        "sub rsp, 32",   // MS x64 shadow space; see this function's doc comment.
        "call {cont}",
        "ud2",           // unreachable: continue_boot_on_new_stack is `-> !`.
        cont = sym continue_boot_on_new_stack,
    )
}

/// Everything that runs after the mainline-stack switch: a self-check that
/// the switch actually landed where expected, PIC/PIT bring-up, the
/// boot-complete debugcon proof line, enabling interrupts, and finally
/// handing off to the shell (which never returns). Reached only via
/// [`switch_stack_and_continue`]'s `call`, and only ever once.
extern "C" fn continue_boot_on_new_stack(ctx: *mut BootContinuation) -> ! {
    // Self-verify the switch landed where expected, immediately, before
    // doing anything else -- Build Order Step 4 is explicitly flagged (see
    // the Phase 2 plan) as the step most likely to introduce a subtle
    // regression, so this is checked, not assumed.
    let rsp_now: u64;
    unsafe { asm!("mov {}, rsp", out(reg) rsp_now, options(nomem, nostack, preserves_flags)) };
    debugcon::write_str("\n[stack] running on the dedicated mainline stack, rsp=0x");
    let _ = debugcon::write_fmt(format_args!("{rsp_now:x}\n"));

    // The first point at which nothing of firmware's is live any more: the
    // CPU translates through our page tables, faults through our IDT,
    // segments through our GDT, and runs on our stack. Only now may the
    // boot-services memory those used to live in be handed out -- see
    // `pmm.rs`'s module doc comment for what overwriting it any earlier
    // could do.
    let released = pmm::release_boot_services_memory();
    let _ = debugcon::write_fmt(format_args!(
        "[pmm] released {released} boot-services frames (firmware's tables, stack and IDT are no longer live)\n"
    ));

    // SAFETY: `ctx` was produced by `Box::into_raw` in `efi_main`,
    // immediately before the switch that reached this function, and its
    // ownership was never used again on that side -- this is the single
    // point that reclaims it.
    let boxed = unsafe { Box::from_raw(ctx) };
    let BootContinuation { mut console, apps, rsdp_addr } = *boxed;

    // 8259 PIC remap + mask, and PIT programming, both with interrupts
    // still disabled -- there is no race with the handlers they configure
    // until the `sti` below. Unaffected by the earlier `CR3` switch or this
    // stack switch: neither touches memory outside the 8259/8253 I/O ports
    // and this kernel's own static data, both identity-mapped RAM under
    // the new tables exactly as they were under the old ones.
    unsafe {
        pic::init();
        pit::init();
    }

    console.write_str("Interrupts, timer, and keyboard online.\n");
    debugcon::write_str("\n[boot] interrupts enabled\n");

    // First point in the entire program that interrupts are enabled. This
    // must happen here -- immediately after `pit::init()`, before AHCI
    // bring-up below -- and not any later.
    //
    // `pit::init()` only programs the 8253/8254's divisor; the hardware
    // counter starts free-running immediately, generating a steady stream
    // of IRQ0 pulses regardless of whether the CPU is currently servicing
    // interrupts. Until `sti` runs, every one of those pulses sits pending
    // and unserviced, so `clock::on_tick()` never runs and `clock::ticks()`
    // never advances off zero. `ahci.rs`'s every bounded-timeout wait loop
    // (HBA reset, port stop/start, device detection) polls exactly
    // `clock::ticks()` against a deadline -- called before this point, a
    // genuinely-absent device (or a wedged controller) would never see its
    // deadline reached and would spin forever instead of producing a
    // reported error, defeating the entire "bounded timeout, no silent
    // hang" design this driver documents as load-bearing. Caught by
    // explicitly testing the "AHCI controller present, no device attached
    // to any port" scenario during Build Order Step 4 verification -- see
    // CHANGES.md.
    unsafe { asm!("sti", options(nomem, nostack, preserves_flags)) };

    // Phase 3, Build Order Step 1: read the firmware's own ACPI tables to
    // learn this machine's real interrupt topology (where the Local APIC
    // and I/O APIC live, and how the legacy ISA IRQ numbers this kernel
    // already speaks map onto Global System Interrupts). Log-only at this
    // step -- nothing routes through the APIC yet, the 8259 PIC brought up
    // above is still the live interrupt controller -- so a parse failure
    // here cannot affect anything that already works. That is the whole
    // point of landing it as its own step: the riskiest part of this
    // phase is *acting* on this data, and acting on it is deliberately
    // not bundled into the same step as first reading it.
    //
    // "No usable ACPI tables" is a first-class, expected outcome (some
    // firmware genuinely publishes none), reported and carried on from,
    // exactly like `ahci::init`'s "no persistent storage this boot" path.
    let acpi_info = match unsafe { acpi::parse(rsdp_addr) } {
        Ok(info) => {
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!(
                    "ACPI: {} CPU(s), {} I/O APIC(s), LAPIC at 0x{:x}.\n",
                    info.cpu_count,
                    info.io_apics.len(),
                    info.lapic_address,
                ),
            );
            Some(info)
        }
        Err(msg) => {
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!("ACPI: {msg} (staying on the legacy 8259 PIC)\n"),
            );
            debugcon::write_str("[acpi] ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
            None
        }
    };
    // Phase 3, Build Order Steps 2-4: bring up the Local APIC and I/O
    // APIC and move this kernel's live interrupt delivery onto them.
    // Skipped entirely (staying on the 8259, which is working right now)
    // when ACPI gave us nothing to work from.
    if let Some(info) = acpi_info.as_ref() {
        bring_up_apic(&mut console, info);
    }

    // Phase 2, Part B, Build Order Step 2: enumerate every PCI function on
    // bus 0 and log what was found -- no AHCI driver code yet, just proof
    // the config-space primitives and enumeration walk actually work
    // against real (emulated) hardware. Pure port I/O, needs neither
    // interrupts nor any particular page mapping, so its exact placement
    // in the boot sequence relative to `sti` is not load-bearing on its
    // own -- it runs after `sti` here only because it feeds straight into
    // `ahci::init()`, which is.
    let pci_devices = pci::enumerate_bus0();
    pci::dump_devices(&pci_devices);

    // Phase 2, Part B, Build Order Steps 3-8: find the AHCI controller,
    // bring it up, and mount an existing save store on it -- read-only;
    // boot never formats or otherwise writes to the disk. Every failure
    // along this chain ("no AHCI controller present," "no usable disk on
    // any port," "disk doesn't support what this driver needs," "disk is
    // not formatted for WISP") is a first-class, expected outcome, not a
    // reason to stop booting -- `bring_up_save_disk` logs whichever one
    // happens, and `save`/`load` report "unavailable."
    //
    // On success the returned `Hba` is handed to the shell rather than
    // dropped: unlike `identify`/the boot-time probe (every command
    // issued through it up to this point has already completed before
    // `bring_up_save_disk` returns), `save`/`load` issue commands *later*,
    // from inside the shell's command loop -- something must keep owning
    // the live command engine for that to be safe, and the shell is it.
    let save_disk = bring_up_save_disk(&mut console);

    // Phase 3, Build Order Steps 5-6: find a USB keyboard. Deliberately
    // last in the boot sequence, after everything else is already
    // working: a machine with no xHCI controller, no USB device, or a
    // USB device that isn't a keyboard hits one of `xhci::init`'s
    // first-class "not available" paths every single boot, and none of
    // them may disturb anything above.
    //
    // On a machine that still has a PS/2 controller (every one QEMU
    // emulates by default, and most desktops) this is purely additive:
    // both keyboards feed the same queue and either one works. On a
    // machine that does not -- the actual reason this phase exists --
    // it is the only way a keystroke reaches this kernel at all.
    match unsafe { xhci::init() } {
        Ok(hc) => {
            if hc.has_keyboard() {
                usb_hid::install(hc);
                console.write_str("USB HID keyboard ready.\n");
            } else {
                console.write_str("USB: controller up, but no HID boot keyboard attached.\n");
            }
        }
        Err(msg) => {
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!("USB: {msg} (PS/2 keyboard only).\n"),
            );
            debugcon::write_str("[xhci] ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
        }
    }

    let mut sh = shell::Shell::new(apps, save_disk);
    sh.run(&mut console);
}

/// Bring up the APIC pair and switch this kernel's live interrupt
/// delivery onto it -- Phase 3, Build Order Steps 2-4.
///
/// Every failure along the way is a first-class, expected outcome that
/// leaves the machine on the legacy 8259 path it is already running on,
/// exactly like `bring_up_save_disk`'s "persistent storage unavailable
/// this boot." That is not a courtesy: the 8259 is *working* at the
/// moment this function is called, so anything that goes wrong here must
/// leave it working rather than strand the kernel between two interrupt
/// controllers with neither delivering -- a state whose only symptom is
/// a shell that never responds to a keystroke again.
///
/// The order below is load-bearing at three separate points, each
/// flagged at its own step.
fn bring_up_apic(console: &mut Console, info: &acpi::AcpiInfo) {
    // Step 2: enable the Local APIC. Nothing is routed to it yet, so
    // this cannot affect anything currently working -- the 8259 is
    // still the live controller and `lapic::init` leaves every LVT
    // entry masked.
    if let Err(msg) = unsafe { lapic::init(info.lapic_address, info.pcat_compat) } {
        report_apic_fallback(console, msg);
        return;
    }

    // Step 3a: calibrate the Local APIC timer *now*, while the PIT is
    // still ticking through the 8259. This has to happen before the
    // switch below, not after: the calibration measures the LAPIC's free
    // counter against `clock::ticks()`, and `clock::ticks()` only advances
    // while something is actually delivering IRQ0. A calibration
    // attempted after the 8259 is masked but before the LAPIC timer is
    // armed would spin forever against a frozen tick count -- the exact
    // shape of the sti-ordering bug this project already found once in
    // the AHCI driver (see CHANGES.md).
    //
    // A failed calibration is survivable and explicitly handled: the PIT
    // simply stays this kernel's clock, delivered through the I/O APIC.
    let counts_per_ms = match unsafe { lapic::calibrate_timer() } {
        Ok(counts) => Some(counts),
        Err(msg) => {
            debugcon::write_str("[lapic] ");
            debugcon::write_str(msg);
            debugcon::write_str(" -- keeping the PIT as this kernel's clock\n");
            None
        }
    };

    // Step 3b: map and quiesce every I/O APIC, then stage the two lines
    // this kernel actually needs -- fully programmed but still *masked*.
    // An unmasked route here would be live while `lapic::is_active()` is
    // still false, so an interrupt arriving before Step 4 would be
    // acknowledged at the 8259 and stay in service at the LAPIC forever,
    // blocking every later interrupt at its priority. A failure staging
    // either line re-masks whatever was staged before falling back, so
    // the 8259 path is left exactly as it was.
    if let Err(msg) = unsafe { ioapic::init(info) } {
        report_apic_fallback(console, msg);
        return;
    }
    let keyboard_route = match unsafe { ioapic::stage_isa_irq(info, 1, 0x21) } {
        Ok(route) => route,
        Err(msg) => {
            report_apic_fallback(console, msg);
            return;
        }
    };
    let timer_route = match unsafe { ioapic::stage_isa_irq(info, 0, 0x20) } {
        Ok(route) => route,
        Err(msg) => {
            unsafe { keyboard_route.mask() };
            report_apic_fallback(console, msg);
            return;
        }
    };

    // Step 4: the switch itself, as one interrupts-disabled commit:
    // silence the 8259, move EOI ownership to the Local APIC, and only
    // then unmask the staged I/O APIC routes. With interrupts off
    // throughout, no handler runs while the two controllers disagree,
    // and no I/O APIC interrupt can exist before the LAPIC owns its EOI.
    //
    // The keyboard line is edge-triggered: a key pressed during the
    // switch raised its edge while both paths were masked, and the 8042
    // will not raise another until that byte is read. Draining it here
    // re-arms the edge instead of leaving the keyboard silent.
    sync::without_interrupts(|| unsafe {
        lapic::enter_apic_mode(info.pcat_compat);
        keyboard_route.activate();
        timer_route.activate();
        keyboard::drain_pending();
    });

    // Step 5: hand the tick over to the Local APIC timer, if it
    // calibrated. Arming it *before* masking the PIT's line -- rather
    // than the other way round -- is deliberate: `lapic::start_timer`
    // can still fail (a computed period that doesn't fit the 32-bit
    // count register), and masking first would mean that failure left
    // the kernel with no clock at all. Every bounded-timeout loop in
    // `ahci.rs` polls `clock::ticks()` against a deadline, so "no clock"
    // does not degrade gracefully -- it hangs. The cost of this ordering
    // is a few microseconds in which both timers are live and a handful
    // of ticks are double-counted; the cost of the other ordering is a
    // dead machine.
    if let Some(counts) = counts_per_ms {
        match unsafe { lapic::start_timer(0x20, clock::TICK_HZ, counts) } {
            Ok(()) => {
                if let Err(msg) = unsafe { ioapic::mask_isa_irq(info, 0) } {
                    // The LAPIC timer is live and the PIT's line is also
                    // still live: both now increment the same tick
                    // counter, so the clock runs fast. Report it loudly
                    // -- it is a real, if minor, correctness problem
                    // (every `sleep_ms` is short) and exactly the kind
                    // of thing that must never be swept under the rug.
                    console.set_fg(gfx::ERROR);
                    let _ = core::fmt::Write::write_fmt(
                        console,
                        format_args!("APIC: could not mask the now-redundant PIT line: {msg}\n"),
                    );
                    console.set_fg(gfx::FOREGROUND);
                    debugcon::write_str("[lapic] WARNING: PIT line still live alongside the LAPIC timer: ");
                    debugcon::write_str(msg);
                    debugcon::write_str("\n");
                }
            }
            Err(msg) => {
                debugcon::write_str("[lapic] ");
                debugcon::write_str(msg);
                debugcon::write_str(" -- keeping the PIT as this kernel's clock\n");
            }
        }
    }

    let clock = if lapic::timer_active() { "LAPIC timer" } else { "PIT via I/O APIC" };
    let _ = core::fmt::Write::write_fmt(
        console,
        format_args!("APIC: interrupts now routed through the I/O APIC; clock = {clock}.\n"),
    );
}

/// Report that APIC bring-up stopped at some step and this boot is
/// staying on the legacy 8259 -- which is still fully live and working,
/// since every step in `bring_up_apic` that can fail does so before
/// anything about the 8259's configuration has been touched.
fn report_apic_fallback(console: &mut Console, msg: &'static str) {
    let _ = core::fmt::Write::write_fmt(
        console,
        format_args!("APIC: {msg} (staying on the legacy 8259 PIC)\n"),
    );
    debugcon::write_str("[apic] bring-up stopped: ");
    debugcon::write_str(msg);
    debugcon::write_str(" -- legacy 8259 remains the live interrupt controller\n");
}

/// Find the AHCI controller (if any), reset and enable it, map its ABAR,
/// bring up the first port with a usable SATA disk attached, IDENTIFY it,
/// and *probe* -- read-only -- whether it already carries a WISP save
/// store. Every step's own doc comment explains what's a first-class
/// expected failure versus a real bug; this function's whole job is
/// turning "did every step succeed" into a single `Option`, logging
/// exactly where the chain stopped either way.
///
/// Boot never writes to the disk. A blank or foreign disk is reported and
/// left untouched; the shell's `format` command is the only way to
/// provision one, and it asks first. (Earlier versions formatted any
/// unrecognized disk automatically and ran a write/read-back/restore
/// self-test against its last sector -- both destroyed data that did not
/// belong to WISP.)
///
/// Returns `None` if no usable disk was found -- callers must treat that
/// as "persistent storage is unavailable this boot," never as a reason to
/// stop booting. `Some` with an unmounted disk means a disk exists but is
/// not (yet) formatted for WISP.
fn bring_up_save_disk(console: &mut Console) -> Option<save_format::SaveDisk> {
    let mut hba = match unsafe { ahci::init() } {
        Ok(hba) => {
            console.write_str("AHCI controller found and ABAR mapped.\n");
            hba
        }
        Err(msg) => {
            let _ = core::fmt::Write::write_fmt(console, format_args!("AHCI: {msg} (persistent storage unavailable)\n"));
            debugcon::write_str("[ahci] ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
            return None;
        }
    };

    let disk = match unsafe { hba.identify() } {
        Ok(disk) => {
            let _ = core::fmt::Write::write_fmt(
                console,
                format_args!(
                    "AHCI disk identified: '{}' on port {}, {} sectors ({} MiB).\n",
                    disk.model(),
                    hba.port(),
                    disk.total_sectors,
                    (disk.total_sectors * u64::from(ahci::SECTOR_SIZE)) / (1024 * 1024),
                ),
            );
            disk
        }
        Err(msg) => {
            let _ = core::fmt::Write::write_fmt(console, format_args!("AHCI IDENTIFY failed: {msg} (persistent storage unavailable)\n"));
            debugcon::write_str("[ahci] identify failed: ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
            return None;
        }
    };

    // Read-only proof that the read path works: LBA 0, dumped byte for
    // byte to debugcon. `probe` below reads the same sector again; the
    // dump is diagnostics only.
    let mut sector0 = [0u8; ahci::SECTOR_SIZE as usize];
    match unsafe { hba.read_sector(0, &mut sector0) } {
        Ok(()) => ahci::debug_dump_sector(&sector0),
        Err(msg) => {
            debugcon::write_str("[ahci] read_sector(0) failed: ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
        }
    }

    let probe = unsafe { save_format::probe(&hba, disk.total_sectors) };
    let mounted = matches!(probe, Ok(save_format::Probe::Formatted));
    let (line, log): (&str, &str) = match probe {
        Ok(save_format::Probe::Formatted) => (
            "AHCI save disk mounted (save/load available).",
            "[save_format] existing save store mounted (read-only probe, no writes)",
        ),
        Ok(save_format::Probe::Blank) => (
            "AHCI disk is blank -- not formatted for WISP. Left untouched; run 'format' to use it for save/load.",
            "[save_format] disk is blank; left untouched (not mounted)",
        ),
        Ok(save_format::Probe::Foreign) => (
            "AHCI disk holds non-WISP data. Left untouched; save/load unavailable ('format' would ERASE it).",
            "[save_format] disk holds foreign data; left untouched (not mounted)",
        ),
        Ok(save_format::Probe::Unsupported) => (
            "AHCI disk has a WISP save store this version does not understand. Left untouched.",
            "[save_format] unsupported save-store version/geometry; left untouched (not mounted)",
        ),
        Err(msg) => {
            console.set_fg(gfx::ERROR);
            let _ = core::fmt::Write::write_fmt(console, format_args!("AHCI save-disk probe FAILED: {msg} (save/load unavailable)\n"));
            console.set_fg(gfx::FOREGROUND);
            debugcon::write_str("[save_format] probe FAILED: ");
            debugcon::write_str(msg);
            debugcon::write_str("\n");
            return Some(save_format::SaveDisk::new(hba, disk.total_sectors, false));
        }
    };
    if !mounted {
        console.set_fg(gfx::ERROR);
    }
    console.write_str(line);
    console.write_str("\n");
    console.set_fg(gfx::FOREGROUND);
    debugcon::write_str(log);
    debugcon::write_str("\n");
    Some(save_format::SaveDisk::new(hba, disk.total_sectors, mounted))
}

/// The running image's base address and size, from the firmware's
/// LoadedImage protocol on this image's own handle. Opened in its own scope
/// and closed again immediately, after `fs::snapshot_apps` has finished
/// with the same protocol, so the two exclusive opens never overlap.
fn image_location(bs: &BootServices, image: Handle) -> Option<(u64, u64)> {
    let loaded = bs
        .open_protocol_exclusive::<uefi::proto::loaded_image::LoadedImage>(image)
        .ok()?;
    let (base, size) = loaded.info();
    if base.is_null() || size == 0 {
        return None;
    }
    Some((base as u64, size))
}

/// Locate the Graphics Output Protocol, open it exclusively, and build a
/// [`Console`] over its framebuffer.
fn init_console(bs: &BootServices) -> Result<Console, Status> {
    let gop_handle = bs
        .get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|e| e.status())?;
    let mut gop = bs
        .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        .map_err(|e| e.status())?;

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let format = mode_info.pixel_format();

    let mut fb = gop.frame_buffer();
    let fb_ptr = fb.as_mut_ptr();
    let fb_len = fb.size();
    // Drop the `FrameBuffer` borrow of `gop` here; we keep the underlying
    // MMIO mapping alive for the rest of the program by simply never
    // dropping `gop` (it's `mem::forget`'ten below), since the mapping's
    // validity does not depend on the Rust wrapper staying in scope, on
    // either side of `exit_boot_services` (and `paging.rs` identity-maps it
    // in the kernel's own tables, so the address never changes).
    drop(fb);
    core::mem::forget(gop);

    // SAFETY: `fb_ptr`/`fb_len` come directly from the just-queried GOP
    // mode, and the mapping remains valid for the life of the program.
    unsafe { Console::new(fb_ptr, fb_len, width, height, stride, format) }
        .map_err(|_| Status::UNSUPPORTED)
}
