//! Legacy 8259 Programmable Interrupt Controller (PIC) driver: remap the two
//! cascaded PICs off their power-on vectors (which collide with CPU
//! exceptions 0-31) and onto 0x20-0x2F, then mask everything except the
//! timer (IRQ0) and keyboard (IRQ1).
//!
//! Deliberately the 8259, not the APIC/IOAPIC: APIC needs ACPI/MADT table
//! parsing and MMIO register access that this phase never scoped, for no
//! benefit on a single core. Every PC-compatible machine QEMU/OVMF emulates
//! boots with the 8259 present and enabled by firmware.

use core::arch::asm;

const MASTER_CMD: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_CMD: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

/// Base vector the master PIC's IRQ0-7 are remapped to. Chosen as the first
/// free vector after the 32 CPU-reserved exception vectors.
pub const MASTER_VECTOR_BASE: u8 = 0x20;
/// Base vector the slave PIC's IRQ8-15 are remapped to.
pub const SLAVE_VECTOR_BASE: u8 = 0x28;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

fn out8(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

fn in8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// A short delay by writing to an unused port (0x80, the traditional
/// "POST diagnostic codes" port on PC hardware), used between the PIC's
/// initialization command words because some real 8259 silicon is too slow
/// to process them back-to-back at modern bus speeds. Harmless and
/// side-effect-free on any machine (real or emulated) that doesn't actually
/// have a POST diagnostic display wired to this port.
fn io_wait() {
    out8(0x80, 0);
}

/// Remap both PICs to `MASTER_VECTOR_BASE`/`SLAVE_VECTOR_BASE` and mask
/// every line except IRQ0 (timer) and IRQ1 (keyboard). Must run before
/// `sti` and after `idt::load` (so that by the time any unmasked IRQ can
/// possibly fire, vectors 0x20/0x21 already have real handlers installed).
///
/// # Safety
/// Performs raw port I/O against the two 8259 PICs; must be called exactly
/// once, with interrupts still disabled, on hardware (real or emulated)
/// that actually has 8259-compatible PICs at the standard ports -- true for
/// every PC-compatible platform this project targets.
pub unsafe fn init() {
    // Save nothing to restore: this project never runs under whatever mask
    // firmware left behind past this point, so there is no prior state
    // worth preserving.

    // ICW1: begin initialization, expect ICW4.
    out8(MASTER_CMD, ICW1_INIT | ICW1_ICW4);
    io_wait();
    out8(SLAVE_CMD, ICW1_INIT | ICW1_ICW4);
    io_wait();

    // ICW2: vector offsets.
    out8(MASTER_DATA, MASTER_VECTOR_BASE);
    io_wait();
    out8(SLAVE_DATA, SLAVE_VECTOR_BASE);
    io_wait();

    // ICW3: cascade wiring -- master has a slave on IRQ2 (bit mask 0x04),
    // slave identifies itself by cascade identity 2.
    out8(MASTER_DATA, 0x04);
    io_wait();
    out8(SLAVE_DATA, 0x02);
    io_wait();

    // ICW4: 8086/88 mode (not the legacy 8080 mode nobody has used since
    // the 1970s).
    out8(MASTER_DATA, ICW4_8086);
    io_wait();
    out8(SLAVE_DATA, ICW4_8086);
    io_wait();

    // Mask (disable) every line except IRQ0 (timer, bit 0) and IRQ1
    // (keyboard, bit 1) on the master; mask everything on the slave (bit 2
    // of the master's mask -- the cascade line itself -- stays unmasked so
    // a slave IRQ *could* reach the CPU if we ever unmask one later, but
    // nothing on the slave is unmasked in this phase).
    out8(MASTER_DATA, 0b1111_1000);
    out8(SLAVE_DATA, 0b1111_1111);
}

/// Mask every line on both PICs, so neither can deliver anything ever
/// again. Called exactly once, by `lapic::enter_apic_mode`, at the moment
/// this kernel stops using the 8259 and starts using the APIC -- the two
/// controllers must never both be live, since an interrupt delivered
/// through the PIC would be EOI'd at the LAPIC (or vice versa) and wedge
/// whichever one actually had it in service.
///
/// Masking is deliberately used rather than leaving the PICs at their
/// power-on vectors: `init` above already remapped them off the CPU's
/// exception range, so a stray, electrically-generated edge on a masked
/// line can still only ever produce a vector this kernel has a real
/// handler for (`idt.rs`'s spurious-IRQ7/IRQ15 arms), never a fault
/// vector misinterpreted as an exception.
///
/// # Safety
/// Raw port I/O against the two 8259 PICs; must be called after [`init`]
/// and with interrupts disabled.
pub unsafe fn mask_all() {
    out8(MASTER_DATA, 0xFF);
    out8(SLAVE_DATA, 0xFF);
}

/// Signal end-of-interrupt for `irq_line` (0-15) so the PIC delivers further
/// interrupts of equal or lower priority. Must be called exactly once per
/// serviced hardware interrupt, after handling it -- an unacknowledged IRQ
/// leaves that line (and, for a slave-PIC line, every master-PIC line too)
/// permanently blocked.
///
/// If `irq_line` came from the slave PIC (8-15), EOI must go to *both*
/// PICs: the slave (so it stops holding its own line) and the master (so it
/// stops holding the cascade line, IRQ2, that the slave's request arrived
/// on).
pub fn end_of_interrupt(irq_line: u8) {
    const EOI: u8 = 0x20;
    if irq_line >= 8 {
        out8(SLAVE_CMD, EOI);
    }
    out8(MASTER_CMD, EOI);
}

/// Read the master PIC's In-Service Register: bit `n` is set while IRQ `n`
/// is currently being serviced (i.e. has been acknowledged by the CPU but
/// not yet EOI'd). Used only to distinguish a genuine IRQ7 from a
/// *spurious* one (see `idt.rs`'s vector-0x27/0x2F handling) -- on real
/// 8259 hardware, electrical noise on an unmasked line can cause the PIC to
/// signal an interrupt that vanishes before the CPU actually reads the
/// vector, and the documented way to tell the difference is that a
/// spurious IRQ7/IRQ15 never sets the corresponding ISR bit.
pub fn master_isr() -> u8 {
    const READ_ISR: u8 = 0x0B;
    out8(MASTER_CMD, READ_ISR);
    in8(MASTER_CMD)
}

/// The slave-PIC equivalent of [`master_isr`], used to detect a spurious
/// IRQ15.
pub fn slave_isr() -> u8 {
    const READ_ISR: u8 = 0x0B;
    out8(SLAVE_CMD, READ_ISR);
    in8(SLAVE_CMD)
}
