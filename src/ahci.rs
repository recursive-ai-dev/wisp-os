//! AHCI (Advanced Host Controller Interface) driver -- built from scratch
//! against the AHCI 1.3.1 specification, no borrowed code, in the same
//! fully-custom spirit as every other driver in this kernel.
//!
//! This is the only way a WISP program's `save`/`load` data survives a
//! reboot: the boot medium itself (a USB stick, mounted read-only -- see
//! `run.sh`'s vvfat-corruption postmortem) is deliberately never written
//! to, so persistence needs a second, separate disk, and AHCI/SATA is how
//! this kernel talks to one.
//!
//! # Scope, deliberately reduced
//!
//! - **Single-outstanding-command mode, no NCQ.** One command slot (slot 0)
//!   is ever used; a command is issued and this driver polls until it
//!   completes before issuing the next. Simpler to reason about and
//!   entirely adequate for the small, infrequent reads/writes a save
//!   format needs -- NCQ's whole benefit (overlapping multiple in-flight
//!   commands) has no payoff here.
//! - **Polling completion with an explicit bounded timeout on every wait
//!   loop**, never an unbounded spin -- a hardware or driver bug must
//!   produce a reported timeout error, not a silent hang, matching this
//!   kernel's established "make failures visible" ethos (`idt.rs`'s
//!   catch-all logging, `pmm::self_test`'s loud failure path).
//! - **"No AHCI storage present" is a first-class, soft-fail condition.**
//!   Zero AHCI-class PCI devices, a controller with no implemented ports,
//!   or a port with no device attached must all be handled and reported
//!   without blocking boot -- real hardware booted from this project's USB
//!   stick may well have no second drive at all. See [`init`]'s doc
//!   comment.
//! - **One port only** -- the first implemented port with a device
//!   present. This driver has no use for more than one save disk.

use core::cell::Cell;
use core::sync::atomic::{compiler_fence, Ordering};

use crate::clock::{self, poll_until};
use crate::{debugcon, paging, pci, pmm};

/// Fixed size mapped for the ABAR (BAR5): 2 pages (8 KiB), a deliberate
/// simplification over general BAR-sizing (querying the BAR's actual size
/// via the standard write-all-1s/read-back/mask dance). The AHCI 1.3.1
/// spec architecturally bounds HBA register space to generic registers
/// (0x00-0x2B) plus up to 32 ports x 0x80 bytes each (port registers start
/// at 0x100) -- worst case a little under 8 KiB, comfortably inside this
/// fixed window regardless of how many ports a real controller implements.
const ABAR_MAP_SIZE: u64 = 2 * paging::PAGE_SIZE;

// --- HBA generic register offsets (AHCI 1.3.1, 3.1) ----------------------
const REG_CAP: usize = 0x00; // Host Capabilities
const REG_GHC: usize = 0x04; // Global Host Control
const REG_PI: usize = 0x0C; // Ports Implemented
const REG_VS: usize = 0x10; // Version

/// GHC bit 31: AHCI Enable -- must be set before any AHCI-mode register
/// (including the port registers) has defined behavior; a controller can
/// power on in a legacy/compatibility mode with AE clear.
const GHC_AE: u32 = 1 << 31;
/// GHC bit 0: HBA Reset -- software sets this to force a full reset of the
/// controller and every port back to a known, idle state (AHCI 1.3.1,
/// 10.4.3). Self-clears when the reset completes; also clears `GHC.AE` as a
/// documented side effect.
const GHC_HR: u32 = 1 << 0;

// --- Port register offsets, relative to `port_offset(port)` (AHCI 1.3.1,
// 3.3) -- the port register block starts at ABAR+0x100, one 0x80-byte block
// per port. -------------------------------------------------------------
const PORT_REGS_BASE: usize = 0x100;
const PORT_REG_STRIDE: usize = 0x80;
const PXCLB: usize = 0x00; // Command List Base Address (low 32 bits)
const PXCLBU: usize = 0x04; // Command List Base Address (upper 32 bits)
const PXFB: usize = 0x08; // FIS Base Address (low 32 bits)
const PXFBU: usize = 0x0C; // FIS Base Address (upper 32 bits)
const PXIS: usize = 0x10; // Interrupt Status (write-1-to-clear)
const PXCMD: usize = 0x18; // Command and Status
const PXTFD: usize = 0x20; // Task File Data (ATA status/error, shadow copy)
const PXSIG: usize = 0x24; // Signature (device type, once known)
const PXSSTS: usize = 0x28; // Serial ATA Status (device detection/Phy state)
const PXSCTL: usize = 0x2C; // Serial ATA Control (COMRESET via DET)
const PXSERR: usize = 0x30; // Serial ATA Error (write-1-to-clear)
const PXCI: usize = 0x38; // Command Issue (one bit per command slot)

/// `PxCMD` bit 0: Start -- the command-list DMA engine processes commands
/// from slot 0 upward only while this is set.
const PXCMD_ST: u32 = 1 << 0;
/// `PxCMD` bit 1: Spin-Up Device -- required on hardware that implements
/// staggered spin-up (`CAP.SSS`); read-only '1' (a harmless no-op to write)
/// on hardware that doesn't (AHCI 1.3.1, 3.3.7).
const PXCMD_SUD: u32 = 1 << 1;
/// `PxCMD` bit 2: Power On Device -- same "harmless no-op when
/// inapplicable" shape as `PXCMD_SUD`, for hardware with a mechanical
/// presence switch.
const PXCMD_POD: u32 = 1 << 2;
/// `PxCMD` bit 4: FIS Receive Enable -- must be set, with `PxCMD.CR`
/// already '0', before `PXCMD_ST` may be set (AHCI 1.3.1, 10.3.1).
const PXCMD_FRE: u32 = 1 << 4;
/// `PxCMD` bit 14: FIS Receive Running (read-only) -- hardware's
/// confirmation that FIS reception is actually active.
const PXCMD_FR: u32 = 1 << 14;
/// `PxCMD` bit 15: Command List Running (read-only) -- hardware's
/// confirmation that the command-list DMA engine is actually active.
const PXCMD_CR: u32 = 1 << 15;

/// `PxSIG` value a plain SATA disk reports after its initial D2H Register
/// FIS (AHCI 1.3.1, 3.3.9) -- the only device type this single-outstanding-
/// command, no-ATAPI driver knows how to talk to. A port reporting anything
/// else (an ATAPI optical drive, a port multiplier, an enclosure management
/// bridge) is treated as "no usable device," the same as no device at all.
const SIG_ATA: u32 = 0x0000_0101;

/// AHCI 1.3.1, 10.4.3 bounds `GHC.HR` clearing to within 1 second; at this
/// kernel's 100 Hz tick rate (`clock::TICK_HZ`) that's 100 ticks.
const HBA_RESET_TIMEOUT_TICKS: u64 = 100;
/// Generous bound (500 ms) for `PxCMD.CR`/`PxCMD.FR` to reflect a
/// start/stop request -- real hardware settles almost immediately; this
/// exists so a genuinely wedged controller produces a reported error
/// instead of a silent hang.
const PORT_ENGINE_TIMEOUT_TICKS: u64 = 50;
/// Bound for `PxSSTS.DET` to report a device present after spin-up. Real
/// link training can legitimately take a few hundred ms on some hardware;
/// 1 second leaves comfortable margin without risking a real hang on a
/// truly-absent device.
const DEVICE_DETECT_TIMEOUT_TICKS: u64 = 100;
/// Bound for a single IDENTIFY/read/write command (`PxCI` bit 0) to clear.
/// One sector (or one IDENTIFY page) is a few microseconds of real disk
/// time even on slow media; 1 second is generous headroom so a genuinely
/// wedged command produces a reported error instead of an infinite spin.
const COMMAND_TIMEOUT_TICKS: u64 = 100;
/// Bound for FLUSH CACHE (EXT). Unlike a single-sector transfer, a flush
/// may have to write back the device's entire volatile cache, which ATA
/// allows to take many seconds; 30 seconds is generous without being an
/// effective hang.
const FLUSH_TIMEOUT_TICKS: u64 = 3000;

/// This driver's fixed logical sector size. QEMU's `ide-hd` backing a raw
/// disk image (this kernel's only save-disk transport, see the module doc
/// comment) always presents plain 512-byte logical sectors; [`Hba::identify`]
/// explicitly confirms this against the real device rather than assuming
/// it, and reports an error instead of silently mis-sizing every future
/// read/write if it's ever wrong.
pub const SECTOR_SIZE: u32 = 512;

/// ATA8-ACS command byte for IDENTIFY DEVICE (7.9).
const ATA_CMD_IDENTIFY_DEVICE: u8 = 0xEC;
/// ATA8-ACS command byte for READ DMA EXT (LBA48, 7.24).
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
/// ATA8-ACS command byte for WRITE DMA EXT (LBA48, 7.63).
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
/// ATA8-ACS command byte for FLUSH CACHE EXT (7.11).
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
/// ATA8-ACS command byte for FLUSH CACHE (7.10), the 28-bit fallback.
const ATA_CMD_FLUSH_CACHE: u8 = 0xE7;

/// `PxTFD.STS` bits (ATA Status register shadow).
const TFD_STS_ERR: u32 = 1 << 0;
const TFD_STS_DRQ: u32 = 1 << 3;
const TFD_STS_BSY: u32 = 1 << 7;

fn port_offset(port: u8) -> usize {
    PORT_REGS_BASE + (port as usize) * PORT_REG_STRIDE
}

unsafe fn read_reg32(abar: *mut u8, offset: usize) -> u32 {
    // SAFETY: caller guarantees `abar` points at a valid, MMIO-mapped ABAR
    // and `offset` falls within `ABAR_MAP_SIZE`. Volatile: this is a real
    // device register, not ordinary memory -- the compiler must not
    // reorder, cache, or elide this read.
    unsafe { core::ptr::read_volatile(abar.add(offset) as *const u32) }
}

unsafe fn write_reg32(abar: *mut u8, offset: usize, value: u32) {
    // SAFETY: same reasoning as `read_reg32`.
    unsafe { core::ptr::write_volatile(abar.add(offset) as *mut u32, value) }
}

/// Whether the one port this driver owns may still be handed commands.
///
/// There is deliberately no "in flight" state visible outside
/// [`Hba::issue`]: every command is submitted and either retired or
/// recovered from before `issue` returns, so between calls the port is
/// always either idle-and-usable or permanently out of service.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PortState {
    /// Idle: no command outstanding, DMA structures free to rewrite.
    Ready,
    /// A command failed and the port could not be proven quiescent
    /// afterwards (AHCI 1.3.1, 6.2.2). The hardware may still own the
    /// command list, command table, and data buffer, so they are never
    /// touched again and the frames are never freed -- every later
    /// request fails immediately without any MMIO or DMA-memory write.
    Failed,
}

/// A live handle to one AHCI controller's memory-mapped registers, once
/// [`init`] has found a controller and mapped its ABAR and brought up a
/// port with a usable SATA disk attached. [`Hba::identify`]/[`Hba::read_sector`]/
/// [`Hba::write_sector`]/[`Hba::flush_cache`] issue commands through it.
pub struct Hba {
    /// Base of the mapped ABAR (identity-mapped: this is both the virtual
    /// and physical base address, matching `paging.rs`'s whole-kernel
    /// identity-map policy).
    abar: *mut u8,
    /// Index (0-31) of the one port this driver brought up.
    port: u8,
    /// Physical address of that port's Command List (32 entries, slot 0
    /// the only one ever populated).
    cmd_list_phys: u64,
    /// Physical address of that port's FIS Receive area. AHCI requires a
    /// valid FIS Receive area to be configured (`PxFB`) for the HBA to
    /// update `PxTFD`/`PxSIG` at all -- but this driver reads command
    /// completion status from `PxTFD`'s shadow copy rather than decoding
    /// the received FIS bytes itself, so this field's own contents are
    /// never read back through it; kept only so the frame is remembered
    /// (and, in principle, freeable) rather than an address this struct
    /// silently forgets it owns.
    #[allow(dead_code)] // by design never dereferenced -- see field doc comment above.
    fis_phys: u64,
    /// Physical address of the single Command Table slot 0's header points
    /// at -- where a command's FIS and PRDT get written before each command
    /// issue.
    cmd_table_phys: u64,
    /// Physical address of a single reusable 4 KiB scratch DMA buffer --
    /// this driver's one command slot only ever has one command in flight,
    /// so one buffer, reused by IDENTIFY/read/write in turn, is all it
    /// needs (matches the "single-outstanding-command, no NCQ" scope this
    /// module's doc comment already commits to).
    data_buf_phys: u64,
    /// See [`PortState`]. A `Cell` because commands are issued through
    /// `&self` (the shell and the WISP host both hold shared borrows).
    state: Cell<PortState>,
    /// What IDENTIFY DEVICE reported, once [`Hba::identify`] has run.
    /// [`Hba::flush_cache`] needs it to choose a supported flush command.
    info: Option<DiskInfo>,
}

/// Everything [`bring_up_first_available_port`] learns about the one port
/// this driver uses, on its way into the returned [`Hba`].
struct PortHandle {
    index: u8,
    cmd_list_phys: u64,
    fis_phys: u64,
    cmd_table_phys: u64,
    data_buf_phys: u64,
}

/// Zero an entire allocated frame. `pmm::alloc_frame` hands back raw,
/// possibly non-zero physical memory (unlike `paging.rs`'s own page-table
/// allocator, which zeroes on the caller's behalf) -- every AHCI structure
/// (command list, FIS receive area, command table) must start zeroed, since
/// stale bytes in a reserved field are exactly the kind of thing a real HBA
/// is entitled to treat as ill-formed.
///
/// # Safety
/// `phys` must be a just-allocated, exclusively-owned, `PAGE_SIZE`-aligned
/// frame (identity-mapped, so writable at that same address).
unsafe fn zero_frame(phys: u64) {
    unsafe {
        core::ptr::write_bytes(phys as *mut u8, 0, paging::PAGE_SIZE as usize);
    }
}


/// Global HBA reset (AHCI 1.3.1, 10.4.3): puts every port and the whole
/// controller back to a known, idle state regardless of what firmware or an
/// earlier boot left behind -- rather than assuming a freshly-found
/// controller is already idle. Clears `GHC.AE` as a documented side effect,
/// so this re-sets it before returning.
///
/// # Safety
/// `abar` must be a valid, mapped ABAR.
unsafe fn hba_reset(abar: *mut u8) -> Result<(), &'static str> {
    unsafe {
        let ghc = read_reg32(abar, REG_GHC);
        compiler_fence(Ordering::SeqCst);
        write_reg32(abar, REG_GHC, ghc | GHC_HR);
        compiler_fence(Ordering::SeqCst);
    }
    let deadline = clock::deadline_after_ticks(HBA_RESET_TIMEOUT_TICKS);
    // SAFETY: `abar` is valid and mapped per this function's own contract.
    let cleared = poll_until(deadline, || unsafe { read_reg32(abar, REG_GHC) } & GHC_HR == 0);
    if !cleared {
        return Err("ahci: HBA reset (GHC.HR) did not complete within 1 second -- controller unrecoverable");
    }
    unsafe {
        compiler_fence(Ordering::SeqCst);
        write_reg32(abar, REG_GHC, GHC_AE);
        compiler_fence(Ordering::SeqCst);
    }
    Ok(())
}

/// Stop a port's command-list and FIS-receive DMA engines and wait for both
/// to confirm idle -- the documented prerequisite (AHCI 1.3.1, 10.3.2)
/// before touching `PxCLB`/`PxFB` or (re-)starting the port. Right after a
/// full [`hba_reset`] every port should already be idle, but this is called
/// unconditionally rather than assumed.
///
/// # Safety
/// `abar` must be a valid, mapped, AHCI-mode ABAR; `port` must be an
/// implemented port index (0-31).
unsafe fn stop_port(abar: *mut u8, port: u8) -> Result<(), &'static str> {
    let base = port_offset(port);
    unsafe {
        let cmd = read_reg32(abar, base + PXCMD);
        write_reg32(abar, base + PXCMD, cmd & !(PXCMD_ST | PXCMD_FRE));
    }
    let deadline = clock::deadline_after_ticks(PORT_ENGINE_TIMEOUT_TICKS);
    // SAFETY: same contract as this function's own.
    let idle = poll_until(deadline, || {
        (unsafe { read_reg32(abar, base + PXCMD) }) & (PXCMD_CR | PXCMD_FR) == 0
    });
    if !idle {
        return Err("ahci: port did not report its command/FIS engines idle after being stopped");
    }
    Ok(())
}

/// Bring up the first implemented port (per `pi`, the Ports Implemented
/// bitmask) that reports a plain SATA disk present and ready -- Build Order
/// Step 4 of the Phase 2 plan. Allocates and wires up that port's Command
/// List, FIS Receive area, and single-slot Command Table, then starts its
/// command-list/FIS-receive DMA engines.
///
/// A port with no device, or with a device that isn't a plain SATA disk
/// (`PxSIG` != [`SIG_ATA`] -- an ATAPI drive, a port multiplier, ...), is
/// skipped, not treated as fatal: the next implemented port (if any) is
/// tried instead. Returns `Err` only once every implemented port has been
/// tried and none qualified -- a first-class, expected outcome on hardware
/// with no second drive (see this module's doc comment), not a driver bug.
///
/// # Safety
/// `abar` must be a valid, mapped, AHCI-mode (`GHC.AE` set) ABAR.
unsafe fn bring_up_first_available_port(abar: *mut u8, pi: u32) -> Result<PortHandle, &'static str> {
    for port in 0..32u8 {
        if pi & (1 << port) == 0 {
            continue;
        }
        let base = port_offset(port);

        unsafe {
            let cmd = read_reg32(abar, base + PXCMD);
            write_reg32(abar, base + PXCMD, cmd | PXCMD_SUD | PXCMD_POD);
        }

        // AHCI 1.3.1, 10.10.1: after spin-up, poll PxSSTS.DET (bits 3:0)
        // for the device to report itself present with Phy communication
        // established (DET == 3) before assuming nothing is attached.
        let deadline = clock::deadline_after_ticks(DEVICE_DETECT_TIMEOUT_TICKS);
        let present = poll_until(deadline, || unsafe { read_reg32(abar, base + PXSSTS) } & 0xF == 3);
        if !present {
            let _ = debugcon::write_fmt(format_args!(
                "[ahci] port {port}: no device detected (PxSSTS.DET != 3), skipping\n",
            ));
            continue;
        }

        unsafe { stop_port(abar, port)? };

        let cmd_list_phys =
            pmm::alloc_frame().ok_or("ahci: out of physical frames allocating the command list")?;
        let fis_phys =
            pmm::alloc_frame().ok_or("ahci: out of physical frames allocating the FIS receive area")?;
        let cmd_table_phys =
            pmm::alloc_frame().ok_or("ahci: out of physical frames allocating the command table")?;
        let data_buf_phys =
            pmm::alloc_frame().ok_or("ahci: out of physical frames allocating the command data buffer")?;
        unsafe {
            zero_frame(cmd_list_phys);
            zero_frame(fis_phys);
            zero_frame(cmd_table_phys);
            zero_frame(data_buf_phys);
        }

        // Command header 0 (the only slot this single-outstanding-command
        // driver ever uses): DW2/DW3 (Command Table Base Address) point at
        // the one command table this port will ever use, set once here.
        // DW0 (CFL/PRDTL/...) and the table's own contents are filled in
        // per-command by whichever of IDENTIFY/read/write (Build Order
        // Steps 5-7) issues the next command -- nothing meaningful belongs
        // there yet.
        unsafe {
            let hdr = cmd_list_phys as *mut u32;
            hdr.add(2).write_volatile((cmd_table_phys & 0xFFFF_FFFF) as u32);
            hdr.add(3).write_volatile((cmd_table_phys >> 32) as u32);
        }

        unsafe {
            write_reg32(abar, base + PXCLB, (cmd_list_phys & 0xFFFF_FFFF) as u32);
            write_reg32(abar, base + PXCLBU, (cmd_list_phys >> 32) as u32);
            write_reg32(abar, base + PXFB, (fis_phys & 0xFFFF_FFFF) as u32);
            write_reg32(abar, base + PXFBU, (fis_phys >> 32) as u32);
            // PxSERR is write-1-to-clear (AHCI 1.3.1, 3.3.12): start this
            // port from a known-clean error state rather than whatever
            // spin-up/reset happened to leave behind.
            write_reg32(abar, base + PXSERR, 0xFFFF_FFFF);
        }

        // AHCI 1.3.1, 10.3.1: FIS Receive Enable must be set, with
        // PxCMD.CR already '0' (guaranteed by `stop_port` above), before
        // PxCMD.ST may be set.
        unsafe {
            let cmd = read_reg32(abar, base + PXCMD);
            write_reg32(abar, base + PXCMD, cmd | PXCMD_FRE);
            let cmd = read_reg32(abar, base + PXCMD);
            write_reg32(abar, base + PXCMD, cmd | PXCMD_ST);
        }

        let deadline = clock::deadline_after_ticks(PORT_ENGINE_TIMEOUT_TICKS);
        let running = poll_until(deadline, || unsafe { read_reg32(abar, base + PXCMD) } & PXCMD_CR != 0);
        if !running {
            return Err("ahci: port command engine did not report running (PxCMD.CR) after start");
        }

        let (cmd, ssts, sig) = unsafe {
            (
                read_reg32(abar, base + PXCMD),
                read_reg32(abar, base + PXSSTS),
                read_reg32(abar, base + PXSIG),
            )
        };
        let _ = debugcon::write_fmt(format_args!(
            "[ahci] port {port}: started -- cmd=0x{cmd:08x} ssts=0x{ssts:08x} sig=0x{sig:08x}\n",
        ));

        if sig != SIG_ATA {
            let _ = debugcon::write_fmt(format_args!(
                "[ahci] port {port}: unsupported device signature 0x{sig:08x} (expected ATA 0x{SIG_ATA:08x}), skipping\n",
            ));
            unsafe { stop_port(abar, port)? };
            pmm::free_frame(cmd_list_phys);
            pmm::free_frame(fis_phys);
            pmm::free_frame(cmd_table_phys);
            pmm::free_frame(data_buf_phys);
            continue;
        }

        return Ok(PortHandle {
            index: port,
            cmd_list_phys,
            fis_phys,
            cmd_table_phys,
            data_buf_phys,
        });
    }

    debugcon::write_str("[ahci] no implemented port reported a usable SATA disk -- persistent storage unavailable\n");
    Err("no SATA disk detected on any implemented AHCI port")
}

/// Find the AHCI controller on PCI bus 0, enable Bus Master + Memory Space
/// on it, map its ABAR (BAR5), and confirm AHCI mode is enabled -- Build
/// Order Step 3 (ABAR discovery) of the Phase 2 plan. Logs the HBA's
/// generic registers (CAP/GHC/PI/VS) to debugcon for verification.
///
/// Returns `Err` (never panics) if no AHCI controller is present, or if
/// BAR5 doesn't have the shape AHCI's spec guarantees (a 32-bit,
/// non-prefetchable memory BAR) -- both are first-class, expected outcomes
/// on hardware with no second drive, not this kernel's problem to treat as
/// fatal. Callers (`main.rs`) must report this cleanly and continue
/// booting; `save`/`load` then report "unavailable" rather than the kernel
/// refusing to start.
///
/// # Safety
/// Must be called after `paging::load` (this function calls
/// `paging::map`, which requires the new page tables to already be
/// active), and at most once (each call maps a fresh ABAR window; calling
/// twice would map it twice, wasting page-table entries harmlessly rather
/// than corrupting anything, but there's no reason to).
pub unsafe fn init() -> Result<Hba, &'static str> {
    let devices = pci::enumerate_bus0();
    let Some(controller) = pci::find_ahci_controller(&devices) else {
        debugcon::write_str(
            "[ahci] no AHCI controller found on PCI bus 0 -- persistent storage unavailable\n",
        );
        return Err("no AHCI controller found");
    };

    let _ = debugcon::write_fmt(format_args!(
        "[ahci] controller found at {:02x}:{:02x}.{:x} (vendor:device={:04x}:{:04x})\n",
        controller.address.bus,
        controller.address.device,
        controller.address.function,
        controller.vendor_id,
        controller.device_id,
    ));

    // Firmware may or may not have already done this; set it unconditionally.
    pci::enable_bus_master_and_memory_space(controller.address);

    let bar5 = pci::read_bar(controller.address, 5);
    // AHCI 1.3.1, 2.1.11: ABAR is architecturally a 32-bit, non-prefetchable
    // memory BAR. Bit 0 clear = memory space (not I/O space); bits 2:1 = 00
    // means 32-bit (not a 64-bit BAR pair). A controller violating either
    // is not a spec-conformant AHCI controller this driver knows how to
    // talk to.
    if bar5 & 0x1 != 0 {
        return Err("ahci: BAR5 is an I/O-space BAR, not memory -- not a conformant AHCI controller");
    }
    let bar_type = (bar5 >> 1) & 0x3;
    if bar_type != 0 {
        return Err("ahci: BAR5 is not a 32-bit memory BAR -- not a conformant AHCI controller");
    }
    let abar_phys = (bar5 & 0xFFFF_FFF0) as u64;
    if abar_phys == 0 {
        return Err("ahci: BAR5 reads back as 0 -- controller not assigned an address by firmware");
    }

    // SAFETY: `abar_phys` is a real, spec-conformant memory BAR just read
    // from the controller's own configuration space; mapping it MMIO
    // (PCD) is exactly what `paging.rs`'s `PageFlags::Mmio` is for.
    // Identity mapping (virt == phys) matches this kernel's whole-image
    // policy -- see `paging.rs`'s module doc comment.
    unsafe { paging::map(abar_phys, abar_phys, ABAR_MAP_SIZE, paging::PageFlags::Mmio)? };
    let abar = abar_phys as *mut u8;

    // Full HBA reset (AHCI 1.3.1, 10.4.3) for a clean, known state --
    // regardless of what firmware (or, on real hardware, a previous boot)
    // left the controller in. This also (re-)sets GHC.AE, since a reset
    // clears it.
    //
    // No BIOS/OS Handoff (the BOHC register, AHCI 1.3.1, 3.1.11): like
    // every other driver here, this assumes the kernel already owns the
    // hardware once `exit_boot_services` has returned -- the same scope
    // reduction as `pci.rs`'s "no bridge recursion."
    unsafe { hba_reset(abar)? };

    // SAFETY: `abar` is mapped, present, and (as of `hba_reset`) in AHCI
    // mode -- every generic register is now well-defined to read.
    let (cap, ghc, pi, vs) = unsafe {
        (
            read_reg32(abar, REG_CAP),
            read_reg32(abar, REG_GHC),
            read_reg32(abar, REG_PI),
            read_reg32(abar, REG_VS),
        )
    };
    let _ = debugcon::write_fmt(format_args!(
        "[ahci] abar=0x{abar_phys:x} cap=0x{cap:08x} ghc=0x{ghc:08x} pi=0x{pi:08x} vs=0x{vs:08x}\n",
    ));
    if ghc & GHC_AE == 0 {
        return Err("ahci: GHC.AE did not stay set after HBA reset -- controller not responding as expected");
    }
    if pi == 0 {
        debugcon::write_str("[ahci] controller reports zero implemented ports -- persistent storage unavailable\n");
        return Err("ahci: controller reports zero implemented ports (PxI == 0)");
    }

    // SAFETY: `abar` is mapped and in AHCI mode; `pi` was just read from
    // this same controller.
    let port = unsafe { bring_up_first_available_port(abar, pi)? };

    Ok(Hba {
        abar,
        port: port.index,
        cmd_list_phys: port.cmd_list_phys,
        fis_phys: port.fis_phys,
        cmd_table_phys: port.cmd_table_phys,
        data_buf_phys: port.data_buf_phys,
        state: Cell::new(PortState::Ready),
        info: None,
    })
}

/// Why a command failed, before [`Hba::issue`] turns it into a message.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandFailure {
    /// `PxCI` never cleared: the HBA may still own the command.
    Timeout,
    /// The command retired with `PxTFD.STS.ERR` set (a task-file error,
    /// which AHCI 1.3.1, 6.2.2 also treats as needing port recovery).
    DeviceError,
}

/// Decode an ATA IDENTIFY string field (`words` consecutive words, each
/// holding two characters high byte first -- ATA8-ACS 3.2.8) into `out`.
fn identify_string(words: &[u16], out: &mut [u8]) {
    for (i, w) in words.iter().enumerate() {
        out[i * 2] = (w >> 8) as u8;
        out[i * 2 + 1] = (w & 0xFF) as u8;
    }
    for b in out.iter_mut() {
        if !(0x20..0x7F).contains(b) {
            *b = b'?';
        }
    }
}

/// What [`Hba::identify`] learns about the attached disk.
#[derive(Clone, Copy)]
pub struct DiskInfo {
    /// Total addressable sectors (LBA48), each [`SECTOR_SIZE`] bytes.
    pub total_sectors: u64,
    /// IDENTIFY words 27-46, space padded ASCII.
    model: [u8; 40],
    /// IDENTIFY words 10-19, space padded ASCII.
    serial: [u8; 20],
    /// The flush command this device supports, if any (FLUSH CACHE EXT
    /// preferred over the 28-bit FLUSH CACHE).
    flush_command: Option<u8>,
    /// Whether IDENTIFY affirmatively reports the volatile write cache as
    /// disabled. Only then is the absence of a flush command acceptable.
    write_cache_disabled: bool,
}

impl DiskInfo {
    /// The device's model string, trailing padding removed.
    pub fn model(&self) -> &str {
        trimmed_ascii(&self.model)
    }

    /// The device's serial number, padding removed.
    pub fn serial(&self) -> &str {
        trimmed_ascii(&self.serial)
    }
}

fn trimmed_ascii(bytes: &[u8]) -> &str {
    // `identify_string` replaced every byte outside printable ASCII.
    core::str::from_utf8(bytes).unwrap_or("?").trim()
}

impl Hba {
    /// Port index this driver owns, for identifying the device to a user.
    pub fn port(&self) -> u8 {
        self.port
    }

    /// The IDENTIFY result, once [`Hba::identify`] has succeeded.
    pub fn info(&self) -> Option<&DiskInfo> {
        self.info.as_ref()
    }

    /// True once the port has been taken out of service after an
    /// unrecoverable command failure.
    pub fn is_failed(&self) -> bool {
        self.state.get() == PortState::Failed
    }

    /// Build a Register H2D FIS (SATA spec, FIS type 0x27, 20 bytes / 5
    /// DWORDs) for `command` directly into the command table's Command FIS
    /// area (AHCI 1.3.1, 5.3.6.1: offset 0x00), issue it through command
    /// slot 0 (the only slot this driver ever uses), and poll for
    /// completion with a bounded timeout.
    ///
    /// `data_len` describes an optional transfer through this port's
    /// single scratch buffer (one PRDT entry, AHCI 1.3.1, 5.3.6.1: PRDT
    /// starts at command-table offset 0x80) -- pass `0` for a command with
    /// no data phase. `write` selects transfer direction: `true` =
    /// host-to-device (a disk write), `false` = device-to-host (a disk
    /// read, or IDENTIFY DEVICE).
    ///
    /// On any failure the port is recovered before this returns (see
    /// [`Hba::recover`]), so a returned error never leaves the HBA owning
    /// the command table or data buffer while a later call rewrites them.
    /// If recovery itself fails the port becomes [`PortState::Failed`]
    /// and every later call fails here, before touching anything.
    ///
    /// # Safety
    /// `self` must have been constructed by [`init`] (a live, running
    /// port whose `PxCLB`/command-header-0 CTBA point at this struct's
    /// frames).
    unsafe fn issue(
        &self,
        command: u8,
        lba: u64,
        sector_count: u16,
        data_len: u32,
        write: bool,
        timeout_ticks: u64,
    ) -> Result<(), &'static str> {
        if self.state.get() == PortState::Failed {
            return Err("ahci: save disk port is offline after an unrecoverable error (reboot to retry)");
        }
        debug_assert!(
            data_len as u64 <= paging::PAGE_SIZE,
            "Hba::issue: this driver's single PRDT entry never exceeds one scratch frame"
        );

        let base = port_offset(self.port);

        // SAFETY: `cmd_table_phys` names this port's command table, whose
        // Command FIS area (offset 0x00, 64 bytes reserved) is exclusively
        // owned by this driver while the port is `Ready` (no command
        // outstanding -- see `PortState`).
        unsafe {
            let cfis = self.cmd_table_phys as *mut u8;
            core::ptr::write_bytes(cfis, 0, 20);
            cfis.write_volatile(0x27); // FIS type: Register H2D.
            cfis.add(1).write_volatile(0x80); // bit 7 (C): this is a command update, not a control one.
            cfis.add(2).write_volatile(command);
            cfis.add(4).write_volatile((lba & 0xFF) as u8);
            cfis.add(5).write_volatile(((lba >> 8) & 0xFF) as u8);
            cfis.add(6).write_volatile(((lba >> 16) & 0xFF) as u8);
            cfis.add(7).write_volatile(0x40); // Device register: LBA mode.
            cfis.add(8).write_volatile(((lba >> 24) & 0xFF) as u8);
            cfis.add(9).write_volatile(((lba >> 32) & 0xFF) as u8);
            cfis.add(10).write_volatile(((lba >> 40) & 0xFF) as u8);
            cfis.add(12).write_volatile((sector_count & 0xFF) as u8);
            cfis.add(13).write_volatile(((sector_count >> 8) & 0xFF) as u8);
        }

        if data_len != 0 {
            // SAFETY: command-table offset 0x80 is this port's (only)
            // PRDT; `data_buf_phys` is this port's own scratch frame.
            unsafe {
                let prdt = (self.cmd_table_phys + 0x80) as *mut u32;
                prdt.write_volatile((self.data_buf_phys & 0xFFFF_FFFF) as u32);
                prdt.add(1).write_volatile((self.data_buf_phys >> 32) as u32);
                prdt.add(2).write_volatile(0);
                // DBC (Data Byte Count) is zero-based (AHCI 1.3.1, 5.3.9);
                // bit 31 (Interrupt on Completion) stays clear -- this
                // driver polls PxCI, it never waits on the interrupt.
                prdt.add(3).write_volatile((data_len - 1) & 0x3F_FFFF);
            }
        }

        // Command header 0: CFL = 5 DWORDs (the 20-byte Register H2D FIS
        // just written), W = transfer direction, PRDTL = 0 or 1 depending
        // on whether this command has a data phase, PRDBC reset to 0.
        let prdtl: u32 = u32::from(data_len != 0);
        let w_bit: u32 = if write { 1 << 6 } else { 0 };
        // SAFETY: header 0 of this port's command list, owned the same way
        // the command table is.
        unsafe {
            let hdr = self.cmd_list_phys as *mut u32;
            hdr.write_volatile(5 | w_bit | (prdtl << 16));
            hdr.add(1).write_volatile(0); // PRDBC -- reset before issue.
        }

        // Every descriptor write above must be visible before the
        // doorbell hands them to the HBA.
        compiler_fence(Ordering::SeqCst);
        // Ring the doorbell: setting PxCI bit 0 (slot 0) hands the command
        // to the HBA. From here until it clears (or `recover` stops the
        // command engine) the HBA owns every structure written above.
        unsafe { write_reg32(self.abar, base + PXCI, 1) };

        let deadline = clock::deadline_after_ticks(timeout_ticks);
        // SAFETY: `abar` is valid and mapped per this function's contract.
        let completed = poll_until(deadline, || unsafe { read_reg32(self.abar, base + PXCI) } & 1 == 0);
        compiler_fence(Ordering::SeqCst);

        // AHCI 1.3.1, 3.3.10: PxTFD is a live shadow of the ATA
        // Status/Error registers from the most recent D2H/PIO-Setup FIS --
        // bit 0 of the Status byte is ERR, set by the device when it
        // failed the command.
        let failure = if !completed {
            Some(CommandFailure::Timeout)
        } else if unsafe { read_reg32(self.abar, base + PXTFD) } & TFD_STS_ERR != 0 {
            Some(CommandFailure::DeviceError)
        } else {
            None
        };
        let Some(failure) = failure else {
            return Ok(());
        };

        // SAFETY: same contract as this function's own.
        let recovered = unsafe { self.recover() };
        let _ = debugcon::write_fmt(format_args!(
            "[ahci] command 0x{command:02x} lba={lba} failed ({}); port recovery {}\n",
            if failure == CommandFailure::Timeout { "timeout" } else { "device error" },
            if recovered { "succeeded" } else { "FAILED -- port taken offline" },
        ));
        Err(match (failure, recovered) {
            (CommandFailure::Timeout, true) => {
                "ahci: command timed out (port recovered; the command's effect on the disk is unknown)"
            }
            (CommandFailure::DeviceError, true) => {
                "ahci: device reported an error completing the command (PxTFD.STS.ERR set)"
            }
            (_, false) => {
                "ahci: command failed and the port could not be recovered -- save disk taken offline"
            }
        })
    }

    /// AHCI 1.3.1, 6.2.2.1 software error recovery for this driver's one
    /// port: stop the command engine and *verify* it stopped (only then
    /// has the HBA released the command list, command table, and data
    /// buffer), clear error status, COMRESET the link if the device is
    /// still busy, and restart the engine.
    ///
    /// Returns `true` if the port is idle and usable again. On `false`
    /// the port is marked [`PortState::Failed`]; its DMA frames are left
    /// allocated and untouched for the rest of the boot, because the HBA
    /// may still be using them.
    ///
    /// # Safety
    /// Same contract as [`Hba::issue`].
    unsafe fn recover(&self) -> bool {
        let ok = unsafe { self.try_recover() };
        if !ok {
            self.state.set(PortState::Failed);
        }
        ok
    }

    unsafe fn try_recover(&self) -> bool {
        let abar = self.abar;
        let base = port_offset(self.port);

        // 1. Clear PxCMD.ST and wait for PxCMD.CR to drop. Clearing ST
        //    also clears PxCI (AHCI 1.3.1, 3.3.14), retiring slot 0.
        unsafe {
            let cmd = read_reg32(abar, base + PXCMD);
            write_reg32(abar, base + PXCMD, cmd & !PXCMD_ST);
        }
        let deadline = clock::deadline_after_ticks(PORT_ENGINE_TIMEOUT_TICKS);
        if !poll_until(deadline, || unsafe { read_reg32(abar, base + PXCMD) } & PXCMD_CR == 0) {
            return false;
        }
        if unsafe { read_reg32(abar, base + PXCI) } != 0 {
            return false;
        }

        // 2. Clear latched error and interrupt status (both RW1C).
        unsafe {
            write_reg32(abar, base + PXSERR, 0xFFFF_FFFF);
            write_reg32(abar, base + PXIS, 0xFFFF_FFFF);
        }

        // 3. A device still showing BSY or DRQ will not accept another
        //    command; only a COMRESET (PxSCTL.DET = 1, then 0) clears
        //    that (AHCI 1.3.1, 10.4.2). ST is already clear, as required.
        let tfd = unsafe { read_reg32(abar, base + PXTFD) };
        if tfd & (TFD_STS_BSY | TFD_STS_DRQ) != 0 {
            unsafe {
                let sctl = read_reg32(abar, base + PXSCTL);
                write_reg32(abar, base + PXSCTL, (sctl & !0xF) | 1);
            }
            // DET=1 must be held for at least 1 ms; one tick is 10 ms.
            let hold = clock::deadline_after_ticks(1);
            let _ = poll_until(hold, || false);
            unsafe {
                let sctl = read_reg32(abar, base + PXSCTL);
                write_reg32(abar, base + PXSCTL, sctl & !0xF);
            }
            let deadline = clock::deadline_after_ticks(DEVICE_DETECT_TIMEOUT_TICKS);
            if !poll_until(deadline, || unsafe { read_reg32(abar, base + PXSSTS) } & 0xF == 3) {
                return false;
            }
            let deadline = clock::deadline_after_ticks(DEVICE_DETECT_TIMEOUT_TICKS);
            if !poll_until(deadline, || {
                (unsafe { read_reg32(abar, base + PXTFD) }) & (TFD_STS_BSY | TFD_STS_DRQ) == 0
            }) {
                return false;
            }
            unsafe { write_reg32(abar, base + PXSERR, 0xFFFF_FFFF) };
        }

        // 4. Restart the command engine (FRE was never cleared).
        unsafe {
            let cmd = read_reg32(abar, base + PXCMD);
            write_reg32(abar, base + PXCMD, cmd | PXCMD_ST);
        }
        let deadline = clock::deadline_after_ticks(PORT_ENGINE_TIMEOUT_TICKS);
        poll_until(deadline, || unsafe { read_reg32(abar, base + PXCMD) } & PXCMD_CR != 0)
    }

    /// Issue IDENTIFY DEVICE (ATA8-ACS, 7.9) and decode the disk's total
    /// LBA48 sector count, identity strings, and cache/flush support --
    /// remembered on `self` for [`Hba::flush_cache`].
    ///
    /// Returns `Err` (never panics) if the device doesn't support LBA48
    /// addressing, or reports (via a validity-checked IDENTIFY word) a
    /// logical sector size other than [`SECTOR_SIZE`] -- both real,
    /// explicit scope limits of this driver, checked against the actual
    /// device rather than assumed.
    ///
    /// # Safety
    /// Must be called on an `Hba` returned by [`init`] (i.e. with a live,
    /// running port).
    pub unsafe fn identify(&mut self) -> Result<DiskInfo, &'static str> {
        // SAFETY: `self` was constructed by `init`; `data_buf_phys` is a
        // whole 4 KiB frame, comfortably large enough for IDENTIFY
        // DEVICE's fixed 512-byte response.
        unsafe { self.issue(ATA_CMD_IDENTIFY_DEVICE, 0, 0, SECTOR_SIZE, false, COMMAND_TIMEOUT_TICKS)? };

        let mut words = [0u16; 256];
        // SAFETY: `issue` returned `Ok`, so the HBA DMA'd exactly 512
        // bytes of IDENTIFY DEVICE data into `data_buf_phys` and has
        // retired the command; `words` is exactly 512 bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(self.data_buf_phys as *const u16, words.as_mut_ptr(), 256);
        }
        let word_valid = |w: u16| w & 0xC000 == 0x4000;

        // Word 83: "Command sets supported." Valid only when bits 15:14
        // read 0b01; bit 10 is "48-bit Address feature set supported."
        let word83 = words[83];
        if !word_valid(word83) || word83 & (1 << 10) == 0 {
            return Err("ahci: attached disk does not support LBA48 addressing -- unsupported by this driver");
        }

        // Word 106: "Physical/Logical Sector size." Same 0b01 validity
        // convention; bit 12 set means the logical sector is larger than
        // 256 words (512 bytes).
        let word106 = words[106];
        if word_valid(word106) && word106 & (1 << 12) != 0 {
            return Err("ahci: attached disk reports a logical sector size other than 512 bytes -- unsupported by this driver");
        }

        let total_sectors = u64::from(words[100])
            | (u64::from(words[101]) << 16)
            | (u64::from(words[102]) << 32)
            | (u64::from(words[103]) << 48);

        // Word 83 bit 13: FLUSH CACHE EXT supported; bit 12: FLUSH CACHE
        // supported. Words 82/85 bit 5: volatile write cache supported /
        // enabled, with word 85 valid only when word 87 reads 0b01.
        let flush_command = if word83 & (1 << 13) != 0 {
            Some(ATA_CMD_FLUSH_CACHE_EXT)
        } else if word83 & (1 << 12) != 0 {
            Some(ATA_CMD_FLUSH_CACHE)
        } else {
            None
        };
        let word82 = words[82];
        let word85_valid = word_valid(words[87]);
        let write_cache_disabled = word_valid(word83)
            && (word82 & (1 << 5) == 0 || (word85_valid && words[85] & (1 << 5) == 0));

        let mut model = [0u8; 40];
        identify_string(&words[27..47], &mut model);
        let mut serial = [0u8; 20];
        identify_string(&words[10..20], &mut serial);

        let info = DiskInfo {
            total_sectors,
            model,
            serial,
            flush_command,
            write_cache_disabled,
        };
        let total_bytes = total_sectors * u64::from(SECTOR_SIZE);
        let _ = debugcon::write_fmt(format_args!(
            "[ahci] identify: model='{}' serial='{}' total_sectors={total_sectors} ({SECTOR_SIZE}-byte sectors) \
             = {total_bytes} bytes flush=0x{:02x} write_cache_disabled={write_cache_disabled}\n",
            info.model(),
            info.serial(),
            flush_command.unwrap_or(0),
        ));
        self.info = Some(info);
        Ok(info)
    }

    /// Read exactly one [`SECTOR_SIZE`]-byte logical sector at LBA `lba`
    /// into `out` via READ DMA EXT (ATA8-ACS, 7.24).
    ///
    /// # Safety
    /// Same contract as [`Hba::identify`]. `lba` must be within the disk's
    /// addressable range -- an out-of-range LBA is a device-reported error
    /// (caught via `PxTFD.STS.ERR`, not undefined behavior), but is still
    /// the caller's responsibility to avoid.
    pub unsafe fn read_sector(&self, lba: u64, out: &mut [u8; SECTOR_SIZE as usize]) -> Result<(), &'static str> {
        // SAFETY: `self` was constructed by `init`.
        unsafe { self.issue(ATA_CMD_READ_DMA_EXT, lba, 1, SECTOR_SIZE, false, COMMAND_TIMEOUT_TICKS)? };
        // SAFETY: `issue` returned `Ok`, meaning the HBA DMA'd exactly
        // `SECTOR_SIZE` bytes into `data_buf_phys` and retired the
        // command; `out` is `SECTOR_SIZE` bytes by its own type, and a
        // stack array cannot overlap the separately-allocated DMA frame.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.data_buf_phys as *const u8,
                out.as_mut_ptr(),
                SECTOR_SIZE as usize,
            );
        }
        Ok(())
    }

    /// Write exactly one [`SECTOR_SIZE`]-byte logical sector at LBA `lba`
    /// from `data` via WRITE DMA EXT (ATA8-ACS, 7.63).
    ///
    /// Completion means the device accepted the data, which may still be
    /// only in its volatile write cache -- call [`Hba::flush_cache`] at a
    /// commit point before reporting anything as durably saved.
    ///
    /// # Safety
    /// Same contract as [`Hba::read_sector`].
    pub unsafe fn write_sector(&self, lba: u64, data: &[u8; SECTOR_SIZE as usize]) -> Result<(), &'static str> {
        if self.state.get() == PortState::Failed {
            // Checked here as well as in `issue`: the scratch buffer may
            // still belong to the HBA, so it must not even be filled.
            return Err("ahci: save disk port is offline after an unrecoverable error (reboot to retry)");
        }
        // SAFETY: the port is `Ready`, so no command owns
        // `data_buf_phys`; filling it before a host-to-device command is
        // exactly what it is for.
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), self.data_buf_phys as *mut u8, SECTOR_SIZE as usize);
        }
        // SAFETY: `self` was constructed by `init`.
        unsafe { self.issue(ATA_CMD_WRITE_DMA_EXT, lba, 1, SECTOR_SIZE, true, COMMAND_TIMEOUT_TICKS) }
    }

    /// Make every write this device has completed so far durable, via
    /// FLUSH CACHE EXT (or FLUSH CACHE). This is the durability barrier
    /// `save_format.rs` places at each commit point.
    ///
    /// Returns `Err` -- rather than silently succeeding -- if the device
    /// supports neither flush command and IDENTIFY did not show its
    /// volatile write cache as disabled: there is then no way to know the
    /// data reached nonvolatile media.
    ///
    /// # Safety
    /// Same contract as [`Hba::read_sector`]; [`Hba::identify`] must have
    /// succeeded first.
    pub unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        let info = self.info.as_ref().ok_or("ahci: flush requested before IDENTIFY")?;
        match info.flush_command {
            // SAFETY: `self` was constructed by `init`.
            Some(cmd) => unsafe { self.issue(cmd, 0, 0, 0, false, FLUSH_TIMEOUT_TICKS) },
            None if info.write_cache_disabled => Ok(()),
            None => Err("ahci: device has a write cache but supports no FLUSH CACHE command -- durability cannot be guaranteed"),
        }
    }
}

/// Dump `sector` (expected to be exactly [`SECTOR_SIZE`] bytes) to debugcon
/// as a conventional 16-bytes-per-line hex listing -- the byte-for-byte
/// verification surface Build Order Step 6 of the Phase 2 plan calls for,
/// and generally useful "prove a raw read actually returned what's on
/// disk" boot diagnostics beyond that one step, in the same spirit as
/// `paging::init`'s unconditional guard-range logging.
pub fn debug_dump_sector(sector: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut line = [0u8; 16 * 3]; // "xx " per byte, 16 bytes per line.
    for (row, chunk) in sector.chunks(16).enumerate() {
        for (i, &b) in chunk.iter().enumerate() {
            line[i * 3] = HEX[(b >> 4) as usize];
            line[i * 3 + 1] = HEX[(b & 0xF) as usize];
            line[i * 3 + 2] = b' ';
        }
        let line_len = chunk.len() * 3;
        // SAFETY: every byte written into `line` above came from the `HEX`
        // table or a literal space -- always valid ASCII, hence valid
        // UTF-8.
        let line_str = unsafe { core::str::from_utf8_unchecked(&line[..line_len]) };
        let offset = row * 16;
        let _ = debugcon::write_fmt(format_args!("[ahci] sector +0x{offset:03x}: {line_str}\n"));
    }
}
