//! Shared fault/panic screen, used by both `#[panic_handler]` and every CPU
//! exception handler in `idt.rs`.
//!
//! Deliberately independent of the interactive `gfx::Console` -- a crash can
//! happen at any point in `Console`'s cursor/scroll state, including
//! mid-mutation, so the fault path draws directly from a raw framebuffer
//! snapshot captured once at boot rather than trying to reuse or lock
//! whatever state the crashed code left behind. It also never returns and
//! never needs to hand any lock back, so there is no risk of a fault
//! deadlocking against a lock the faulting code happened to be holding.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use crate::debugcon;
use crate::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

// Plain atomics, not a `Locked<Option<..>>`: this must be readable from a
// fault handler that may already be running with interrupts disabled and in
// a context where taking any lock -- even one that's normally safe -- risks
// deadlocking if the very code that just faulted happened to be holding it.
// A panic/fault path must never be able to block.
static FB_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static FB_LEN: AtomicUsize = AtomicUsize::new(0);
static FB_WIDTH: AtomicUsize = AtomicUsize::new(0);
static FB_HEIGHT: AtomicUsize = AtomicUsize::new(0);
static FB_STRIDE: AtomicUsize = AtomicUsize::new(0);
static FB_BGR: AtomicBool = AtomicBool::new(true);
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Capture the raw framebuffer fields the fault screen needs. Call once,
/// right after the GOP console is initialized, while Boot Services is still
/// alive -- the values themselves (a physical/MMIO pointer and plain
/// integers) remain valid long after Boot Services is gone, exactly like
/// `gfx::Console`'s own framebuffer pointer.
pub fn register(ptr: *mut u8, len: usize, width: usize, height: usize, stride: usize, bgr: bool) {
    FB_PTR.store(ptr, Ordering::SeqCst);
    FB_LEN.store(len, Ordering::SeqCst);
    FB_WIDTH.store(width, Ordering::SeqCst);
    FB_HEIGHT.store(height, Ordering::SeqCst);
    FB_STRIDE.store(stride, Ordering::SeqCst);
    FB_BGR.store(bgr, Ordering::SeqCst);
    REGISTERED.store(true, Ordering::SeqCst);
}

const FG: (u8, u8, u8) = (0xff, 0x55, 0x55); // same RGB as gfx::ERROR
const BG: (u8, u8, u8) = (0x00, 0x00, 0x00);

struct FaultWriter {
    col: usize,
    row: usize,
}

impl FaultWriter {
    fn encode(&self, c: (u8, u8, u8)) -> u32 {
        if FB_BGR.load(Ordering::SeqCst) {
            (c.2 as u32) | ((c.1 as u32) << 8) | ((c.0 as u32) << 16)
        } else {
            (c.0 as u32) | ((c.1 as u32) << 8) | ((c.2 as u32) << 16)
        }
    }

    fn put_pixel(&self, x: usize, y: usize, value: u32) {
        let ptr = FB_PTR.load(Ordering::SeqCst);
        if ptr.is_null() {
            return;
        }
        let stride = FB_STRIDE.load(Ordering::SeqCst);
        let len = FB_LEN.load(Ordering::SeqCst);
        let byte_offset = (y * stride + x) * 4;
        if byte_offset + 4 > len {
            return;
        }
        // SAFETY: `ptr` was captured from a live GOP framebuffer mapping
        // that outlives Boot Services (see `register`'s doc comment), and
        // `byte_offset` was just bounds-checked against `len`.
        unsafe {
            (ptr.add(byte_offset) as *mut u32).write_volatile(value);
        }
    }

    fn clear(&self) {
        let width = FB_WIDTH.load(Ordering::SeqCst);
        let height = FB_HEIGHT.load(Ordering::SeqCst);
        let bg = self.encode(BG);
        for y in 0..height {
            for x in 0..width {
                self.put_pixel(x, y, bg);
            }
        }
    }

    fn put_ascii(&mut self, byte: u8) {
        let cols = FB_WIDTH.load(Ordering::SeqCst) / GLYPH_WIDTH;
        let rows = FB_HEIGHT.load(Ordering::SeqCst) / GLYPH_HEIGHT;
        if self.col >= cols {
            self.newline();
        }
        if self.row >= rows {
            return; // fault screen never scrolls -- overflow is simply dropped
        }
        let bitmap = *font::glyph_for(byte);
        let fg_px = self.encode(FG);
        let bg_px = self.encode(BG);
        let base_x = self.col * GLYPH_WIDTH;
        let base_y = self.row * GLYPH_HEIGHT;
        for (row_idx, glyph_row) in bitmap.iter().enumerate() {
            let y = base_y + row_idx;
            for bit in 0..GLYPH_WIDTH {
                let x = base_x + bit;
                let set = (glyph_row >> (7 - bit)) & 1 != 0;
                self.put_pixel(x, y, if set { fg_px } else { bg_px });
            }
        }
        self.col += 1;
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
    }
}

impl Write for FaultWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            match byte {
                b'\n' => self.newline(),
                0x20..=0x7e => self.put_ascii(byte),
                _ => self.put_ascii(b'?'),
            }
        }
        Ok(())
    }
}

/// Report `title`/`args` to the debug port (always) and the framebuffer (if
/// registered), disable interrupts, and halt forever. Never returns.
pub fn fault_screen(title: &str, args: core::fmt::Arguments) -> ! {
    // SAFETY: disabling interrupts unconditionally is correct here -- this
    // function never returns, so there is no "restore the previous state"
    // to honor the way `sync::without_interrupts` does for a call that will
    // return.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };

    debugcon::write_str("\n[");
    debugcon::write_str(title);
    debugcon::write_str("] ");
    let _ = debugcon::write_fmt(args);
    debugcon::write_str("\n");

    if REGISTERED.load(Ordering::SeqCst) {
        let mut w = FaultWriter { col: 0, row: 0 };
        w.clear();
        let _ = write!(w, "{title}\n\n");
        let _ = write!(w, "{args}\n");
    }

    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    fault_screen("KERNEL PANIC", format_args!("{info}"))
}
