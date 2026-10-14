//! Local APIC driver -- built from scratch against Intel SDM Vol 3,
//! Chapter 11 ("Advanced Programmable Interrupt Controller"), replacing
//! the legacy 8259 PIC (`pic.rs`) as this kernel's live interrupt
//! controller.
//!
//! # Why replace a working 8259
//!
//! `pic.rs` works perfectly under QEMU and is the right answer for a
//! single-core kernel that only ever needs a timer and a PS/2 keyboard.
//! It is the wrong answer for real hardware, for one specific reason:
//! there is no way to receive a PCI device's interrupts through it
//! without the firmware's own legacy IRQ routing, and on a machine
//! without a PS/2 controller (most laptops built in the last fifteen
//! years) the keyboard *is* a PCI device's problem -- it hangs off an
//! xHCI host controller. Moving to the APIC is what makes an interrupt
//! from anything other than the two legacy ISA lines reachable at all.
//!
//! # What this module owns, and what it doesn't
//!
//! This is the *local* half of the APIC: the per-CPU unit that actually
//! delivers an interrupt to this core, acknowledges it (EOI), and
//! provides a per-CPU timer. Getting an external device's pin *to* the
//! Local APIC is the I/O APIC's job -- see `ioapic.rs`. The two are
//! separate hardware blocks with separate register windows and are kept
//! as separate modules here for the same reason.
//!
//! # Scope, deliberately reduced
//!
//! - **xAPIC (MMIO) only, not x2APIC (MSR).** Every machine that
//!   supports x2APIC also supports xAPIC, and xAPIC's 32-bit MMIO
//!   register window is the mode firmware leaves the BSP in. Detecting
//!   and switching to x2APIC would buy this kernel nothing (its benefits
//!   are larger APIC ID space and faster IPIs -- both meaningless on one
//!   core with no IPIs).
//! - **No IPIs, no AP startup.** The Interrupt Command Register is never
//!   written. This kernel is single-core by design; `acpi.rs` counts the
//!   other CPUs purely so the boot log is honest about them.
//! - **No LAPIC NMI wiring from MADT type-4 entries.** LINT0/LINT1 are
//!   masked outright rather than configured, which is correct for a
//!   kernel with no NMI watchdog and no profiling: an unconfigured LVT
//!   entry left at its power-on value could otherwise deliver a
//!   vector this kernel never asked for.

use core::arch::asm;
use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU64, Ordering};

use crate::{clock, debugcon, paging};

/// `IA32_APIC_BASE` (Intel SDM Vol 3, 11.4.4): bits 51:12 hold the
/// LAPIC's physical base address, bit 11 is the global enable, bit 8
/// marks the bootstrap processor.
const IA32_APIC_BASE_MSR: u32 = 0x1B;
const APIC_BASE_BSP: u64 = 1 << 8;
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;
const APIC_BASE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

// --- Local APIC register offsets (Intel SDM Vol 3, Table 11-1) ---------
const REG_ID: usize = 0x020;
const REG_VERSION: usize = 0x030;
/// Task Priority Register: the CPU refuses interrupts whose priority
/// class is at or below this. Must be zeroed, or every device interrupt
/// this kernel routes is silently blocked by whatever value firmware
/// happened to leave here.
const REG_TPR: usize = 0x080;
/// End Of Interrupt: written (with 0) exactly once per serviced
/// interrupt, the LAPIC's equivalent of `pic::end_of_interrupt`.
const REG_EOI: usize = 0x0B0;
/// Spurious Interrupt Vector Register: bit 8 is the APIC software
/// enable; bits 7:0 are the vector delivered for a spurious interrupt.
const REG_SVR: usize = 0x0F0;
const REG_LVT_TIMER: usize = 0x320;
const REG_LVT_THERMAL: usize = 0x330;
const REG_LVT_PERF: usize = 0x340;
const REG_LVT_LINT0: usize = 0x350;
const REG_LVT_LINT1: usize = 0x360;
const REG_LVT_ERROR: usize = 0x370;
const REG_TIMER_INITIAL_COUNT: usize = 0x380;
const REG_TIMER_CURRENT_COUNT: usize = 0x390;
const REG_TIMER_DIVIDE_CONFIG: usize = 0x3E0;

/// LVT bit 16: mask. Set on every LVT entry this kernel does not
/// actively use.
const LVT_MASKED: u32 = 1 << 16;
/// LVT Timer bit 17: periodic mode (vs. the power-on default, one-shot).
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// LVT delivery mode field (bits 10:8) value 0b111: ExtINT -- "an
/// external interrupt controller (the 8259) supplies the vector; take it
/// from the bus." This is the one delivery mode that has nothing to do
/// with the vector field in the same register, and the only way a legacy
/// PIC interrupt can reach a CPU whose Local APIC is enabled. See
/// [`init`]'s "The LINT0 trap" section for why this kernel must program
/// it rather than mask LINT0.
const LVT_DELIVERY_EXTINT: u32 = 0b111 << 8;
/// SVR bit 8: APIC software enable. With this clear the LAPIC accepts
/// nothing, regardless of `IA32_APIC_BASE`'s global enable.
const SVR_APIC_ENABLE: u32 = 1 << 8;

/// Vector delivered when the LAPIC has to report a spurious interrupt.
/// Deliberately 0xFF: the SVR's low 4 bits are hardwired to 1 on some
/// older (P6-era) implementations, so a vector whose low nibble is 0xF
/// is the one choice guaranteed to read back as written everywhere.
/// `idt.rs` recognizes it and, correctly, does *not* EOI it -- a
/// spurious interrupt was never actually accepted by the LAPIC, so
/// acknowledging one would retire a real, still-in-service interrupt
/// instead.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// Divide Configuration Register value for "divide by 16" (Intel SDM
/// Vol 3, Figure 11-10: the divisor is encoded across bits 3, 1 and 0,
/// with bit 2 reserved -- which is why this is `0b0011` and not `0b0100`
/// for a value that reads like "4"). Divide-by-16 keeps the 32-bit
/// count register from wrapping during the calibration window below even
/// on a fast bus clock, while still leaving far more resolution than a
/// 100 Hz tick needs.
const TIMER_DIVIDE_BY_16: u32 = 0b0011;

/// One page is mapped for the LAPIC register window: the architectural
/// register set is 4 KiB (Intel SDM Vol 3, Table 11-1's offsets top out
/// at 0x3E0), and the base is guaranteed 4-KiB aligned.
const LAPIC_MAP_SIZE: u64 = paging::PAGE_SIZE;

/// Physical (== virtual, this kernel identity-maps everything) base of
/// the LAPIC register window, or 0 before [`init`] has run. A plain
/// atomic rather than a `Locked<Option<..>>` for exactly the reason
/// `panic.rs`'s framebuffer statics are: [`eoi`] is called from inside
/// an interrupt handler, where taking any lock risks deadlocking against
/// whatever the interrupted code was holding.
static LAPIC_BASE: AtomicU64 = AtomicU64::new(0);

/// True once the LAPIC is enabled and this kernel's interrupts are being
/// acknowledged here rather than at the 8259. Read by `idt.rs`'s IRQ
/// dispatch to decide where an EOI goes -- there is exactly one correct
/// answer at any moment, and sending it to the wrong controller wedges
/// interrupt delivery silently.
static APIC_MODE: AtomicBool = AtomicBool::new(false);

/// True once [`start_timer`] has taken over the tick; `main.rs` uses it
/// to decide whether the PIT's IRQ still needs routing at all.
static TIMER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// # Safety
/// `offset` must be a valid LAPIC register offset and [`init`] must have
/// mapped the window.
unsafe fn read(offset: usize) -> u32 {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    debug_assert!(base != 0, "lapic::read before lapic::init");
    // Volatile: a real device register, not memory -- the compiler must
    // not cache, reorder, or elide this access. Same reasoning as
    // `ahci.rs`'s `read_reg32`.
    unsafe { core::ptr::read_volatile((base as *const u8).add(offset) as *const u32) }
}

/// # Safety
/// Same contract as [`read`].
unsafe fn write(offset: usize, value: u32) {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    debug_assert!(base != 0, "lapic::write before lapic::init");
    unsafe { core::ptr::write_volatile((base as *mut u8).add(offset) as *mut u32, value) }
}

/// # Safety
/// `msr` must be a model-specific register this CPU implements.
unsafe fn rdmsr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}

/// # Safety
/// `msr` must be writable on this CPU and `value` must be a legal value
/// for it -- a reserved-bit violation raises `#GP`.
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem, nostack, preserves_flags));
    }
}

/// Is the Local APIC live and acknowledging this kernel's interrupts?
/// `idt.rs`'s IRQ dispatch consults this to send each EOI to the right
/// controller.
pub fn is_active() -> bool {
    APIC_MODE.load(Ordering::SeqCst)
}

/// Is the LAPIC timer the live tick source (rather than the PIT)?
pub fn timer_active() -> bool {
    TIMER_ACTIVE.load(Ordering::SeqCst)
}

/// Signal end-of-interrupt to the Local APIC. Exactly one call per
/// serviced interrupt, after handling it -- an unacknowledged interrupt
/// leaves its priority level in service forever, blocking every
/// equal-or-lower-priority interrupt from then on (the LAPIC's analogue
/// of `pic::end_of_interrupt`'s "wedged line," and just as silent).
///
/// Writing any value works; 0 is written because the SDM requires the
/// value to be 0 for future compatibility.
pub fn eoi() {
    // SAFETY: only ever reached from `idt.rs`'s IRQ dispatch, which
    // checks `is_active()` (set only by `init`, after the window is
    // mapped) before calling this.
    unsafe { write(REG_EOI, 0) };
}

/// This CPU's Local APIC ID -- the destination field `ioapic.rs` writes
/// into a redirection table entry so a routed interrupt actually arrives
/// *here*.
pub fn id() -> u8 {
    // SAFETY: callers are all post-`init`; the ID register is read-only.
    ((unsafe { read(REG_ID) }) >> 24) as u8
}

/// Map the Local APIC's register window, enable it, and put every LVT
/// entry into a known state.
///
/// `lapic_phys` must be the address `acpi::parse` reported (this kernel
/// never assumes the architectural default 0xFEE00000 -- a Local APIC
/// Address Override entry can legally move it, and reading a hardcoded
/// address on a machine that did would fault with no useful diagnostic).
///
/// After this returns, the LAPIC is enabled and will accept interrupts
/// delivered to it, but *nothing is routed to it yet* -- that is
/// `ioapic.rs`'s job, deliberately a separate step. The legacy 8259 is
/// still the live source of the timer and PS/2 keyboard IRQs at this
/// point and must stay that way until the caller explicitly switches
/// over via [`enter_apic_mode`].
///
/// # The LINT0 trap
///
/// Enabling the Local APIC changes, immediately and invisibly, how the
/// legacy 8259's interrupts reach this CPU. While the LAPIC is software-
/// disabled the CPU takes the PIC's `INTR` assertion directly; the
/// moment it is enabled, that same assertion has to arrive through the
/// LAPIC's LINT0 pin, programmed in ExtINT delivery mode -- which is
/// exactly how firmware leaves it, and exactly what a well-meaning
/// "mask every LVT entry we don't use" sweep destroys.
///
/// The first version of this function did precisely that, and the result
/// was a boot that stopped dead midway through APIC bring-up with no
/// fault, no error, and nothing further in the log: masking LINT0 cut
/// off the PIT's IRQ0, [`calibrate_timer`] then spun forever waiting for
/// a `clock::ticks()` value that could no longer advance, and the shell
/// was never reached. A hang, not a crash -- so the fault screen this
/// project built for exactly this kind of moment had nothing to report.
/// See `CHANGES.md` for the full account.
///
/// So LINT0 is programmed as ExtINT and left *unmasked* here, on any
/// machine that reports having an 8259 at all, and is masked only later
/// -- in [`enter_apic_mode`], at the same moment the 8259 itself is
/// masked and its pass-through stops being wanted. On a machine with no
/// 8259 (`pcat_compat` clear) there is nothing to pass through and LINT0
/// is masked from the start.
///
/// # Safety
/// Must be called after `paging::load` (this maps a page), exactly once,
/// and with `lapic_phys` from a validated MADT.
pub unsafe fn init(lapic_phys: u64, pcat_compat: bool) -> Result<(), &'static str> {
    if lapic_phys % paging::PAGE_SIZE != 0 {
        return Err("lapic: MADT reports a non-page-aligned Local APIC base");
    }

    // SAFETY: a real MMIO window from the firmware's own tables; MMIO
    // (cache-disabled) is exactly what `PageFlags::Mmio` is for, and
    // identity mapping matches this kernel's whole-image policy.
    unsafe { paging::map(lapic_phys, lapic_phys, LAPIC_MAP_SIZE, paging::PageFlags::Mmio)? };
    LAPIC_BASE.store(lapic_phys, Ordering::SeqCst);

    // Set (never assume) the MSR's global enable, and re-point its base
    // field at what ACPI reported. Firmware normally leaves the BSP's
    // LAPIC already globally enabled at the architectural default, but
    // "normally" is not "always" -- and a disabled LAPIC here would mean
    // every register access below silently reads back garbage rather
    // than failing in any visible way.
    //
    // Deliberately preserves the existing BSP bit rather than setting
    // it: bit 8 is read-only-ish (writing it on an AP is a `#GP`), so
    // the correct move is to carry forward whatever the CPU reports.
    let base_msr = unsafe { rdmsr(IA32_APIC_BASE_MSR) };
    let new_msr = (base_msr & !APIC_BASE_ADDR_MASK)
        | (lapic_phys & APIC_BASE_ADDR_MASK)
        | APIC_BASE_GLOBAL_ENABLE;
    unsafe { wrmsr(IA32_APIC_BASE_MSR, new_msr) };
    compiler_fence(Ordering::SeqCst);

    unsafe {
        // Accept every priority class. Firmware has no obligation to
        // leave TPR at 0, and a nonzero value here blocks interrupts in
        // a way that looks exactly like "the I/O APIC routing is wrong"
        // from every other vantage point -- so it is zeroed explicitly
        // rather than assumed.
        write(REG_TPR, 0);

        // Mask every LVT entry this kernel does not use, rather than
        // leaving them at whatever firmware left behind -- an entry left
        // live from firmware's own use would deliver a vector this
        // kernel never configured.
        //
        // LINT0 is the deliberate exception: see this function's "The
        // LINT0 trap" section. On a PCAT-compatible machine it carries
        // the 8259's interrupts, which are still this kernel's only
        // source of a timer tick and a keystroke at this point in boot,
        // so it is programmed as ExtINT and left live until
        // `enter_apic_mode` retires the 8259 for good.
        //
        // LINT1 is masked rather than wired as an NMI: this kernel
        // deliberately does not parse the MADT's type-4 Local APIC NMI
        // entries (see `acpi.rs`'s scope notes), and guessing which pin
        // carries NMI -- rather than reading it from the firmware's own
        // tables -- is exactly the kind of assumption this phase exists
        // to remove.
        write(REG_LVT_TIMER, LVT_MASKED);
        write(REG_LVT_THERMAL, LVT_MASKED);
        write(REG_LVT_PERF, LVT_MASKED);
        write(
            REG_LVT_LINT0,
            if pcat_compat { LVT_DELIVERY_EXTINT } else { LVT_MASKED },
        );
        write(REG_LVT_LINT1, LVT_MASKED);
        write(REG_LVT_ERROR, LVT_MASKED);

        // Software-enable the APIC and install the spurious vector.
        // This is the last step, so nothing can be delivered through a
        // half-configured LVT.
        write(REG_SVR, SVR_APIC_ENABLE | u32::from(SPURIOUS_VECTOR));
    }
    compiler_fence(Ordering::SeqCst);

    let (apic_id, version) = unsafe { (read(REG_ID) >> 24, read(REG_VERSION)) };
    let max_lvt = ((version >> 16) & 0xFF) + 1;
    let _ = debugcon::write_fmt(format_args!(
        "[lapic] base=0x{lapic_phys:x} id={apic_id} version=0x{:02x} max_lvt={max_lvt} bsp={} lint0={}\n",
        version & 0xFF,
        base_msr & APIC_BASE_BSP != 0,
        if pcat_compat { "ExtINT (8259 pass-through, still live)" } else { "masked (no 8259 on this machine)" },
    ));

    Ok(())
}

/// Switch this kernel from 8259-delivered interrupts to APIC-delivered
/// ones: mask both PICs completely, leave the legacy PIC-mode
/// pass-through path (the IMCR) if this machine has one, and mark the
/// LAPIC as the controller every subsequent EOI goes to.
///
/// Call this *after* `ioapic.rs` has programmed the redirection entries
/// for every line that must keep working, and never before -- between
/// masking the PIC and having a live I/O APIC route, nothing is
/// delivered at all, and on a kernel whose only input device is an
/// interrupt-driven keyboard that window is indistinguishable from a
/// hang.
///
/// # The IMCR
/// On machines that implement the MP specification's "PIC mode" (every
/// PC with a real 8259, which `pcat_compat` reports), the 8259's output
/// is wired to the BSP's LINT0 through an Interrupt Mode Configuration
/// Register, and the documented way to move to symmetric I/O mode is to
/// select IMCR (port 0x22 <- 0x70) and write 1 to it (port 0x23 <- 0x01)
/// -- MP spec 1.4, 3.6.2.1. Machines without an IMCR simply have nothing
/// decoding those ports, so the writes are harmlessly discarded; this is
/// exactly why the procedure is safe to perform unconditionally on any
/// PCAT-compatible machine and is skipped entirely on one that reports
/// it has no 8259 at all.
///
/// # Safety
/// Must be called after [`init`], with interrupts disabled, and after
/// every line this kernel needs has a live I/O APIC redirection entry.
pub unsafe fn enter_apic_mode(pcat_compat: bool) {
    // SAFETY: raw port I/O against the two 8259s; masking every line is
    // always safe, and is the point.
    unsafe { crate::pic::mask_all() };

    // Retire the 8259 pass-through path `init` deliberately left live
    // (see its "The LINT0 trap" section). This happens here, and only
    // here: LINT0 must stay in ExtINT mode for the entire window between
    // enabling the LAPIC and having real I/O APIC routes for everything
    // that matters, because for all of that window it is the only way a
    // timer tick or a keystroke reaches this CPU at all.
    unsafe { write(REG_LVT_LINT0, LVT_MASKED) };

    if pcat_compat {
        // SAFETY: see this function's "The IMCR" doc section.
        unsafe {
            asm!(
                "out dx, al",
                in("dx") 0x22u16,
                in("al") 0x70u8,
                options(nomem, nostack, preserves_flags),
            );
            asm!(
                "out dx, al",
                in("dx") 0x23u16,
                in("al") 0x01u8,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    compiler_fence(Ordering::SeqCst);
    APIC_MODE.store(true, Ordering::SeqCst);
    compiler_fence(Ordering::SeqCst);
    debugcon::write_str("[lapic] APIC mode active: 8259 fully masked, EOIs now go to the Local APIC\n");
}

/// Spin until `cond` holds, giving up after [`CALIBRATION_TSC_LIMIT`]
/// cycles of the CPU's time-stamp counter. Returns whether `cond` held.
///
/// The limit is deliberately enormous relative to what calibration
/// actually needs (~100 ms): 2^36 cycles is roughly 17 seconds even at
/// 4 GHz, and over 60 seconds on a 1 GHz part. It exists only to turn a
/// genuinely frozen reference clock into a reported error; any machine
/// whose PIT is alive satisfies `cond` long before it matters.
fn spin_until_tsc_bounded(mut cond: impl FnMut() -> bool) -> bool {
    // SAFETY: `rdtsc` is unprivileged and side-effect free.
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    loop {
        if cond() {
            return true;
        }
        let now = unsafe { core::arch::x86_64::_rdtsc() };
        if now.wrapping_sub(start) > CALIBRATION_TSC_LIMIT {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// See [`spin_until_tsc_bounded`].
const CALIBRATION_TSC_LIMIT: u64 = 1 << 36;

/// Measure how many LAPIC timer counts elapse per millisecond, by
/// running the timer's free counter against the PIT's already-live,
/// known-rate tick -- the one clock in this machine whose frequency is
/// an architectural constant rather than something that has to be
/// discovered (see `pit.rs`; the shared tick counter it feeds lives in
/// `clock.rs`).
///
/// Returns `Err` if the measurement is implausible, which is a real
/// possibility rather than a theoretical one: under some hypervisors the
/// LAPIC timer's count does not advance at all until it is armed in a
/// particular way, and a calibration that silently returned "0 counts
/// per millisecond" would arm a periodic timer with an initial count of
/// zero -- which the SDM defines as *stopping* the timer. That failure
/// mode looks exactly like a hang, so it is detected here and reported
/// instead, leaving the caller free to keep the PIT as the tick source.
///
/// # Safety
/// Must be called after [`init`], with interrupts enabled and the PIT
/// actively ticking (`clock::ticks()` must be advancing, or this never
/// returns a usable measurement).
pub unsafe fn calibrate_timer() -> Result<u32, &'static str> {
    /// How long to measure for. 100 ms is long enough that the PIT's own
    /// 10 ms granularity contributes at most 10% error before the
    /// division below, and short enough not to be noticeable in boot
    /// time.
    const CALIBRATION_MS: u64 = 100;
    const CALIBRATION_TICKS: u64 = CALIBRATION_MS / clock::MS_PER_TICK;

    unsafe {
        write(REG_TIMER_DIVIDE_CONFIG, TIMER_DIVIDE_BY_16);
        // Masked throughout: this measures the counter, it does not want
        // the interrupt. The counter runs regardless of the mask.
        write(REG_LVT_TIMER, LVT_MASKED);
    }

    // Start the measurement on a PIT tick boundary rather than partway
    // through one -- without this, up to a full 10 ms of the window is
    // unaccounted for, which is a 10% error on a 100 ms measurement.
    //
    // Both waits below are on the tick counter itself, so they cannot use
    // `clock::poll_until` (whose deadline *is* a tick count). They are
    // bounded instead by the CPU's own cycle counter: if the reference
    // clock is not advancing at all -- interrupts somehow off, or the
    // PIT not delivering -- calibration reports that as an error rather
    // than spinning forever, and the caller keeps whatever clock it has.
    if !clock::interrupts_enabled() {
        return Err("LAPIC timer calibration needs interrupts enabled (reference tick is frozen)");
    }
    let start_tick = clock::ticks();
    if !spin_until_tsc_bounded(|| clock::ticks() != start_tick) {
        return Err("LAPIC timer calibration: reference tick never advanced");
    }
    let base_tick = clock::ticks();

    unsafe { write(REG_TIMER_INITIAL_COUNT, u32::MAX) };

    if !spin_until_tsc_bounded(|| clock::ticks() >= base_tick + CALIBRATION_TICKS) {
        unsafe { write(REG_TIMER_INITIAL_COUNT, 0) };
        return Err("LAPIC timer calibration: reference tick stalled mid-measurement");
    }

    let remaining = unsafe { read(REG_TIMER_CURRENT_COUNT) };
    unsafe { write(REG_TIMER_INITIAL_COUNT, 0) }; // stop the counter.

    let elapsed = u32::MAX - remaining;
    let counts_per_ms = elapsed / (CALIBRATION_MS as u32);

    let _ = debugcon::write_fmt(format_args!(
        "[lapic] timer calibration: {elapsed} counts in {CALIBRATION_MS}ms (divide-by-16) = {counts_per_ms} counts/ms\n",
    ));

    // A plausible xAPIC timer runs off the bus/core crystal clock --
    // hundreds of thousands to a few million counts per second after the
    // divide-by-16, i.e. hundreds to thousands per millisecond. Anything
    // at or below a hundred means the counter effectively did not move
    // (see this function's doc comment); anything absurdly large means
    // the PIT was not actually ticking and the loop above exited
    // immediately.
    if counts_per_ms < 100 {
        return Err("lapic: timer calibration measured almost no counts -- the LAPIC timer does not appear to run here");
    }
    if elapsed == u32::MAX {
        return Err("lapic: timer calibration counter wrapped -- measurement is not trustworthy");
    }
    Ok(counts_per_ms)
}

/// Arm the Local APIC timer in periodic mode at `hz`, delivering
/// `vector`, using a `counts_per_ms` measurement from
/// [`calibrate_timer`].
///
/// # Safety
/// Must be called after [`init`] and a successful [`calibrate_timer`];
/// `vector` must have a live IDT gate (every vector does -- see
/// `idt.rs`) whose handler EOIs the LAPIC.
pub unsafe fn start_timer(vector: u8, hz: u32, counts_per_ms: u32) -> Result<(), &'static str> {
    let period_ms = 1000 / hz;
    let initial = counts_per_ms
        .checked_mul(period_ms)
        .ok_or("lapic: timer period overflows the 32-bit initial-count register")?;
    if initial == 0 {
        // The SDM defines an initial count of 0 as stopping the timer,
        // so arming one here would silently produce no ticks at all --
        // exactly the "looks like a hang" failure this whole path is
        // written to avoid.
        return Err("lapic: computed timer initial count is zero -- refusing to arm a stopped timer");
    }

    unsafe {
        write(REG_TIMER_DIVIDE_CONFIG, TIMER_DIVIDE_BY_16);
        // Vector and mode first, initial count last: writing the initial
        // count is what actually starts the counter, so this ordering
        // guarantees the first expiry can never be delivered through a
        // half-written LVT entry.
        write(REG_LVT_TIMER, u32::from(vector) | LVT_TIMER_PERIODIC);
        write(REG_TIMER_INITIAL_COUNT, initial);
    }
    compiler_fence(Ordering::SeqCst);
    TIMER_ACTIVE.store(true, Ordering::SeqCst);

    let _ = debugcon::write_fmt(format_args!(
        "[lapic] timer armed: vector=0x{vector:02x} {hz}Hz initial_count={initial}\n",
    ));
    Ok(())
}
