//! 8253/8254 Programmable Interval Timer driver: programs channel 0 for a
//! steady [`clock::TICK_HZ`] tick and nothing else.
//!
//! The PIT is one of two hardware sources that can drive the kernel's
//! timebase; the other is the Local APIC timer (`lapic.rs`), which takes
//! over on machines with usable ACPI tables. Neither owns the tick
//! counter -- `clock.rs` does, and `idt.rs` feeds it from vector 0x20
//! whichever source raised the interrupt. See `clock.rs`'s module doc
//! comment for why that split exists.

use core::arch::asm;

use crate::clock::TICK_HZ;

const CHANNEL0_DATA: u16 = 0x40;
const COMMAND: u16 = 0x43;

/// The PIT's fixed input oscillator frequency, in Hz. Architectural
/// constant of the 8253/8254, not configurable.
const PIT_INPUT_HZ: u32 = 1_193_182;

fn out8(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Program channel 0 for square-wave mode (mode 3) at [`TICK_HZ`]. Must run
/// before `sti` (interrupts are still disabled at this point in the boot
/// sequence, so there is no race with the ISR this configures).
///
/// # Safety
/// Raw port I/O against the PIT; must be called exactly once.
pub unsafe fn init() {
    let divisor = PIT_INPUT_HZ / TICK_HZ;
    // Channel 0, access mode lobyte/hibyte, mode 3 (square wave generator),
    // binary (not BCD) counting.
    const SELECT_CHANNEL0_LOHI_MODE3: u8 = 0b00_11_011_0;
    out8(COMMAND, SELECT_CHANNEL0_LOHI_MODE3);
    out8(CHANNEL0_DATA, (divisor & 0xFF) as u8);
    out8(CHANNEL0_DATA, ((divisor >> 8) & 0xFF) as u8);
}
