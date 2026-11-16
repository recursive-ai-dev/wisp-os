//! Kernel log ring buffer and 16550 UART serial console (`COM1`, `0x3F8`).
//!
//! Every `debugcon::write_*` call routes through [`write_str`], which fans
//! each byte out to:
//! 1. The in-memory [`LogRing`] (32 KiB static circular buffer, readable at
//!    any time via the shell's `dmesg` command), and
//! 2. The 16550 UART on `COM1` (`0x3F8`, 115200 8N1) when detected at boot
//!    via scratch-register and loopback self-test.
//!
//! Zero heap allocation in the write path: safe to call before the heap is
//! initialized, with interrupts disabled, and inside CPU fault/panic handlers.

/// Capacity of the static kernel log ring buffer (32 KiB).
pub const RING_CAPACITY: usize = 32 * 1024;

/// Hardware-independent circular byte buffer backing `dmesg`.
pub struct LogRing {
    buf: [u8; RING_CAPACITY],
    /// Index of the oldest byte currently stored when `len == RING_CAPACITY`,
    /// or `0` before the buffer has wrapped.
    head: usize,
    /// Number of valid bytes currently stored (`0..=RING_CAPACITY`).
    len: usize,
    /// Total bytes ever written (including overwritten bytes).
    total_written: u64,
}

impl Default for LogRing {
    fn default() -> Self {
        Self::new()
    }
}

impl LogRing {
    pub const fn new() -> Self {
        Self {
            buf: [0; RING_CAPACITY],
            head: 0,
            len: 0,
            total_written: 0,
        }
    }

    /// Append `bytes` to the ring, overwriting the oldest bytes on wraparound.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push_byte(b);
        }
    }

    #[inline]
    pub fn push_byte(&mut self, byte: u8) {
        let write_idx = (self.head + self.len) % RING_CAPACITY;
        self.buf[write_idx] = byte;
        if self.len < RING_CAPACITY {
            self.len += 1;
        } else {
            self.head = (self.head + 1) % RING_CAPACITY;
        }
        self.total_written = self.total_written.wrapping_add(1);
    }

    /// Number of bytes currently retained in the ring (`0..=RING_CAPACITY`).
    pub const fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Total bytes ever written since boot (exceeds `len()` once the ring wraps).
    pub const fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Return the buffered contents as one or two chronological slices.
    pub fn slices(&self) -> (&[u8], &[u8]) {
        if self.len == 0 {
            (&[], &[])
        } else if self.head + self.len <= RING_CAPACITY {
            (&self.buf[self.head..self.head + self.len], &[])
        } else {
            let first_len = RING_CAPACITY - self.head;
            let second_len = self.len - first_len;
            (&self.buf[self.head..RING_CAPACITY], &self.buf[..second_len])
        }
    }

    /// Visit the buffered log in chronological order as printable ASCII/UTF-8
    /// chunks. If wraparound split a multi-byte UTF-8 sequence at the very
    /// start of the ring, leading continuation bytes (`10xxxxxx`) are skipped.
    pub fn visit_bytes(&self, mut f: impl FnMut(&[u8])) {
        let (first, second) = self.slices();
        let skip = first
            .iter()
            .take_while(|&&b| (b & 0b1100_0000) == 0b1000_0000)
            .count();
        let first = &first[skip..];
        if !first.is_empty() {
            f(first);
        }
        if !second.is_empty() {
            f(second);
        }
    }
}

#[cfg(target_os = "uefi")]
mod imp {
    use super::LogRing;
    use core::arch::asm;
    use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};

    const COM1_BASE: u16 = 0x3F8;
    const COM1_DATA: u16 = COM1_BASE;
    const COM1_IER: u16 = COM1_BASE + 1;
    const COM1_FCR: u16 = COM1_BASE + 2;
    const COM1_LCR: u16 = COM1_BASE + 3;
    const COM1_MCR: u16 = COM1_BASE + 4;
    const COM1_LSR: u16 = COM1_BASE + 5;
    const COM1_SCR: u16 = COM1_BASE + 7;

    const LSR_DATA_READY: u8 = 1 << 0;
    const LSR_TX_EMPTY: u8 = 1 << 5;
    const MAX_TX_SPINS: usize = 10_000;
    const MAX_LOOPBACK_SPINS: usize = 1_000;

    static SERIAL_READY: AtomicBool = AtomicBool::new(false);
    static RING_BUSY: AtomicBool = AtomicBool::new(false);
    static mut RING: LogRing = LogRing::new();

    #[inline]
    fn out8(port: u16, val: u8) {
        unsafe {
            asm!(
                "out dx, al",
                in("dx") port,
                in("al") val,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    #[inline]
    fn in8(port: u16) -> u8 {
        let val: u8;
        unsafe {
            asm!(
                "in al, dx",
                in("dx") port,
                out("al") val,
                options(nomem, nostack, preserves_flags),
            );
        }
        val
    }

    fn serial_write_raw(byte: u8) {
        for _ in 0..MAX_TX_SPINS {
            if in8(COM1_LSR) & LSR_TX_EMPTY != 0 {
                out8(COM1_DATA, byte);
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn serial_write_byte(byte: u8) {
        if !SERIAL_READY.load(Ordering::Relaxed) {
            return;
        }
        if byte == b'\n' {
            serial_write_raw(b'\r');
        }
        serial_write_raw(byte);
    }

    /// Probe and initialize the 16550 UART at `COM1` (`0x3F8`, 115200 8N1).
    /// Silently leaves serial output disabled if the scratch-register or
    /// loopback check fails (e.g. hardware with no legacy UART).
    pub fn init_serial() -> bool {
        // Disable UART interrupts.
        out8(COM1_IER, 0x00);

        // Scratch register probe: verify an I/O decode responds at 0x3FF.
        out8(COM1_SCR, 0x5A);
        if in8(COM1_SCR) != 0x5A {
            return false;
        }
        out8(COM1_SCR, 0xA5);
        if in8(COM1_SCR) != 0xA5 {
            return false;
        }
        out8(COM1_SCR, 0x00);

        // Set DLAB=1 and program divisor 1 (115200 baud).
        out8(COM1_LCR, 0x80);
        out8(COM1_DATA, 0x01);
        out8(COM1_IER, 0x00);

        // 8 data bits, no parity, 1 stop bit (DLAB=0).
        out8(COM1_LCR, 0x03);
        // Enable FIFO, clear TX/RX FIFOs, 14-byte threshold.
        out8(COM1_FCR, 0xC7);

        // Loopback self-test (MCR: RTS | OUT1 | OUT2 | LOOP).
        out8(COM1_MCR, 0x1E);
        if in8(COM1_LSR) & LSR_DATA_READY != 0 {
            let _ = in8(COM1_DATA);
        }
        out8(COM1_DATA, 0xAE);
        let mut loopback_ok = false;
        for _ in 0..MAX_LOOPBACK_SPINS {
            if in8(COM1_LSR) & LSR_DATA_READY != 0 {
                loopback_ok = in8(COM1_DATA) == 0xAE;
                break;
            }
            core::hint::spin_loop();
        }
        if !loopback_ok {
            out8(COM1_MCR, 0x00);
            return false;
        }

        // Normal operating mode (DTR | RTS | OUT2).
        out8(COM1_MCR, 0x0B);
        SERIAL_READY.store(true, Ordering::SeqCst);
        true
    }

    /// Clear the re-entrancy guard before writing a fatal fault/panic report.
    /// Safe because `fault_screen` disables interrupts and never returns.
    pub fn force_unlock_for_fault() {
        RING_BUSY.store(false, Ordering::SeqCst);
    }

    /// Record `s` in the 32 KiB ring buffer and transmit over `COM1` if active.
    pub fn record_str(s: &str) {
        let rflags: u64;
        unsafe {
            asm!("pushfq", "pop {}", out(reg) rflags, options(preserves_flags));
            asm!("cli", options(nomem, nostack, preserves_flags));
        }
        compiler_fence(Ordering::SeqCst);

        if !RING_BUSY.swap(true, Ordering::Acquire) {
            unsafe {
                let ring = &mut *core::ptr::addr_of_mut!(RING);
                ring.write_bytes(s.as_bytes());
            }
            RING_BUSY.store(false, Ordering::Release);
        }

        for byte in s.bytes() {
            serial_write_byte(byte);
        }

        compiler_fence(Ordering::SeqCst);
        if rflags & (1 << 9) != 0 {
            unsafe {
                asm!("sti", options(nomem, nostack, preserves_flags));
            }
        }
    }

    /// Snapshot the current log ring contents and total bytes ever written.
    pub fn with_ring<R>(f: impl FnOnce(&LogRing) -> R) -> R {
        crate::sync::without_interrupts(|| {
            if !RING_BUSY.swap(true, Ordering::Acquire) {
                let res = unsafe { f(&*core::ptr::addr_of!(RING)) };
                RING_BUSY.store(false, Ordering::Release);
                res
            } else {
                let empty = LogRing::new();
                f(&empty)
            }
        })
    }
}

#[cfg(target_os = "uefi")]
pub use imp::{force_unlock_for_fault, init_serial, record_str, with_ring};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn collect(ring: &LogRing) -> Vec<u8> {
        let mut out = Vec::new();
        ring.visit_bytes(|chunk| out.extend_from_slice(chunk));
        out
    }

    #[test]
    fn ring_preserves_order_before_and_after_wraparound() {
        let mut ring = LogRing::new();
        assert!(ring.is_empty());
        ring.write_bytes(b"hello ");
        ring.write_bytes(b"world");
        assert_eq!(ring.len(), 11);
        assert_eq!(ring.total_written(), 11);
        assert_eq!(collect(&ring), b"hello world");

        // Fill exactly to capacity, then overwrite the first 5 bytes.
        let mut ring = LogRing::new();
        let pattern: Vec<u8> = (0..RING_CAPACITY).map(|i| b'a' + (i % 26) as u8).collect();
        ring.write_bytes(&pattern);
        assert_eq!(ring.len(), RING_CAPACITY);
        assert_eq!(collect(&ring), pattern);

        ring.write_bytes(b"12345");
        assert_eq!(ring.len(), RING_CAPACITY);
        assert_eq!(ring.total_written(), (RING_CAPACITY + 5) as u64);
        let collected = collect(&ring);
        assert_eq!(collected.len(), RING_CAPACITY);
        assert_eq!(&collected[..RING_CAPACITY - 5], &pattern[5..]);
        assert_eq!(&collected[RING_CAPACITY - 5..], b"12345");
    }

    #[test]
    fn ring_skips_leading_utf8_continuation_bytes_after_wrap() {
        let mut ring = LogRing::new();
        let filler = alloc::vec![b'x'; RING_CAPACITY - 1];
        ring.write_bytes(&filler);
        // 3-byte UTF-8 character '€' (0xE2, 0x82, 0xAC): the first byte lands
        // at the end of the ring and the next two wrap around, overwriting two
        // 'x' bytes. Then write one more byte so the leading 0xE2 is overwritten,
        // leaving 0x82, 0xAC at the head of the ring.
        ring.write_bytes("€!".as_bytes());
        let collected = collect(&ring);
        assert_eq!(collected[0], b'x');
        assert_eq!(collected.last().copied(), Some(b'!'));

        // Now force the oldest byte to be a continuation byte (0x80).
        let mut ring2 = LogRing::new();
        ring2.push_byte(0xE2);
        ring2.push_byte(0x82);
        ring2.push_byte(0xAC);
        ring2.write_bytes(&alloc::vec![b'a'; RING_CAPACITY - 2]);
        // Now 0xE2 was overwritten, so head points at 0x82, followed by 0xAC.
        let out = collect(&ring2);
        assert_eq!(out[0], b'a');
        assert_eq!(out.len(), RING_CAPACITY - 2);
    }
}
