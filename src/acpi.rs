//! ACPI table discovery: RSDP -> XSDT (or RSDT) -> MADT, parsed from
//! scratch against the ACPI 6.x specification -- no borrowed code, no
//! `acpi` crate, in the same spirit as every other driver in this kernel.
//!
//! # Why this exists at all
//!
//! Every interrupt this kernel has ever taken came through the legacy
//! 8259 PIC (`pic.rs`), which `pic.rs`'s own module doc comment justified
//! as "APIC needs ACPI/MADT table parsing and MMIO register access that
//! this phase never scoped, for no benefit on a single core." That
//! trade-off was correct for QEMU and wrong for real hardware: a machine
//! built after roughly 2010 may have no 8259 at all (or a vestigial one
//! the firmware has already disabled), no PS/2 controller behind IRQ1,
//! and -- on anything modern -- no way to receive a USB keyboard's
//! interrupts without a real interrupt controller. This module is the
//! first half of closing that gap: finding out, from the firmware's own
//! tables, *where* the Local APIC and I/O APIC live and *how* the legacy
//! ISA IRQ numbers this kernel already speaks map onto the I/O APIC's
//! Global System Interrupt space.
//!
//! # Scope, deliberately reduced
//!
//! - **Tables only, no AML.** This module reads the fixed-format
//!   `MADT` (`APIC`) table and nothing else. It contains no AML
//!   interpreter, so it cannot evaluate `_PRT` (PCI interrupt routing),
//!   `_CRS`, or any other method-backed object. That is a real,
//!   deliberate limit, and it is exactly why `xhci.rs` polls its event
//!   ring rather than taking a legacy PCI INTx interrupt -- see that
//!   module's own doc comment.
//! - **One I/O APIC's worth of routing is all this kernel needs**, but
//!   every I/O APIC the MADT reports is still parsed and recorded, so a
//!   GSI that belongs to a second controller resolves correctly instead
//!   of being silently misrouted to the first one's redirection table.
//! - **No SRAT/SLIT/NUMA, no multi-processor startup.** Processor Local
//!   APIC entries are counted and logged (useful, honest boot
//!   diagnostics: it says how many CPUs this machine actually has) but
//!   nothing is ever started on them. This kernel is still
//!   single-core by design.
//!
//! # Why the RSDP is captured before `exit_boot_services`
//!
//! The RSDP is not at a fixed address; it is found through the UEFI
//! system table's configuration-table array, which is only reachable
//! while a `SystemTable` handle exists. [`find_rsdp`] is therefore called
//! from `main.rs` *before* `exit_boot_services` consumes that handle, and
//! only the resulting physical address is carried across the boundary --
//! the same pattern `gfx::Console`'s framebuffer pointer and
//! `fs::snapshot_apps`'s owned `Vec` already use. The tables themselves
//! live in memory the firmware types `ACPI_RECLAIM`/`ACPI_NVS`, which
//! `pmm.rs` deliberately never hands out as free frames and `paging.rs`
//! identity-maps along with every other memory-map region, so the
//! captured address stays valid and readable for the life of the program.

use alloc::vec::Vec;

use uefi::table::cfg::{ACPI2_GUID, ACPI_GUID};
use uefi::table::{Boot, SystemTable};

use crate::debugcon;

/// `"RSD PTR "` -- the Root System Description Pointer's 8-byte
/// signature (ACPI 6.5, 5.2.5.3). Note the trailing space: it is part of
/// the signature, not padding.
const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
/// `"APIC"` -- the Multiple APIC Description Table's signature (ACPI
/// 6.5, 5.2.12). The table everyone calls the MADT is signed `APIC`.
const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// Every ACPI table (and the RSDP's own v2 extension) is validated by
/// this rule: the 8-bit sum of every byte in the structure must be zero
/// (ACPI 6.5, 5.2.5.3 / 5.2.6). A table failing it is rejected outright
/// rather than trusted -- firmware bugs here are rare but real, and
/// reading a bad `LAPIC` address would fault in a place with no useful
/// diagnostic, long after the actual mistake.
fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) == 0
}

/// Read `len` bytes starting at physical address `addr` as a slice.
///
/// # Safety
/// `addr..addr+len` must be a readable, identity-mapped region -- true
/// for every ACPI table under this kernel's page tables (see the module
/// doc comment), and checked by the caller against a plausible length
/// before being used for anything larger than a header.
unsafe fn phys_slice<'a>(addr: u64, len: usize) -> &'a [u8] {
    unsafe { core::slice::from_raw_parts(addr as *const u8, len) }
}

/// Read a `u32` from `bytes` at `offset`, little-endian. ACPI tables are
/// byte-packed with no alignment guarantees whatsoever (a MADT's
/// variable-length entry list routinely leaves a 4-byte field on an odd
/// address), so every multi-byte field in this module is assembled from
/// individual bytes rather than read through a `#[repr(packed)]` struct
/// pointer -- which would be an unaligned load, and is exactly the kind
/// of thing that works on x86 and is undefined behavior in Rust.
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

/// Length of the System Description Table header every ACPI table
/// (except the RSDP) begins with: signature, length, revision, checksum,
/// OEM id/table id/revision, creator id/revision (ACPI 6.5, 5.2.6).
const SDT_HEADER_LEN: usize = 36;

/// One I/O APIC, from a MADT type-1 entry (ACPI 6.5, 5.2.12.3).
#[derive(Clone, Copy)]
pub struct IoApicEntry {
    pub id: u8,
    /// Physical base of this controller's 32-bit MMIO register window.
    pub address: u64,
    /// The first Global System Interrupt number this controller handles;
    /// its redirection table entry `n` serves GSI `gsi_base + n`.
    pub gsi_base: u32,
}

/// One Interrupt Source Override, from a MADT type-2 entry (ACPI 6.5,
/// 5.2.12.5): "ISA IRQ `source` is actually wired to Global System
/// Interrupt `gsi`, with these polarity/trigger settings." The single
/// most important thing in the whole MADT for this kernel: on virtually
/// every PC, ISA IRQ0 (the timer) is overridden onto GSI 2, so a kernel
/// that assumed `gsi == irq` would route the timer to the wrong pin and
/// never see a tick.
#[derive(Clone, Copy)]
pub struct InterruptSourceOverride {
    /// ISA IRQ number being overridden (0-15).
    pub source: u8,
    /// The Global System Interrupt it is really wired to.
    pub gsi: u32,
    /// MPS INTI flags: bits 1:0 polarity (00 = bus default, 01 = active
    /// high, 11 = active low), bits 3:2 trigger mode (00 = bus default,
    /// 01 = edge, 11 = level). "Bus default" for the ISA bus means
    /// active-high, edge-triggered.
    pub flags: u16,
}

/// Everything [`parse`] learns from the MADT -- the complete interrupt
/// topology this kernel needs to bring up the Local APIC (`lapic.rs`)
/// and I/O APIC (`ioapic.rs`) without guessing a single address.
pub struct AcpiInfo {
    /// Physical base of the Local APIC's MMIO register window. Taken
    /// from the MADT's own 32-bit field, then replaced if (and only if)
    /// a type-5 Local APIC Address Override entry is present -- which is
    /// the whole reason that entry type exists, and the only way a
    /// 64-bit LAPIC base is ever reported.
    pub lapic_address: u64,
    /// Every I/O APIC the MADT reports, in table order.
    pub io_apics: Vec<IoApicEntry>,
    /// Every Interrupt Source Override, in table order.
    pub overrides: Vec<InterruptSourceOverride>,
    /// Count of enabled Processor Local APIC entries -- logged as boot
    /// diagnostics only; this kernel never starts a second CPU.
    pub cpu_count: usize,
    /// MADT flags bit 0 (`PCAT_COMPAT`): the machine also has dual 8259
    /// PICs that must be masked before APIC interrupts are relied on.
    /// When clear, there is no legacy PIC at all -- a real possibility on
    /// modern hardware, and precisely the case a PIC-only kernel cannot
    /// boot usefully on.
    pub pcat_compat: bool,
}

impl AcpiInfo {
    /// Translate an ISA IRQ number (what `pic.rs`-era code speaks) into
    /// the Global System Interrupt the I/O APIC actually serves it on,
    /// plus the polarity/trigger flags to program, honoring any
    /// Interrupt Source Override for that IRQ.
    ///
    /// With no override, ACPI 6.5, 5.2.12.5 specifies identity mapping
    /// (`gsi == irq`) with ISA bus defaults (active high, edge
    /// triggered) -- so this returns exactly that, rather than treating
    /// a missing override as an error.
    pub fn resolve_isa_irq(&self, irq: u8) -> (u32, u16) {
        for o in &self.overrides {
            if o.source == irq {
                return (o.gsi, o.flags);
            }
        }
        (u32::from(irq), 0)
    }

    /// Find the I/O APIC whose redirection table actually serves `gsi`,
    /// and that entry's index within it. `None` if no reported
    /// controller covers this GSI -- a real possibility on a malformed
    /// MADT, and something to report rather than paper over by assuming
    /// the first controller.
    ///
    /// `max_entries_for` is supplied by the caller (`ioapic.rs` reads it
    /// from each controller's own version register) because the MADT
    /// itself does not report how many redirection entries a controller
    /// has -- only where its GSI range starts.
    pub fn io_apic_for_gsi(
        &self,
        gsi: u32,
        max_entries_for: impl Fn(&IoApicEntry) -> u32,
    ) -> Option<(IoApicEntry, u32)> {
        for io in &self.io_apics {
            let count = max_entries_for(io);
            if gsi >= io.gsi_base && gsi < io.gsi_base + count {
                return Some((*io, gsi - io.gsi_base));
            }
        }
        None
    }
}

/// Locate the ACPI RSDP through the UEFI configuration table, preferring
/// the ACPI 2.0+ entry (which is the only one that can report an XSDT)
/// over the legacy ACPI 1.0 entry, and return its physical address.
///
/// Must be called while Boot Services is still alive -- see this
/// module's doc comment for why. Returns `None` on a machine whose
/// firmware publishes no ACPI tables at all, which is a first-class
/// outcome this kernel must keep booting through (falling back to the
/// legacy 8259 path), not an error.
pub fn find_rsdp(st: &SystemTable<Boot>) -> Option<u64> {
    let mut v1: Option<u64> = None;
    for entry in st.config_table() {
        if entry.guid == ACPI2_GUID {
            return Some(entry.address as u64);
        }
        if entry.guid == ACPI_GUID {
            v1 = Some(entry.address as u64);
        }
    }
    v1
}

/// Validate the RSDP at `rsdp_addr` and return the physical address of
/// the root table it points at, along with whether that table is an XSDT
/// (64-bit entries) or an RSDT (32-bit entries).
///
/// # Safety
/// `rsdp_addr` must be the address [`find_rsdp`] returned, and the ACPI
/// region must be mapped (true after `paging::load`, see the module doc
/// comment).
unsafe fn root_table(rsdp_addr: u64) -> Result<(u64, bool), &'static str> {
    // The v1 RSDP is exactly 20 bytes and is what the first checksum
    // covers; a v2+ RSDP extends it and carries a second checksum over
    // its own self-reported length. Read the v1 part first so the
    // `length` field is only trusted after the structure has identified
    // itself as a real RSDP.
    let v1 = unsafe { phys_slice(rsdp_addr, 20) };
    if &v1[0..8] != RSDP_SIGNATURE {
        return Err("acpi: RSDP signature mismatch (not an 'RSD PTR ' structure)");
    }
    if !checksum_ok(v1) {
        return Err("acpi: RSDP v1 checksum failed -- refusing to trust the table pointers in it");
    }

    let revision = v1[15];
    let rsdt_address = u64::from(read_u32(v1, 16));

    if revision >= 2 {
        let length = read_u32(unsafe { phys_slice(rsdp_addr, 24) }, 20) as usize;
        // A v2 RSDP is 36 bytes; anything shorter than the v1 part it
        // extends, or implausibly long, means a corrupt structure rather
        // than a newer revision this code should try to honor.
        if !(20..=256).contains(&length) {
            return Err("acpi: RSDP v2 reports an implausible length -- corrupt table");
        }
        let full = unsafe { phys_slice(rsdp_addr, length) };
        if !checksum_ok(full) {
            return Err("acpi: RSDP v2 extended checksum failed");
        }
        let xsdt_address = read_u64(full, 24);
        if xsdt_address != 0 {
            return Ok((xsdt_address, true));
        }
        // A v2 RSDP with a zero XSDT pointer is unusual but legal; fall
        // back to the v1 RSDT rather than failing outright.
    }

    if rsdt_address == 0 {
        return Err("acpi: RSDP reports neither an XSDT nor an RSDT address");
    }
    Ok((rsdt_address, false))
}

/// Read and validate one System Description Table header at `addr`,
/// returning `(signature, total_length)`.
///
/// # Safety
/// Same contract as [`phys_slice`].
unsafe fn sdt_header(addr: u64) -> Result<([u8; 4], usize), &'static str> {
    let header = unsafe { phys_slice(addr, SDT_HEADER_LEN) };
    let signature = [header[0], header[1], header[2], header[3]];
    let length = read_u32(header, 4) as usize;
    // A table shorter than its own header, or larger than any real ACPI
    // table has ever been, means the pointer that led here was wrong --
    // catch it now rather than by faulting partway through a walk.
    if !(SDT_HEADER_LEN..=1024 * 1024).contains(&length) {
        return Err("acpi: SDT reports an implausible length -- corrupt table or bad pointer");
    }
    Ok((signature, length))
}

/// Walk the root table's entry list and return the physical address of
/// the table whose signature is `wanted`, or `None` if it isn't present.
///
/// # Safety
/// Same contract as [`phys_slice`]; `root_addr`/`is_xsdt` must come from
/// [`root_table`].
unsafe fn find_table(root_addr: u64, is_xsdt: bool, wanted: &[u8; 4]) -> Result<Option<u64>, &'static str> {
    let (signature, length) = unsafe { sdt_header(root_addr)? };
    let expected: &[u8; 4] = if is_xsdt { b"XSDT" } else { b"RSDT" };
    if &signature != expected {
        return Err("acpi: root table's signature is neither 'XSDT' nor 'RSDT' as the RSDP claimed");
    }
    let root = unsafe { phys_slice(root_addr, length) };
    if !checksum_ok(root) {
        return Err("acpi: root table (XSDT/RSDT) checksum failed");
    }

    let entry_size = if is_xsdt { 8 } else { 4 };
    let entries = (length - SDT_HEADER_LEN) / entry_size;
    for i in 0..entries {
        let offset = SDT_HEADER_LEN + i * entry_size;
        let addr = if is_xsdt {
            read_u64(root, offset)
        } else {
            u64::from(read_u32(root, offset))
        };
        if addr == 0 {
            continue;
        }
        // A single unreadable/implausible entry is skipped rather than
        // failing the whole walk: the table we actually want may well be
        // a later entry, and a firmware that publishes one bad pointer
        // is not a reason to fall back to the legacy PIC path entirely.
        let Ok((sig, _len)) = (unsafe { sdt_header(addr) }) else {
            continue;
        };
        if &sig == wanted {
            return Ok(Some(addr));
        }
    }
    Ok(None)
}

/// Parse the MADT at `madt_addr` into an [`AcpiInfo`].
///
/// # Safety
/// Same contract as [`phys_slice`]; `madt_addr` must be a validated MADT
/// address from [`find_table`].
unsafe fn parse_madt(madt_addr: u64) -> Result<AcpiInfo, &'static str> {
    let (_sig, length) = unsafe { sdt_header(madt_addr)? };
    let madt = unsafe { phys_slice(madt_addr, length) };
    if !checksum_ok(madt) {
        return Err("acpi: MADT checksum failed");
    }
    // MADT-specific fixed fields sit immediately after the standard SDT
    // header: a 32-bit Local APIC address, then 32 bits of flags (ACPI
    // 6.5, 5.2.12). The variable-length entry list starts at +44.
    if length < SDT_HEADER_LEN + 8 {
        return Err("acpi: MADT is too short to contain its own fixed fields");
    }
    let mut info = AcpiInfo {
        lapic_address: u64::from(read_u32(madt, SDT_HEADER_LEN)),
        io_apics: Vec::new(),
        overrides: Vec::new(),
        cpu_count: 0,
        pcat_compat: read_u32(madt, SDT_HEADER_LEN + 4) & 1 != 0,
    };

    let mut offset = SDT_HEADER_LEN + 8;
    while offset + 2 <= length {
        let entry_type = madt[offset];
        let entry_len = madt[offset + 1] as usize;
        // A zero-length entry would loop forever, and an entry claiming
        // to run past the end of the table is corrupt -- stop the walk
        // with what was parsed so far rather than spin or read out of
        // bounds. Everything already collected is still valid and
        // usable; this is the "degrade, don't crash" shape every other
        // driver in this kernel uses for malformed hardware data.
        if entry_len < 2 || offset + entry_len > length {
            debugcon::write_str("[acpi] MADT entry list is malformed -- stopping the walk early\n");
            break;
        }
        let entry = &madt[offset..offset + entry_len];
        match entry_type {
            // Type 0: Processor Local APIC. Flags bit 0 = "enabled".
            0 if entry_len >= 8 => {
                if read_u32(entry, 4) & 1 != 0 {
                    info.cpu_count += 1;
                }
            }
            // Type 1: I/O APIC.
            1 if entry_len >= 12 => {
                info.io_apics.push(IoApicEntry {
                    id: entry[2],
                    address: u64::from(read_u32(entry, 4)),
                    gsi_base: read_u32(entry, 8),
                });
            }
            // Type 2: Interrupt Source Override.
            2 if entry_len >= 10 => {
                info.overrides.push(InterruptSourceOverride {
                    source: entry[3],
                    gsi: read_u32(entry, 4),
                    flags: read_u16(entry, 8),
                });
            }
            // Type 5: Local APIC Address Override -- a 64-bit base that
            // supersedes the MADT's own 32-bit field. This is the only
            // mechanism by which a LAPIC can live above 4 GiB, so it is
            // honored rather than ignored even though no machine this
            // project has been run on actually uses it.
            5 if entry_len >= 12 => {
                info.lapic_address = read_u64(entry, 4);
            }
            _ => {}
        }
        offset += entry_len;
    }

    if info.lapic_address == 0 {
        return Err("acpi: MADT reports a zero Local APIC address");
    }
    Ok(info)
}

/// Find and parse the MADT, starting from the RSDP address captured
/// before `exit_boot_services`.
///
/// Returns `Err` for every "this machine's ACPI tables can't give us an
/// interrupt topology" outcome -- no RSDP published, a failed checksum, no
/// MADT in the root table, a zero LAPIC address. Every one of those is a
/// first-class, expected condition on some real machine, and callers
/// (`main.rs`) must report it and fall back to the legacy 8259 path
/// rather than refusing to boot, exactly as `ahci::init`'s "no
/// persistent storage this boot" path already does.
///
/// # Safety
/// Must be called after `paging::load` (the ACPI regions are only
/// guaranteed mapped under this kernel's own tables at that point) and
/// with `rsdp_addr` as returned by [`find_rsdp`].
pub unsafe fn parse(rsdp_addr: Option<u64>) -> Result<AcpiInfo, &'static str> {
    let Some(rsdp_addr) = rsdp_addr else {
        return Err("acpi: firmware published no ACPI configuration table");
    };
    let (root_addr, is_xsdt) = unsafe { root_table(rsdp_addr)? };
    let _ = debugcon::write_fmt(format_args!(
        "[acpi] rsdp=0x{rsdp_addr:x} {}=0x{root_addr:x}\n",
        if is_xsdt { "xsdt" } else { "rsdt" },
    ));

    let Some(madt_addr) = (unsafe { find_table(root_addr, is_xsdt, MADT_SIGNATURE)? }) else {
        return Err("acpi: no MADT ('APIC') table present in the XSDT/RSDT");
    };
    let info = unsafe { parse_madt(madt_addr)? };

    let _ = debugcon::write_fmt(format_args!(
        "[acpi] madt=0x{madt_addr:x} lapic=0x{:x} cpus={} pcat_compat={} io_apics={} overrides={}\n",
        info.lapic_address,
        info.cpu_count,
        info.pcat_compat,
        info.io_apics.len(),
        info.overrides.len(),
    ));
    for io in &info.io_apics {
        let _ = debugcon::write_fmt(format_args!(
            "[acpi]   io_apic id={} address=0x{:x} gsi_base={}\n",
            io.id, io.address, io.gsi_base,
        ));
    }
    for o in &info.overrides {
        let _ = debugcon::write_fmt(format_args!(
            "[acpi]   override isa_irq={} -> gsi={} flags=0x{:04x}\n",
            o.source, o.gsi, o.flags,
        ));
    }

    Ok(info)
}
