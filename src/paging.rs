//! Our own 4-level x86_64 page tables -- built from scratch, replacing
//! whatever flat/identity mapping UEFI's firmware left active.
//!
//! # We are not "enabling paging"
//!
//! Long mode *requires* paging to already be on (`CR0.PG=1`) -- the CPU
//! could not be executing this code otherwise. What's true today is that
//! `CR3` still points at tables *firmware* built, which this kernel has
//! never inspected and does not own. This module builds a brand new set of
//! tables and switches `CR3` to point at them instead. The mapping itself
//! stays identity (virtual == physical) for every region this kernel is
//! already relying on -- so, done correctly, the switch is invisible to
//! everything running today -- but it is now enforced through structures we
//! built, with real permission bits, and it leaves room for real guard
//! pages, which an opaque firmware-owned mapping never could.
//!
//! # No huge pages, no higher-half
//!
//! Every mapping is a plain 4 KiB page (`PT` leaf entries only, never a
//! 2 MiB `PD` leaf or a 1 GiB `PDPT` leaf) -- simpler to reason about, and
//! at this kernel's actual memory scale (a few hundred MiB to a few GiB)
//! nowhere near enough table pages to matter. There is also no higher-half
//! kernel layout: the `x86_64-unknown-uefi` target has no linker script and
//! no fixed load address, so the running image floats wherever UEFI's PE
//! loader placed it. A fixed high virtual base is real, separate future
//! work; this module's whole job is "the same addresses as today, but
//! through tables we own."
//!
//! # What gets mapped
//!
//! Every region the post-`exit_boot_services` UEFI memory map reports gets
//! identity-mapped, not just the "usable" subset `pmm.rs` tracks --
//! critically, this includes `LOADER_CODE`/`LOADER_DATA` (the running
//! kernel image itself, deliberately excluded from `pmm`'s free pool since
//! it belongs to this loaded image, but still very much needed at whatever
//! address it's executing from). Skipping it would mean the very next
//! instruction fetch after the `CR3` switch page-faults into the kernel's
//! own code. Regions typed `MMIO`/`MMIO_PORT_SPACE` get the cache-disable
//! bit set (`PCD`) in addition to present/writable, since caching real
//! device registers is undefined behavior -- functionally invisible under
//! QEMU, load-bearing on real hardware. The caller also passes explicit
//! extra ranges (the GOP framebuffer, captured by `main.rs` from
//! `Console::raw_parts` / already registered with `panic.rs`) to identity-
//! map with the same MMIO treatment, since firmware does not reliably
//! describe a GOP framebuffer's BAR as its own memory-map entry -- relying
//! on the bulk walk to happen to cover it would be exactly the kind of
//! "invisible until it isn't" gap this project's own testing philosophy
//! exists to catch before it ships silently.
//!
//! # Guard pages
//!
//! The caller also passes a list of `(addr, len)` ranges to leave
//! deliberately *unmapped* -- these are guard pages sitting directly below
//! a stack (see `gdt.rs`'s `DoubleFaultStack`/`PrimaryStack`, both
//! constructed so their guard field is guaranteed adjacent to the real
//! stack, never sharing space with unrelated static data). Any per-page
//! identity-map write inside one of these ranges is skipped, so a real
//! stack overflow walks off the bottom of the stack into a genuinely
//! unmapped page and takes a clean, reported `#PF` instead of silently
//! corrupting whatever used to be there under the old flat map.

use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};

use alloc::vec::Vec;
use uefi::table::boot::{MemoryMap, MemoryType};

use crate::{debugcon, pmm};

pub(crate) const PAGE_SIZE: u64 = 4096;
const ENTRIES_PER_TABLE: usize = 512;

// --- Page table entry flags (Intel SDM Vol 3, 4.5, Table 4-19) ---------
//
// Only the bits this kernel actually sets are named. NX (bit 63) is only
// ever set when the CPU reports support for it (CPUID 0x8000_0001
// EDX[20]) and [`load`] enables `IA32_EFER.NXE` before the `CR3` switch --
// without NXE the CPU treats bit 63 as a reserved-bit violation and every
// "protected" page instantly `#PF`s. See the "W^X" section below.

/// Present (bit 0): must be set for the CPU to treat this entry as valid at
/// all. An entry with this bit clear faults on any access through it,
/// regardless of every other bit -- exactly what a guard page relies on.
const PRESENT: u64 = 1 << 0;
/// Writable (bit 1): without it, a write through this mapping faults even
/// though the page is present -- including a write from ring 0, once
/// [`load`] has set `CR0.WP`.
const WRITABLE: u64 = 1 << 1;
/// Page-level cache disable (bit 4): forces every access through this
/// mapping to bypass the cache and go straight to the underlying memory or
/// device. Mandatory for MMIO (the framebuffer, and later the AHCI
/// controller's registers) -- caching a device register's read/write
/// protocol is undefined behavior. Meaningless (but harmless) on ordinary
/// RAM, so this is only ever set for `PageFlags::MMIO`.
const PCD: u64 = 1 << 4;
/// Mask over bits 12-51: the physical page-frame address a leaf entry
/// points at (4 KiB pages), or the physical address of the next-level
/// table for a non-leaf entry. Bits 0-11 and 52-63 are flags/reserved, not
/// part of the address.
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Execute-disable (bit 63). Only honoured -- and only legal -- with
/// `IA32_EFER.NXE` set; see [`nx_supported`].
const NO_EXECUTE: u64 = 1 << 63;

/// `IA32_EFER` MSR and its NX-enable bit (Intel SDM Vol 3, 2.2.1).
const IA32_EFER: u32 = 0xC000_0080;
const EFER_NXE: u64 = 1 << 11;
/// `CR0.WP` (bit 16): when set, ring-0 writes honour read-only pages too.
/// Without it, the kernel itself could still scribble over its own code
/// and constants; W^X would only bind user mode, which this kernel has none
/// of.
const CR0_WP: u64 = 1 << 16;

/// Set once by [`init`] from CPUID; read by [`PageFlags::bits`] and
/// [`load`]. Never changes afterwards.
static NX_SUPPORTED: AtomicBool = AtomicBool::new(false);

/// Whether this CPU supports the execute-disable bit (CPUID
/// 0x8000_0001, EDX bit 20). Every x86_64 CPU this kernel can plausibly
/// meet has it, but it is checked rather than assumed: setting bit 63 on a
/// CPU without it faults on every access.
fn detect_nx() -> bool {
    // CPUID is unprivileged and always available in long mode (the
    // intrinsic is safe to call on this target).
    let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
    if max_ext < 0x8000_0001 {
        return false;
    }
    let leaf = core::arch::x86_64::__cpuid(0x8000_0001);
    leaf.edx & (1 << 20) != 0
}

/// Whether NX is in use for this boot's page tables.
pub fn nx_enabled() -> bool {
    NX_SUPPORTED.load(Ordering::Relaxed)
}

/// Permission/caching profile for one identity-mapped region.
///
/// # W^X
/// Since v1.0 no page in this kernel is both writable and executable,
/// with one deliberately loud exception (`Rwx`, below). Executable pages
/// are exactly the running image's code sections, mapped read-only; every
/// other page -- RAM, the heap and stacks, page tables, DMA buffers, MMIO,
/// firmware tables -- is non-executable. A stray write into code, or a
/// jump into data (a corrupted function pointer, a smashed return
/// address), now faults immediately at the offending instruction with a
/// precise `#PF` error code, instead of silently running whatever bytes
/// happen to be there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageFlags {
    /// Ordinary RAM: present, writable, normally cached, non-executable.
    Ram,
    /// A memory-mapped device register window: present, writable,
    /// cache-disabled (`PCD`), non-executable. Used for the GOP
    /// framebuffer, the AHCI ABAR, the xHCI registers, and the APICs.
    Mmio,
    /// Kernel code (`.text`): present, read-only, executable.
    Code,
    /// Read-only data (`.rdata`, the PE headers, relocation data): present,
    /// read-only, non-executable.
    ReadOnly,
    /// Writable *and* executable. Used only as the fallback for the
    /// running image when its section table cannot be read (see
    /// [`image_sections`]) -- the pre-v1.0 behaviour, kept so a malformed
    /// or unexpected image layout degrades to "boots without W^X" rather
    /// than "faults on the first instruction after the `CR3` switch." Its
    /// use is always logged.
    Rwx,
}

impl PageFlags {
    fn bits(self) -> u64 {
        let nx = if nx_enabled() { NO_EXECUTE } else { 0 };
        match self {
            PageFlags::Ram => PRESENT | WRITABLE | nx,
            PageFlags::Mmio => PRESENT | WRITABLE | PCD | nx,
            PageFlags::Code => PRESENT,
            PageFlags::ReadOnly => PRESENT | nx,
            PageFlags::Rwx => PRESENT | WRITABLE,
        }
    }
}

// --- The running image's own sections (PE/COFF) ---------------------------

/// `IMAGE_SCN_CNT_CODE` / `IMAGE_SCN_MEM_EXECUTE` / `IMAGE_SCN_MEM_WRITE`
/// (PE/COFF specification, "Section Flags").
const SCN_CNT_CODE: u32 = 0x0000_0020;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

/// Read a little-endian integer out of the loaded image, bounds-checked
/// against the image's own size.
fn image_read<const N: usize>(base: u64, size: u64, offset: u64) -> Result<[u8; N], &'static str> {
    let end = offset.checked_add(N as u64).ok_or("image header offset overflow")?;
    if end > size {
        return Err("image header field lies outside the loaded image");
    }
    let mut out = [0u8; N];
    // SAFETY: `base..base+size` is the loaded image as reported by the
    // firmware's LoadedImage protocol, identity-mapped and readable under
    // the tables live while `init` runs; the read is bounds-checked above.
    unsafe { core::ptr::copy_nonoverlapping((base + offset) as *const u8, out.as_mut_ptr(), N) };
    Ok(out)
}

fn image_u16(base: u64, size: u64, offset: u64) -> Result<u16, &'static str> {
    image_read::<2>(base, size, offset).map(u16::from_le_bytes)
}

fn image_u32(base: u64, size: u64, offset: u64) -> Result<u32, &'static str> {
    image_read::<4>(base, size, offset).map(u32::from_le_bytes)
}

/// Parse the running image's PE/COFF section table (it is still sitting
/// in memory exactly as the firmware's loader laid it out) into
/// page-granular `(start, len, flags)` ranges: code sections become
/// [`PageFlags::Code`], writable sections (`.data`, which here also holds
/// `.bss`, the heap and every kernel stack) become [`PageFlags::Ram`], and
/// everything else read-only data. The PE headers themselves and any gap
/// between sections are covered by the caller mapping the whole image
/// [`PageFlags::ReadOnly`] first.
///
/// Every field is validated rather than trusted: the section alignment
/// must be page-sized (otherwise code and data could share a page, and no
/// per-page permission could be correct), each section must be
/// page-aligned and lie inside the image, and the magic numbers must
/// match. Any violation returns `Err`, and the caller falls back to
/// mapping the image [`PageFlags::Rwx`].
pub fn image_sections(base: u64, size: u64) -> Result<Vec<(u64, u64, PageFlags)>, &'static str> {
    if base % PAGE_SIZE != 0 {
        return Err("image base is not page-aligned");
    }
    if image_u16(base, size, 0)? != 0x5A4D {
        return Err("image has no MZ signature");
    }
    let pe = u64::from(image_u32(base, size, 0x3C)?);
    if image_u32(base, size, pe)? != 0x0000_4550 {
        return Err("image has no PE signature");
    }
    let coff = pe + 4;
    let section_count = u64::from(image_u16(base, size, coff + 2)?);
    let optional_size = u64::from(image_u16(base, size, coff + 16)?);
    let optional = coff + 20;
    if image_u16(base, size, optional)? != 0x020B {
        return Err("image is not PE32+");
    }
    let section_alignment = u64::from(image_u32(base, size, optional + 32)?);
    if section_alignment == 0 || section_alignment % PAGE_SIZE != 0 {
        return Err("image section alignment is smaller than a page");
    }

    let table = optional + optional_size;
    let mut out = Vec::with_capacity(section_count as usize);
    for i in 0..section_count {
        let header = table + i * 40;
        let virtual_size = u64::from(image_u32(base, size, header + 8)?);
        let virtual_address = u64::from(image_u32(base, size, header + 12)?);
        let characteristics = image_u32(base, size, header + 36)?;
        if virtual_size == 0 {
            continue;
        }
        if virtual_address % PAGE_SIZE != 0 {
            return Err("image section is not page-aligned");
        }
        let end = virtual_address.checked_add(virtual_size).ok_or("image section overflows")?;
        if end > size {
            return Err("image section extends past the loaded image");
        }
        let executable = characteristics & (SCN_MEM_EXECUTE | SCN_CNT_CODE) != 0;
        let writable = characteristics & SCN_MEM_WRITE != 0;
        let flags = match (executable, writable) {
            (true, true) => PageFlags::Rwx,
            (true, false) => PageFlags::Code,
            (false, true) => PageFlags::Ram,
            (false, false) => PageFlags::ReadOnly,
        };
        out.push((base + virtual_address, virtual_size, flags));
    }
    if !out.iter().any(|&(_, _, f)| f == PageFlags::Code) {
        return Err("image has no executable section");
    }
    Ok(out)
}

// --- Guard-paged stacks --------------------------------------------------

/// A stack with its own dedicated guard page immediately below it,
/// guaranteed adjacent by construction -- both fields live in the same
/// `#[repr(C, align(4096))]` object -- rather than assumed via "the page
/// below this address happens to be unmapped." Reused by `gdt.rs` for the
/// double-fault (`#DF`) stack and, in a later build step, the dedicated
/// mainline kernel stack.
///
/// This replaces an earlier, insufficient approach (a bare
/// `#[repr(align(16))]` byte array): at 16-byte alignment, "the page below
/// it" is not well-defined and could overlap unrelated static data instead
/// of a genuinely-unmapped guard page -- caught during planning for this
/// phase, before any code shipped with the bug.
///
/// `guard` is always exactly one page and always the *first* field, so
/// `#[repr(C)]`'s declaration-order layout guarantee, combined with the
/// struct's own 4-KiB alignment, together guarantee `stack` starts exactly
/// one page after `guard` starts -- both page-aligned, with no gap and no
/// overlap.
#[repr(C, align(4096))]
pub struct GuardedStack<const N: usize> {
    guard: [u8; PAGE_SIZE as usize],
    stack: [u8; N],
}

impl<const N: usize> GuardedStack<N> {
    pub const fn new() -> Self {
        Self {
            guard: [0; PAGE_SIZE as usize],
            stack: [0; N],
        }
    }
}

/// Given a raw pointer to a (typically `static mut`) [`GuardedStack`],
/// compute its guard page's identity-mapped address range (for `init`'s /
/// a future `guard_ranges` argument) and its stack's top-of-stack address
/// (the initial RSP/IST value to install -- stacks grow down, so this is
/// one-past-the-end, never itself dereferenced).
///
/// Takes a raw pointer, not `&GuardedStack`, so a caller holding a
/// `static mut GuardedStack` never needs to form a reference to it just to
/// learn its layout -- consistent with how this codebase's descriptor-
/// table setup (`gdt.rs::init`) already avoids referencing its
/// `static mut`s directly, only ever taking their address via `addr_of!`/
/// `addr_of_mut!`.
///
/// # Safety
/// `stack` must point at a valid `GuardedStack<N>` (it is never
/// dereferenced as a whole here -- only `addr_of_mut!` through it -- so a
/// dangling-but-correctly-typed pointer is fine as long as the memory it
/// names is what it claims to be; in practice this is always the address
/// of a real `static mut GuardedStack`).
pub unsafe fn guard_stack_layout<const N: usize>(stack: *mut GuardedStack<N>) -> ((u64, u64), u64) {
    let guard_ptr = unsafe { core::ptr::addr_of_mut!((*stack).guard) } as u64;
    let stack_ptr = unsafe { core::ptr::addr_of_mut!((*stack).stack) } as u64;
    ((guard_ptr, guard_ptr + PAGE_SIZE), stack_ptr + N as u64)
}

/// One level of an x86_64 page table: 512 eight-byte entries, exactly one
/// physical page in size. The same type is reused for the PML4, every
/// PDPT, every PD, and every PT -- the four levels differ only in how their
/// entries are interpreted (pointer to the next table, vs. for a PT, a
/// leaf 4 KiB physical page), not in their layout.
#[repr(C, align(4096))]
struct PageTable([u64; ENTRIES_PER_TABLE]);

impl PageTable {
    const fn empty() -> Self {
        PageTable([0; ENTRIES_PER_TABLE])
    }
}

/// Allocate one zeroed physical frame from `pmm` to hold a new page table,
/// returning its physical address. `pmm::alloc_frame` returns a physical
/// address that is, at the point this module calls it, still identity-
/// mapped under the *old* (firmware) tables -- so writing zeros into it
/// directly at that same address is valid, exactly like `pmm::self_test`'s
/// own pattern-write/read-back already does.
fn alloc_table_frame() -> u64 {
    let phys = pmm::alloc_frame().expect(
        "paging::init: out of physical frames while building the initial page tables -- \
         this must never happen this early in boot; something is badly wrong with the \
         memory map or the frame allocator itself",
    );
    // SAFETY: `phys` was just allocated, is 4-KiB aligned (a multiple of
    // the frame size), and is identity-mapped (writable at that same
    // address) under whatever tables are live right now.
    unsafe {
        let ptr = phys as *mut PageTable;
        ptr.write(PageTable::empty());
    }
    phys
}

/// Split a virtual address into its four 9-bit page-table indices (Intel
/// SDM Vol 3, 4.5, Figure 4-8): PML4 index (bits 47:39), PDPT index (bits
/// 38:30), PD index (bits 29:21), PT index (bits 20:12).
fn table_indices(virt: u64) -> [usize; 4] {
    [
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    ]
}

/// Walk (allocating any missing intermediate table on the way) from `pml4`
/// down to the leaf PT entry for `entry.2`, and set it. This is the one
/// place that ever mutates a page table -- every mapping request, boot-time
/// bulk identity map or later runtime call, goes through here.
///
/// # Safety
/// The tables being walked/written must not be the ones the CPU is
/// currently executing under with a mismatched notion of what's mapped
/// where (in practice: this is safe to call at any point, since it only
/// ever *adds* mappings, and the caller is responsible for not aliasing a
/// physical frame already in use as something else).
unsafe fn map_page(pml4_phys: u64, virt: u64, phys: u64, flags: PageFlags) {
    debug_assert_eq!(virt % PAGE_SIZE, 0, "paging::map_page: unaligned virtual address");
    debug_assert_eq!(phys % PAGE_SIZE, 0, "paging::map_page: unaligned physical address");

    let indices = table_indices(virt);
    let mut table_phys = pml4_phys;

    // Levels 0-2 (PML4, PDPT, PD) are non-leaf: walk to the next table,
    // allocating and linking a fresh one if the entry is not yet present.
    // Every non-leaf entry this module creates is itself present+writable
    // (a coarser permission than the leaf may end up with is fine -- the
    // leaf's own flags are what actually govern the final access).
    for &index in &indices[..3] {
        // SAFETY: `table_phys` is a page-table physical address that is
        // (by construction: everything this module allocates comes from
        // `pmm`, and every region `paging::init` maps is exactly the set of
        // physical memory the identity map covers) valid to dereference at
        // its own address under the *current* live mapping.
        let table = unsafe { &mut *(table_phys as *mut PageTable) };
        let entry = table.0[index];
        if entry & PRESENT == 0 {
            let new_table_phys = alloc_table_frame();
            table.0[index] = (new_table_phys & ADDR_MASK) | PRESENT | WRITABLE;
            table_phys = new_table_phys;
        } else {
            table_phys = entry & ADDR_MASK;
        }
    }

    // SAFETY: same reasoning as above -- `table_phys` now names the PT for
    // this virtual address's containing 2 MiB region.
    let pt = unsafe { &mut *(table_phys as *mut PageTable) };
    pt.0[indices[3]] = (phys & ADDR_MASK) | flags.bits();
}

/// Identity-map every 4 KiB page in `[start, start+len)` with `flags`,
/// skipping any page that falls inside one of `guard_ranges` -- the one
/// mechanism by which a guard page ends up genuinely unmapped instead of
/// merely "not yet visited by the bulk walk."
fn map_identity_range(pml4_phys: u64, start: u64, len: u64, flags: PageFlags, guard_ranges: &[(u64, u64)]) {
    let start = start - (start % PAGE_SIZE);
    let end = (start + len).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let mut addr = start;
    while addr < end {
        let in_guard = guard_ranges
            .iter()
            .any(|&(g_start, g_end)| addr >= g_start && addr < g_end);
        if !in_guard {
            // SAFETY: see `map_page`'s own safety comment; `pml4_phys` was
            // freshly allocated by this same boot-time build and nothing
            // else is concurrently touching it (single-threaded boot).
            unsafe { map_page(pml4_phys, addr, addr, flags) };
        }
        addr += PAGE_SIZE;
    }
}

/// Default flags for a firmware memory-map region. `image_protected` is
/// whether the running image's own sections are about to be mapped with
/// precise permissions: if they are not, `LOADER_CODE` (which contains the
/// image) must stay executable or the kernel faults on its next
/// instruction fetch after the `CR3` switch.
fn flags_for_memory_type(ty: MemoryType, image_protected: bool) -> PageFlags {
    if ty == MemoryType::MMIO || ty == MemoryType::MMIO_PORT_SPACE {
        PageFlags::Mmio
    } else if ty == MemoryType::LOADER_CODE && !image_protected {
        PageFlags::Rwx
    } else {
        PageFlags::Ram
    }
}

/// Build a brand new set of page tables that identity-map every region the
/// firmware's final memory map reports, plus `extra_ranges` (regions --
/// like the GOP framebuffer -- that firmware does not reliably describe as
/// their own memory-map entry), minus `guard_ranges` (pages deliberately
/// left unmapped), with the running image (`image` = its base and size, as
/// reported by the firmware's LoadedImage protocol) mapped section by
/// section under W^X. Returns the new PML4's physical address, ready to be
/// passed to [`load`].
///
/// Pure allocation and memory writes into freshly-obtained frames -- the
/// CPU is still running under whatever tables were live before this call
/// (today, firmware's), so nothing here changes what's actually mapped
/// until [`load`] runs. Must be called after `pmm::init` (this function
/// allocates every table page through `pmm::alloc_frame`).
///
/// # Safety
/// Must be called after `pmm::init` and before [`load`]; must be called at
/// most once (it always allocates a fresh PML4, so calling it twice just
/// leaks the first set of tables rather than corrupting anything, but
/// there is no legitimate reason to).
pub unsafe fn init(
    mmap: &MemoryMap,
    extra_ranges: &[(u64, u64, PageFlags)],
    guard_ranges: &[(u64, u64)],
    image: Option<(u64, u64)>,
) -> u64 {
    NX_SUPPORTED.store(detect_nx(), Ordering::Relaxed);

    // Decide how the image will be mapped before anything is mapped at
    // all, since that decision also changes how `LOADER_CODE` is treated
    // in the bulk walk below.
    let sections = match image {
        Some((base, size)) => match image_sections(base, size) {
            Ok(sections) => Some((base, size, sections)),
            Err(msg) => {
                debugcon::write_str("[paging] WARNING: cannot read image sections (");
                debugcon::write_str(msg);
                debugcon::write_str(") -- image stays writable+executable, W^X disabled for it\n");
                None
            }
        },
        None => {
            debugcon::write_str("[paging] WARNING: image location unknown -- W^X disabled for it\n");
            None
        }
    };
    let image_protected = sections.is_some();

    let pml4_phys = alloc_table_frame();

    let mut pages_mapped: u64 = 0;
    for d in mmap.entries() {
        let len = d.page_count * PAGE_SIZE;
        if len == 0 {
            continue;
        }
        let flags = flags_for_memory_type(d.ty, image_protected);
        map_identity_range(pml4_phys, d.phys_start, len, flags, guard_ranges);
        pages_mapped += d.page_count;
    }

    for &(start, len, flags) in extra_ranges {
        if len == 0 {
            continue;
        }
        map_identity_range(pml4_phys, start, len, flags, guard_ranges);
        pages_mapped += len.div_ceil(PAGE_SIZE);
    }

    // The image last, so its precise per-section permissions overwrite
    // whatever the bulk walk gave the same pages: first the whole image
    // read-only (covering the PE headers and inter-section padding), then
    // each section with its own flags. Guard pages are skipped here too --
    // both kernel stacks and their guards live inside `.data`.
    if let Some((base, size, sections)) = &sections {
        map_identity_range(pml4_phys, *base, *size, PageFlags::ReadOnly, guard_ranges);
        for &(start, len, flags) in sections {
            map_identity_range(pml4_phys, start, len, flags, guard_ranges);
            let label = match flags {
                PageFlags::Code => "RX",
                PageFlags::Ram => "RW",
                PageFlags::ReadOnly => "R",
                PageFlags::Rwx => "RWX (section requests write+execute)",
                PageFlags::Mmio => "MMIO",
            };
            let _ = debugcon::write_fmt(format_args!(
                "[paging] image section 0x{start:x}..0x{:x} {label}\n",
                start + len.div_ceil(PAGE_SIZE) * PAGE_SIZE,
            ));
        }
    }

    debugcon::write_str("\n[paging] tables built: pml4=0x");
    let _ = debugcon::write_fmt(format_args!("{pml4_phys:x} pages_mapped~={pages_mapped}\n"));

    // Logged unconditionally (not just under a test feature): knowing
    // exactly where every guard page landed is generally useful boot
    // diagnostics, and it's what lets `tests/fault_test.py`'s guard-page
    // overflow scenario assert the faulting address (`cr2`) against a
    // kernel-reported range instead of a guessed/hardcoded one -- ground
    // truth from the same run, not an assumption about link-time layout
    // that could silently drift out of sync with reality.
    for &(g_start, g_end) in guard_ranges {
        debugcon::write_str("[paging] guard page: 0x");
        let _ = debugcon::write_fmt(format_args!("{g_start:x}..0x{g_end:x}\n"));
    }

    pml4_phys
}

/// Map a single already-page-aligned virtual/physical range into an
/// already-active set of tables (i.e. after [`load`] has run), for a
/// runtime consumer that discovers a physical address after boot -- the
/// AHCI controller's PCI BAR, in the storage phase this exists ahead of.
/// Not used by the boot-time bulk identity map itself (that goes through
/// [`init`]/`map_identity_range` directly, before any table is live).
///
/// # Safety
/// Must be called after [`load`]; `virt`/`phys` must both be 4-KiB aligned;
/// the caller is responsible for not aliasing a physical range already
/// mapped as something else with incompatible flags.
pub unsafe fn map(virt: u64, phys: u64, len: u64, flags: PageFlags) -> Result<(), &'static str> {
    if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err("paging::map: virt/phys must be 4-KiB aligned");
    }
    if virt != phys {
        return Err("paging::map: only identity mappings are supported");
    }
    let pml4_phys = current_pml4_phys();
    map_identity_range(pml4_phys, virt, len, flags, &[]);
    // These tables are live: any page here that was already mapped (an
    // MMIO window the firmware memory map also described, say) may have a
    // stale translation cached with its old flags. Invalidate each one.
    let end = (virt + len).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let mut page = virt;
    while page < end {
        // SAFETY: `invlpg` only drops a cached translation; it cannot
        // change what is mapped.
        unsafe { core::arch::asm!("invlpg [{}]", in(reg) page, options(nostack, preserves_flags)) };
        page += PAGE_SIZE;
    }
    Ok(())
}

/// Read `CR3` back to find the currently-active PML4's physical address
/// (masking off the low control bits `CR3` also carries -- PCD/PWT when
/// PCID is disabled, which it is here). Used by [`map`] so a runtime caller
/// never needs to thread the boot-time `pml4_phys` value through every
/// layer that might eventually need to add a mapping.
fn current_pml4_phys() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags)) };
    cr3 & ADDR_MASK
}

/// Switch the CPU to the page tables built by [`init`]. This is the single
/// highest-blast-radius instruction in this kernel so far: a wrong mapping
/// here can fault on the very next instruction fetch, into a fault handler
/// whose own code just became unmapped -- worse than any failure mode
/// Phase 1 had, since Phase 1 could always fall back on the firmware's
/// still-live identity map under a bug. `mov cr3` implicitly flushes the
/// entire TLB (no explicit `invlpg` needed for a full table switch), so
/// every subsequent memory access is translated through the new tables
/// starting with the instruction immediately after this one.
///
/// Deliberately **not** marked `options(nomem)`, unlike most other raw asm
/// in this codebase: changing `CR3` changes the meaning of every memory
/// access that follows, which is exactly the kind of effect `nomem` tells
/// the compiler *isn't* happening. Getting this wrong is not hypothetical --
/// see `sync.rs`'s `without_interrupts` and `CHANGES.md`'s account of the
/// missing-compiler-fence bug that silently broke the keyboard driver for
/// exactly this reason. `compiler_fence` on both sides makes the same
/// guarantee explicit here: the optimizer may not reorder or cache any
/// memory operation across this call in either direction.
///
/// # Safety
/// `pml4_phys` must be a value returned by [`init`] (or otherwise point at
/// a fully-built, valid PML4). Must be called after `gdt::load`/`idt::load`
/// (so this kernel's own fault handler is already the live safety net if
/// the new mapping is subtly wrong) and with interrupts still disabled.
pub unsafe fn load(pml4_phys: u64) {
    // NX must be architecturally enabled *before* any table using bit 63
    // becomes live, or the very first translation through one is a
    // reserved-bit fault.
    if nx_enabled() {
        unsafe {
            let (lo, hi): (u32, u32);
            core::arch::asm!("rdmsr", in("ecx") IA32_EFER, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
            let efer = (u64::from(hi) << 32 | u64::from(lo)) | EFER_NXE;
            core::arch::asm!("wrmsr", in("ecx") IA32_EFER, in("eax") efer as u32, in("edx") (efer >> 32) as u32, options(nomem, nostack, preserves_flags));
        }
    }

    compiler_fence(Ordering::SeqCst);
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
    }
    compiler_fence(Ordering::SeqCst);

    // Make ring 0 honour read-only pages. Done after the switch so that it
    // only ever applies to tables this module built: firmware's tables may
    // map things read-only that this kernel never had a reason to know
    // about.
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 |= CR0_WP;
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
    }

    debugcon::write_str("\n[paging] active, cr3=0x");
    let _ = debugcon::write_fmt(format_args!(
        "{pml4_phys:x} nx={} wp=on\n",
        if nx_enabled() { "on" } else { "unsupported" }
    ));
}
