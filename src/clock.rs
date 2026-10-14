//! The kernel's single timebase: one monotonic tick counter, and every
//! time-related primitive built on it (`uptime_ms`, `sleep_ms`, bounded
//! hardware waits via [`poll_until`]).
//!
//! # Why this is its own module
//!
//! The tick counter used to live in `pit.rs`, with a doc comment calling
//! the PIT "the kernel's only clock." That stopped being true the moment
//! Phase 3 landed: on a machine with usable ACPI tables the Local APIC
//! timer drives vector 0x20 and the PIT's line is masked at the I/O APIC.
//! Both sources deliver the same vector at the same [`TICK_HZ`] rate, and
//! `idt.rs` routes that vector here regardless of which one raised it, so
//! *what keeps time* is a per-boot hardware decision while *what the time
//! is* is a single, source-agnostic counter. Keeping the counter inside
//! one of the two drivers that can feed it made every consumer
//! (`ahci.rs`, `xhci.rs`, `lapic.rs`'s calibration, the shell) depend on
//! the PIT by name even when the PIT was silent. `pit.rs` is now purely a
//! hardware driver -- it programs the 8253/8254 and nothing else.
//!
//! It also replaces two byte-for-byte duplicate `poll_until` helpers that
//! lived privately in `ahci.rs` and `xhci.rs`, so the "every hardware wait
//! is bounded" rule is enforced in exactly one place -- including the
//! interrupt-flag guard described on [`poll_until`], which turns this
//! project's historical sti-ordering hang (see CHANGES.md) from a silent
//! freeze into an immediate, reported panic.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// The tick rate every timer source is programmed for: `pit::init`
/// divides the PIT's oscillator down to this, and `main.rs` arms the
/// Local APIC timer at this same rate after calibrating it. 100 Hz
/// (10 ms/tick) is granular enough for interactive `sleep_ms()` calls in
/// WISP scripts and cheap enough that the timer ISR's overhead is nowhere
/// close to significant.
pub const TICK_HZ: u32 = 100;

/// Milliseconds represented by one tick.
pub const MS_PER_TICK: u64 = 1000 / TICK_HZ as u64;

/// Incremented exactly once per timer interrupt (see `idt.rs`'s dispatch of
/// vector 0x20 to [`on_tick`]), whichever hardware source raised it.
/// Wraps only after ~5.8 billion years at 100 Hz, so wraparound is not a
/// practical concern.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Called by the timer interrupt handler (vector 0x20) once per tick.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Ticks elapsed since interrupts were first enabled.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Milliseconds elapsed since interrupts were first enabled, derived from
/// the tick count (so it advances in [`MS_PER_TICK`] steps).
pub fn uptime_ms() -> u64 {
    ticks() * MS_PER_TICK
}

/// An absolute tick value `ticks` ticks from now -- the deadline shape
/// [`poll_until`] takes.
pub fn deadline_after_ticks(ticks_from_now: u64) -> u64 {
    ticks().saturating_add(ticks_from_now)
}

/// Whether maskable interrupts are currently enabled (RFLAGS.IF, bit 9).
pub fn interrupts_enabled() -> bool {
    let rflags: u64;
    // SAFETY: `pushfq; pop` reads RFLAGS and touches only the stack slot it
    // pushes and immediately pops; no other state changes.
    unsafe { asm!("pushfq", "pop {}", out(reg) rflags, options(preserves_flags)) };
    rflags & (1 << 9) != 0
}

/// Poll `cond` until it returns `true` (returns `true`), or until the
/// absolute tick value `deadline` is reached (returns `false`). Every
/// bounded wait on hardware in this kernel goes through here, so a
/// hardware or driver bug produces a reported timeout error instead of a
/// silent hang.
///
/// # Interrupt-flag guard
/// The deadline is measured in ticks, and ticks only advance while
/// interrupts are enabled. Called with interrupts disabled, a `cond` that
/// never becomes true would spin forever -- exactly the shape of the
/// AHCI sti-ordering bug this project already shipped once and caught in
/// testing. Rather than trust every future call site to get the ordering
/// right, this checks RFLAGS.IF up front and panics with a precise
/// message if it is clear: a loud, attributable failure on the panic
/// screen instead of a frozen machine. `cond` is still evaluated once
/// first, so a condition that is *already* true succeeds even with
/// interrupts off -- only a wait that would actually need the clock is
/// refused.
pub fn poll_until(deadline: u64, mut cond: impl FnMut() -> bool) -> bool {
    if cond() {
        return true;
    }
    assert!(
        interrupts_enabled(),
        "clock::poll_until called with interrupts disabled: the tick deadline could never be reached"
    );
    loop {
        if cond() {
            return true;
        }
        if ticks() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Block the calling context for at least `ms` milliseconds (rounded up to
/// whole ticks; `0` returns immediately), idling the CPU between ticks via
/// `hlt` rather than busy-waiting. `stop` is consulted after every wakeup
/// and the sleep ends early as soon as it returns `true`. Returns `true` if the full
/// duration elapsed, `false` if `stop` cut it short. Used by the shell to
/// make a WISP script's `sleep_ms()` respond to the Esc stop key
/// immediately rather than only once the sleep ends.
///
/// Uses the `cli` / `sti; hlt` / `cli` interrupt-shadow idiom: x86
/// guarantees no interrupt is serviced until the instruction *after* `sti`
/// has executed, so `sti` immediately followed by `hlt` can never miss an
/// interrupt that becomes pending in between -- the CPU either has one
/// already pending (taken right after the `hlt`, because `hlt` itself is
/// the "instruction after sti") or it goes to sleep and wakes on the next
/// one. Interrupts are re-disabled before re-checking the tick count, so a
/// tick that lands between the wakeup and the check is never lost. `stop`
/// runs with interrupts *enabled* (it may poll a device, as the USB
/// keyboard path does), and the function always returns with interrupts
/// enabled, matching how it is entered.
pub fn sleep_ms_unless(ms: u64, mut stop: impl FnMut() -> bool) -> bool {
    if ms == 0 {
        return true;
    }
    let target = ticks().saturating_add(ms.div_ceil(MS_PER_TICK));
    loop {
        if stop() {
            return false;
        }
        // SAFETY: see this function's doc comment -- the classic race-free
        // idle sequence; every path out of this block re-enables
        // interrupts.
        unsafe { asm!("cli", options(nomem, nostack, preserves_flags)) };
        if ticks() >= target {
            unsafe { asm!("sti", options(nomem, nostack, preserves_flags)) };
            return true;
        }
        // No `nomem`: the wakeup is precisely the point at which memory
        // (the tick counter, the keyboard queue) changed underneath us.
        unsafe { asm!("sti", "hlt", options(nostack)) };
    }
}
