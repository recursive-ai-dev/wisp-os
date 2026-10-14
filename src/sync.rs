//! Single-core, interrupt-disable-based mutual exclusion.
//!
//! This kernel has no multicore support (the `x86_64-unknown-uefi` target
//! already bakes in `singlethread: true`), so the only concurrent execution
//! possible is an interrupt handler preempting mainline code mid-access to
//! shared state. Disabling interrupts for the duration of a critical section
//! is both necessary and sufficient to make that safe, and needs no
//! spinlock, no atomics-based lock, and no external crate.

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{compiler_fence, Ordering};

/// Wraps a value shared between mainline code and interrupt handlers,
/// guarding every access with an interrupt-disabled critical section.
pub struct Locked<A> {
    inner: UnsafeCell<A>,
}

// SAFETY: every access to `inner` goes through `with`, which disables
// interrupts for the duration of the closure. On this single-core kernel
// that is the only possible source of concurrent access, so this is sound.
unsafe impl<A> Sync for Locked<A> {}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: UnsafeCell::new(inner),
        }
    }

    /// Run `f` with exclusive, interrupt-safe access to the wrapped value.
    pub fn with<R>(&self, f: impl FnOnce(&mut A) -> R) -> R {
        without_interrupts(|| {
            // SAFETY: interrupts are disabled for the duration of this
            // closure (see `without_interrupts`), and this kernel is
            // single-core, so no other execution context can be touching
            // `inner` concurrently with this access.
            let r = unsafe { &mut *self.inner.get() };
            f(r)
        })
    }
}

/// Save the current interrupt-enable flag (`RFLAGS.IF`), disable interrupts,
/// run `f`, then restore the flag to whatever it was *before* this call --
/// not unconditionally re-enabling it. That makes nested critical sections
/// compose correctly: an inner `without_interrupts` call does not
/// prematurely re-enable interrupts on exit just because it happened to run
/// inside an outer one that isn't done yet.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    const IF_BIT: u64 = 1 << 9;
    let was_enabled = flags & IF_BIT != 0;

    if was_enabled {
        unsafe { asm!("cli", options(nomem, nostack, preserves_flags)) };
    }
    // Compiler fence -- NOT a CPU fence; this is a single-core kernel with
    // no other CPU to synchronize against, so there is no cache-coherency
    // problem to solve here. What there is: an interrupt handler that can
    // read and write `inner` asynchronously, on this same CPU, at any
    // point between two calls to `with`. LLVM's optimizer has no model of
    // that -- it only sees this function's own straight-line control flow
    // -- and both `cli` and `sti` above/below are marked `options(nomem)`
    // (correctly: neither instruction itself touches memory), which is
    // exactly the signal LLVM needs to conclude it's free to hoist, sink,
    // or cache plain (non-atomic) reads/writes of `inner` across this
    // critical section, including across repeated calls to `with` inside
    // a `cli` / check / `sti-hlt` spin-wait loop (`keyboard::wait_for_key`,
    // notably). That is not a hypothetical: it reproduced exactly this way
    // under `opt-level = "s"` + LTO -- `RingBuffer::pop` kept reading a
    // stale "empty" result even after `on_irq` had genuinely pushed and
    // logged new bytes, because nothing here told the compiler the queue's
    // backing memory could change out from under it. `compiler_fence`
    // forbids the optimizer from reordering memory operations across this
    // point in program order, which is precisely (and only) what's needed:
    // it makes every access inside `f` observe, and be observed by, the
    // world as it stood right at entry/exit of this critical section,
    // matching what `cli`/`sti` already guarantee at the hardware level.
    // See `CHANGES.md` for the full account of diagnosing this.
    compiler_fence(Ordering::SeqCst);
    let ret = f();
    compiler_fence(Ordering::SeqCst);
    if was_enabled {
        unsafe { asm!("sti", options(nomem, nostack, preserves_flags)) };
    }
    ret
}
