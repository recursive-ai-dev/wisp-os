//! I/O APIC driver -- built from scratch against the Intel 82093AA I/O
//! APIC datasheet, the external half of the interrupt controller pair
//! `lapic.rs` completes.
//!
//! # What an I/O APIC actually is
//!
//! A pin-to-message translator. Each of its redirection table entries
//! says "when input pin `n` is asserted, send an interrupt message with
//! this vector, to the Local APIC with this ID." That is the whole
//! device. The legacy 8259 did the same job with a fixed, hardwired
//! mapping from pin to vector and a shared "which CPU" of exactly one;
//! the I/O APIC makes both programmable, which is precisely what lets a
//! PCI device's interrupt reach a vector this kernel chose.
//!
//! # Global System Interrupts, and the trap in ISA IRQ numbering
//!
//! ACPI numbers every interrupt input in the machine in one flat space:
//! Global System Interrupts. Each I/O APIC serves a contiguous run of
//! them starting at the `gsi_base` its MADT entry reports, so GSI `g` is
//! redirection entry `g - gsi_base` of whichever controller covers it.
//!
//! The trap: a legacy ISA IRQ number is *not* generally its GSI. On
//! essentially every PC, the timer's ISA IRQ0 is wired to GSI 2, and
//! firmware reports that as a MADT Interrupt Source Override -- which is
//! exactly what this kernel's own boot log shows on QEMU
//! (`override isa_irq=0 -> gsi=2`). A kernel that assumed `gsi == irq`
//! would program the wrong redirection entry and never see a timer tick,
//! with nothing anywhere reporting an error. `acpi::AcpiInfo::resolve_isa_irq`
//! exists to make that translation impossible to skip, and every routing
//! call in this module goes through it.
//!
//! # Scope, deliberately reduced
//!
//! - **Physical destination mode, one destination: the BSP.** This
//!   kernel has one CPU. Logical/flat destination mode and lowest-
//!   priority delivery exist to spread interrupts across several, and buy
//!   nothing here.
//! - **Fixed delivery mode only.** No NMI, SMI, or ExtINT redirection
//!   entries are ever programmed.
//! - **No masking/unmasking API beyond what boot needs.** Entries are
//!   programmed once, during bring-up, and left alone.

use core::sync::atomic::{compiler_fence, Ordering};

use crate::acpi::{AcpiInfo, IoApicEntry};
use crate::{debugcon, lapic, paging};

/// I/O APIC register *selector*: write the index of the register to
/// access here, then read or write [`IOWIN`] to actually touch it. The
/// device deliberately exposes only these two 32-bit MMIO locations --
/// its whole register file is reached indirectly through them (82093AA
/// datasheet, 3.1).
const IOREGSEL: usize = 0x00;
/// I/O APIC register *window*: reads/writes go to whichever register
/// [`IOREGSEL`] currently selects.
const IOWIN: usize = 0x10;

/// Register index 0x01: version. Bits 23:16 hold the highest valid
/// redirection table entry index (so the entry *count* is that plus
/// one) -- the only way to learn how many inputs a given controller
/// actually has, since the MADT reports only where its GSI range starts.
const REG_VERSION: u32 = 0x01;
/// Register index 0x10 + 2n: the low half of redirection table entry n.
/// Each entry is 64 bits, exposed as two consecutive 32-bit registers.
const REG_REDIRECTION_BASE: u32 = 0x10;

/// Redirection entry bit 13: interrupt input pin polarity, 1 = active
/// low. Clear (active high) is the ISA bus default.
const REDIR_ACTIVE_LOW: u64 = 1 << 13;
/// Redirection entry bit 15: trigger mode, 1 = level. Clear (edge) is
/// the ISA bus default.
const REDIR_LEVEL_TRIGGERED: u64 = 1 << 15;
/// Redirection entry bit 16: mask. Set means this input delivers
/// nothing.
const REDIR_MASKED: u64 = 1 << 16;

/// One page covers an I/O APIC's entire MMIO footprint: it exposes
/// exactly two 32-bit locations (at +0x00 and +0x10), and its base is
/// page-aligned.
const IOAPIC_MAP_SIZE: u64 = paging::PAGE_SIZE;

/// # Safety
/// `base` must be a mapped I/O APIC MMIO window.
unsafe fn read_reg(base: u64, index: u32) -> u32 {
    unsafe {
        core::ptr::write_volatile((base as *mut u8).add(IOREGSEL) as *mut u32, index);
        // The select write must land before the window read -- these are
        // two halves of one indivisible operation, and letting the
        // optimizer reorder them would read a register nobody asked for.
        compiler_fence(Ordering::SeqCst);
        core::ptr::read_volatile((base as *const u8).add(IOWIN) as *const u32)
    }
}

/// # Safety
/// Same contract as [`read_reg`].
unsafe fn write_reg(base: u64, index: u32, value: u32) {
    unsafe {
        core::ptr::write_volatile((base as *mut u8).add(IOREGSEL) as *mut u32, index);
        compiler_fence(Ordering::SeqCst);
        core::ptr::write_volatile((base as *mut u8).add(IOWIN) as *mut u32, value);
    }
}

/// How many redirection table entries `io` implements, read from its own
/// version register rather than assumed. Returns 0 if the controller
/// reports something impossible, so a caller's GSI-range check simply
/// never matches it instead of programming a nonexistent entry.
///
/// # Safety
/// `io.address` must already be mapped by [`init`].
pub unsafe fn redirection_entry_count(io: &IoApicEntry) -> u32 {
    let version = unsafe { read_reg(io.address, REG_VERSION) };
    let max_index = (version >> 16) & 0xFF;
    // An all-ones read is the classic "nothing is decoding this address"
    // signature; a controller claiming 256 entries is equally impossible
    // (the field is 8 bits and the architectural maximum is 239).
    if version == u32::MAX || max_index >= 0xFF {
        return 0;
    }
    max_index + 1
}

/// Map every I/O APIC the MADT reported and log what each one actually
/// is (version, entry count), so a machine whose topology differs from
/// the one this kernel was developed against is visible in the boot log
/// rather than a mystery later.
///
/// # Safety
/// Must be called after `paging::load` (this maps pages) and at most
/// once.
pub unsafe fn init(info: &AcpiInfo) -> Result<(), &'static str> {
    if info.io_apics.is_empty() {
        return Err("ioapic: MADT reported no I/O APIC at all");
    }
    for io in &info.io_apics {
        if io.address % paging::PAGE_SIZE != 0 {
            return Err("ioapic: MADT reports a non-page-aligned I/O APIC base");
        }
        // SAFETY: a real MMIO window from the firmware's own tables.
        unsafe { paging::map(io.address, io.address, IOAPIC_MAP_SIZE, paging::PageFlags::Mmio)? };

        let version = unsafe { read_reg(io.address, REG_VERSION) };
        let count = unsafe { redirection_entry_count(io) };
        let _ = debugcon::write_fmt(format_args!(
            "[ioapic] id={} base=0x{:x} gsi_base={} version=0x{:02x} entries={}\n",
            io.id,
            io.address,
            io.gsi_base,
            version & 0xFF,
            count,
        ));
        if count == 0 {
            return Err("ioapic: controller reports an implausible redirection table size -- not responding as expected");
        }

        // Mask every entry before anything is routed. Firmware has no
        // obligation to hand this controller over masked, and an entry
        // left live from the firmware's own use would deliver a vector
        // this kernel never configured the moment the 8259 stops
        // shadowing it -- landing in `idt.rs`'s "unexpected interrupt
        // vector" catch-all at best, and in a wrongly-EOI'd interrupt
        // storm at worst.
        for entry in 0..count {
            unsafe { write_entry_raw(io.address, entry, REDIR_MASKED) };
        }
    }
    Ok(())
}

/// Write one full 64-bit redirection table entry, high half first.
///
/// High-half-first is not stylistic: the low half holds the mask bit, so
/// writing it last means the entry can never be briefly live with a
/// stale destination field -- the destination is already correct by the
/// time anything can be delivered through it.
///
/// # Safety
/// `base` must be mapped and `entry` must be within that controller's
/// redirection table.
unsafe fn write_entry_raw(base: u64, entry: u32, value: u64) {
    let index = REG_REDIRECTION_BASE + entry * 2;
    unsafe {
        write_reg(base, index + 1, (value >> 32) as u32);
        compiler_fence(Ordering::SeqCst);
        write_reg(base, index, value as u32);
    }
}

/// Route the legacy ISA IRQ `irq` to `vector` on this CPU, honoring any
/// MADT Interrupt Source Override for it (both the GSI translation and
/// the polarity/trigger flags).
///
/// This is the one function that turns "this kernel expects the keyboard
/// on IRQ1" into a real, programmed hardware path, and every part of the
/// translation it performs is read from the firmware's tables rather than
/// assumed -- see this module's doc comment for what goes wrong when any
/// piece of it is guessed instead.
///
/// # Safety
/// Must be called after [`init`] and `lapic::init` (the entry's
/// destination field is this CPU's Local APIC ID, which only means
/// anything once the LAPIC is enabled).
pub unsafe fn route_isa_irq(info: &AcpiInfo, irq: u8, vector: u8) -> Result<(), &'static str> {
    let (gsi, flags) = info.resolve_isa_irq(irq);
    let Some((io, entry)) = info.io_apic_for_gsi(gsi, |io| unsafe { redirection_entry_count(io) }) else {
        return Err("ioapic: no I/O APIC covers the GSI this ISA IRQ maps to");
    };

    // MPS INTI flags (ACPI 6.5, 5.2.12.5, Table 5-25): bits 1:0 are
    // polarity -- 00 "conforms to bus specification" (for ISA: active
    // high), 01 active high, 11 active low -- and bits 3:2 are trigger
    // mode -- 00 conforms (for ISA: edge), 01 edge, 11 level. "Conforms"
    // is resolved to the ISA defaults here rather than passed through,
    // because the I/O APIC register has no "conforms" encoding: every
    // bit has to be a concrete choice by the time it's written.
    let polarity = flags & 0b11;
    let trigger = (flags >> 2) & 0b11;
    let mut value = u64::from(vector);
    if polarity == 0b11 {
        value |= REDIR_ACTIVE_LOW;
    }
    if trigger == 0b11 {
        value |= REDIR_LEVEL_TRIGGERED;
    }
    // Destination: this CPU's Local APIC ID, in the entry's top byte.
    // Delivery mode (bits 10:8) and destination mode (bit 11) are both
    // left zero -- "fixed delivery, physical destination" -- which is
    // exactly what a single-CPU kernel wants (see the module doc
    // comment). Mask bit left clear: this entry goes live immediately.
    value |= u64::from(lapic::id()) << 56;

    // SAFETY: `io` came from the MADT and was mapped by `init`; `entry`
    // was bounds-checked against that controller's own reported entry
    // count by `io_apic_for_gsi`.
    unsafe { write_entry_raw(io.address, entry, value) };

    let _ = debugcon::write_fmt(format_args!(
        "[ioapic] routed ISA IRQ{irq} -> GSI {gsi} (io_apic id={} entry {entry}) -> vector 0x{vector:02x} \
         polarity={} trigger={}\n",
        io.id,
        if value & REDIR_ACTIVE_LOW != 0 { "low" } else { "high" },
        if value & REDIR_LEVEL_TRIGGERED != 0 { "level" } else { "edge" },
    ));
    Ok(())
}

/// Mask the redirection entry serving legacy ISA IRQ `irq`, so it stops
/// delivering. Used when the LAPIC timer takes over the tick and the
/// PIT's IRQ0 becomes redundant -- leaving it live would deliver two
/// interrupts per tick period on the same vector, double-counting
/// `clock::ticks()` and quietly making every `sleep_ms` half as long as it
/// claims.
///
/// # Safety
/// Same contract as [`route_isa_irq`].
pub unsafe fn mask_isa_irq(info: &AcpiInfo, irq: u8) -> Result<(), &'static str> {
    let (gsi, _flags) = info.resolve_isa_irq(irq);
    let Some((io, entry)) = info.io_apic_for_gsi(gsi, |io| unsafe { redirection_entry_count(io) }) else {
        return Err("ioapic: no I/O APIC covers the GSI this ISA IRQ maps to");
    };
    unsafe { write_entry_raw(io.address, entry, REDIR_MASKED) };
    let _ = debugcon::write_fmt(format_args!(
        "[ioapic] masked ISA IRQ{irq} (GSI {gsi}, io_apic id={} entry {entry})\n",
        io.id,
    ));
    Ok(())
}
