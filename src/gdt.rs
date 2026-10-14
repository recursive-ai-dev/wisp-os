//! Our own Global Descriptor Table, Task State Segment, and Interrupt Stack
//! Table -- built from scratch, not borrowed from any other kernel or
//! bootloader.
//!
//! # Why this exists at all, in long mode
//!
//! Segmentation is almost entirely disabled in 64-bit mode: the CPU ignores
//! the base/limit fields of CS/DS/ES/SS/GS/FS for addressing and bounds
//! checks. But a GDT is still mandatory, for two reasons that have nothing
//! to do with old-style segmentation:
//!
//! 1. Every code and data access still goes through a *selector* loaded into
//!    a segment register, and that selector's GDT entry is where the CPU
//!    reads the privilege level (DPL) and the "is this a 64-bit code
//!    segment" (L) bit that puts the CPU into 64-bit instruction decoding in
//!    the first place. `iretq`/`retfq`/interrupt gates all consult it.
//! 2. The Task State Segment -- which on x86_64 no longer holds a full task
//!    switch context, only the privilege-level stack pointers (RSP0-2) and
//!    the Interrupt Stack Table (IST1-7) -- is itself installed *through* a
//!    GDT entry (`ltr`), because that's the only mechanism the ISA provides
//!    for pointing the CPU at it.
//!
//! # Why a TSS/IST at all
//!
//! We only use it for one thing: giving the double-fault handler (vector 8,
//! `#DF`) a dedicated 16 KiB stack via IST1, independent of whatever stack
//! the interrupted code was using. Without this, a stack overflow that
//! itself triggers `#DF` (because the CPU can't push the exception frame
//! onto the already-exhausted stack) fails silently and triple-faults the
//! whole machine into a hard reset -- exactly the "no debugger, no output,
//! nothing to look at" failure mode this whole phase is designed to avoid.
//! Every other vector runs on whatever stack was already active (IST=0),
//! which is fine for a single-ring-0-stack kernel with no user mode yet.

use core::arch::asm;
use core::mem::size_of;

use crate::paging::{self, GuardedStack};

/// GDT selector for the 64-bit kernel code segment. Loaded into CS by every
/// interrupt/exception gate in `idt.rs`, and by the `retfq` reload below.
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
/// GDT selector for the kernel data segment. Loaded into SS (and, for
/// hygiene, DS/ES/FS/GS) after the CS reload. Its base/limit are irrelevant
/// in long mode; only the descriptor's presence, type, and DPL matter.
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
/// GDT selector for the TSS. Loaded into the task register via `ltr`.
pub const TSS_SELECTOR: u16 = 0x18;

/// Size, in bytes, of the dedicated double-fault stack. Double-fault
/// handling itself does very little work (read the frame, call into the
/// fault screen, halt), so this is generous headroom, not a tight budget.
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

/// IST index (1-7) that the double-fault gate in `idt.rs` is configured to
/// use. IST0 is reserved (it means "don't switch stacks" in a gate
/// descriptor), so the first usable index is 1.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 1;

/// Guard-paged (see `paging::GuardedStack`'s doc comment for why a bare
/// `#[repr(align(16))]` array -- what this used to be -- was not good
/// enough): the page immediately below this stack is left deliberately
/// unmapped by `paging::init` (via [`double_fault_stack_guard_range`]), so
/// a stack overflow that itself triggers `#DF` -- the one scenario IST1
/// exists to survive -- walks off the bottom into a genuinely unmapped
/// page and takes a clean, reported fault instead of silently corrupting
/// whatever used to sit there.
static mut DOUBLE_FAULT_STACK: GuardedStack<DOUBLE_FAULT_STACK_SIZE> = GuardedStack::new();

/// The double-fault stack's guard page's identity-mapped address range,
/// for `paging::init`'s `guard_ranges` argument. Safe to call at any point
/// (including before [`init`] runs) -- the static's address is fixed at
/// link time regardless of whether its contents have been initialized yet.
pub fn double_fault_stack_guard_range() -> (u64, u64) {
    // SAFETY: `addr_of_mut!` on the static, passed straight into
    // `guard_stack_layout` without ever forming a `&`/`&mut` reference to
    // `DOUBLE_FAULT_STACK` itself, is exactly the pattern
    // `guard_stack_layout` requires and this module already relies on
    // elsewhere (see `init`, below).
    unsafe {
        let stack_ptr = core::ptr::addr_of_mut!(DOUBLE_FAULT_STACK);
        paging::guard_stack_layout(stack_ptr).0
    }
}

/// Size, in bytes, of the dedicated mainline kernel stack -- the stack the
/// shell and every WISP script run on for the rest of this program's life,
/// once `main.rs`'s one-way stack switch (Build Order Step 4) lands on it.
/// Generous relative to this kernel's actual call depth (the interpreter's
/// own recursion is bounded by `value.rs`'s `MAX_VALUE_DEPTH`, and nothing
/// in this codebase approaches even a fraction of this), matching the same
/// "headroom, not a tight budget" reasoning as `DOUBLE_FAULT_STACK_SIZE`.
/// Must be a multiple of 16 -- see [`mainline_stack_top`]'s doc comment.
const MAINLINE_STACK_SIZE: usize = 256 * 1024;

/// Guard-paged for the identical reason as `DOUBLE_FAULT_STACK`, but this
/// one matters far more in practice: this is the stack the shell and every
/// WISP script actually run on, so a real runaway-recursion bug here (an
/// interpreter bug, not a deliberate test) now takes a clean, reported
/// fault instead of silently corrupting whatever used to sit below a bare
/// array.
static mut MAINLINE_STACK: GuardedStack<MAINLINE_STACK_SIZE> = GuardedStack::new();

/// The mainline stack's guard page's identity-mapped address range, for
/// `paging::init`'s `guard_ranges` argument -- see
/// [`double_fault_stack_guard_range`] for the identical reasoning.
pub fn mainline_stack_guard_range() -> (u64, u64) {
    // SAFETY: same reasoning as `double_fault_stack_guard_range`.
    unsafe {
        let stack_ptr = core::ptr::addr_of_mut!(MAINLINE_STACK);
        paging::guard_stack_layout(stack_ptr).0
    }
}

/// The mainline stack's top-of-stack address (one-past-the-end; stacks
/// grow down) -- the value `main.rs`'s one-way trampoline loads directly
/// into RSP. Exposed separately from the double-fault stack's top (which
/// only the TSS ever needs) because this value is consumed by hand-written
/// trampoline asm, not through any GDT/TSS structure.
///
/// Guaranteed 16-byte aligned: `GuardedStack`'s `stack` field starts
/// exactly one page after the struct's own 4-KiB-aligned base (itself a
/// multiple of 16), and `MAINLINE_STACK_SIZE` is a multiple of 16 -- so
/// `stack_start + MAINLINE_STACK_SIZE` is 16-aligned too. `main.rs`'s
/// trampoline depends on exactly this property.
pub fn mainline_stack_top() -> u64 {
    // SAFETY: same reasoning as `double_fault_stack_guard_range`.
    unsafe {
        let stack_ptr = core::ptr::addr_of_mut!(MAINLINE_STACK);
        paging::guard_stack_layout(stack_ptr).1
    }
}

/// The x86_64 Task State Segment. Despite the name, this is *not* used for
/// task switching (that mechanism was removed from long mode) -- only the
/// privilege-stack and interrupt-stack tables and the I/O permission bitmap
/// base survive. Layout and field sizes are architectural (Intel SDM Vol 3,
/// section 8.7), not a choice we get to make.
#[repr(C, packed)]
struct TaskStateSegment {
    reserved_0: u32,
    /// RSP0-2: stack pointers to load on a privilege-level change into rings
    /// 0-2. Unused here (no user mode yet; ring 0 code never changes
    /// privilege on interrupt), left zeroed.
    privilege_stack_table: [u64; 3],
    reserved_1: u64,
    /// IST1-7: stack pointers interrupt gates can request unconditionally
    /// (independent of the interrupted code's own stack), selected by the
    /// gate descriptor's IST field. Only IST1 is populated, for `#DF`.
    interrupt_stack_table: [u64; 7],
    reserved_2: u64,
    reserved_3: u16,
    /// Offset (from the start of the TSS) to the I/O permission bitmap.
    /// Setting this to `size_of::<TaskStateSegment>()` places it just past
    /// the end of the TSS, which the CPU correctly treats as "no I/O
    /// bitmap present" (every port is then implicitly denied at CPL>0, and
    /// this kernel never drops below CPL0 anyway, where the bitmap is not
    /// consulted at all).
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        TaskStateSegment {
            reserved_0: 0,
            privilege_stack_table: [0; 3],
            reserved_1: 0,
            interrupt_stack_table: [0; 7],
            reserved_2: 0,
            reserved_3: 0,
            iomap_base: size_of::<TaskStateSegment>() as u16,
        }
    }
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();

// --- GDT entry encoding -----------------------------------------------
//
// Standard 8-byte segment descriptor layout (Intel SDM Vol 3, 3.4.5):
//   bits 0:15   limit 0:15
//   bits 16:39  base 0:23
//   bits 40:47  access byte: P(47) DPL(46:45) S(44) Type(43:40)
//   bits 48:51  limit 16:19
//   bits 52:55  flags: G(55) D/B(54) L(53) AVL(52)
//   bits 56:63  base 24:31
//
// Base/limit are architecturally ignored for code and data segments once
// the CPU is in 64-bit mode (SDM Vol 3, 3.4.5: "In 64-bit mode... the
// processor does not perform segment limit checks at runtime for CS, DS,
// ES, SS, GS, FS"), but every field is still filled in with the
// conventional "flat" values (base 0, limit 0xFFFFF, G=1) rather than left
// at zero, purely so a hex dump of this GDT looks like every other x86_64
// kernel's and nothing here depends on an unusual, easy-to-typo encoding
// choice mattering.

const fn segment_descriptor(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    let mut d: u64 = 0;
    d |= (limit as u64) & 0xFFFF;
    d |= ((base as u64) & 0xFF_FFFF) << 16;
    d |= (access as u64) << 40;
    d |= (((limit as u64) >> 16) & 0xF) << 48;
    d |= ((flags as u64) & 0xF) << 52;
    d |= (((base as u64) >> 24) & 0xFF) << 56;
    d
}

// Access byte: P=1, DPL=00, S=1 (code/data, not a system descriptor).
/// Type 1010: code, executable, readable, non-conforming.
const ACCESS_KERNEL_CODE: u8 = 0b1_00_1_1010;
/// Type 0010: data, read/write.
const ACCESS_KERNEL_DATA: u8 = 0b1_00_1_0010;
/// Flags nibble (G, D/B, L, AVL): G=1, D/B=0, L=1 -- this is what actually
/// tells the CPU "this is a 64-bit code segment" and switches instruction
/// decoding into long mode when CS is loaded from it.
const FLAGS_CODE_64BIT: u8 = 0b1_0_1_0;
/// Flags nibble for the data segment: G=1, D/B=0, L=0 (L must be 0 outside
/// 64-bit code segments -- SDM Vol 3, 3.4.5 marks it reserved there).
const FLAGS_DATA: u8 = 0b1_0_0_0;

const GDT_NULL: u64 = 0;
const GDT_KERNEL_CODE: u64 =
    segment_descriptor(0, 0xF_FFFF, ACCESS_KERNEL_CODE, FLAGS_CODE_64BIT);
const GDT_KERNEL_DATA: u64 = segment_descriptor(0, 0xF_FFFF, ACCESS_KERNEL_DATA, FLAGS_DATA);

/// Access byte for a 64-bit TSS descriptor: P=1, DPL=00, S=0 (system
/// descriptor), Type=1001 ("64-bit TSS (available)" -- SDM Vol 3, Table
/// 3-2). S=0 is what makes this a *system* descriptor, which is why it
/// needs a second GDT slot for the upper 32 bits of its base address that a
/// normal code/data descriptor doesn't have room for.
const ACCESS_TSS: u8 = 0b1_00_0_1001;

/// Our GDT: null, kernel code (0x08), kernel data (0x10), TSS low (0x18),
/// TSS high. Five `u64` slots; the TSS descriptor occupies the last two.
#[repr(C, align(16))]
struct Gdt([u64; 5]);

static mut GDT: Gdt = Gdt([GDT_NULL, GDT_KERNEL_CODE, GDT_KERNEL_DATA, 0, 0]);

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// Build the GDT and TSS contents. Pure memory writes -- no CPU
/// descriptor-table state (`lgdt`/`ltr`) changes here, so this is safe to
/// call at any point, including while Boot Services is still active. Must
/// be called before [`load`].
///
/// # Safety
/// Must be called exactly once, before [`load`], and after
/// [`crate::allocator::init`] has nothing to do with this function's safety
/// (it touches no heap memory) -- the real precondition is simply
/// single-threaded, one-time initialization of the `static mut` GDT/TSS.
pub unsafe fn init() {
    // SAFETY: single-core, single-threaded initialization; nothing else
    // reads or writes `DOUBLE_FAULT_STACK`/`TSS`/`GDT` concurrently, and
    // this is the only place that ever mutates them.
    unsafe {
        // One-past-the-end address: stacks grow down, so this is the
        // initial RSP value the CPU should switch to for IST1, not a
        // pointer that's ever dereferenced itself.
        let stack_ptr = core::ptr::addr_of_mut!(DOUBLE_FAULT_STACK);
        let (_guard_range, stack_top) = paging::guard_stack_layout(stack_ptr);
        // IST slots are 1-indexed in the field name but 0-indexed in the
        // array; IST1 is `interrupt_stack_table[0]`.
        TSS.interrupt_stack_table[(DOUBLE_FAULT_IST_INDEX - 1) as usize] = stack_top;

        let tss_base = core::ptr::addr_of!(TSS) as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u32;
        let low = segment_descriptor(
            (tss_base & 0xFFFF_FFFF) as u32,
            tss_limit,
            ACCESS_TSS,
            0, // G=0: the TSS is small and its limit is byte-exact, not page-granular.
        );
        let high = (tss_base >> 32) as u64;
        GDT.0[3] = low;
        GDT.0[4] = high;
    }
}

/// Load the GDT and TSS into the CPU, reload every segment register, and
/// point the task register at the TSS. This is the first point in the boot
/// sequence that live CPU descriptor-table state changes, and per the boot
/// plan it happens strictly after `exit_boot_services` returns -- loading a
/// new GDT while firmware code might still run against its own would be
/// asking for trouble for no benefit.
///
/// # Safety
/// Must be called after [`init`], after `exit_boot_services`, and before
/// interrupts are enabled or [`crate::idt::load`] is called (the IDT's
/// interrupt gates name [`KERNEL_CODE_SELECTOR`], which must already be a
/// valid, loaded GDT entry by the time any interrupt can fire).
pub unsafe fn load() {
    unsafe {
        let ptr = DescriptorTablePointer {
            limit: (size_of::<Gdt>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };
        asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));

        // x86_64 has no `mov cs, imm`/short-form `ljmp` that can load an
        // immediate selector into CS (the immediate-operand far jump only
        // exists in legacy/compatibility encodings). The standard way to
        // reload CS in long mode is a far *return*: push the target
        // selector and offset, then `retfq` pops both and reloads CS as
        // part of the control transfer, landing on the label right after.
        asm!(
            "lea {tmp}, [rip + 55f]",
            "push {sel}",
            "push {tmp}",
            "retfq",
            "55:",
            sel = in(reg) KERNEL_CODE_SELECTOR as u64,
            // Deliberately `out`, not `lateout`: `lateout` tells the
            // register allocator it may reuse an `in` operand's register
            // because the write happens only after every input is done
            // being read. That's false here -- the `lea` writing `tmp` is
            // the *first* instruction in this block, executed before
            // `sel` is read by the `push` two lines later. Using
            // `lateout` here was a real bug: it let the allocator assign
            // `sel` and `tmp` to the *same* register, so `lea` silently
            // clobbered `sel` before it was pushed, and both pushes ended
            // up pushing the same (offset) value -- loading CS from
            // garbage and triple-faulting the machine. Plain `out` must
            // never share a register with a live `in`, which is what
            // this actually needs.
            tmp = out(reg) _,
            options(preserves_flags),
        );

        asm!(
            "mov ds, {sel:x}",
            "mov es, {sel:x}",
            "mov fs, {sel:x}",
            "mov gs, {sel:x}",
            "mov ss, {sel:x}",
            sel = in(reg) KERNEL_DATA_SELECTOR,
            options(nostack, preserves_flags),
        );

        asm!("ltr {sel:x}", sel = in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}
