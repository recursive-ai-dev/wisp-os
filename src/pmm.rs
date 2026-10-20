//! Physical frame allocator: a bitmap over every 4 KiB page of RAM the
//! firmware's final memory map reports as usable, built from the
//! `MemoryMap` handed back by `exit_boot_services`.
//!
//! Its consumers are `paging.rs` (every page-table page), `ahci.rs` and
//! `xhci.rs` (every DMA structure, one whole frame each so alignment is
//! free). It was built and self-tested (see [`self_test`], run every boot)
//! a phase before paging needed it, so the first real consumer met
//! verified code rather than an untested stub.
//!
//! # Two phases: firmware's memory is not free until firmware is gone
//!
//! `exit_boot_services` makes `BOOT_SERVICES_CODE`/`_DATA` reusable -- but
//! not immediately safe to *overwrite*. Until this kernel has switched to
//! its own page tables, descriptor tables and stack, the CPU is still
//! running on firmware's, and on every firmware this kernel has been
//! measured against they live in exactly that memory: under QEMU/OVMF
//! (256 MiB) the live `CR3` hierarchy is 1029 frames, all
//! `BOOT_SERVICES_DATA`, and so are the live stack and IDT. `paging::init`
//! allocates thousands of frames and zeroes each one *while those are
//! live*; handing it one of them would unmap memory out from under the
//! running code -- most likely a triple fault, a silent reset with nothing
//! on screen. (OVMF happens to place them high and this allocator is
//! first-fit from the bottom, which is the only reason that never fired;
//! no firmware promises that layout.)
//!
//! So [`init`] frees only `CONVENTIONAL` memory, which firmware describes
//! as unallocated, and *withholds* boot-services memory.
//! [`release_boot_services_memory`] hands it over once `main.rs` is running
//! on the kernel's own tables, GDT/IDT and stack. As a second line of
//! defence against firmware that breaks the rules, [`init`] also walks the
//! live paging hierarchy and pins any frame of it -- and the pages holding
//! the live stack, GDT and IDT -- that the memory map calls conventional.

use alloc::vec;
use alloc::vec::Vec;
use uefi::table::boot::{MemoryMap, MemoryType};

use crate::sync::Locked;
use crate::{debugcon, paging};

const FRAME_SIZE: u64 = 4096;

struct FrameAllocator {
    /// One bit per frame: 1 = in use or never usable, 0 = free. Sized
    /// exactly to `frame_count` bits (rounded up to a whole byte), not to
    /// the full 64-bit address space -- `frame_count` is derived from the
    /// highest usable address the firmware actually reported.
    bitmap: Vec<u8>,
    frame_count: usize,
    /// Number of clear bits in `bitmap` -- kept in step by `alloc`/`free`
    /// so the shell's `mem` command never has to rescan the bitmap.
    free_frames: usize,
    /// Boot-services memory, as `(first_frame, frame_count)` runs, still
    /// marked in use until [`release_boot_services_memory`] frees it. Empty
    /// afterwards. See the module doc comment.
    withheld: Vec<(usize, usize)>,
}

fn test_bit(bitmap: &[u8], frame: usize) -> bool {
    bitmap[frame / 8] & (1 << (frame % 8)) != 0
}

fn set_bit(bitmap: &mut [u8], frame: usize) {
    bitmap[frame / 8] |= 1 << (frame % 8);
}

fn clear_bit(bitmap: &mut [u8], frame: usize) {
    bitmap[frame / 8] &= !(1 << (frame % 8));
}

impl FrameAllocator {
    /// First-fit: return the lowest-numbered free frame. Simple and, for
    /// this kernel's actual usage (a boot-time self-test; no fragmentation
    /// pattern to speak of yet), fast enough -- revisit with a free-list
    /// or a next-fit cursor only if a real, frequent allocator becomes a
    /// consumer later.
    fn alloc(&mut self) -> Option<u64> {
        for frame in 0..self.frame_count {
            if !test_bit(&self.bitmap, frame) {
                set_bit(&mut self.bitmap, frame);
                self.free_frames -= 1;
                return Some(frame as u64 * FRAME_SIZE);
            }
        }
        None
    }

    fn free(&mut self, addr: u64) {
        let frame = (addr / FRAME_SIZE) as usize;
        assert!(frame < self.frame_count, "pmm::free: address outside the frame bitmap");
        assert!(test_bit(&self.bitmap, frame), "pmm::free: double-free of frame {frame}");
        clear_bit(&mut self.bitmap, frame);
        self.free_frames += 1;
    }
}

static ALLOCATOR: Locked<Option<FrameAllocator>> = Locked::new(None);

/// Memory firmware describes as unallocated: free the moment
/// `exit_boot_services` returns, with nothing of firmware's in it.
fn is_free_now(ty: MemoryType) -> bool {
    ty == MemoryType::CONVENTIONAL
}

/// Memory that `exit_boot_services` makes reusable (see its own safety
/// docs) but that can still hold firmware's live page tables, stack and
/// IDT until this kernel has replaced them -- see the module doc comment.
/// Code/data belonging to *this* image is `LOADER_CODE`/`LOADER_DATA`,
/// never freed at all.
fn is_boot_services(ty: MemoryType) -> bool {
    matches!(ty, MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA)
}

/// How much of firmware's live stack, around the stack pointer at
/// [`init`], to pin if it turns out to sit in conventional memory: the
/// frames `paging::init` will still run on below it, and the callers'
/// frames above it. Clipped to the stack's own memory-map descriptor.
const STACK_PIN_WINDOW: u64 = 128 * 1024;

/// Mark `addr`'s frame in use if it is currently free; `true` if it was.
fn pin(bitmap: &mut [u8], frame_count: usize, addr: u64) -> bool {
    let frame = (addr / FRAME_SIZE) as usize;
    if frame < frame_count && !test_bit(bitmap, frame) {
        set_bit(bitmap, frame);
        true
    } else {
        false
    }
}

/// Pin every frame overlapping `[start, end)`; returns how many were free.
fn pin_range(bitmap: &mut [u8], frame_count: usize, start: u64, end: u64) -> usize {
    let mut pinned = 0;
    let mut addr = start - start % FRAME_SIZE;
    while addr < end {
        pinned += usize::from(pin(bitmap, frame_count, addr));
        addr += FRAME_SIZE;
    }
    pinned
}

/// `(base, limit)` of the live GDT and IDT, from `sgdt`/`sidt`.
fn descriptor_tables() -> [(u64, u64); 2] {
    let mut gdtr = [0u8; 10];
    let mut idtr = [0u8; 10];
    // SAFETY: `sgdt`/`sidt` only store the 10-byte register image into
    // the buffers given.
    unsafe {
        core::arch::asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack, preserves_flags));
        core::arch::asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack, preserves_flags));
    }
    let parse = |r: [u8; 10]| {
        let limit = u64::from(u16::from_le_bytes([r[0], r[1]]));
        let base = u64::from_le_bytes([r[2], r[3], r[4], r[5], r[6], r[7], r[8], r[9]]);
        (base, limit)
    };
    [parse(gdtr), parse(idtr)]
}

/// Build the frame bitmap from the final UEFI memory map, freeing only
/// conventional memory and withholding boot-services memory (see the
/// module doc comment). Must be called after `exit_boot_services` (using
/// its returned `MemoryMap`), while firmware's page tables, GDT/IDT and
/// stack are still the live ones, and before anything calls
/// [`alloc_frame`]/[`free_frame`].
pub fn init(mmap: &MemoryMap) {
    let mut highest_addr: u64 = 0;
    for d in mmap.entries() {
        if is_free_now(d.ty) || is_boot_services(d.ty) {
            let end = d.phys_start + d.page_count * FRAME_SIZE;
            highest_addr = highest_addr.max(end);
        }
    }
    let frame_count = (highest_addr / FRAME_SIZE) as usize;
    let bitmap_bytes = frame_count.div_ceil(8);

    // Default every frame to "in use": frames belonging to memory types we
    // never scan below (firmware runtime code/data, ACPI tables, MMIO,
    // this image's own LOADER_CODE/LOADER_DATA, reserved ranges, ...) must
    // never be handed out, and starting all-reserved-then-clear-the-free-
    // ones can't accidentally miss marking one of them free the way
    // starting all-free-then-set-the-reserved-ones could.
    let mut bitmap = vec![0xFFu8; bitmap_bytes];

    for d in mmap.entries() {
        if is_free_now(d.ty) {
            let start_frame = (d.phys_start / FRAME_SIZE) as usize;
            for frame in start_frame..start_frame + d.page_count as usize {
                clear_bit(&mut bitmap, frame);
            }
        }
    }

    // Boot-services runs to free later. Built from the bitmap as it stands
    // now, so a frame some (overlapping, malformed) descriptor also called
    // conventional is never in a run: it may be handed out before the
    // release, and the release must not free it a second time. Frame 0
    // stays reserved for good (below).
    let mut withheld: Vec<(usize, usize)> = Vec::new();
    for d in mmap.entries() {
        if !is_boot_services(d.ty) {
            continue;
        }
        let start_frame = (d.phys_start / FRAME_SIZE) as usize;
        let mut run: Option<(usize, usize)> = None;
        for frame in start_frame..start_frame + d.page_count as usize {
            let eligible = frame != 0 && test_bit(&bitmap, frame);
            match (&mut run, eligible) {
                (Some((_, len)), true) => *len += 1,
                (None, true) => run = Some((frame, 1)),
                (Some(_), false) => withheld.push(run.take().expect("matched Some")),
                (None, false) => {}
            }
        }
        withheld.extend(run);
    }
    let withheld_frames: usize = withheld.iter().map(|&(_, n)| n).sum();

    // Frame 0 (physical address 0) is never handed out even if firmware
    // reports it as conventional memory, so that a null pointer can never
    // coincide with a validly-allocated frame.
    if frame_count > 0 {
        set_bit(&mut bitmap, 0);
    }

    // Second line of defence: everything firmware is still running on
    // right now -- its page-table hierarchy, the stack this code is on, its
    // GDT and IDT -- must not be handed out before the switch, even if the
    // memory map wrongly calls it conventional. Withholding boot-services
    // memory already covers all of it on every firmware measured; anything
    // pinned here is firmware breaking the UEFI memory-map rules, so it is
    // reported.
    let in_withheld = |frame: usize| withheld.iter().any(|&(s, n)| frame >= s && frame < s + n);
    let mut fw_tables = 0usize;
    let mut fw_tables_withheld = 0usize;
    let mut pinned = 0usize;
    paging::for_each_live_table_frame(|addr| {
        fw_tables += 1;
        fw_tables_withheld += usize::from(in_withheld((addr / FRAME_SIZE) as usize));
        pinned += usize::from(pin(&mut bitmap, frame_count, addr));
    });
    // The stack's extent is not recorded anywhere, so pin a window around
    // the stack pointer -- clipped to the memory-map descriptor that holds
    // it, because the stack is one allocation: the window must not spill
    // into a neighbouring region (under OVMF the 128 KiB boot stack sits
    // directly above conventional memory, and an unclipped window would
    // pin a frame that firmware never used).
    let rsp: u64;
    // SAFETY: reads the stack pointer, nothing else.
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags)) };
    let (mut stack_lo, mut stack_hi) = (rsp.saturating_sub(STACK_PIN_WINDOW), rsp.saturating_add(STACK_PIN_WINDOW));
    if let Some(d) = mmap
        .entries()
        .find(|d| rsp >= d.phys_start && rsp < d.phys_start + d.page_count * FRAME_SIZE)
    {
        stack_lo = stack_lo.max(d.phys_start);
        stack_hi = stack_hi.min(d.phys_start + d.page_count * FRAME_SIZE);
    }
    pinned += pin_range(&mut bitmap, frame_count, stack_lo, stack_hi);
    for (base, limit) in descriptor_tables() {
        pinned += pin_range(&mut bitmap, frame_count, base, base.saturating_add(limit + 1));
    }

    // Counted from the finished bitmap rather than summed from the memory
    // map, so overlapping or duplicate descriptors can never make the count
    // disagree with what `alloc` can actually hand out.
    let free_frames = (0..frame_count).filter(|&f| !test_bit(&bitmap, f)).count();

    let _ = debugcon::write_fmt(format_args!(
        "[pmm] {free_frames} frames free; {withheld_frames} boot-services frames withheld until the kernel runs on its own tables and stack\n\
         [pmm] firmware's live paging hierarchy: {fw_tables} frames, {fw_tables_withheld} of them in withheld boot-services memory\n"
    ));
    if pinned > 0 {
        let _ = debugcon::write_fmt(format_args!(
            "[pmm] WARNING: {pinned} frame(s) of live firmware state (page tables/stack/GDT/IDT) are typed conventional -- pinned, never handed out\n"
        ));
    }

    ALLOCATOR.with(|slot| *slot = Some(FrameAllocator { bitmap, frame_count, free_frames, withheld }));
}

/// Free the boot-services memory [`init`] withheld; returns how many frames
/// that was. The second half of the two-phase handover (see the module doc
/// comment): call exactly once, only after the CPU is running on this
/// kernel's own page tables, GDT/IDT and stack -- at that point nothing
/// firmware left in that memory can be in use any more. Frames [`init`]
/// pinned because firmware state sat in conventional memory stay reserved;
/// they are a handful at most, and only on misbehaving firmware.
pub fn release_boot_services_memory() -> usize {
    ALLOCATOR.with(|slot| {
        let a = slot
            .as_mut()
            .expect("pmm::release_boot_services_memory called before pmm::init");
        let mut released = 0;
        for (start, count) in core::mem::take(&mut a.withheld) {
            for frame in start..(start + count).min(a.frame_count) {
                if test_bit(&a.bitmap, frame) {
                    clear_bit(&mut a.bitmap, frame);
                    released += 1;
                }
            }
        }
        a.free_frames += released;
        released
    })
}

/// `(free, total)` frame counts, where `total` is the number of frames the
/// bitmap spans (up to the highest usable address firmware reported --
/// including reserved holes, so `total - free` is not "frames allocated by
/// this kernel" but "frames not available to it").
pub fn stats() -> (usize, usize) {
    ALLOCATOR.with(|slot| match slot.as_ref() {
        Some(a) => (a.free_frames, a.frame_count),
        None => (0, 0),
    })
}

/// Size of one physical frame in bytes.
pub const FRAME_BYTES: u64 = FRAME_SIZE;

/// Allocate one free 4 KiB physical frame, returning its physical
/// address -- which is also its virtual address, since `paging.rs`
/// identity-maps all of physical memory -- or `None` if every tracked
/// frame is in use.
pub fn alloc_frame() -> Option<u64> {
    ALLOCATOR.with(|slot| {
        slot.as_mut()
            .expect("pmm::alloc_frame called before pmm::init")
            .alloc()
    })
}

/// Return a previously-allocated frame to the free pool.
///
/// # Panics
/// Panics on a double-free or an address outside the tracked range --
/// silently accepting either would hide a real bug in whatever eventually
/// consumes this allocator (once paging exists) behind memory corruption
/// far away from its actual cause.
pub fn free_frame(addr: u64) {
    ALLOCATOR.with(|slot| {
        slot.as_mut()
            .expect("pmm::free_frame called before pmm::init")
            .free(addr)
    });
}

/// Exercise alloc / write / read-back / free / realloc against real
/// memory, so a bug in the bitmap logic is caught here, loudly, at boot --
/// not silently, much later, whenever paging first becomes a real consumer
/// of this allocator.
pub fn self_test() -> Result<(), &'static str> {
    const PATTERN: u64 = 0xDEAD_BEEF_CAFE_F00D;

    let a = alloc_frame().ok_or("self-test: no free frame for first allocation")?;
    let b = alloc_frame().ok_or("self-test: no free frame for second allocation")?;
    if a == b {
        return Err("self-test: two allocations returned the same frame");
    }

    // SAFETY: `a` was just allocated, is 4-KiB aligned (a multiple of
    // `FRAME_SIZE`), and under this phase's flat/identity mapping is
    // directly writable at that same address.
    unsafe {
        let ptr = a as *mut u64;
        ptr.write_volatile(PATTERN);
        if ptr.read_volatile() != PATTERN {
            return Err("self-test: pattern readback mismatch");
        }
    }

    free_frame(a);
    let c = alloc_frame().ok_or("self-test: reallocation after free failed")?;
    if c != a {
        return Err("self-test: freed frame was not the next one reused (first-fit invariant broken)");
    }

    free_frame(b);
    free_frame(c);
    Ok(())
}
