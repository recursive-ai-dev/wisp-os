//! Legacy PCI configuration-space access (the 0xCF8/0xCFC I/O port
//! mechanism, "PCI configuration mechanism #1" -- PCI 2.2 spec, 3.2.2.3.1)
//! and a flat bus-0 device enumeration, built from scratch like every other
//! driver in this kernel.
//!
//! # Why bus 0 only, no PCI-to-PCI bridge recursion
//!
//! A real AHCI controller (including QEMU's `ich9-ahci`) is a chipset
//! device that lives on bus 0 -- there's no need to walk through any
//! bridge to reach it. Recursing through every PCI-to-PCI bridge to
//! enumerate every bus a machine might have is real, separate engineering
//! effort with no payoff for what this kernel actually needs: exactly one
//! AHCI controller, always found without ever needing to leave bus 0. Same
//! scope-reduction spirit as `pic.rs`'s "no APIC, single core."
//!
//! # Why the legacy mechanism, not ECAM/MMCONFIG
//! PCI Express's MMCONFIG mechanism needs the base address from the
//! firmware's ACPI `MCFG` table, which `acpi.rs` (MADT only) does not
//! parse. The legacy 0xCF8/0xCFC I/O ports are simple port I/O (already this
//! kernel's native idiom -- see `pic.rs`/`pit.rs`), universally supported by
//! every PC-compatible platform for backward compatibility (including every
//! machine QEMU emulates), and entirely sufficient for finding one chipset
//! device on bus 0.

use core::arch::asm;

/// I/O port for selecting which configuration-space register the next
/// `CONFIG_DATA` access reads/writes (PCI 2.2 spec, 3.2.2.3.2).
const CONFIG_ADDRESS: u16 = 0xCF8;
/// I/O port through which the selected configuration-space dword is
/// actually read or written.
const CONFIG_DATA: u16 = 0xCFC;

fn out32(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

fn in32(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// One PCI function's location: bus, device (0-31), function (0-7).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

/// Build the 32-bit value `CONFIG_ADDRESS` expects: enable bit (31), bus
/// (23:16), device (15:11), function (10:8), register dword index (7:2,
/// i.e. `offset` with its low 2 bits masked off -- configuration space is
/// only ever addressed a dword at a time through this mechanism).
fn config_address(addr: PciAddress, offset: u8) -> u32 {
    (1u32 << 31)
        | ((addr.bus as u32) << 16)
        | ((addr.device as u32) << 11)
        | ((addr.function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

/// Read one dword from `addr`'s configuration space at `offset` (rounded
/// down to a dword boundary by [`config_address`]).
pub fn config_read32(addr: PciAddress, offset: u8) -> u32 {
    out32(CONFIG_ADDRESS, config_address(addr, offset));
    in32(CONFIG_DATA)
}

/// Write one dword to `addr`'s configuration space at `offset` (rounded
/// down to a dword boundary).
pub fn config_write32(addr: PciAddress, offset: u8, value: u32) {
    out32(CONFIG_ADDRESS, config_address(addr, offset));
    out32(CONFIG_DATA, value);
}

/// Read a 16-bit field at `offset` (need not be dword-aligned; the
/// containing dword is read and the right half-word extracted).
pub fn config_read16(addr: PciAddress, offset: u8) -> u16 {
    let dword = config_read32(addr, offset);
    let shift = (offset & 0x02) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Read-modify-write a 16-bit field at `offset`, leaving the other half of
/// the containing dword untouched. Used for the Command register (offset
/// 0x04): configuration-space writes are dword-granular, but flipping only
/// the Bus Master/Memory Space Enable bits must not disturb the read-only
/// Status half living in the upper 16 bits of that same dword.
pub fn config_write16(addr: PciAddress, offset: u8, value: u16) {
    let dword = config_read32(addr, offset);
    let shift = (offset & 0x02) * 8;
    let mask = !(0xFFFFu32 << shift);
    let new_dword = (dword & mask) | ((value as u32) << shift);
    config_write32(addr, offset, new_dword);
}

/// One PCI function discovered by [`enumerate_bus0`]: identity (vendor,
/// device), classification (class/subclass/prog-if, PCI 2.2 spec Appendix
/// D), and header type -- everything [`ahci::init`](crate::ahci::init)
/// needs to recognize an AHCI controller and everything
/// [`enable_bus_master_and_memory_space`]/[`read_bar`] need to bring it up.
#[derive(Clone, Copy)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Base class code (e.g. `0x01` = mass storage controller).
    pub class: u8,
    /// Subclass within `class` (e.g. `0x06` = SATA controller).
    pub subclass: u8,
    /// Programming interface within `class`/`subclass` (e.g. `0x01` = AHCI
    /// 1.0, for class 0x01 subclass 0x06).
    pub prog_if: u8,
    /// Header type (offset 0x0E, bits 6:0; bit 7 -- multi-function -- is
    /// stripped before storing here, see [`enumerate_bus0`]). `0x00` is a
    /// standard device header, the only kind this kernel needs to
    /// understand.
    pub header_type: u8,
}

/// Read one PCI function's identity/classification, or `None` if nothing
/// is present at that address. Vendor ID `0xFFFF` is PCI's own documented
/// "no device here" sentinel (PCI 2.2 spec, 6.2.1) -- a real vendor ID is
/// never all-ones.
fn probe_function(addr: PciAddress) -> Option<PciDevice> {
    let vendor_id = config_read16(addr, 0x00);
    if vendor_id == 0xFFFF {
        return None;
    }
    let device_id = config_read16(addr, 0x02);
    let class_reg = config_read32(addr, 0x08);
    let header_type_reg = config_read16(addr, 0x0E);
    Some(PciDevice {
        address: addr,
        vendor_id,
        device_id,
        class: ((class_reg >> 24) & 0xFF) as u8,
        subclass: ((class_reg >> 16) & 0xFF) as u8,
        prog_if: ((class_reg >> 8) & 0xFF) as u8,
        header_type: (header_type_reg & 0x7F) as u8,
    })
}

/// Enumerate every PCI function present on bus 0: every device (0-31),
/// function 0 always probed, functions 1-7 probed only when function 0's
/// header type marks the device multi-function (bit 7 of the header-type
/// register) -- probing every function of a genuinely single-function
/// device would just read the same "not present" sentinel eight times
/// over, harmlessly but pointlessly.
pub fn enumerate_bus0() -> alloc::vec::Vec<PciDevice> {
    let mut found = alloc::vec::Vec::new();
    for device in 0..32u8 {
        let function0_addr = PciAddress { bus: 0, device, function: 0 };
        let Some(function0) = probe_function(function0_addr) else {
            continue;
        };
        let header_type_reg = config_read16(function0_addr, 0x0E);
        let multi_function = header_type_reg & 0x0080 != 0;
        found.push(function0);
        if multi_function {
            for function in 1..8u8 {
                let addr = PciAddress { bus: 0, device, function };
                if let Some(dev) = probe_function(addr) {
                    found.push(dev);
                }
            }
        }
    }
    found
}

/// Log every discovered device to debugcon: `bus:device.function`,
/// `vendor:device`, `class:subclass:prog_if`, header type -- boot
/// diagnostics in the same spirit as `paging::init`'s guard-range logging,
/// and what `tests/ahci_test.py` greps to confirm the AHCI controller
/// (class `01:06:01`) was actually found.
pub fn dump_devices(devices: &[PciDevice]) {
    crate::debugcon::write_str("[pci] devices found:\n");
    for dev in devices {
        let _ = crate::debugcon::write_fmt(format_args!(
            "[pci]   {:02x}:{:02x}.{:x}  vendor:device={:04x}:{:04x}  class:subclass:prog_if={:02x}:{:02x}:{:02x}  header_type={:02x}\n",
            dev.address.bus,
            dev.address.device,
            dev.address.function,
            dev.vendor_id,
            dev.device_id,
            dev.class,
            dev.subclass,
            dev.prog_if,
            dev.header_type,
        ));
    }
}

/// Find the (first) AHCI controller among `devices`: class `0x01` (mass
/// storage), subclass `0x06` (SATA), prog-if `0x01` (AHCI 1.0) -- matched
/// by classification, not a hardcoded vendor:device ID, so this recognizes
/// any real AHCI controller, not just QEMU's `ich9-ahci`.
pub fn find_ahci_controller(devices: &[PciDevice]) -> Option<PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.class == 0x01 && d.subclass == 0x06 && d.prog_if == 0x01)
}

/// Set the Bus Master Enable (bit 2) and Memory Space Enable (bit 1) bits
/// in `addr`'s Command register (offset 0x04) -- required before touching
/// a device's memory-mapped registers (Memory Space Enable) or letting it
/// initiate its own DMA transfers (Bus Master Enable). Firmware may or may
/// not have already done this; it's set unconditionally here rather than
/// assumed.
pub fn enable_bus_master_and_memory_space(addr: PciAddress) {
    const MEMORY_SPACE_ENABLE: u16 = 1 << 1;
    const BUS_MASTER_ENABLE: u16 = 1 << 2;
    let command = config_read16(addr, 0x04);
    config_write16(addr, 0x04, command | MEMORY_SPACE_ENABLE | BUS_MASTER_ENABLE);
}

/// Read Base Address Register `index` (0-5) from `addr`'s configuration
/// space (offsets 0x10, 0x14, ..., 0x24). Returned raw, including the
/// low type/flag bits (PCI 2.2 spec, 6.2.5.1) -- the AHCI controller's
/// ABAR is BAR5, architecturally a 32-bit, non-prefetchable memory BAR, so
/// [`ahci::init`](crate::ahci::init) just masks off the low 4 bits itself
/// rather than this function trying to generically handle every BAR type
/// (I/O-space BARs, 64-bit BAR pairs) nothing here needs.
pub fn read_bar(addr: PciAddress, index: u8) -> u32 {
    debug_assert!(index < 6, "pci::read_bar: BAR index must be 0-5");
    config_read32(addr, 0x10 + index * 4)
}
