//! Polled NVMe NVM-command-set storage over PCIe, including PCI VMD domains.
//!
//! One admin queue, one I/O queue, and one command in flight. Queue/data pages
//! are physically contiguous 4-KiB frames. Native 512-byte and 4-KiB LBAs are
//! exposed as 512-byte sectors; writes to a 4-KiB namespace preserve its other
//! seven sectors with a read/modify/write. No formatting command is issued.
//!
//! Register, queue, Identify and NVM command layouts: NVM Express, Inc.,
//! NVMe Base Specification 1.4c, sections 3, 4, 5.3/5.4/5.15 and 6.8/6.9/6.15:
//! https://nvmexpress.org/wp-content/uploads/NVM-Express-1_4c-2021.06.28-Ratified.pdf

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use core::sync::atomic::{fence, Ordering};

use crate::nvme_format as format;
use crate::{clock, debugcon, paging, pci, pmm};

const CAP: u64 = 0x00;
const VS: u64 = 0x08;
const INTMS: u64 = 0x0c;
const CC: u64 = 0x14;
const CSTS: u64 = 0x1c;
const AQA: u64 = 0x24;
const ASQ: u64 = 0x28;
const ACQ: u64 = 0x30;
const DOORBELLS: u64 = 0x1000;
const COMMAND_TICKS: u64 = 3000;
const MAX_QUEUE_DEPTH: u16 = 16;

unsafe fn read32(base: u64, offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

unsafe fn write32(base: u64, offset: u64, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}

unsafe fn write64(base: u64, offset: u64, value: u64) {
    unsafe {
        write32(base, offset, value as u32);
        write32(base, offset + 4, (value >> 32) as u32);
    }
}

unsafe fn disable(mmio: u64, timeout: u64) -> bool {
    unsafe { write32(mmio, CC, read32(mmio, CC) & !1) };
    fence(Ordering::SeqCst);
    clock::poll_until(clock::deadline_after_ticks(timeout), || unsafe {
        read32(mmio, CSTS) & 1 == 0
    })
}

/// Pages are returned only after the controller has acknowledged disable.
/// A wedged controller's pages stay pinned, so late DMA cannot corrupt RAM.
struct DmaPages {
    frames: [u64; 5],
    count: usize,
    quarantine: bool,
}

impl DmaPages {
    fn allocate() -> Result<Self, &'static str> {
        let mut pages = Self {
            frames: [0; 5],
            count: 0,
            quarantine: false,
        };
        for index in 0..pages.frames.len() {
            let phys = pmm::alloc_frame().ok_or("nvme: out of physical frames for queues")?;
            unsafe { core::ptr::write_bytes(phys as *mut u8, 0, paging::PAGE_SIZE as usize) };
            pages.frames[index] = phys;
            pages.count += 1;
        }
        Ok(pages)
    }
}

impl Drop for DmaPages {
    fn drop(&mut self) {
        if !self.quarantine {
            for &frame in &self.frames[..self.count] {
                pmm::free_frame(frame);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct QueueState {
    tail: u16,
    head: u16,
    phase: u16,
    next_id: u16,
}

struct Queue {
    id: u16,
    sq: u64,
    cq: u64,
    state: Cell<QueueState>,
}

impl Queue {
    fn new(id: u16, sq: u64, cq: u64) -> Self {
        Self {
            id,
            sq,
            cq,
            state: Cell::new(QueueState {
                tail: 0,
                head: 0,
                phase: 1,
                next_id: 1,
            }),
        }
    }
}

pub struct DiskInfo {
    pub total_sectors: u64,
    model: String,
    serial: String,
}

impl DiskInfo {
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
}

pub struct Nvme {
    mmio: u64,
    stride: u64,
    depth: u16,
    ready_timeout: u64,
    admin: Queue,
    io: Queue,
    pages: DmaPages,
    failed: Cell<bool>,
    busy: Cell<bool>,
    namespace: u32,
    block_size: u32,
    info: Option<DiskInfo>,
}

impl Drop for Nvme {
    fn drop(&mut self) {
        if !unsafe { disable(self.mmio, self.ready_timeout) } {
            self.pages.quarantine = true;
            debugcon::write_str("[nvme] disable timed out; DMA pages quarantined\n");
        }
    }
}

impl Nvme {
    pub fn info(&self) -> Option<&DiskInfo> {
        self.info.as_ref()
    }
    pub fn is_failed(&self) -> bool {
        self.failed.get()
    }
    pub fn logical_block_size(&self) -> u32 {
        self.block_size
    }
    pub fn namespace_id(&self) -> u32 {
        self.namespace
    }

    fn buffer(&self) -> u64 {
        self.pages.frames[4]
    }

    /// # Safety
    /// Called only with exclusively owned DMA pages and no other outstanding
    /// command. NVMe objects are !Sync and no interrupt handler uses them.
    unsafe fn execute(&self, queue: &Queue, mut command: [u32; 16]) -> Result<u32, &'static str> {
        if self.failed.get() {
            return Err("nvme: controller is offline after an I/O failure");
        }
        if self.busy.replace(true) {
            return Err("nvme: a command is already in flight");
        }
        let mut state = queue.state.get();
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        command[0] = (command[0] & 0xffff) | (u32::from(id) << 16);
        let submission = (queue.sq + u64::from(state.tail) * 64) as *mut u32;
        for (index, dword) in command.into_iter().enumerate() {
            unsafe { core::ptr::write_volatile(submission.add(index), dword) };
        }
        state.tail = (state.tail + 1) % self.depth;
        fence(Ordering::SeqCst);
        unsafe {
            write32(
                self.mmio,
                DOORBELLS + u64::from(queue.id) * 2 * self.stride,
                u32::from(state.tail),
            )
        };
        let completion = (queue.cq + u64::from(state.head) * 16) as *const u32;
        let mut fatal = false;
        let complete = clock::poll_until(clock::deadline_after_ticks(COMMAND_TICKS), || unsafe {
            fatal = read32(self.mmio, CSTS) & 2 != 0;
            fatal || ((core::ptr::read_volatile(completion.add(3)) >> 16) as u16 & 1) == state.phase
        });
        if !complete || fatal {
            self.failed.set(true);
            self.busy.set(false);
            let _ = unsafe { disable(self.mmio, self.ready_timeout) };
            return Err(if fatal {
                "nvme: controller reported a fatal status"
            } else {
                "nvme: command timed out; controller disabled"
            });
        }
        fence(Ordering::Acquire);
        let result = unsafe { core::ptr::read_volatile(completion) };
        let sq = unsafe { core::ptr::read_volatile(completion.add(2)) };
        let status = unsafe { core::ptr::read_volatile(completion.add(3)) };
        if status as u16 != id || (sq >> 16) as u16 != queue.id || sq as u16 >= self.depth {
            self.failed.set(true);
            self.busy.set(false);
            let _ = unsafe { disable(self.mmio, self.ready_timeout) };
            return Err("nvme: completion does not match the submitted command");
        }
        state.head += 1;
        if state.head == self.depth {
            state.head = 0;
            state.phase ^= 1;
        }
        queue.state.set(state);
        fence(Ordering::SeqCst);
        unsafe {
            write32(
                self.mmio,
                DOORBELLS + (u64::from(queue.id) * 2 + 1) * self.stride,
                u32::from(state.head),
            )
        };
        self.busy.set(false);
        let code = (status >> 17) as u16;
        if code != 0 {
            let _ = debugcon::write_fmt(format_args!(
                "[nvme] queue {} command status=0x{code:04x}\n",
                queue.id
            ));
            return Err("nvme: command rejected by the controller");
        }
        Ok(result)
    }

    unsafe fn identify(&self, namespace: u32, kind: u32) -> Result<(), &'static str> {
        if self.failed.get() || self.busy.get() {
            return Err("nvme: controller is not available for Identify");
        }
        unsafe { core::ptr::write_bytes(self.buffer() as *mut u8, 0, 4096) };
        let mut command = [0; 16];
        command[0] = 0x06;
        command[1] = namespace;
        command[6] = self.buffer() as u32;
        command[7] = (self.buffer() >> 32) as u32;
        command[10] = kind;
        unsafe {
            self.execute(&self.admin, command)?;
        }
        Ok(())
    }

    unsafe fn transfer_block(&self, block: u64, write: bool) -> Result<(), &'static str> {
        let mut command = [0; 16];
        command[0] = if write { 0x01 } else { 0x02 };
        command[1] = self.namespace;
        command[6] = self.buffer() as u32;
        command[7] = (self.buffer() >> 32) as u32;
        command[10] = block as u32;
        command[11] = (block >> 32) as u32;
        // NLB=0 transfers exactly one native logical block. The store's
        // flush protocol supplies durability barriers after writes.
        unsafe {
            self.execute(&self.io, command)?;
        }
        Ok(())
    }

    /// Read a 512-byte logical sector; all bounds are checked before DMA.
    pub unsafe fn read_sector(&self, lba: u64, out: &mut [u8; 512]) -> Result<(), &'static str> {
        let info = self
            .info
            .as_ref()
            .ok_or("nvme: namespace has not been identified")?;
        if lba >= info.total_sectors {
            return Err("nvme: read past the end of the namespace");
        }
        let sectors = u64::from(self.block_size / 512);
        unsafe {
            self.transfer_block(lba / sectors, false)?;
            core::ptr::copy_nonoverlapping(
                (self.buffer() + lba % sectors * 512) as *const u8,
                out.as_mut_ptr(),
                512,
            );
        }
        Ok(())
    }

    /// Write one sector, preserving neighboring sub-sectors on 4-KiB media.
    pub unsafe fn write_sector(&self, lba: u64, data: &[u8; 512]) -> Result<(), &'static str> {
        // A timed-out write may still be reading this DMA buffer if disable
        // failed. Keep its contents pinned as well as the allocation itself.
        if self.failed.get() || self.busy.get() {
            return Err("nvme: controller is offline or a command is still in flight");
        }
        let info = self
            .info
            .as_ref()
            .ok_or("nvme: namespace has not been identified")?;
        if lba >= info.total_sectors {
            return Err("nvme: write past the end of the namespace");
        }
        let sectors = u64::from(self.block_size / 512);
        unsafe {
            if sectors > 1 {
                self.transfer_block(lba / sectors, false)?;
            }
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                (self.buffer() + lba % sectors * 512) as *mut u8,
                512,
            );
            self.transfer_block(lba / sectors, true)?;
        }
        Ok(())
    }

    pub unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        let mut command = [0; 16];
        command[0] = 0x00;
        command[1] = self.namespace;
        unsafe {
            self.execute(&self.io, command)?;
        }
        Ok(())
    }
}

fn identify_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '?'
            }
        })
        .collect::<String>()
        .trim()
        .into()
}

/// Initialize a controller without writing any namespace data.
///
/// # Safety
/// Called once during driver bring-up, with interrupts and the physical
/// allocator/page tables active, after firmware has released the controller.
pub unsafe fn init() -> Result<Nvme, &'static str> {
    let devices = pci::enumerate_devices();
    let controller = devices
        .iter()
        .find(|device| device.class == 1 && device.subclass == 8 && device.prog_if == 2)
        .ok_or("no NVMe controller found")?;
    pci::enable_bus_master_and_memory_space(controller.address);
    // INTMS alone does not mask MSI-X. Firmware may have enabled a PCI
    // interrupt mode before handing over the device; this driver polls.
    pci::disable_interrupts(controller.address)?;
    let mmio = pci::memory_bar(controller.address, 0)?;
    if mmio == 0 || mmio % paging::PAGE_SIZE != 0 {
        return Err("nvme: invalid controller BAR alignment/address");
    }
    unsafe {
        paging::map(mmio, mmio, 4096, paging::PageFlags::Mmio)?;
    }
    let cap = u64::from(unsafe { read32(mmio, CAP) })
        | (u64::from(unsafe { read32(mmio, CAP + 4) }) << 32);
    if cap == u64::MAX || cap & (1 << 37) == 0 {
        return Err("nvme: controller does not support the NVM command set");
    }
    if (cap >> 48) & 0xf != 0 {
        return Err("nvme: controller does not support 4-KiB host pages");
    }
    let depth = ((cap & 0xffff) + 1).min(u64::from(MAX_QUEUE_DEPTH)) as u16;
    if depth < 2 {
        return Err("nvme: controller queue capacity is too small");
    }
    let stride = 4u64 << ((cap >> 32) & 0xf);
    let window = (DOORBELLS + 4 * stride).div_ceil(paging::PAGE_SIZE) * paging::PAGE_SIZE;
    mmio.checked_add(window)
        .ok_or("nvme: controller register window overflows")?;
    unsafe {
        paging::map(mmio, mmio, window, paging::PageFlags::Mmio)?;
    }
    let ready_timeout = ((cap >> 24) & 0xff).max(1) * (u64::from(clock::TICK_HZ) / 2);
    if !unsafe { disable(mmio, ready_timeout) } {
        return Err("nvme: controller did not become disabled");
    }
    let pages = DmaPages::allocate()?;
    let mut device = Nvme {
        mmio,
        stride,
        depth,
        ready_timeout,
        admin: Queue::new(0, pages.frames[0], pages.frames[1]),
        io: Queue::new(1, pages.frames[2], pages.frames[3]),
        pages,
        failed: Cell::new(false),
        busy: Cell::new(false),
        namespace: 0,
        block_size: 512,
        info: None,
    };
    unsafe {
        write32(mmio, INTMS, u32::MAX);
        write32(
            mmio,
            AQA,
            (u32::from(depth - 1) << 16) | u32::from(depth - 1),
        );
        write64(mmio, ASQ, device.admin.sq);
        write64(mmio, ACQ, device.admin.cq);
        fence(Ordering::SeqCst);
        write32(mmio, CC, 1 | (6 << 16) | (4 << 20));
    }
    let ready = clock::poll_until(clock::deadline_after_ticks(ready_timeout), || unsafe {
        read32(mmio, CSTS) & 3 != 0
    });
    if !ready || unsafe { read32(mmio, CSTS) & 3 } != 1 {
        return Err("nvme: controller did not become ready");
    }
    unsafe {
        device.identify(0, 1)?;
    }
    let page = unsafe { &*(device.buffer() as *const [u8; 4096]) };
    let model = identify_text(&page[24..64]);
    let serial = identify_text(&page[4..24]);
    if page[512] & 0xf > 6 || page[512] >> 4 < 6 || page[513] & 0xf > 4 || page[513] >> 4 < 4 {
        return Err("nvme: controller queue entry sizes are unsupported");
    }
    let namespace_count = u32::from_le_bytes(page[516..520].try_into().unwrap());
    if namespace_count == 0 {
        return Err("nvme: controller exposes no namespaces");
    }
    // Negotiate one I/O queue pair before creating queues 1/1.
    let mut command = [0; 16];
    command[0] = 0x09;
    command[10] = 0x07;
    unsafe {
        device.execute(&device.admin, command)?;
    }
    command = [0; 16];
    command[0] = 0x05;
    command[6] = device.io.cq as u32;
    command[7] = (device.io.cq >> 32) as u32;
    command[10] = (u32::from(depth - 1) << 16) | 1;
    command[11] = 1; // contiguous, interrupts disabled
    unsafe {
        device.execute(&device.admin, command)?;
    }
    command[0] = 0x01;
    command[6] = device.io.sq as u32;
    command[7] = (device.io.sq >> 32) as u32;
    command[11] = (1 << 16) | 1; // CQID=1, contiguous, normal priority
    unsafe {
        device.execute(&device.admin, command)?;
    }
    let mut namespaces = Vec::new();
    if unsafe { device.identify(0, 2) }.is_ok() {
        let page = unsafe { &*(device.buffer() as *const [u8; 4096]) };
        for bytes in page.chunks_exact(4).take(32) {
            let id = u32::from_le_bytes(bytes.try_into().unwrap());
            if id == 0 {
                break;
            }
            if id == u32::MAX {
                continue;
            }
            namespaces.push(id);
        }
    } else if !device.failed.get() {
        // NVMe 1.0 lacks the active namespace list; NSID 1 is its usual
        // namespace. Never guess further IDs on command/transport failure.
        namespaces.push(1);
    }
    let mut last_error = "nvme: no active NVM namespace with a supported LBA format";
    for namespace in namespaces {
        if let Err(error) = unsafe { device.identify(namespace, 0) } {
            last_error = error;
            if device.failed.get() {
                return Err(error);
            }
            continue;
        }
        let page = unsafe { &*(device.buffer() as *const [u8; 4096]) };
        match format::namespace(page) {
            Ok(layout) => {
                device.namespace = namespace;
                device.block_size = layout.block_size;
                device.info = Some(DiskInfo {
                    total_sectors: layout.total_sectors,
                    model,
                    serial,
                });
                let version = unsafe { read32(mmio, VS) };
                let _ = debugcon::write_fmt(format_args!("[nvme] controller {:02x}:{:02x}.{:x} version=0x{version:08x} namespace={namespace} blocks={} block_bytes={} sectors={}\n", controller.address.bus, controller.address.device, controller.address.function, layout.blocks, layout.block_size, layout.total_sectors));
                return Ok(device);
            }
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}
