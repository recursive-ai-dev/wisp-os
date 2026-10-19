//! xHCI (eXtensible Host Controller Interface) driver -- built from
//! scratch against the xHCI 1.2 specification, no borrowed code, in the
//! same fully-custom spirit as every other driver in this kernel.
//!
//! # Why this exists
//!
//! `keyboard.rs` talks to a PS/2 controller on ISA IRQ1. That works on
//! every machine QEMU emulates and on essentially no laptop built in the
//! last fifteen years: the PS/2 controller is gone, and the keyboard --
//! built-in or not -- is a USB HID device behind an xHCI host
//! controller. A kernel that can only read a PS/2 keyboard boots on real
//! hardware and then sits there, unable to accept a single keystroke.
//! This driver is what closes that gap, and it is the reason the whole
//! ACPI/APIC half of this phase had to land first: reaching a PCI
//! device's interrupts at all needs a real interrupt controller.
//!
//! # Scope, deliberately reduced
//!
//! - **Polled, not interrupt-driven.** The controller's event ring is
//!   drained from `keyboard::wait_for_key`'s idle loop, which already
//!   wakes on every timer tick (100 Hz), rather than from an interrupt
//!   handler. This is a real decision with a real reason, not laziness:
//!   a legacy PCI INTx interrupt's routing lives in ACPI's `_PRT`
//!   object, which is AML -- and this kernel has no AML interpreter (see
//!   `acpi.rs`'s scope notes), so that path is genuinely unavailable.
//!   MSI/MSI-X would sidestep AML entirely and is the right long-term
//!   answer, but it is a separate, independently-testable piece of work;
//!   bundling it into the same step as first bringing up xHCI at all
//!   would mean a failure could be either, with no way to tell which.
//!   The cost of polling here is bounded and tiny: at 100 Hz a keystroke
//!   is noticed within 10 ms, which is below the threshold anyone can
//!   perceive while typing, and the poll itself is a handful of MMIO
//!   reads on a ring that is almost always empty.
//! - **One device, one interface, one endpoint.** The first port
//!   reporting a connected device whose descriptors identify a HID
//!   boot-protocol keyboard is brought up and used; anything else
//!   (a mouse, a hub, a mass-storage device) is enumerated far enough to
//!   identify it, logged, and left alone. This kernel has no USB stack
//!   ambitions beyond "read the keyboard."
//! - **No hubs.** A device behind an external hub is not reachable. Root
//!   ports only.
//! - **No USB3 streams, no LTM, no bandwidth negotiation.** None of it
//!   is needed for a single interrupt-IN endpoint moving 8 bytes at a
//!   time.
//!
//! # Memory model
//!
//! Every structure the controller reads by DMA (the Device Context Base
//! Address Array, the command/event/transfer rings, the input and device
//! contexts, the transfer buffers) is one whole `pmm::alloc_frame` page,
//! zeroed, and referred to by its physical address. That is more memory
//! than each strictly needs, and it is deliberate: a page is guaranteed
//! 4-KiB aligned, which satisfies every alignment requirement xHCI
//! states (64-byte for contexts and the DCBAA, 16-byte for TRBs, 64 KiB
//! boundary-crossing rules for rings) without this driver having to
//! implement a second allocator to carve smaller aligned blocks out of
//! one. This kernel identity-maps all of physical memory (see
//! `paging.rs`), so a frame's physical address is directly usable as a
//! pointer here with no translation at all.

use alloc::vec::Vec;
use core::sync::atomic::{compiler_fence, Ordering};

use crate::clock::{self, poll_until};
use crate::{debugcon, paging, pci, pmm};

/// PCI classification for an xHCI host controller: class 0x0C (serial
/// bus controller), subclass 0x03 (USB), programming interface 0x30
/// (xHCI). Matched by classification rather than a hardcoded
/// vendor:device ID, exactly like `pci::find_ahci_controller`, so this
/// recognizes any spec-conformant controller and not just QEMU's.
const PCI_CLASS_SERIAL_BUS: u8 = 0x0C;
const PCI_SUBCLASS_USB: u8 = 0x03;
const PCI_PROGIF_XHCI: u8 = 0x30;

/// How much of the controller's MMIO window to map. The capability,
/// operational, runtime and doorbell register blocks are all reached at
/// offsets from one base; the spec places no hard bound on the total,
/// but a controller with the architectural maximum of 255 ports needs
/// under 16 KiB and every real one is far smaller. 64 KiB is generous
/// headroom that still costs only sixteen page-table entries.
const MMIO_MAP_SIZE: u64 = 16 * paging::PAGE_SIZE;

// --- Capability registers (xHCI 1.2, 5.3), at MMIO base + 0 -----------
const CAP_CAPLENGTH: usize = 0x00;
// (HCIVERSION lives in the upper half of the dword at CAP_CAPLENGTH and
// is read from there -- see `init`'s comment on the dword-read fix.)
const CAP_HCSPARAMS1: usize = 0x04;
const CAP_HCSPARAMS2: usize = 0x08;
const CAP_HCCPARAMS1: usize = 0x10;
const CAP_DBOFF: usize = 0x14;
const CAP_RTSOFF: usize = 0x18;

// --- Operational registers (xHCI 1.2, 5.4), at base + CAPLENGTH -------
const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_CRCR: usize = 0x18;
const OP_DCBAAP: usize = 0x30;
const OP_CONFIG: usize = 0x38;
/// Port register sets start here and are 0x10 bytes apart, one per root
/// port, numbered from 1.
const OP_PORT_BASE: usize = 0x400;
const OP_PORT_STRIDE: usize = 0x10;

/// `USBCMD` bit 0: Run/Stop.
const USBCMD_RUN: u32 = 1 << 0;
/// `USBCMD` bit 1: Host Controller Reset. Self-clears when the reset
/// completes.
const USBCMD_HCRST: u32 = 1 << 1;
/// `USBSTS` bit 0: HCHalted. Set while the controller is stopped.
const USBSTS_HCH: u32 = 1 << 0;
/// `USBSTS` bit 11: Controller Not Ready. Set while the controller is
/// still initializing after a reset (or power-on); every operational
/// register write before this clears is discarded, silently.
const USBSTS_CNR: u32 = 1 << 11;

/// `PORTSC` bit 0: Current Connect Status.
const PORTSC_CCS: u32 = 1 << 0;
/// `PORTSC` bit 1: Port Enabled/Disabled. Write-1-to-*clear* -- writing
/// a 1 here disables the port, which is why it must be masked out of
/// every read-modify-write.
const PORTSC_PED: u32 = 1 << 1;
/// `PORTSC` bit 4: Port Reset.
const PORTSC_PR: u32 = 1 << 4;
/// `PORTSC` bit 9: Port Power.
const PORTSC_PP: u32 = 1 << 9;
/// `PORTSC` bits 13:10: Port Speed, as an index into the controller's
/// speed ID table (values 1-4 are the standard Full/Low/High/Super).
const PORTSC_SPEED_SHIFT: u32 = 10;
/// Every write-1-to-clear status-change bit in `PORTSC` (bits 17-23),
/// plus `PED`. These must be masked out of any read-modify-write, or the
/// write silently clears change flags this driver hasn't looked at yet
/// -- or, for `PED`, disables the port outright.
const PORTSC_RW1C_MASK: u32 = PORTSC_PED | (0x7F << 17);

// --- Runtime registers (xHCI 1.2, 5.5), at base + RTSOFF --------------
/// Interrupter Register Sets start at runtime offset 0x20, 32 bytes
/// apart. This driver uses interrupter 0 only.
const RT_IR0: usize = 0x20;
const IR_IMAN: usize = 0x00;
const IR_ERSTSZ: usize = 0x08;
const IR_ERSTBA: usize = 0x10;
const IR_ERDP: usize = 0x18;

/// TRB type codes this driver produces or consumes (xHCI 1.2, 6.4.6).
const TRB_TYPE_NORMAL: u32 = 1;
const TRB_TYPE_SETUP_STAGE: u32 = 2;
const TRB_TYPE_DATA_STAGE: u32 = 3;
const TRB_TYPE_STATUS_STAGE: u32 = 4;
const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

/// TRB dword3 bit 0: Cycle. The producer toggles this every time it
/// wraps the ring; the consumer treats a TRB whose cycle bit differs
/// from its own Consumer Cycle State as "not yet written." This one bit
/// is the entire ring-ownership protocol.
const TRB_CYCLE: u32 = 1 << 0;
/// TRB dword3 bit 1: Toggle Cycle (on a Link TRB): the consumer inverts
/// its cycle state when it follows this link.
const TRB_TOGGLE_CYCLE: u32 = 1 << 1;
/// TRB dword3 bit 5: Interrupt On Completion -- generate a Transfer
/// Event when this TRB completes. Set on every transfer TRB this driver
/// needs to observe.
const TRB_IOC: u32 = 1 << 5;
/// TRB dword3 bit 6: Immediate Data -- the TRB's own first 8 bytes are
/// the payload rather than a pointer to it. Used for the Setup Stage
/// TRB, whose payload is exactly the 8-byte USB setup packet.
const TRB_IDT: u32 = 1 << 6;

/// Completion code 1: Success (xHCI 1.2, 6.4.5). Anything else from a
/// command or transfer is a real failure this driver reports rather
/// than proceeds past.
const COMPLETION_SUCCESS: u32 = 1;
/// Completion code 13: Short Packet -- the transfer completed with fewer
/// bytes than requested; the event's residual length says how many.
const COMPLETION_SHORT_PACKET: u32 = 13;
/// A HID boot-protocol keyboard input report is exactly 8 bytes
/// (HID 1.11, Appendix B.1).
const BOOT_REPORT_LEN: u32 = 8;

/// TRBs per ring. One page holds 256 16-byte TRBs; the last is reserved
/// for the Link TRB that closes the ring, leaving 255 usable.
const TRBS_PER_RING: usize = (paging::PAGE_SIZE as usize) / 16;

/// Bound on every wait loop in this driver, in `clock::ticks()` (100 Hz,
/// so this is one second). Same discipline as `ahci.rs`: no unbounded
/// spins anywhere, so a wedged or absent controller produces a reported
/// timeout instead of a silent hang -- the failure mode this project has
/// already been bitten by once (see `CHANGES.md`'s sti-ordering
/// postmortem) and now tests for explicitly.
const TIMEOUT_TICKS: u64 = 100;

/// USB standard request codes (USB 2.0 spec, 9.4).
const USB_REQ_GET_DESCRIPTOR: u8 = 6;
const USB_REQ_SET_CONFIGURATION: u8 = 9;
/// HID class request: SET_PROTOCOL (HID 1.11, 7.2.6).
const HID_REQ_SET_PROTOCOL: u8 = 0x0B;
/// HID protocol value 0: boot protocol -- the fixed 8-byte report format
/// `usb_hid.rs` decodes, defined precisely so a system with no HID
/// report-descriptor parser (this one) can still read a keyboard.
const HID_PROTOCOL_BOOT: u16 = 0;

const DESC_TYPE_DEVICE: u8 = 1;
const DESC_TYPE_CONFIGURATION: u8 = 2;
const DESC_TYPE_INTERFACE: u8 = 4;
const DESC_TYPE_ENDPOINT: u8 = 5;

/// USB interface class/subclass/protocol identifying a HID device using
/// the boot-protocol keyboard report format (HID 1.11, 4.3).
const USB_CLASS_HID: u8 = 3;
const HID_SUBCLASS_BOOT: u8 = 1;
const HID_PROTOCOL_KEYBOARD: u8 = 1;

/// Allocate one zeroed physical frame for a controller-visible
/// structure.
///
/// Zeroing is not optional: every xHCI context and ring is defined with
/// reserved fields that must be zero, and `pmm::alloc_frame` hands back
/// whatever bytes were previously there. The same reasoning (and the
/// same helper shape) as `ahci.rs`'s `zero_frame`.
fn alloc_zeroed_frame() -> Result<u64, &'static str> {
    let phys = pmm::alloc_frame().ok_or("xhci: out of physical frames")?;
    // SAFETY: just allocated, exclusively owned, page-aligned, and
    // identity-mapped (so writable at its own physical address).
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, paging::PAGE_SIZE as usize) };
    Ok(phys)
}

/// # Safety
/// `addr` must be a mapped MMIO address.
unsafe fn read32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// # Safety
/// Same contract as [`read32`].
unsafe fn write32(addr: u64, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

/// Write a 64-bit xHCI register as two 32-bit accesses, low half first.
///
/// Not a single 64-bit store: the xHCI spec explicitly permits a
/// controller to implement its 64-bit registers as two independent
/// 32-bit registers (1.2, 5.1), and several real ones do. Writing the
/// low half first is the documented order -- for the registers this
/// driver writes (`CRCR`, `DCBAAP`, `ERSTBA`, `ERDP`), the high half is
/// what the controller latches on, so it must see a complete low half
/// already in place.
///
/// # Safety
/// Same contract as [`read32`].
unsafe fn write64(addr: u64, value: u64) {
    unsafe {
        write32(addr, value as u32);
        compiler_fence(Ordering::SeqCst);
        write32(addr + 4, (value >> 32) as u32);
    }
}


/// A producer ring (command ring, or an endpoint's transfer ring): a
/// page of TRBs with a Link TRB in the last slot pointing back to the
/// start, plus this driver's enqueue pointer and cycle state.
/// `Copy` deliberately: a `ProducerRing` is three plain integers
/// describing a frame this driver owns, not the frame itself, so copying
/// it is exactly as cheap and exactly as meaningful as copying a
/// pointer. That is what lets `control_transfer` move the ring out of
/// `self` and back for the duration of an enqueue (see its comment)
/// without fighting the borrow checker or reaching for interior
/// mutability.
#[derive(Clone, Copy)]
struct ProducerRing {
    phys: u64,
    /// Index of the next TRB this driver will write.
    enqueue: usize,
    /// The cycle bit value this driver currently writes. Inverted every
    /// time the ring wraps, which is what tells the controller where
    /// freshly-produced TRBs end.
    cycle: u32,
}

impl ProducerRing {
    fn new() -> Result<Self, &'static str> {
        let phys = alloc_zeroed_frame()?;
        let ring = ProducerRing { phys, enqueue: 0, cycle: 1 };
        // The last slot is a Link TRB back to the ring's own start, with
        // Toggle Cycle set so the controller flips its consumer cycle
        // state when it follows the link -- that toggle is what keeps
        // producer and consumer agreeing about which TRBs are new after
        // a wrap.
        let link_index = TRBS_PER_RING - 1;
        // SAFETY: `phys` is a freshly allocated, zeroed, identity-mapped
        // frame this ring exclusively owns.
        unsafe {
            let trb = (phys as *mut u32).add(link_index * 4);
            trb.write_volatile(phys as u32);
            trb.add(1).write_volatile((phys >> 32) as u32);
            trb.add(2).write_volatile(0);
            trb.add(3)
                .write_volatile((TRB_TYPE_LINK << 10) | TRB_TOGGLE_CYCLE | ring.cycle);
        }
        Ok(ring)
    }

    /// Write one TRB and advance, handling the wrap (including flipping
    /// the Link TRB's own cycle bit so the controller will follow it).
    /// Returns the physical address the TRB was written at, which is
    /// what a Transfer/Command Completion Event reports back so a caller
    /// can match an event to the request that produced it.
    ///
    /// # Safety
    /// The controller must not be concurrently consuming the slot being
    /// written -- guaranteed here by this driver's one-request-at-a-time
    /// discipline (see [`Xhci::run_command`]).
    unsafe fn push(&mut self, d0: u32, d1: u32, d2: u32, d3: u32) -> u64 {
        let index = self.enqueue;
        let trb_phys = self.phys + (index * 16) as u64;
        unsafe {
            let trb = (self.phys as *mut u32).add(index * 4);
            // Dwords 0-2 first, dword 3 (which carries the cycle bit)
            // last: the cycle bit is what hands ownership of this TRB to
            // the controller, so every other field must already be in
            // place before it is written. Exactly the same "the
            // ownership-transferring write goes last" discipline as
            // `ioapic.rs`'s high-half-first redirection entry write.
            trb.write_volatile(d0);
            trb.add(1).write_volatile(d1);
            trb.add(2).write_volatile(d2);
            compiler_fence(Ordering::SeqCst);
            trb.add(3).write_volatile(d3 | self.cycle);
        }

        self.enqueue += 1;
        if self.enqueue == TRBS_PER_RING - 1 {
            // Hand the Link TRB over too, then wrap and invert our cycle.
            unsafe {
                let link = (self.phys as *mut u32).add((TRBS_PER_RING - 1) * 4);
                compiler_fence(Ordering::SeqCst);
                link.add(3)
                    .write_volatile((TRB_TYPE_LINK << 10) | TRB_TOGGLE_CYCLE | self.cycle);
            }
            self.enqueue = 0;
            self.cycle ^= 1;
        }
        trb_phys
    }
}

/// One event read off the event ring.
struct Event {
    trb_type: u32,
    completion_code: u32,
    /// Dword 0/1 of the event: for a Command Completion Event, the
    /// physical address of the command TRB; for a Transfer Event, the
    /// address of the transfer TRB.
    pointer: u64,
    /// Dword 2's low 24 bits: for a Transfer Event, the residual
    /// untransferred byte count.
    transfer_length: u32,
    /// Dword 3 bits 20:16: for a Transfer Event, the Device Context
    /// Index of the endpoint that generated it.
    endpoint_id: u8,
    /// Dword 3 bits 31:24: the slot ID this event belongs to (or, for
    /// Enable Slot, the slot just assigned).
    slot_id: u8,
}

/// The event ring: one segment, plus this driver's dequeue pointer and
/// consumer cycle state.
struct EventRing {
    phys: u64,
    erst_phys: u64,
    dequeue: usize,
    cycle: u32,
}

/// A live xHCI host controller, brought up and (if a HID boot keyboard
/// was found) with that keyboard's interrupt-IN endpoint armed.
pub struct Xhci {
    mmio: u64,
    op: u64,
    runtime: u64,
    doorbell: u64,
    /// 64-byte contexts rather than 32-byte, per `HCCPARAMS1.CSZ`. Every
    /// offset into a device/input context has to be scaled by this, and
    /// getting it wrong produces a controller that rejects Address
    /// Device with a Parameter Error and nothing more specific.
    context_size: usize,
    command_ring: ProducerRing,
    event_ring: EventRing,
    /// The endpoint-0 (control) transfer ring of whichever device is
    /// currently being enumerated, or of the adopted keyboard once one
    /// is found. Exactly one device is ever addressed at a time, so one
    /// ring is all this driver needs.
    ep0_ring: Option<ProducerRing>,
    dcbaa_phys: u64,
    /// Frames this driver allocated and must keep alive for the
    /// controller to keep DMAing into. Never freed -- the controller is
    /// live for the rest of the program's life, and handing one of these
    /// back to `pmm` while the hardware still holds its address is
    /// exactly the kind of use-after-free that presents as random memory
    /// corruption hours later.
    owned_frames: Vec<u64>,
    /// The connected HID boot keyboard, once enumerated.
    keyboard: Option<Keyboard>,
}

/// Everything needed to keep pulling reports from one HID boot-protocol
/// keyboard.
#[derive(Clone, Copy)]
struct Keyboard {
    slot_id: u8,
    /// Doorbell target for the interrupt-IN endpoint: `2 * ep_number +
    /// 1` for an IN endpoint (xHCI 1.2, 4.5.1's Device Context Index).
    endpoint_dci: u32,
    transfer_ring: ProducerRing,
    /// Physical address of the 8-byte report buffer the controller DMAs
    /// each report into.
    report_phys: u64,
    /// Bytes requested per transfer: always [`BOOT_REPORT_LEN`]. This is
    /// the HID boot report size, *not* the endpoint's `wMaxPacketSize`,
    /// which is only a transport limit -- a keyboard advertising 16 or 64
    /// still sends 8-byte reports, and requesting the larger size turns
    /// every report into a Short Packet completion.
    report_len: u32,
    /// Physical address of the Normal TRB currently queued, so its
    /// Transfer Event can be matched to it (valid while `armed`).
    pending_trb: u64,
    /// True while a Normal TRB is queued and awaiting its Transfer
    /// Event. Exactly one is ever outstanding: this driver re-arms only
    /// after consuming the previous report, which keeps the transfer
    /// ring's producer/consumer relationship trivially correct without
    /// any queue accounting.
    armed: bool,
}

impl Xhci {
    fn cap_read8(&self, offset: usize) -> u8 {
        // SAFETY: `self.mmio` is mapped for `MMIO_MAP_SIZE` bytes and
        // every capability offset is within the first 0x20 of it.
        unsafe { core::ptr::read_volatile((self.mmio + offset as u64) as *const u8) }
    }

    fn cap_read32(&self, offset: usize) -> u32 {
        unsafe { read32(self.mmio + offset as u64) }
    }

    fn op_read(&self, offset: usize) -> u32 {
        unsafe { read32(self.op + offset as u64) }
    }

    fn op_write(&self, offset: usize, value: u32) {
        unsafe { write32(self.op + offset as u64, value) }
    }

    fn port_reg(&self, port: u8) -> u64 {
        self.op + (OP_PORT_BASE + (port as usize - 1) * OP_PORT_STRIDE) as u64
    }

    /// Ring a doorbell. Doorbell 0 is the command ring; doorbell `slot`
    /// with target `dci` is that slot's endpoint.
    fn ring_doorbell(&self, slot: u8, target: u32) {
        // SAFETY: the doorbell array is within the mapped window and has
        // one 32-bit entry per slot, including slot 0.
        unsafe { write32(self.doorbell + (slot as u64) * 4, target) };
    }
}

/// Find, reset, and start the first xHCI controller on PCI bus 0, then
/// enumerate its root ports looking for a HID boot-protocol keyboard.
///
/// Returns `Err` for every "there is no usable USB keyboard here"
/// outcome -- no xHCI controller, a controller that won't reset, no
/// device connected, a connected device that isn't a boot keyboard.
/// Every one of those is a first-class, expected condition (a desktop
/// with a PS/2 keyboard and nothing in its USB ports hits the
/// second-to-last one every single boot), and callers must report it and
/// carry on, exactly as `ahci::init`'s "persistent storage unavailable"
/// path already does.
///
/// # Safety
/// Must be called after `paging::load` (this maps MMIO), after `sti`
/// (every wait loop here polls `clock::ticks()` against a deadline, which
/// only advances once timer interrupts are being delivered -- the exact
/// ordering requirement that bit this project once already in `ahci.rs`;
/// see `CHANGES.md`), and at most once.
pub unsafe fn init() -> Result<Xhci, &'static str> {
    let devices = pci::enumerate_bus0();
    let Some(controller) = devices.iter().copied().find(|d| {
        d.class == PCI_CLASS_SERIAL_BUS && d.subclass == PCI_SUBCLASS_USB && d.prog_if == PCI_PROGIF_XHCI
    }) else {
        return Err("no xHCI controller found on PCI bus 0");
    };

    let _ = debugcon::write_fmt(format_args!(
        "[xhci] controller found at {:02x}:{:02x}.{:x} (vendor:device={:04x}:{:04x})\n",
        controller.address.bus,
        controller.address.device,
        controller.address.function,
        controller.vendor_id,
        controller.device_id,
    ));

    pci::enable_bus_master_and_memory_space(controller.address);

    // xHCI's MMIO window is BAR0, and unlike AHCI's ABAR it is
    // architecturally allowed to be a 64-bit BAR -- which on a real
    // controller it invariably is. A 64-bit memory BAR is encoded as
    // bits 2:1 == 0b10 in the low BAR, with the upper 32 bits of the
    // address living in the *next* BAR slot; reading only BAR0 and
    // masking would silently truncate a window mapped above 4 GiB to a
    // wrong low address.
    let bar0 = pci::read_bar(controller.address, 0);
    if bar0 & 0x1 != 0 {
        return Err("xhci: BAR0 is an I/O-space BAR -- not a conformant xHCI controller");
    }
    let mmio_phys = match (bar0 >> 1) & 0x3 {
        0b00 => u64::from(bar0 & 0xFFFF_FFF0),
        0b10 => {
            let bar1 = pci::read_bar(controller.address, 1);
            u64::from(bar0 & 0xFFFF_FFF0) | (u64::from(bar1) << 32)
        }
        _ => return Err("xhci: BAR0 reports a reserved memory BAR type"),
    };
    if mmio_phys == 0 {
        return Err("xhci: BAR0 reads back as 0 -- controller not assigned an address by firmware");
    }
    if mmio_phys % paging::PAGE_SIZE != 0 {
        return Err("xhci: BAR0 is not page-aligned");
    }

    // SAFETY: a real memory BAR from the controller's own configuration
    // space; MMIO (cache-disabled) identity mapping, same as `ahci.rs`.
    unsafe { paging::map(mmio_phys, mmio_phys, MMIO_MAP_SIZE, paging::PageFlags::Mmio)? };

    let mut hc = Xhci {
        mmio: mmio_phys,
        op: 0,
        runtime: 0,
        doorbell: 0,
        context_size: 32,
        command_ring: ProducerRing::new()?,
        event_ring: EventRing {
            phys: 0,
            erst_phys: 0,
            dequeue: 0,
            cycle: 1,
        },
        ep0_ring: None,
        dcbaa_phys: 0,
        owned_frames: Vec::new(),
        keyboard: None,
    };
    hc.owned_frames.push(hc.command_ring.phys);

    let caplength = hc.cap_read8(CAP_CAPLENGTH);
    // CAPLENGTH and HCIVERSION share one dword, and are read as one
    // here rather than as a byte and a halfword. That is not tidiness:
    // read as a 16-bit access at offset 0x02, HCIVERSION came back as 0
    // from QEMU's own xHCI model while the 8-bit CAPLENGTH read at 0x00
    // worked fine -- a sub-dword MMIO access this register window simply
    // does not decode. The xHCI spec is explicit that the capability
    // registers are dword-sized (1.2, 5.3), so a dword read is both the
    // correct access and the one that works everywhere; a version of 0
    // would otherwise have been reported in the boot log forever as if
    // it were what the controller actually says.
    let cap_dword0 = hc.cap_read32(CAP_CAPLENGTH);
    let hciversion = (cap_dword0 >> 16) as u16;
    let hcsparams1 = hc.cap_read32(CAP_HCSPARAMS1);
    let hcsparams2 = hc.cap_read32(CAP_HCSPARAMS2);
    let hccparams1 = hc.cap_read32(CAP_HCCPARAMS1);
    // CAPLENGTH of 0 (or an all-ones version) is the signature of
    // nothing actually decoding this address -- catch it here rather
    // than by computing an operational base of `mmio + 0` and writing
    // the reset bit into the capability registers.
    if caplength == 0 || hciversion == 0xFFFF {
        return Err("xhci: capability registers read back as nothing -- the MMIO window is not responding");
    }

    hc.op = mmio_phys + u64::from(caplength);
    hc.runtime = mmio_phys + u64::from(hc.cap_read32(CAP_RTSOFF) & !0x1F);
    hc.doorbell = mmio_phys + u64::from(hc.cap_read32(CAP_DBOFF) & !0x3);
    // HCCPARAMS1 bit 2 (CSZ): contexts are 64 bytes rather than 32.
    hc.context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    // Max Scratchpad Buffers is split across two non-adjacent fields
    // (HCSPARAMS2 bits 25:21 high, bits 31:27 low) -- a genuinely
    // awkward encoding that is easy to read as one field and get wrong.
    let scratchpads = (((hcsparams2 >> 21) & 0x1F) << 5) | ((hcsparams2 >> 27) & 0x1F);

    let _ = debugcon::write_fmt(format_args!(
        "[xhci] mmio=0x{mmio_phys:x} version=0x{hciversion:04x} caplength={caplength} \
         max_slots={max_slots} max_ports={max_ports} context_size={} scratchpads={scratchpads}\n",
        hc.context_size,
    ));

    unsafe { hc.reset_and_start(max_slots, scratchpads)? };
    unsafe { hc.find_keyboard(max_ports)? };
    Ok(hc)
}

impl Xhci {
    /// Stop, reset, and re-initialize the controller into a known state,
    /// then start it -- xHCI 1.2, 4.2 ("Host Controller Initialization").
    ///
    /// Resetting unconditionally, rather than trusting whatever state
    /// firmware left behind, is the same choice `ahci.rs`'s `hba_reset`
    /// makes and for the same reason: UEFI's own USB stack has almost
    /// certainly been driving this controller moments ago, with its own
    /// rings and contexts at addresses this kernel knows nothing about,
    /// and a controller still holding those pointers will happily DMA
    /// into memory `pmm` has since handed out.
    ///
    /// # Safety
    /// Register access on a mapped, live controller; call once.
    unsafe fn reset_and_start(&mut self, max_slots: u8, scratchpads: u32) -> Result<(), &'static str> {
        // Wait for the controller to finish whatever it is already doing
        // before touching a single operational register: every write
        // while CNR is set is silently discarded (xHCI 1.2, 5.4.2), so
        // skipping this wait produces a reset that appears to have been
        // issued and simply never happened.
        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        if !poll_until(deadline, || self.op_read(OP_USBSTS) & USBSTS_CNR == 0) {
            return Err("xhci: controller never reported ready (USBSTS.CNR stayed set)");
        }

        // Stop it, then reset it. A reset while running is undefined.
        self.op_write(OP_USBCMD, self.op_read(OP_USBCMD) & !USBCMD_RUN);
        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        if !poll_until(deadline, || self.op_read(OP_USBSTS) & USBSTS_HCH != 0) {
            return Err("xhci: controller did not halt when Run/Stop was cleared");
        }

        self.op_write(OP_USBCMD, USBCMD_HCRST);
        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        if !poll_until(deadline, || {
            self.op_read(OP_USBCMD) & USBCMD_HCRST == 0 && self.op_read(OP_USBSTS) & USBSTS_CNR == 0
        }) {
            return Err("xhci: host controller reset did not complete");
        }

        // Device Context Base Address Array: one 64-bit entry per slot,
        // plus entry 0 (reserved for the scratchpad buffer array).
        let dcbaa = alloc_zeroed_frame()?;
        self.owned_frames.push(dcbaa);
        self.dcbaa_phys = dcbaa;

        // Scratchpad buffers are memory the *controller* needs for its
        // own internal use; a controller that asks for them and doesn't
        // get them fails in undefined ways later rather than at the
        // point of the omission. QEMU's xHCI asks for none, so this
        // whole block is untested there -- it is written from the spec
        // and kept deliberately simple (one page per buffer, which is
        // always at least the required page size) precisely because it
        // cannot be exercised here.
        if scratchpads > 0 {
            let array = alloc_zeroed_frame()?;
            self.owned_frames.push(array);
            let max_in_page = (paging::PAGE_SIZE / 8) as u32;
            if scratchpads > max_in_page {
                return Err("xhci: controller requests more scratchpad buffers than one page can index");
            }
            for i in 0..scratchpads {
                let buf = alloc_zeroed_frame()?;
                self.owned_frames.push(buf);
                // SAFETY: `array` is an owned, zeroed, identity-mapped
                // frame and `i` is bounds-checked above.
                unsafe { core::ptr::write_volatile((array as *mut u64).add(i as usize), buf) };
            }
            // SAFETY: DCBAA entry 0 is architecturally the scratchpad
            // array pointer (xHCI 1.2, 4.20).
            unsafe { core::ptr::write_volatile(dcbaa as *mut u64, array) };
        }

        // Event ring: one segment, described by a one-entry Event Ring
        // Segment Table.
        let event_seg = alloc_zeroed_frame()?;
        let erst = alloc_zeroed_frame()?;
        self.owned_frames.push(event_seg);
        self.owned_frames.push(erst);
        self.event_ring.phys = event_seg;
        self.event_ring.erst_phys = erst;
        self.event_ring.dequeue = 0;
        self.event_ring.cycle = 1;
        // SAFETY: `erst` is an owned, zeroed, identity-mapped frame; an
        // ERST entry is {64-bit segment base, 32-bit size in TRBs, 32
        // reserved}.
        unsafe {
            core::ptr::write_volatile(erst as *mut u64, event_seg);
            core::ptr::write_volatile((erst as *mut u32).add(2), TRBS_PER_RING as u32);
        }

        unsafe {
            write32(self.op + OP_CONFIG as u64, u32::from(max_slots));
            write64(self.op + OP_DCBAAP as u64, dcbaa);
            // CRCR's low bits carry the Ring Cycle State, which must
            // match the cycle bit this driver will first produce with.
            write64(self.op + OP_CRCR as u64, self.command_ring.phys | 1);

            let ir0 = self.runtime + RT_IR0 as u64;
            write32(ir0 + IR_ERSTSZ as u64, 1);
            // ERDP before ERSTBA: writing ERSTBA is what makes the
            // controller start using the table, so the dequeue pointer
            // must already be valid (xHCI 1.2, 4.9.4).
            write64(ir0 + IR_ERDP as u64, event_seg);
            write64(ir0 + IR_ERSTBA as u64, erst);
            // Interrupter 0 stays disabled: this driver polls the event
            // ring (see the module doc comment). IMAN's Interrupt
            // Pending bit is write-1-to-clear and is cleared here so a
            // pending flag left over from firmware's own use doesn't
            // confuse a later reader.
            write32(ir0 + IR_IMAN as u64, 1);
        }

        compiler_fence(Ordering::SeqCst);
        self.op_write(OP_USBCMD, USBCMD_RUN);
        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        if !poll_until(deadline, || self.op_read(OP_USBSTS) & USBSTS_HCH == 0) {
            return Err("xhci: controller did not start running after Run/Stop was set");
        }

        debugcon::write_str("[xhci] controller reset and running\n");
        Ok(())
    }

    /// Read the next event off the event ring, or `None` if the
    /// controller hasn't produced one.
    ///
    /// "Hasn't produced one" is decided purely by the cycle bit: a TRB
    /// whose cycle differs from this driver's consumer cycle state is
    /// one the controller has not written since the last wrap. There is
    /// no count, no head pointer, and no other signal -- that single bit
    /// is the whole protocol (xHCI 1.2, 4.9.2).
    fn next_event(&mut self) -> Option<Event> {
        let index = self.event_ring.dequeue;
        // SAFETY: `event_ring.phys` is an owned, identity-mapped frame
        // and `index` is always < TRBS_PER_RING.
        let trb = unsafe { (self.event_ring.phys as *const u32).add(index * 4) };

        // Ownership first: dword 3 carries the cycle bit, and until it
        // matches this consumer's cycle state the controller has not
        // published this TRB -- dwords 0-2 may be stale. Reading the
        // payload before the cycle bit would allow an event published
        // between the two reads to be accepted with the *previous*
        // occupant's payload (xHCI 1.2, 4.9.4).
        // SAFETY: as above.
        let d3 = unsafe { trb.add(3).read_volatile() };
        if (d3 & TRB_CYCLE) != self.event_ring.cycle {
            return None;
        }
        // Order the payload reads after the ownership read. x86 does not
        // reorder loads with other loads, and the controller writes the
        // cycle dword last, so a compiler barrier is all this needs here.
        compiler_fence(Ordering::SeqCst);
        // SAFETY: as above.
        let (d0, d1, d2) = unsafe {
            (
                trb.read_volatile(),
                trb.add(1).read_volatile(),
                trb.add(2).read_volatile(),
            )
        };
        // Every field must be read before the dequeue pointer is
        // advanced and published: once ERDP names a later TRB, the
        // controller is free to overwrite this one.
        compiler_fence(Ordering::SeqCst);

        let event = Event {
            trb_type: (d3 >> 10) & 0x3F,
            completion_code: (d2 >> 24) & 0xFF,
            pointer: u64::from(d0) | (u64::from(d1) << 32),
            transfer_length: d2 & 0x00FF_FFFF,
            endpoint_id: ((d3 >> 16) & 0x1F) as u8,
            slot_id: ((d3 >> 24) & 0xFF) as u8,
        };

        self.event_ring.dequeue += 1;
        if self.event_ring.dequeue == TRBS_PER_RING {
            self.event_ring.dequeue = 0;
            self.event_ring.cycle ^= 1;
        }
        // Publish the new dequeue position. Bit 3 (Event Handler Busy)
        // is write-1-to-clear and is set here as part of every update,
        // which is what tells the controller this driver is done with
        // everything up to that point.
        let erdp = self.event_ring.phys + (self.event_ring.dequeue * 16) as u64;
        // SAFETY: interrupter 0's ERDP, within the mapped runtime window.
        unsafe { write64(self.runtime + (RT_IR0 + IR_ERDP) as u64, erdp | (1 << 3)) };
        Some(event)
    }

    /// Wait for the next event of `wanted_type`, discarding (and
    /// logging) anything else that arrives first.
    ///
    /// Port Status Change Events are the common "something else"
    /// here -- they are generated asynchronously by the hardware, in the
    /// middle of whatever else is happening, and a driver that treated
    /// an unexpected event type as a failure would break the moment
    /// someone unplugged something at the wrong moment.
    fn wait_for_event(&mut self, wanted_type: u32) -> Result<Event, &'static str> {
        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        let mut found: Option<Event> = None;
        // Drains every event currently on the ring per poll, so a burst of
        // unrelated events ahead of the wanted one is consumed in one pass
        // rather than one event per deadline check. `clock::poll_until`
        // supplies the bound (and its interrupts-enabled guard) shared by
        // every other wait in this driver.
        let arrived = poll_until(deadline, || {
            while let Some(event) = self.next_event() {
                if event.trb_type == wanted_type {
                    found = Some(event);
                    return true;
                }
                if event.trb_type == TRB_TYPE_PORT_STATUS_CHANGE {
                    continue;
                }
                let _ = debugcon::write_fmt(format_args!(
                    "[xhci] discarding unexpected event type {} while waiting for type {wanted_type}\n",
                    event.trb_type,
                ));
            }
            false
        });
        match (arrived, found) {
            (true, Some(event)) => Ok(event),
            _ => Err("xhci: timed out waiting for an event from the controller"),
        }
    }

    /// Push one command TRB, ring doorbell 0, and wait for its Command
    /// Completion Event.
    ///
    /// One command at a time, always: the event carries the physical
    /// address of the command TRB it completes, and this driver checks
    /// it, so a mismatched completion is caught rather than silently
    /// attributed to the wrong request.
    ///
    /// # Safety
    /// Controller must be running.
    unsafe fn run_command(&mut self, d0: u32, d1: u32, d2: u32, d3: u32) -> Result<Event, &'static str> {
        let trb_phys = unsafe { self.command_ring.push(d0, d1, d2, d3) };
        compiler_fence(Ordering::SeqCst);
        self.ring_doorbell(0, 0);

        let event = self.wait_for_event(TRB_TYPE_COMMAND_COMPLETION)?;
        if event.pointer != trb_phys {
            return Err("xhci: command completion event names a different command TRB than the one issued");
        }
        if event.completion_code != COMPLETION_SUCCESS {
            let _ = debugcon::write_fmt(format_args!(
                "[xhci] command failed with completion code {}\n",
                event.completion_code,
            ));
            return Err("xhci: controller rejected a command");
        }
        Ok(event)
    }

    /// Reset one root port and wait for it to enable.
    ///
    /// # Safety
    /// Controller must be running and `port` must be 1..=max_ports.
    unsafe fn reset_port(&self, port: u8) -> Result<(), &'static str> {
        let reg = self.port_reg(port);
        // SAFETY: within the mapped operational window.
        let portsc = unsafe { read32(reg) };
        // Preserve everything except the write-1-to-clear bits (see
        // `PORTSC_RW1C_MASK`): a naive read-modify-write here silently
        // disables the port via PED and throws away change flags.
        unsafe { write32(reg, (portsc & !PORTSC_RW1C_MASK) | PORTSC_PR) };

        let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
        // SAFETY: same.
        if !poll_until(deadline, || {
            let v = unsafe { read32(reg) };
            v & PORTSC_PR == 0 && v & PORTSC_PED != 0
        }) {
            return Err("xhci: port did not enable after reset");
        }
        Ok(())
    }

    /// Walk every root port, and for the first one with a connected
    /// device, try to enumerate it as a HID boot keyboard.
    ///
    /// # Safety
    /// Controller must be running.
    unsafe fn find_keyboard(&mut self, max_ports: u8) -> Result<(), &'static str> {
        for port in 1..=max_ports {
            let reg = self.port_reg(port);
            // SAFETY: within the mapped operational window.
            let portsc = unsafe { read32(reg) };
            if portsc & PORTSC_CCS == 0 {
                continue;
            }
            // Power the port if it isn't already. On a controller whose
            // ports are software-powered, a device can report connected
            // and still never enumerate without this.
            if portsc & PORTSC_PP == 0 {
                unsafe { write32(reg, (portsc & !PORTSC_RW1C_MASK) | PORTSC_PP) };
                let deadline = clock::deadline_after_ticks(TIMEOUT_TICKS);
                if !poll_until(deadline, || unsafe { read32(reg) } & PORTSC_PP != 0) {
                    let _ = debugcon::write_fmt(format_args!("[xhci] port {port}: would not power on, skipping\n"));
                    continue;
                }
            }

            let portsc = unsafe { read32(reg) };
            let speed = (portsc >> PORTSC_SPEED_SHIFT) & 0xF;
            let _ = debugcon::write_fmt(format_args!(
                "[xhci] port {port}: device connected (portsc=0x{portsc:08x} speed={speed})\n",
            ));

            // USB3 ports enable themselves as part of link training;
            // USB2 ports need an explicit reset. Resetting an
            // already-enabled port is legal but pointless, so this only
            // resets what needs it.
            if portsc & PORTSC_PED == 0 {
                if let Err(msg) = unsafe { self.reset_port(port) } {
                    let _ = debugcon::write_fmt(format_args!("[xhci] port {port}: {msg}, skipping\n"));
                    continue;
                }
            }

            match unsafe { self.enumerate_device(port, speed) } {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    let _ = debugcon::write_fmt(format_args!(
                        "[xhci] port {port}: device is not a HID boot keyboard, leaving it alone\n",
                    ));
                }
                Err(msg) => {
                    // One unenumerable device is not a reason to give up
                    // on the others -- a hub or an unpowered dock on one
                    // port must not stop the keyboard on the next one
                    // from being found.
                    let _ = debugcon::write_fmt(format_args!("[xhci] port {port}: enumeration failed: {msg}\n"));
                }
            }
        }
        Err("no USB HID boot keyboard found on any root port")
    }
}

// --- Device enumeration: contexts, control transfers, descriptors -----

/// Endpoint Type field values for an Endpoint Context's dword1 (xHCI
/// 1.2, Table 6-9).
const EP_TYPE_CONTROL: u32 = 4;
const EP_TYPE_INTERRUPT_IN: u32 = 7;

/// Input Control Context add-flag bits: A0 is the Slot Context, A1 is
/// endpoint 0, and A(n) is Device Context Index n (xHCI 1.2, 6.2.5.1).
const ADD_FLAG_SLOT: u32 = 1 << 0;
const ADD_FLAG_EP0: u32 = 1 << 1;

/// xHCI speed IDs for the default speed table (xHCI 1.2, Table 7-12).
const SPEED_FULL: u32 = 1;
const SPEED_LOW: u32 = 2;
const SPEED_HIGH: u32 = 3;
const SPEED_SUPER: u32 = 4;

/// The maximum control-transfer packet size to configure endpoint 0
/// with before the device has had a chance to report its own, derived
/// purely from link speed (USB 2.0 spec, 5.5.3 / USB 3.2, 9.6.1).
///
/// For Full speed this is a genuine guess -- 8, 16, 32 and 64 are all
/// legal -- which is exactly why [`Xhci::enumerate_device`] reads the
/// first 8 bytes of the device descriptor (a length every legal value
/// can deliver in one packet) and corrects endpoint 0 with an Evaluate
/// Context command before reading anything longer. Low speed is fixed
/// at 8 and High/Super are fixed at 64/512, so those need no correction.
fn default_max_packet_size(speed: u32) -> u16 {
    match speed {
        SPEED_LOW => 8,
        SPEED_FULL => 8,
        SPEED_HIGH => 64,
        SPEED_SUPER => 512,
        // An unrecognized speed ID means this controller uses a custom
        // speed table (declared through an xHCI extended capability this
        // driver does not parse). 64 is the safest single choice: it is
        // legal for every speed at or above Full.
        _ => 64,
    }
}

/// Write one 32-bit field of a context, scaling by this controller's
/// context size.
///
/// Contexts are 32 or 64 bytes apart depending on `HCCPARAMS1.CSZ`, and
/// the difference is invisible: a 64-byte-context controller handed
/// 32-byte-spaced contexts reads the slot context where the endpoint
/// context should be and rejects the command with a bare Parameter
/// Error. Every context access in this driver goes through this one
/// function so the scaling cannot be forgotten at a call site.
///
/// # Safety
/// `base` must be an owned, identity-mapped context frame; `index` must
/// be a valid context index within it and `dword` within that context.
unsafe fn ctx_write(base: u64, context_size: usize, index: usize, dword: usize, value: u32) {
    unsafe {
        let ptr = (base + (index * context_size) as u64) as *mut u32;
        ptr.add(dword).write_volatile(value);
    }
}

/// Everything one control transfer needs. A plain struct rather than
/// eight positional arguments because a USB setup packet's fields are
/// famously easy to transpose, and a transposed `wValue`/`wIndex` fails
/// as an opaque Stall Error with nothing naming which one was wrong.
struct Setup {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
}

impl Xhci {
    /// Run one control transfer on endpoint 0 of `slot`, optionally
    /// moving `length` bytes into `buffer_phys`.
    ///
    /// # Safety
    /// `slot` must be an addressed slot whose EP0 transfer ring is
    /// `ring`; `buffer_phys` must be an owned frame of at least
    /// `setup.length` bytes when that is nonzero.
    unsafe fn control_transfer(
        &mut self,
        slot: u8,
        setup: &Setup,
        buffer_phys: u64,
    ) -> Result<u32, &'static str> {
        // Transfer Type (dword3 bits 17:16 of the Setup Stage TRB): 0 =
        // no data stage, 2 = OUT data, 3 = IN data. Derived from the
        // request's own direction bit and length rather than passed in
        // separately, so the two can never disagree.
        let has_data = setup.length > 0;
        let dir_in = setup.request_type & 0x80 != 0;
        let trt: u32 = if !has_data {
            0
        } else if dir_in {
            3
        } else {
            2
        };

        // The endpoint-0 ring is taken out of `self` for the duration of
        // the enqueue and put straight back. That is not a stylistic
        // choice: pushing TRBs needs `&mut` on the ring while ringing
        // the doorbell and draining events need `&mut self`, and the
        // borrow checker is right to refuse both at once. Moving the
        // ring out for exactly the window in which nothing else can
        // touch it makes the two disjoint, with no `RefCell` and no
        // raw-pointer aliasing to reason about.
        let mut ring = self
            .ep0_ring
            .take()
            .ok_or("xhci: control transfer attempted with no endpoint-0 ring")?;
        let ring = &mut ring;
        // Setup Stage: the 8-byte setup packet travels *inside* the TRB
        // (the Immediate Data bit), which is why dwords 0 and 1 are the
        // packet itself rather than a pointer to it.
        unsafe {
            ring.push(
                u32::from(setup.request_type)
                    | (u32::from(setup.request) << 8)
                    | (u32::from(setup.value) << 16),
                u32::from(setup.index) | (u32::from(setup.length) << 16),
                8,
                (TRB_TYPE_SETUP_STAGE << 10) | TRB_IDT | (trt << 16),
            );
            if has_data {
                ring.push(
                    buffer_phys as u32,
                    (buffer_phys >> 32) as u32,
                    u32::from(setup.length),
                    (TRB_TYPE_DATA_STAGE << 10) | (u32::from(dir_in) << 16),
                );
            }
        }
        // Status Stage always runs in the opposite direction to the data
        // stage (USB 2.0, 8.5.3), and carries this transfer's only
        // Interrupt On Completion -- so exactly one Transfer Event is
        // produced per control transfer, naming this TRB.
        let status_dir_in = !has_data || !dir_in;
        let status_trb = unsafe {
            ring.push(
                0,
                0,
                0,
                (TRB_TYPE_STATUS_STAGE << 10) | (u32::from(status_dir_in) << 16) | TRB_IOC,
            )
        };

        // Put the ring back before anything that needs `&mut self`.
        self.ep0_ring = Some(*ring);
        compiler_fence(Ordering::SeqCst);
        // Doorbell target 1 is Device Context Index 1 -- endpoint 0.
        self.ring_doorbell(slot, 1);

        let event = self.wait_for_event(TRB_TYPE_TRANSFER_EVENT)?;
        if event.pointer != status_trb {
            return Err("xhci: transfer event names a different TRB than the control transfer issued");
        }
        if event.completion_code != COMPLETION_SUCCESS {
            let _ = debugcon::write_fmt(format_args!(
                "[xhci] control transfer failed with completion code {} (bRequest={})\n",
                event.completion_code, setup.request,
            ));
            return Err("xhci: device rejected a control transfer");
        }
        // A Transfer Event reports the *residual* untransferred bytes,
        // not the transferred count -- reading it as the latter is a
        // classic xHCI mistake that produces plausible-looking but
        // wrong short-read handling.
        Ok(u32::from(setup.length).saturating_sub(event.transfer_length))
    }
}


/// Command TRB type 13: Evaluate Context -- used here only to correct
/// endpoint 0's Max Packet Size once the device has reported its real
/// one (xHCI 1.2, 4.6.7).
const TRB_TYPE_EVALUATE_CONTEXT: u32 = 13;

/// What [`Xhci::find_hid_keyboard_interface`] extracts from a
/// configuration descriptor.
struct HidKeyboardInterface {
    configuration_value: u8,
    interface_number: u8,
    /// Endpoint number (1-15) of the interrupt IN endpoint carrying
    /// reports.
    endpoint_number: u8,
    max_packet_size: u16,
    /// The endpoint descriptor's `bInterval`, in whatever units this
    /// device's speed defines it -- see [`encode_interval`].
    b_interval: u8,
}

/// Translate a USB endpoint descriptor's `bInterval` into the xHCI
/// Endpoint Context's Interval field, which is always expressed as
/// "125 microseconds times 2^Interval" regardless of speed (xHCI 1.2,
/// 6.2.3.6).
///
/// The two speed families define `bInterval` completely differently, and
/// conflating them is a real bug with a quiet symptom: an interrupt
/// endpoint polled far too rarely still *works*, it just drops
/// keystrokes under fast typing, which is easy to blame on anything else.
///
/// - High and Super speed: `bInterval` is already a power-of-two
///   exponent in 125 us units, 1-16, so the field is simply
///   `bInterval - 1`.
/// - Full and Low speed: `bInterval` is a period in whole milliseconds
///   (1-255). One millisecond is 8 x 125 us, so the equivalent exponent
///   is `3 + floor(log2(bInterval))`, clamped to the 3..=10 range the
///   spec defines for these speeds.
fn encode_interval(speed: u32, b_interval: u8) -> u32 {
    match speed {
        SPEED_HIGH | SPEED_SUPER => u32::from(b_interval.max(1) - 1).min(15),
        _ => {
            let ms = u32::from(b_interval.max(1));
            // `ilog2` of a nonzero u32 is exactly floor(log2(n)).
            (3 + ms.ilog2()).clamp(3, 10)
        }
    }
}

impl Xhci {
    /// Fully enumerate the device on `port`, and adopt it if it turns
    /// out to be a HID boot-protocol keyboard.
    ///
    /// Returns `Ok(false)` for a device that enumerated fine but isn't a
    /// keyboard -- a completely ordinary outcome (a mouse, a webcam, a
    /// USB stick) that must not look like an error.
    ///
    /// # Safety
    /// Controller must be running and `port` must be enabled.
    unsafe fn enumerate_device(&mut self, port: u8, speed: u32) -> Result<bool, &'static str> {
        let event = unsafe { self.run_command(0, 0, 0, TRB_TYPE_ENABLE_SLOT << 10)? };
        let slot = event.slot_id;
        if slot == 0 {
            return Err("xhci: Enable Slot returned slot 0");
        }

        let input_ctx = alloc_zeroed_frame()?;
        let device_ctx = alloc_zeroed_frame()?;
        let data_buf = alloc_zeroed_frame()?;
        let ep0_ring = ProducerRing::new()?;
        self.owned_frames.push(input_ctx);
        self.owned_frames.push(device_ctx);
        self.owned_frames.push(data_buf);
        self.owned_frames.push(ep0_ring.phys);
        self.ep0_ring = Some(ep0_ring);

        let cs = self.context_size;
        let mut max_packet_size = default_max_packet_size(speed);

        // Input context layout: context 0 is the Input Control Context,
        // context 1 is the Slot Context, and context 1+n is Device
        // Context Index n's Endpoint Context. Endpoint 0 is DCI 1, so
        // its context is index 2 here.
        // SAFETY: `input_ctx` is an owned, zeroed, identity-mapped frame
        // and every index below is within the 33 contexts it holds even
        // at the 64-byte context size (33 * 64 = 2112 < 4096).
        unsafe {
            // Input Control Context: add the Slot Context and endpoint 0,
            // drop nothing.
            ctx_write(input_ctx, cs, 0, 0, 0);
            ctx_write(input_ctx, cs, 0, 1, ADD_FLAG_SLOT | ADD_FLAG_EP0);

            // Slot Context: route string 0 (a root-port device has no
            // hub path), this link's speed, exactly one valid endpoint
            // context so far, and the root hub port this device is on.
            ctx_write(input_ctx, cs, 1, 0, (speed << 20) | (1 << 27));
            ctx_write(input_ctx, cs, 1, 1, u32::from(port) << 16);
            ctx_write(input_ctx, cs, 1, 2, 0);
            ctx_write(input_ctx, cs, 1, 3, 0);

            // Endpoint 0 Context: a control endpoint, three retries
            // before reporting an error (CErr = 3, the value the spec
            // recommends and every real stack uses -- 0 means "never
            // retry", which turns one lost packet on a marginal cable
            // into a hard enumeration failure).
            ctx_write(input_ctx, cs, 2, 0, 0);
            ctx_write(
                input_ctx,
                cs,
                2,
                1,
                (3 << 1) | (EP_TYPE_CONTROL << 3) | (u32::from(max_packet_size) << 16),
            );
            let ep0_phys = self.ep0_ring.as_ref().unwrap().phys;
            // Dequeue Cycle State (bit 0) must match the cycle bit this
            // driver will first produce with on that ring.
            ctx_write(input_ctx, cs, 2, 2, (ep0_phys as u32) | 1);
            ctx_write(input_ctx, cs, 2, 3, (ep0_phys >> 32) as u32);
            ctx_write(input_ctx, cs, 2, 4, 8);

            // Point this slot's Device Context Base Address Array entry
            // at the device context the controller will fill in.
            core::ptr::write_volatile((self.dcbaa_phys as *mut u64).add(slot as usize), device_ctx);
        }

        unsafe {
            self.run_command(
                input_ctx as u32,
                (input_ctx >> 32) as u32,
                0,
                (TRB_TYPE_ADDRESS_DEVICE << 10) | (u32::from(slot) << 24),
            )?
        };
        let _ = debugcon::write_fmt(format_args!("[xhci] port {port}: slot {slot} addressed\n"));

        // Read just the first 8 bytes of the device descriptor. Eight is
        // the one length every legal `bMaxPacketSize0` (8, 16, 32, 64,
        // and Super speed's 512) can deliver in a single packet, so this
        // read is safe to issue before the real packet size is known --
        // which is the entire reason the USB spec puts that field at
        // offset 7.
        let setup = Setup {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: u16::from(DESC_TYPE_DEVICE) << 8,
            index: 0,
            length: 8,
        };
        unsafe { self.control_transfer(slot, &setup, data_buf)? };
        // SAFETY: the controller DMA'd 8 bytes into `data_buf`, an owned
        // identity-mapped frame.
        let reported_mps0 = unsafe { core::ptr::read_volatile((data_buf as *const u8).add(7)) };
        // For Super speed the field is a power-of-two exponent, not a
        // byte count (USB 3.2, 9.6.1): 9 means 512.
        let reported_mps0 = if speed == SPEED_SUPER {
            1u16 << reported_mps0
        } else {
            u16::from(reported_mps0)
        };

        if reported_mps0 != 0 && reported_mps0 != max_packet_size {
            let _ = debugcon::write_fmt(format_args!(
                "[xhci] slot {slot}: endpoint 0 max packet size is really {reported_mps0} \
                 (guessed {max_packet_size} from link speed) -- correcting\n",
            ));
            // SAFETY: same frame, same layout as above. Evaluate Context
            // consults only the contexts named by the add flags, so only
            // endpoint 0's needs to be correct here.
            unsafe {
                ctx_write(input_ctx, cs, 0, 0, 0);
                ctx_write(input_ctx, cs, 0, 1, ADD_FLAG_EP0);
                ctx_write(
                    input_ctx,
                    cs,
                    2,
                    1,
                    (3 << 1) | (EP_TYPE_CONTROL << 3) | (u32::from(reported_mps0) << 16),
                );
                self.run_command(
                    input_ctx as u32,
                    (input_ctx >> 32) as u32,
                    0,
                    (TRB_TYPE_EVALUATE_CONTEXT << 10) | (u32::from(slot) << 24),
                )?;
            }
            max_packet_size = reported_mps0;
        }
        let _ = max_packet_size;

        // Configuration descriptor: nine bytes first, purely to learn
        // `wTotalLength`, then the whole thing. Asking for the full
        // length up front is not possible -- nothing else reports it.
        let setup = Setup {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: u16::from(DESC_TYPE_CONFIGURATION) << 8,
            index: 0,
            length: 9,
        };
        unsafe { self.control_transfer(slot, &setup, data_buf)? };
        // SAFETY: nine bytes were DMA'd into `data_buf`.
        let total_length = unsafe {
            let p = data_buf as *const u8;
            u16::from(p.add(2).read_volatile()) | (u16::from(p.add(3).read_volatile()) << 8)
        };
        // A configuration descriptor longer than the scratch frame would
        // overrun it. Clamping rather than failing is the right call: a
        // device with an enormous configuration still has its first
        // interfaces at the front, and this driver only ever needs the
        // HID one -- but the read itself must never be allowed past the
        // frame.
        let fetch_len = total_length.min(paging::PAGE_SIZE as u16).max(9);
        let setup = Setup {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: u16::from(DESC_TYPE_CONFIGURATION) << 8,
            index: 0,
            length: fetch_len,
        };
        let got = unsafe { self.control_transfer(slot, &setup, data_buf)? };

        // SAFETY: `got` bytes (never more than `fetch_len`, itself
        // clamped to the frame) were DMA'd into `data_buf`.
        let config = unsafe { core::slice::from_raw_parts(data_buf as *const u8, got as usize) };
        let Some(iface) = find_hid_keyboard_interface(config) else {
            return Ok(false);
        };
        let _ = debugcon::write_fmt(format_args!(
            "[xhci] slot {slot}: HID boot keyboard on interface {} endpoint {} \
             (wMaxPacketSize={} bInterval={})\n",
            iface.interface_number, iface.endpoint_number, iface.max_packet_size, iface.b_interval,
        ));

        // SET_CONFIGURATION moves the device out of the Address state
        // and makes its interfaces' endpoints real. It has to happen
        // before Configure Endpoint, not after: the endpoint the
        // Configure Endpoint command describes does not exist on the
        // device until its configuration is selected.
        let setup = Setup {
            request_type: 0x00,
            request: USB_REQ_SET_CONFIGURATION,
            value: u16::from(iface.configuration_value),
            index: 0,
            length: 0,
        };
        unsafe { self.control_transfer(slot, &setup, 0)? };

        // Device Context Index for an IN endpoint is 2n+1 (xHCI 1.2,
        // 4.5.1) -- the odd/even split is how one index space covers both
        // directions of every endpoint number.
        let dci = 2 * u32::from(iface.endpoint_number) + 1;
        let kbd_ring = ProducerRing::new()?;
        let report_buf = alloc_zeroed_frame()?;
        self.owned_frames.push(kbd_ring.phys);
        self.owned_frames.push(report_buf);

        // SAFETY: same owned input-context frame; `dci` is at most 31 so
        // context index `1 + dci` is at most 32, within the frame at
        // either context size.
        unsafe {
            ctx_write(input_ctx, cs, 0, 0, 0);
            ctx_write(input_ctx, cs, 0, 1, ADD_FLAG_SLOT | (1 << dci));
            // The Slot Context's Context Entries field must name the
            // highest valid endpoint context, or the controller ignores
            // everything above whatever it currently says.
            ctx_write(input_ctx, cs, 1, 0, (speed << 20) | (dci << 27));
            ctx_write(input_ctx, cs, 1, 1, u32::from(port) << 16);

            let ep_index = 1 + dci as usize;
            ctx_write(
                input_ctx,
                cs,
                ep_index,
                0,
                encode_interval(speed, iface.b_interval) << 16,
            );
            ctx_write(
                input_ctx,
                cs,
                ep_index,
                1,
                (3 << 1) | (EP_TYPE_INTERRUPT_IN << 3) | (u32::from(iface.max_packet_size) << 16),
            );
            ctx_write(input_ctx, cs, ep_index, 2, (kbd_ring.phys as u32) | 1);
            ctx_write(input_ctx, cs, ep_index, 3, (kbd_ring.phys >> 32) as u32);
            // Average TRB Length (the boot report size every TRB on this
            // ring requests) and Max ESIT Payload (the most the endpoint
            // can move per service interval: one max-size packet). These
            // are scheduling hints, but a zero Average TRB Length is
            // rejected outright by some controllers rather than treated
            // as "unknown."
            ctx_write(
                input_ctx,
                cs,
                ep_index,
                4,
                BOOT_REPORT_LEN | (u32::from(iface.max_packet_size) << 16),
            );

            self.run_command(
                input_ctx as u32,
                (input_ctx >> 32) as u32,
                0,
                (TRB_TYPE_CONFIGURE_ENDPOINT << 10) | (u32::from(slot) << 24),
            )?;
        }

        // Boot protocol: the whole point of the HID boot subclass is
        // that the report format is fixed and needs no report-descriptor
        // parser. Devices power up in report protocol, so this request
        // is what actually makes the 8-byte format `usb_hid.rs` decodes
        // the one being sent.
        let setup = Setup {
            request_type: 0x21,
            request: HID_REQ_SET_PROTOCOL,
            value: HID_PROTOCOL_BOOT,
            index: u16::from(iface.interface_number),
            length: 0,
        };
        if let Err(msg) = unsafe { self.control_transfer(slot, &setup, 0) } {
            // A device that stalls SET_PROTOCOL is already in boot
            // protocol (the request is optional for boot-subclass
            // devices that support nothing else), so this is a warning,
            // not a failure -- refusing to use an otherwise perfectly
            // good keyboard over it would be the wrong call.
            let _ = debugcon::write_fmt(format_args!(
                "[xhci] slot {slot}: SET_PROTOCOL(boot) was rejected ({msg}); \
                 assuming the device is already in boot protocol\n",
            ));
        }

        self.keyboard = Some(Keyboard {
            slot_id: slot,
            endpoint_dci: dci,
            transfer_ring: kbd_ring,
            report_phys: report_buf,
            report_len: BOOT_REPORT_LEN,
            pending_trb: 0,
            armed: false,
        });
        unsafe { self.arm_report_transfer() };
        debugcon::write_str("[xhci] USB HID boot keyboard ready\n");
        Ok(true)
    }

    /// Queue one Normal TRB on the keyboard's interrupt-IN ring and ring
    /// its doorbell, so the controller delivers the next report.
    ///
    /// Exactly one is outstanding at a time; see [`Keyboard::armed`].
    ///
    /// # Safety
    /// A keyboard must be adopted and the controller running.
    unsafe fn arm_report_transfer(&mut self) {
        let Some(mut kbd) = self.keyboard else {
            return;
        };
        if kbd.armed {
            return;
        }
        kbd.pending_trb = unsafe {
            kbd.transfer_ring.push(
                kbd.report_phys as u32,
                (kbd.report_phys >> 32) as u32,
                kbd.report_len,
                (TRB_TYPE_NORMAL << 10) | TRB_IOC,
            )
        };
        kbd.armed = true;
        self.keyboard = Some(kbd);
        compiler_fence(Ordering::SeqCst);
        self.ring_doorbell(kbd.slot_id, kbd.endpoint_dci);
    }

    /// Drain the event ring, delivering any completed keyboard report to
    /// `on_report`, and re-arm.
    ///
    /// This is the whole of this driver's steady-state work, and it is
    /// called from `keyboard::wait_for_key`'s idle loop rather than an
    /// interrupt handler -- see the module doc comment for why, and what
    /// that costs.
    ///
    /// # Safety
    /// Controller must be running.
    pub unsafe fn poll(&mut self, mut on_report: impl FnMut(&[u8; 8])) {
        while let Some(event) = self.next_event() {
            match event.trb_type {
                TRB_TYPE_TRANSFER_EVENT => {
                    let Some(kbd) = self.keyboard else { continue };
                    // Only the keyboard's own outstanding transfer retires
                    // it: same slot, same endpoint, same TRB.
                    if !kbd.armed
                        || event.slot_id != kbd.slot_id
                        || u32::from(event.endpoint_id) != kbd.endpoint_dci
                        || event.pointer != kbd.pending_trb
                    {
                        let _ = debugcon::write_fmt(format_args!(
                            "[xhci] ignoring transfer event for slot {} ep {} trb 0x{:x} (code {})\n",
                            event.slot_id, event.endpoint_id, event.pointer, event.completion_code,
                        ));
                        continue;
                    }
                    // A Transfer Event retires the TRB it names, so the
                    // endpoint is idle again regardless of whether the
                    // transfer succeeded.
                    if let Some(k) = self.keyboard.as_mut() {
                        k.armed = false;
                    }
                    match event.completion_code {
                        COMPLETION_SUCCESS | COMPLETION_SHORT_PACKET => {
                            // The event's length field is the residual
                            // (untransferred) byte count; it can never
                            // legitimately exceed what was requested.
                            let transferred = kbd.report_len.checked_sub(event.transfer_length);
                            match transferred {
                                Some(n) if n >= BOOT_REPORT_LEN => {
                                    let mut report = [0u8; 8];
                                    // SAFETY: `report_phys` is an owned,
                                    // identity-mapped frame the controller
                                    // just DMA'd at least 8 bytes into.
                                    unsafe {
                                        core::ptr::copy_nonoverlapping(
                                            kbd.report_phys as *const u8,
                                            report.as_mut_ptr(),
                                            8,
                                        );
                                    }
                                    on_report(&report);
                                }
                                _ => {
                                    // A truncated report: decoding it would
                                    // read stale bytes from the last one.
                                    let _ = debugcon::write_fmt(format_args!(
                                        "[xhci] keyboard report rejected: residual {} of {} requested bytes\n",
                                        event.transfer_length, kbd.report_len,
                                    ));
                                }
                            }
                        }
                        code => {
                            let _ = debugcon::write_fmt(format_args!(
                                "[xhci] keyboard transfer completed with code {code}\n",
                            ));
                        }
                    }
                    unsafe { self.arm_report_transfer() };
                }
                TRB_TYPE_PORT_STATUS_CHANGE => {
                    // Hot-plug is out of scope (see the module doc
                    // comment): the event is consumed so the ring keeps
                    // draining, and noted so a disconnected keyboard is
                    // at least visible in the log rather than a mystery.
                    debugcon::write_str("[xhci] port status change (hot-plug is not handled)\n");
                }
                other => {
                    let _ = debugcon::write_fmt(format_args!("[xhci] unhandled event type {other}\n"));
                }
            }
        }
    }

    /// True once a HID boot keyboard has been found and armed.
    pub fn has_keyboard(&self) -> bool {
        self.keyboard.is_some()
    }
}

/// Walk a configuration descriptor's packed descriptor list looking for
/// a HID boot-protocol keyboard interface and its interrupt IN endpoint.
///
/// USB descriptor lists are a flat sequence of `[bLength, bDescriptorType,
/// ...]` records with no index and no terminator other than running out
/// of buffer -- so this walks byte by byte, and treats a zero or
/// out-of-range `bLength` as the end (a malformed descriptor must stop
/// the walk, not spin on it forever or read past the buffer).
fn find_hid_keyboard_interface(config: &[u8]) -> Option<HidKeyboardInterface> {
    if config.len() < 9 {
        return None;
    }
    let configuration_value = config[5];

    let mut offset = 0usize;
    let mut current: Option<(u8, u8)> = None; // (interface number, is a boot keyboard)
    let mut candidate: Option<u8> = None;

    while offset + 2 <= config.len() {
        let len = config[offset] as usize;
        if len < 2 || offset + len > config.len() {
            break;
        }
        let desc = &config[offset..offset + len];
        match desc[1] {
            DESC_TYPE_INTERFACE if len >= 9 => {
                let is_boot_keyboard = desc[5] == USB_CLASS_HID
                    && desc[6] == HID_SUBCLASS_BOOT
                    && desc[7] == HID_PROTOCOL_KEYBOARD;
                current = Some((desc[2], u8::from(is_boot_keyboard)));
                candidate = if is_boot_keyboard { Some(desc[2]) } else { None };
            }
            DESC_TYPE_ENDPOINT if len >= 7 => {
                // Only endpoints belonging to a boot-keyboard interface
                // are of interest, and only the interrupt IN one:
                // bEndpointAddress bit 7 is direction (1 = IN), and
                // bmAttributes bits 1:0 are the transfer type (3 =
                // interrupt).
                let address = desc[2];
                let attributes = desc[3];
                let is_interrupt_in = address & 0x80 != 0 && attributes & 0x03 == 3;
                if let (Some(iface), true, Some(_)) = (current, is_interrupt_in, candidate) {
                    if iface.1 == 1 {
                        let max_packet_size = u16::from(desc[4]) | (u16::from(desc[5]) << 8);
                        return Some(HidKeyboardInterface {
                            configuration_value,
                            interface_number: iface.0,
                            endpoint_number: address & 0x0F,
                            // Bits 12:11 of wMaxPacketSize are the
                            // high-speed "additional transactions"
                            // field, not part of the size.
                            max_packet_size: max_packet_size & 0x07FF,
                            b_interval: desc[6],
                        });
                    }
                }
            }
            _ => {}
        }
        offset += len;
    }
    None
}
