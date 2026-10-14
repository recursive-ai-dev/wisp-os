//! A hand-written heap allocator: an intrusive, **address-ordered**,
//! singly-linked free list with first-fit allocation, block splitting, and
//! coalescing on free -- installed as the global allocator over a static
//! byte array baked into the kernel's own `.bss`.
//!
//! This is deliberately never backed by UEFI Boot Services in any way, so it
//! needs no reinitialization at the `exit_boot_services()` boundary --
//! `init()` is called exactly once, as the very first statement in
//! `efi_main`, before anything else (including `uefi::helpers::init`, which
//! may allocate internally) has a chance to allocate. See `CHANGES.md`.
//!
//! # Why address-ordered with coalescing (v1.0)
//!
//! Up to v0.x the free list was unordered and never merged neighbours, a
//! documented trade-off borrowed from the classic teaching allocator. It
//! does not survive contact with this kernel's real workload: every `run`
//! of a WISP script builds and tears down an AST, scope maps and value
//! lists of many different sizes, so over a long session the heap
//! degrades into thousands of small free fragments that can no longer
//! satisfy a large allocation even though plenty of total memory is free.
//! Keeping the list sorted by address makes merging a free block with the
//! free neighbours directly before and after it an O(1) check at the
//! point of insertion, so freed memory always returns to the largest
//! possible contiguous blocks. `self_test()` proves this at every boot by
//! churning the heap and checking it returns to the exact same shape.
//!
//! # Integrity checks
//!
//! Because the list is ordered, a `dealloc` whose range overlaps a block
//! that is already free -- a double free, or a corrupted pointer -- is
//! detectable at insertion time for free, and is turned into an immediate
//! panic naming the address instead of silently cross-linking the list
//! (which would surface much later as an unrelated, undiagnosable crash).
//!
//! # Alignment padding
//!
//! An allocation whose alignment pushes its start past the beginning of
//! the chosen free block leaves a gap in front of it. That gap is returned
//! to the free list rather than leaked. A gap too small to hold a free-list
//! header is avoided by moving the allocation up by one more alignment
//! step, which always yields a gap of at least `align` bytes -- at least
//! one header, since every alignment this allocator hands out is at least
//! the header's own.

use core::alloc::{GlobalAlloc, Layout};
use core::mem;
use core::ptr;

use crate::sync::Locked;

/// Total heap size. Generous relative to the workload (an interactive
/// shell running small WISP scripts); `mem` in the shell reports live use.
pub const HEAP_SIZE: usize = 8 * 1024 * 1024;

#[repr(align(16))]
struct AlignedHeap([u8; HEAP_SIZE]);

static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_SIZE]);

/// One free block. Lives *inside* the free memory it describes -- an
/// intrusive linked list, so tracking free space costs no separate
/// allocation of its own. `next` always points at a block at a strictly
/// higher address (or is null).
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

/// Smallest unit the allocator tracks: every block (free or allocated) is
/// at least one header in size and aligned to the header's alignment, so
/// any freed block can always host a header of its own.
const HEADER_SIZE: usize = mem::size_of::<FreeBlock>();
const HEADER_ALIGN: usize = mem::align_of::<FreeBlock>();

/// A point-in-time view of the heap, for the shell's `mem` command and the
/// boot self-test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HeapStats {
    /// Total bytes managed by the allocator.
    pub total: usize,
    /// Bytes currently on the free list.
    pub free: usize,
    /// Number of separate free blocks (1 == perfectly unfragmented).
    pub free_blocks: usize,
    /// Size of the largest single free block -- the biggest allocation
    /// that can currently succeed.
    pub largest_free: usize,
}

pub struct FreeListAllocator {
    /// First free block (lowest address), or null if the heap is full.
    head: *mut FreeBlock,
    total: usize,
}

// SAFETY: the raw pointers here only ever refer into the private static
// `HEAP`, and every access goes through `Locked`, which serializes access
// by disabling interrupts on this single-CPU kernel.
unsafe impl Send for FreeListAllocator {}

impl FreeListAllocator {
    pub const fn empty() -> Self {
        FreeListAllocator {
            head: ptr::null_mut(),
            total: 0,
        }
    }

    /// # Safety
    /// Must be called exactly once, before any allocation, with a region
    /// that is valid, writable, not aliased by anything else, aligned to
    /// `HEADER_ALIGN` and a multiple of `HEADER_ALIGN` in size.
    unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        assert!(self.head.is_null() && self.total == 0, "allocator::init called twice");
        assert_eq!(heap_start % HEADER_ALIGN, 0);
        assert_eq!(heap_size % HEADER_ALIGN, 0);
        assert!(heap_size >= HEADER_SIZE);
        self.total = heap_size;
        unsafe { self.insert_free(heap_start, heap_size) };
    }

    /// Return `addr..addr+size` to the free list at its address-ordered
    /// position, merging it with the free block directly before and/or
    /// after it when they touch.
    ///
    /// Panics on any overlap with an already-free block: that can only be
    /// a double free or a corrupted pointer, and continuing would cross-link
    /// the list.
    ///
    /// # Safety
    /// `addr..addr+size` must lie inside the heap, be header-aligned, at
    /// least `HEADER_SIZE` long, and not be in use by anything else.
    unsafe fn insert_free(&mut self, addr: usize, size: usize) {
        debug_assert!(addr % HEADER_ALIGN == 0 && size % HEADER_ALIGN == 0 && size >= HEADER_SIZE);
        let end = addr + size;

        // Find the last free block below `addr` (`prev`) and the first at
        // or above it (`next`).
        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut next = self.head;
        while !next.is_null() && (next as usize) < addr {
            prev = next;
            next = unsafe { (*next).next };
        }

        if !prev.is_null() {
            let prev_end = prev as usize + unsafe { (*prev).size };
            assert!(
                prev_end <= addr,
                "heap corruption: freeing {addr:#x} which overlaps an already-free block (double free?)"
            );
        }
        if !next.is_null() {
            assert!(
                end <= next as usize,
                "heap corruption: freeing {addr:#x} which overlaps an already-free block (double free?)"
            );
        }

        // Merge forward into `next` if adjacent.
        let (block_size, after) = if !next.is_null() && end == next as usize {
            (size + unsafe { (*next).size }, unsafe { (*next).next })
        } else {
            (size, next)
        };

        // Merge backward into `prev` if adjacent; otherwise write a new
        // header at `addr` and link it after `prev` (or as the head).
        if !prev.is_null() && prev as usize + unsafe { (*prev).size } == addr {
            unsafe {
                (*prev).size += block_size;
                (*prev).next = after;
            }
        } else {
            let node = addr as *mut FreeBlock;
            unsafe {
                node.write(FreeBlock { size: block_size, next: after });
            }
            if prev.is_null() {
                self.head = node;
            } else {
                unsafe { (*prev).next = node };
            }
        }
    }

    /// First-fit search. On success, carves the allocation out of the
    /// chosen block (returning any front alignment gap and any tail to the
    /// free list) and returns its start address.
    fn allocate(&mut self, size: usize, align: usize) -> Option<usize> {
        let mut prev: *mut FreeBlock = ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            let block_start = cur as usize;
            let block_size = unsafe { (*cur).size };
            let block_end = block_start + block_size;

            if let Some((alloc_start, alloc_end)) = Self::fit(block_start, block_end, size, align) {
                // Unlink `cur` entirely, then give back the pieces of it the
                // allocation does not use. Both pieces lie strictly inside
                // the just-unlinked block, so re-inserting them can never
                // trip the overlap check, and their neighbours are exactly
                // `prev`/`cur.next` -- no coalescing with anything else is
                // possible, since `cur` was itself maximal.
                let after = unsafe { (*cur).next };
                if prev.is_null() {
                    self.head = after;
                } else {
                    unsafe { (*prev).next = after };
                }
                let front = alloc_start - block_start;
                let tail = block_end - alloc_end;
                if front > 0 {
                    unsafe { self.insert_free(block_start, front) };
                }
                if tail > 0 {
                    unsafe { self.insert_free(alloc_end, tail) };
                }
                return Some(alloc_start);
            }
            prev = cur;
            cur = unsafe { (*cur).next };
        }
        None
    }

    /// Where an allocation of `size` bytes at `align` would sit inside the
    /// free block `block_start..block_end`, if it fits with every leftover
    /// piece either empty or large enough to be tracked again.
    fn fit(block_start: usize, block_end: usize, size: usize, align: usize) -> Option<(usize, usize)> {
        let mut alloc_start = align_up(block_start, align)?;
        let front = alloc_start - block_start;
        if front > 0 && front < HEADER_SIZE {
            // Too small to track: step up one more alignment unit so the
            // gap is at least `align` (>= HEADER_SIZE, see `size_align`).
            alloc_start = alloc_start.checked_add(align)?;
        }
        let alloc_end = alloc_start.checked_add(size)?;
        if alloc_end > block_end {
            return None;
        }
        let tail = block_end - alloc_end;
        if tail > 0 && tail < HEADER_SIZE {
            // A sliver that could never host a header would be leaked.
            return None;
        }
        Some((alloc_start, alloc_end))
    }

    /// Normalize a request so every block is header-aligned, a whole number
    /// of headers, and (for alignments above the header's) aligned to at
    /// least `HEADER_SIZE` -- which is what guarantees `fit`'s
    /// one-more-step front gap can always host a header.
    fn size_align(layout: Layout) -> (usize, usize) {
        let align = layout.align().max(HEADER_ALIGN);
        let align = if align > HEADER_ALIGN { align.max(HEADER_SIZE) } else { align };
        let size = layout.size().max(HEADER_SIZE);
        let size = match align_up(size, HEADER_ALIGN) {
            Some(s) => s,
            None => usize::MAX, // can never fit; `allocate` returns None.
        };
        (size, align)
    }

    fn stats(&self) -> HeapStats {
        let mut free = 0;
        let mut free_blocks = 0;
        let mut largest_free = 0;
        let mut cur = self.head;
        while !cur.is_null() {
            let size = unsafe { (*cur).size };
            free += size;
            free_blocks += 1;
            largest_free = largest_free.max(size);
            cur = unsafe { (*cur).next };
        }
        HeapStats {
            total: self.total,
            free,
            free_blocks,
            largest_free,
        }
    }
}

fn align_up(addr: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    Some(addr.checked_add(align - 1)? & !(align - 1))
}

unsafe impl GlobalAlloc for Locked<FreeListAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (size, align) = FreeListAllocator::size_align(layout);
        self.with(|allocator| match allocator.allocate(size, align) {
            Some(addr) => addr as *mut u8,
            None => ptr::null_mut(),
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let (size, _) = FreeListAllocator::size_align(layout);
        // SAFETY: the caller of `dealloc` guarantees `ptr..ptr+size` was
        // previously returned by `alloc` with the same layout (so the same
        // normalized size) and has not been freed already; `insert_free`
        // additionally panics if that guarantee was violated in a way that
        // overlaps an already-free block.
        self.with(|allocator| unsafe { allocator.insert_free(ptr as usize, size) });
    }
}

#[global_allocator]
static ALLOCATOR: Locked<FreeListAllocator> = Locked::new(FreeListAllocator::empty());

/// # Safety
/// Must be called exactly once, as the very first statement of `efi_main`,
/// before any code that might allocate (directly or transitively, including
/// inside `uefi::helpers::init`) has run.
pub unsafe fn init() {
    // SAFETY: `HEAP` is a private static with no other reader/writer in the
    // program, and this function's own safety contract guarantees this runs
    // exactly once before anything else touches the allocator.
    let heap_start = unsafe { core::ptr::addr_of_mut!(HEAP.0) } as *mut u8 as usize;
    ALLOCATOR.with(|allocator| unsafe { allocator.init(heap_start, HEAP_SIZE) });
}

/// Current heap occupancy and fragmentation.
pub fn stats() -> HeapStats {
    ALLOCATOR.with(|allocator| allocator.stats())
}

/// Boot-time proof that allocation, splitting, alignment-gap handling and
/// coalescing all work on the live allocator: snapshot the heap, churn it
/// with a deliberately fragmenting pattern (interleaved sizes, odd
/// alignments, frees out of allocation order), and require that it returns
/// to the *identical* shape -- same free byte count and same number of
/// free blocks. Without coalescing, the block count would grow; with a
/// leak, the free byte count would shrink. Also verifies every allocation
/// honours its requested alignment and that writing a pattern across all
/// of them reads back intact (no two allocations overlap).
pub fn self_test() -> Result<(), &'static str> {
    use alloc::alloc::{alloc, dealloc};

    const COUNT: usize = 48;
    let before = stats();

    let mut slots: [(*mut u8, Layout); COUNT] = [(ptr::null_mut(), Layout::new::<u8>()); COUNT];
    for (i, slot) in slots.iter_mut().enumerate() {
        let size = 1 + (i * 97) % 3000;
        let align = 1usize << (i % 8); // 1..=128
        let layout = Layout::from_size_align(size, align).map_err(|_| "self-test: bad layout")?;
        // SAFETY: non-zero-size layout; result checked for null below.
        let p = unsafe { alloc(layout) };
        if p.is_null() {
            return Err("self-test: allocation failed on a nearly empty heap");
        }
        if (p as usize) % align != 0 {
            return Err("self-test: allocation returned a misaligned pointer");
        }
        // SAFETY: `p` is a fresh allocation of exactly `size` bytes.
        unsafe { ptr::write_bytes(p, (i as u8).wrapping_mul(31).wrapping_add(7), size) };
        *slot = (p, layout);
    }
    for (i, (p, layout)) in slots.iter().enumerate() {
        let want = (i as u8).wrapping_mul(31).wrapping_add(7);
        // SAFETY: still-live allocation of `layout.size()` bytes.
        let bytes = unsafe { core::slice::from_raw_parts(*p, layout.size()) };
        if bytes.iter().any(|&b| b != want) {
            return Err("self-test: allocations overlap (pattern readback mismatch)");
        }
    }
    // Free evens first, then odds in reverse: maximally fragmenting order.
    for (p, layout) in slots.iter().step_by(2) {
        // SAFETY: each pointer freed exactly once with its own layout.
        unsafe { dealloc(*p, *layout) };
    }
    for (p, layout) in slots.iter().skip(1).step_by(2).rev() {
        // SAFETY: as above.
        unsafe { dealloc(*p, *layout) };
    }

    let after = stats();
    if after.free != before.free {
        return Err("self-test: free byte count changed across alloc/free (leak)");
    }
    if after.free_blocks != before.free_blocks {
        return Err("self-test: free list did not coalesce back to its original shape");
    }
    Ok(())
}
