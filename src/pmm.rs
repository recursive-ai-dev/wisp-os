//! Physical frame allocator: a bitmap over every 4 KiB page of RAM the
//! firmware's final memory map reports as usable, built from the
//! `MemoryMap` handed back by `exit_boot_services`.
//!
//! Its consumers are `paging.rs` (every page-table page), `ahci.rs` and
//! `xhci.rs` (every DMA structure, one whole frame each so alignment is
//! free). It was built and self-tested (see [`self_test`], run every boot)
//! a phase before paging needed it, so the first real consumer met
//! verified code rather than an untested stub.

use alloc::vec;
use alloc::vec::Vec;
use uefi::table::boot::{MemoryMap, MemoryType};

use crate::sync::Locked;

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

/// True for the memory types that are legitimately reusable general-purpose
/// RAM immediately after `exit_boot_services` returns. `CONVENTIONAL` is
/// memory that was already free; `BOOT_SERVICES_CODE`/`BOOT_SERVICES_DATA`
/// become free as an explicit, documented side effect of exiting boot
/// services (see `SystemTable::exit_boot_services`'s own safety docs) --
/// only code/data belonging to *this* loaded image is exempt, and that
/// lives under `LOADER_CODE`/`LOADER_DATA`, which this function correctly
/// does not include.
fn is_usable(ty: MemoryType) -> bool {
    matches!(
        ty,
        MemoryType::CONVENTIONAL | MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA
    )
}

/// Build the frame bitmap from the final UEFI memory map. Must be called
/// after `exit_boot_services` (using its returned `MemoryMap`) and before
/// anything calls [`alloc_frame`]/[`free_frame`].
pub fn init(mmap: &MemoryMap) {
    let mut highest_addr: u64 = 0;
    for d in mmap.entries() {
        if is_usable(d.ty) {
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
        if is_usable(d.ty) {
            let start_frame = (d.phys_start / FRAME_SIZE) as usize;
            for frame in start_frame..start_frame + d.page_count as usize {
                clear_bit(&mut bitmap, frame);
            }
        }
    }

    // Frame 0 (physical address 0) is never handed out even if firmware
    // reports it as conventional memory, so that a null pointer can never
    // coincide with a validly-allocated frame.
    if frame_count > 0 {
        set_bit(&mut bitmap, 0);
    }

    // Counted from the finished bitmap rather than summed from the memory
    // map, so overlapping or duplicate descriptors can never make the count
    // disagree with what `alloc` can actually hand out.
    let free_frames = (0..frame_count).filter(|&f| !test_bit(&bitmap, f)).count();

    ALLOCATOR.with(|slot| *slot = Some(FrameAllocator { bitmap, frame_count, free_frames }));
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
