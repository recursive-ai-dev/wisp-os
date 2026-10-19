//! PS/2 keyboard driver: IRQ1, scancode set 1 (the set every PS/2
//! controller -- real or emulated -- speaks unless explicitly reprogrammed,
//! which QEMU's default `i8042` never is), decoded into the same ASCII
//! byte space `shell.rs` already expects (`ENTER = 0x0d`, `BACKSPACE =
//! 0x08`, printable ASCII 0x20-0x7e). The editing keys -- arrows, Home,
//! End, Delete, all 0xE0-prefixed -- become `line_editor`'s `KEY_*` codes;
//! keypad Enter is Enter. Every other extended or non-printable key
//! (function keys, Ctrl/Alt, CapsLock, NumLock, ScrollLock, the "fake
//! shift" codes some keyboards wrap arrows in) is recognized and discarded
//! rather than mis-decoded.

use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};

use crate::line_editor::{KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT, KEY_UP};
use crate::sync::Locked;

const DATA_PORT: u16 = 0x60;

/// ASCII carriage return, matching `shell.rs`'s `ENTER` constant.
const ENTER: u8 = 0x0d;
/// ASCII backspace, matching `shell.rs`'s `BACKSPACE` constant.
const BACKSPACE: u8 = 0x08;
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

/// Unshifted scancode-set-1 make-code -> ASCII table, indexed by scancode.
/// `0` marks a code with no ASCII mapping (function keys, modifiers,
/// unmapped punctuation-adjacent codes, etc.) -- those are dropped, not
/// substituted with a wrong character.
#[rustfmt::skip]
const UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6', // 0x00-0x07 (0x01=Esc)
    b'7', b'8', b'9', b'0', b'-', b'=', BACKSPACE, 9, // 0x08-0x0F (0x0F=Tab)
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', // 0x10-0x17
    b'o', b'p', b'[', b']', ENTER, 0,    b'a', b's', // 0x18-0x1F (0x1D=LCtrl)
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', // 0x20-0x27
    b'\'', b'`', 0,    b'\\', b'z', b'x', b'c', b'v', // 0x28-0x2F (0x2A=LShift)
    b'b', b'n', b'm', b',', b'.', b'/', 0,    b'*', // 0x30-0x37 (0x36=RShift)
    0,    b' ', 0,    0,    0,    0,    0,    0,    // 0x38-0x3F (0x38=LAlt,0x39=Space,0x3A=CapsLock,0x3B..=F1..)
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x40-0x47
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x48-0x4F
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x50-0x57
    0,                                                // 0x58
];

#[rustfmt::skip]
const SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', BACKSPACE, 9,
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', ENTER, 0,    b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"', b'~', 0,    b'|', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,
];

const LSHIFT_MAKE: u8 = 0x2A;
const LSHIFT_BREAK: u8 = 0xAA;
const RSHIFT_MAKE: u8 = 0x36;
const RSHIFT_BREAK: u8 = 0xB6;
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
    shift_held: bool,
}

impl RingBuffer {
    const fn new() -> Self {
        RingBuffer {
            buf: [0; RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            pending_extended: false,
            shift_held: false,
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

    QUEUE.with(|q| {
        if q.pending_extended {
            // Second byte of an 0xE0-prefixed sequence: an editing key, or
            // something (right Ctrl/Alt, a release, a fake shift) that is
            // not bound to anything.
            q.pending_extended = false;
            let key = extended_key(scancode);
            if key != 0 {
                q.push(key);
            }
            return;
        }
        if scancode == EXTENDED_PREFIX {
            q.pending_extended = true;
            return;
        }
        match scancode {
            LSHIFT_MAKE | RSHIFT_MAKE => {
                q.shift_held = true;
                return;
            }
            LSHIFT_BREAK | RSHIFT_BREAK => {
                q.shift_held = false;
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
        let index = scancode as usize;
        let table = if q.shift_held { &SHIFTED } else { &UNSHIFTED };
        let Some(&ascii) = table.get(index) else {
            return;
        };
        if ascii == ESC {
            request_stop();
        }
        if ascii != 0 {
            q.push(ascii);
        }
    });
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
        0x1C => ENTER, // keypad Enter
        _ => 0,
    }
}

/// Enqueue an already-decoded ASCII byte from an input source other than
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
