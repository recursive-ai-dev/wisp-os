//! Raw writes to I/O port 0xE9, the Bochs/QEMU "debugcon" convention: an
//! output-only virtual debug port with no protocol, no handshake, and no
//! guest-visible side effect beyond appearing in a host-side log file (see
//! `run.sh`'s `-debugcon file:...` flag). This is output-only and entirely
//! local to the emulator/host boundary -- it is not networking of any kind
//! and does not touch this project's zero-networking constraint.
//!
//! Zero allocation, zero locking, a single `out` instruction per byte: safe
//! to call from panic/fault-handler context, including from inside an
//! interrupt handler with interrupts already disabled, where nothing that
//! could block or allocate is safe to call.

use core::arch::asm;
use core::fmt;

const DEBUGCON_PORT: u16 = 0xE9;

fn write_byte(byte: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") DEBUGCON_PORT,
            in("al") byte,
            options(nomem, nostack, preserves_flags),
        );
    }
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}

struct DebugconWriter;

impl fmt::Write for DebugconWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

pub fn write_fmt(args: fmt::Arguments) -> fmt::Result {
    use fmt::Write;
    DebugconWriter.write_fmt(args)
}
