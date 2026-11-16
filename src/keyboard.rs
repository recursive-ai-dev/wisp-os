//! PS/2 keyboard driver: IRQ1, scancode set 1 (the set every PS/2
//! controller -- real or emulated -- speaks unless explicitly reprogrammed,
//! which QEMU's default `i8042` never is), decoded into the same ASCII
//! byte space `shell.rs` already expects (`ENTER = 0x0d`, `BACKSPACE =
//! 0x08`, printable ASCII 0x20-0x7e). The editing keys -- arrows, Home,
//! End, Delete and PageUp/PageDown, all 0xE0-prefixed -- and F1 through
//! F10 become `line_editor`'s `KEY_*` codes; keypad Enter is Enter.
//! Every other extended or non-printable key
//! (Ctrl/Alt, CapsLock, NumLock, ScrollLock, the "fake
//! shift" codes some keyboards wrap arrows in) is recognized and discarded
//! rather than mis-decoded.

use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};

use crate::line_editor::{
    KEY_DELETE, KEY_DOWN, KEY_END, KEY_F1, KEY_HOME, KEY_LEFT, KEY_PAGE_DOWN,
    KEY_PAGE_UP, KEY_RIGHT, KEY_UP,
};
use crate::sync::Locked;

const DATA_PORT: u16 = 0x60;

/// ASCII carriage return, matching `shell.rs`'s `ENTER` constant.
const ENTER: u8 = 0x0d;
/// ASCII escape. Decoded from the Esc key on both keyboards; besides being
/// queued like any other byte, it raises the stop request (see
/// [`request_stop`]).
pub const ESC: u8 = 0x1b;

fn in8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

const LSHIFT_MAKE: u8 = 0x2A;
const LSHIFT_BREAK: u8 = 0xAA;
const RSHIFT_MAKE: u8 = 0x36;
const RSHIFT_BREAK: u8 = 0xB6;
const CAPSLOCK_MAKE: u8 = 0x3A;
const RALT_MAKE: u8 = 0x38;
const RALT_BREAK: u8 = 0xB8;
const EXTENDED_PREFIX: u8 = 0xE0;
/// Highest bit of a scancode marks a break (key-release) code in set 1.
const BREAK_BIT: u8 = 0x80;

const RING_CAPACITY: usize = 64;

struct RingBuffer {
    buf: [u8; RING_CAPACITY],
    head: usize, // next slot to pop
    tail: usize, // next slot to push
    len: usize,
    /// True once a decoded 0xE0 prefix byte has been consumed, so the next
    /// scancode byte is known to be part of an extended sequence (see
    /// [`extended_key`]).
    pending_extended: bool,
    left_shift: bool,
    right_shift: bool,
    right_alt: bool,
    caps_lock: bool,
}

impl RingBuffer {
    const fn new() -> Self {
        RingBuffer {
            buf: [0; RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            pending_extended: false,
            left_shift: false,
            right_shift: false,
            right_alt: false,
            caps_lock: false,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.len == RING_CAPACITY {
            // Buffer full because nothing is reading fast enough (or at
            // all, e.g. between shell commands): drop the oldest queued
            // byte rather than the new one, so a burst of typing doesn't
            // permanently wedge input on a stale key. Losing a keystroke
            // here is preferable to blocking an interrupt handler.
            self.head = (self.head + 1) % RING_CAPACITY;
            self.len -= 1;
        }
        self.buf[self.tail] = byte;
        self.tail = (self.tail + 1) % RING_CAPACITY;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % RING_CAPACITY;
        self.len -= 1;
        Some(byte)
    }
}

static QUEUE: Locked<RingBuffer> = Locked::new(RingBuffer::new());

/// Set when Esc is pressed on any keyboard; consumed by whatever long-
/// running activity wants to be stoppable -- today, a running WISP
/// program (see `shell.rs`'s `ShellHost::stop_requested`).
///
/// A separate flag rather than "look for ESC in the queue": a program
/// stuck in a pure compute loop never reads the queue at all, and the
/// stop has to reach it anyway. The ESC byte is *also* queued, so a
/// program blocked in `input()` is woken by it.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Raise the stop request. Safe from interrupt context (a single atomic
/// store).
pub fn request_stop() {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Whether a stop has been requested since the last [`clear_stop_request`].
pub fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

/// Forget any pending stop request, and drop keystrokes typed before now.
/// Called when a WISP program starts and again when it ends, so an Esc
/// pressed at the shell prompt never kills the *next* program, and keys
/// mashed into a program that was stopped never leak into the prompt.
pub fn clear_stop_request() {
    QUEUE.with(|q| {
        STOP_REQUESTED.store(false, Ordering::SeqCst);
        while q.pop().is_some() {}
    });
}

/// 8042 status port: bit 0 = output buffer full, bit 5 = that byte came
/// from the auxiliary (mouse) device rather than the keyboard.
const STATUS_PORT: u16 = 0x64;
const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_AUX_DATA: u8 = 1 << 5;

/// Consume a keyboard byte that is already waiting in the 8042 output
/// buffer, exactly as the interrupt handler would have. Used once, during
/// the 8259-to-APIC handover: IRQ1 is edge-triggered, and a byte that
/// arrived while both controllers had the line masked would otherwise
/// never be read -- and the 8042 raises no further edge until it is.
pub fn drain_pending() {
    let status = in8(STATUS_PORT);
    if status & STATUS_OUTPUT_FULL != 0 && status & STATUS_AUX_DATA == 0 {
        on_irq();
    }
}

/// Called by the IDT's vector-0x21 dispatch, once per keyboard interrupt.
/// Reads exactly one scancode byte (the PS/2 controller signals a fresh
/// IRQ1 per byte; there is never more than one waiting) and, if it decodes
/// to a mapped ASCII byte, enqueues it for [`wait_for_key`].
pub fn on_irq() {
    let scancode = in8(DATA_PORT);

    QUEUE.with(|q| q.on_scancode(scancode));
}

impl RingBuffer {
    fn on_scancode(&mut self, scancode: u8) {
        if self.pending_extended {
            // Second byte of an 0xE0-prefixed sequence: an editing key,
            // Right Alt (AltGr), or something (right Ctrl, a release, a fake
            // shift) that is not bound to anything.
            self.pending_extended = false;
            match scancode {
                RALT_MAKE => {
                    self.right_alt = true;
                    return;
                }
                RALT_BREAK => {
                    self.right_alt = false;
                    return;
                }
                _ => {}
            }
            let key = extended_key(scancode);
            if key != 0 {
                self.push(key);
            }
            return;
        }
        if scancode == EXTENDED_PREFIX {
            self.pending_extended = true;
            return;
        }
        match scancode {
            LSHIFT_MAKE | LSHIFT_BREAK => {
                self.left_shift = scancode == LSHIFT_MAKE;
                return;
            }
            RSHIFT_MAKE | RSHIFT_BREAK => {
                self.right_shift = scancode == RSHIFT_MAKE;
                return;
            }
            CAPSLOCK_MAKE => {
                self.caps_lock = !self.caps_lock;
                return;
            }
            _ => {}
        }
        if scancode & BREAK_BIT != 0 {
            // Break (release) code for anything else: we only act on
            // make codes for regular keys (matching the old firmware
            // `Input` protocol's own key-repeat-free behavior).
            return;
        }
        // Set 1 gives F1 through F10 consecutive make codes. Shift does
        // not change the command code, matching USB's function-key path.
        if (0x3b..=0x44).contains(&scancode) {
            self.push(KEY_F1 + scancode - 0x3b);
            return;
        }
        let shift = self.left_shift || self.right_shift;
        let ascii = crate::keymap::translate_ps2(
            crate::keymap::active(),
            scancode,
            shift,
            self.caps_lock,
            self.right_alt,
        );
        if ascii == ESC {
            request_stop();
        }
        if ascii != 0 {
            self.push(ascii);
        }
    }
}

/// The byte an 0xE0-prefixed make code decodes to, or 0 for one this
/// kernel does not use. Release codes (bit 7 set) are all 0: like ordinary
/// keys, only presses count -- and the keyboard's own typematic repeat
/// resends the make code while an arrow is held.
fn extended_key(scancode: u8) -> u8 {
    match scancode {
        0x48 => KEY_UP,
        0x50 => KEY_DOWN,
        0x4B => KEY_LEFT,
        0x4D => KEY_RIGHT,
        0x47 => KEY_HOME,
        0x4F => KEY_END,
        0x53 => KEY_DELETE,
        0x49 => KEY_PAGE_UP,
        0x51 => KEY_PAGE_DOWN,
        0x1C => ENTER, // keypad Enter
        _ => 0,
    }
}

/// Enqueue an already-decoded ASCII or `KEY_*` byte from a source other than
/// the PS/2 controller -- today, `usb_hid.rs`'s HID boot-protocol
/// decoder.
///
/// Deliberately the *same* queue the PS/2 ISR pushes into, rather than a
/// second one: `wait_for_key` below stays the single blocking entry
/// point every reader in this kernel (`shell.rs`'s line editor, WISP's
/// `input()`) already goes through, two attached keyboards simply
/// interleave into one stream instead of one of them being silently
/// ignored, and -- most importantly -- nothing above this layer has to
/// learn that USB exists at all.
pub fn push_decoded(byte: u8) {
    if byte == 0 {
        return;
    }
    if byte == ESC {
        request_stop();
    }
    QUEUE.with(|q| q.push(byte));
}

/// Block until a key decodes to a mapped ASCII byte, and return it. Uses
/// the same `sti; hlt` interrupt-shadow idiom as `clock::sleep_ms_unless` to idle
/// the CPU race-free between keystrokes instead of busy-polling.
pub fn wait_for_key() -> u8 {
    loop {
        // Drain any USB keyboard reports the xHCI controller has
        // produced since the last pass, pushing them into the same queue
        // the PS/2 ISR feeds. This is where `xhci.rs`'s "polled, not
        // interrupt-driven" decision actually lands: this loop already
        // wakes on every timer interrupt (the `sti; hlt` below), so a
        // report is picked up within one tick -- 10 ms -- of the
        // controller producing it, with no interrupt routing needed for
        // the controller at all. A no-op when no USB keyboard was found.
        //
        // Deliberately before the `cli`: `usb_hid::poll` issues real
        // MMIO reads and can walk a whole event ring, which is more work
        // than belongs inside an interrupt-disabled window when nothing
        // requires it to be there. Nothing is lost by a timer tick
        // landing mid-poll -- the event ring is exactly as full
        // afterward as before.
        crate::usb_hid::poll();

        unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };
        // See `sync::without_interrupts`'s doc comment for the full account
        // of why this fence is required, not optional: without it, LLVM is
        // free to hoist/cache `QUEUE`'s state across loop iterations, since
        // nothing here tells it an interrupt handler can mutate that memory
        // between one iteration and the next.
        compiler_fence(Ordering::SeqCst);
        if let Some(byte) = QUEUE.with(|q| q.pop()) {
            compiler_fence(Ordering::SeqCst);
            unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)) };
            return byte;
        }
        compiler_fence(Ordering::SeqCst);
        // Nothing queued: re-enable interrupts and halt in the same
        // shadow so a keystroke arriving between the check above and this
        // `hlt` is never missed (see `clock::sleep_ms_unless`'s doc comment for the
        // full race-freedom argument, identical here).
        unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_shift_keys_keep_the_other_modifier_active() {
        for (first_break, second_break) in [(LSHIFT_BREAK, RSHIFT_BREAK), (RSHIFT_BREAK, LSHIFT_BREAK)] {
            let mut q = RingBuffer::new();
            for key in [LSHIFT_MAKE, RSHIFT_MAKE, first_break, 0x1e, second_break, 0x1e] {
                q.on_scancode(key);
            }
            assert_eq!(q.pop(), Some(b'A'));
            assert_eq!(q.pop(), Some(b'a'));
            assert_eq!(q.pop(), None);
        }
    }

    #[test]
    fn repeated_shift_makes_and_extended_fake_shifts_are_independent() {
        let mut q = RingBuffer::new();
        for key in [LSHIFT_MAKE, LSHIFT_MAKE, EXTENDED_PREFIX, RSHIFT_BREAK, 0x1e, LSHIFT_BREAK, 0x1e] {
            q.on_scancode(key);
        }
        assert_eq!(q.pop(), Some(b'A'));
        assert_eq!(q.pop(), Some(b'a'));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn function_and_navigation_keys_preserve_modifier_state() {
        let mut q = RingBuffer::new();
        for key in [RSHIFT_MAKE, 0x3b, EXTENDED_PREFIX, 0x48, 0x1e, RSHIFT_BREAK] {
            q.on_scancode(key);
        }
        assert_eq!(q.pop(), Some(KEY_F1));
        assert_eq!(q.pop(), Some(KEY_UP));
        assert_eq!(q.pop(), Some(b'A'));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn saturated_queue_drops_oldest_and_recovers() {
        let mut q = RingBuffer::new();
        for i in 0..(RING_CAPACITY + 20) { q.push(i as u8); }
        for i in 20..(RING_CAPACITY + 20) { assert_eq!(q.pop(), Some(i as u8)); }
        assert_eq!(q.pop(), None);
        q.on_scancode(0x1e);
        assert_eq!(q.pop(), Some(b'a'));
    }
}
