//! USB HID boot-protocol keyboard decoding, and the one place the xHCI
//! controller this kernel found is kept alive and polled.
//!
//! # The boot protocol, and why it's the right target
//!
//! A general USB HID device describes its own report format in a HID
//! report descriptor -- a small stack language that a host has to parse
//! and interpret to know which bits of a report mean which key. That is
//! a real parser, for a real (if small) language, and this kernel does
//! not have one.
//!
//! The HID specification anticipates exactly this: a device whose
//! interface declares subclass 1 ("boot interface") and protocol 1
//! ("keyboard") must also support a *fixed*, parser-free report format,
//! defined in HID 1.11 Appendix B, specifically so a system with no HID
//! parser -- historically a BIOS, here this kernel -- can still read a
//! keyboard. That format is eight bytes: one modifier bitmap, one
//! reserved byte, then up to six simultaneously-held key usage codes.
//!
//! Every USB keyboard supports it. `xhci.rs` asks for it explicitly with
//! a SET_PROTOCOL request, and this module decodes it.
//!
//! # Press edges, not held keys
//!
//! A boot report is a *state* -- "these keys are down right now" -- not
//! an event. A held key appears in report after report, unchanged. So
//! this module diffs each report against the previous one and emits a
//! keystroke only for a code that was absent before and is present now.
//! Without that, holding a key for a tenth of a second would type it ten
//! times, and `shell.rs`'s line editor would fill with repeats -- the
//! same reason `keyboard.rs`'s PS/2 decoder acts on make codes and
//! ignores break codes.
//!
//! No key repeat is synthesized: holding a key types it once. (A PS/2
//! keyboard repeats in its own hardware, by resending the make code, so
//! a held key does repeat there; the USB boot protocol has no such thing,
//! and a host-side repeat timer is not worth its complexity here.)

use crate::sync::Locked;
use crate::xhci::Xhci;
use crate::keyboard::ESC;
use crate::line_editor::{
    KEY_DELETE as DEL, KEY_DOWN as DOWN, KEY_END as END, KEY_HOME as HOME, KEY_LEFT as LEFT,
    KEY_RIGHT as RIGHT, KEY_UP as UP,
};
use crate::{debugcon, keyboard};

/// ASCII carriage return, matching `shell.rs`'s and `keyboard.rs`'s
/// `ENTER`.
const ENTER: u8 = 0x0d;
/// ASCII backspace, matching `shell.rs`'s and `keyboard.rs`'s
/// `BACKSPACE`.
const BACKSPACE: u8 = 0x08;

/// HID Keyboard/Keypad usage codes (HID Usage Tables 1.12, section 10)
/// mapped to the same ASCII byte space `keyboard.rs` already produces --
/// so `shell.rs` and WISP's `input()` cannot tell which physical
/// keyboard a byte came from, which is the point.
///
/// `0` marks a usage with no ASCII meaning here (modifiers, function
/// keys, arrows, lock keys); those are dropped rather than
/// substituted with a wrong character, exactly as `keyboard.rs` drops
/// unmapped scancodes.
#[rustfmt::skip]
const UNSHIFTED: [u8; 0x68] = [
    0,    0,    0,    0,    b'a', b'b', b'c', b'd', // 0x00-0x07
    b'e', b'f', b'g', b'h', b'i', b'j', b'k', b'l', // 0x08-0x0F
    b'm', b'n', b'o', b'p', b'q', b'r', b's', b't', // 0x10-0x17
    b'u', b'v', b'w', b'x', b'y', b'z', b'1', b'2', // 0x18-0x1F
    b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', // 0x20-0x27
    ENTER, ESC, BACKSPACE, 9, b' ', b'-', b'=', b'[', // 0x28-0x2F (0x29=Esc, 0x2B=Tab)
    b']', b'\\', 0,   b';', b'\'', b'`', b',', b'.', // 0x30-0x37 (0x32=non-US #)
    b'/', 0,    0,    0,    0,    0,    0,    0,    // 0x38-0x3F (0x39=CapsLock, 0x3A..=F1..)
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x40-0x47
    0,    0,    HOME, 0,    DEL,  END,  0,    RIGHT, // 0x48-0x4F (0x4A=Home, 0x4C=Delete, 0x4D=End, 0x4F=Right)
    LEFT, DOWN, UP,   0,    b'/', b'*', b'-', b'+', // 0x50-0x57 (Left, Down, Up, NumLock, keypad / * - +)
    ENTER, b'1', b'2', b'3', b'4', b'5', b'6', b'7', // 0x58-0x5F (keypad Enter, 1-7)
    b'8', b'9', b'0', b'.', 0,    0,    0,    0,    // 0x60-0x67 (keypad 8, 9, 0, .)
];

#[rustfmt::skip]
const SHIFTED: [u8; 0x68] = [
    0,    0,    0,    0,    b'A', b'B', b'C', b'D',
    b'E', b'F', b'G', b'H', b'I', b'J', b'K', b'L',
    b'M', b'N', b'O', b'P', b'Q', b'R', b'S', b'T',
    b'U', b'V', b'W', b'X', b'Y', b'Z', b'!', b'@',
    b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')',
    ENTER, ESC, BACKSPACE, 9, b' ', b'_', b'+', b'{',
    b'}', b'|', 0,    b':', b'"', b'~', b'<', b'>',
    b'?', 0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    HOME, 0,    DEL,  END,  0,    RIGHT,
    LEFT, DOWN, UP,   0,    b'/', b'*', b'-', b'+',
    ENTER, b'1', b'2', b'3', b'4', b'5', b'6', b'7',
    b'8', b'9', b'0', b'.', 0,    0,    0,    0,
];

/// Modifier byte (report byte 0) bits for left and right Shift (HID
/// 1.11, Appendix B). Nothing else in the modifier byte is acted on:
/// this kernel binds no Ctrl or Alt combination, exactly like the PS/2
/// path.
const MOD_LEFT_SHIFT: u8 = 1 << 1;
const MOD_RIGHT_SHIFT: u8 = 1 << 5;

/// The live controller, plus the previous report needed for press-edge
/// detection.
///
/// Wrapped in `Locked` for the same reason every other shared structure
/// in this kernel is: `poll` runs from `keyboard::wait_for_key`, which
/// can be reached from mainline code at any moment, and the state here
/// must not be observed half-updated. (`poll` is deliberately *not*
/// called from an interrupt handler -- see `xhci.rs`'s module doc
/// comment -- so this lock is never taken from interrupt context and
/// cannot deadlock against itself.)
struct UsbInput {
    controller: Option<Xhci>,
    /// The six key usage codes present in the previous report.
    previous: [u8; 6],
    /// Whether any USB keystroke has been delivered yet this boot.
    ///
    /// Exists for one reason: `tests/usb_test.py` needs to prove a
    /// keystroke arrived *through the USB path specifically*, and on a
    /// machine that also has a PS/2 controller (which is every machine
    /// QEMU emulates by default) a character appearing on screen proves
    /// only that some keyboard worked. One line in the debug log, on the
    /// first USB keystroke, is an unambiguous discriminator -- and
    /// deliberately only the first: logging every keystroke would write
    /// everything anyone types into the debug log, which is a thing this
    /// project has no reason to do.
    first_key_logged: bool,
}

static USB: Locked<UsbInput> = Locked::new(UsbInput {
    controller: None,
    previous: [0; 6],
    first_key_logged: false,
});

/// Adopt a brought-up controller with a live keyboard. Called once, from
/// `main.rs`'s boot sequence, if `xhci::init` found one.
pub fn install(controller: Xhci) {
    USB.with(|usb| {
        usb.controller = Some(controller);
        usb.previous = [0; 6];
        usb.first_key_logged = false;
    });
    debugcon::write_str("[usb_hid] USB keyboard installed as an input source\n");
}

/// Drain the controller's event ring and enqueue a byte for every key
/// *press* it reports.
///
/// Called from `keyboard::wait_for_key`'s idle loop, which wakes on
/// every timer tick, so this runs at the tick rate (100 Hz) whenever
/// anything is waiting for input -- see `xhci.rs`'s module doc comment
/// for why the driver is polled rather than interrupt-driven, and what
/// the 10 ms worst-case latency that buys costs in practice (nothing a
/// person can perceive).
///
/// A no-op when no controller was installed, so callers never need to
/// check first.
pub fn poll() {
    USB.with(|usb| {
        let Some(controller) = usb.controller.as_mut() else {
            return;
        };
        let previous = usb.previous;
        let mut latest: Option<[u8; 6]> = None;
        let mut saw_key = false;

        // SAFETY: `controller` was brought up by `xhci::init` and is
        // still running; nothing else touches it (it lives only here,
        // behind this lock).
        unsafe {
            controller.poll(|report| {
                let modifiers = report[0];
                let shift = modifiers & (MOD_LEFT_SHIFT | MOD_RIGHT_SHIFT) != 0;
                let keys: [u8; 6] = [report[2], report[3], report[4], report[5], report[6], report[7]];

                for &usage in &keys {
                    if usage == 0 {
                        continue;
                    }
                    // Usage 1 is ErrorRollOver: the device is reporting
                    // "more keys are down than I can represent," not a
                    // key. Usages 2 and 3 are the other error codes.
                    // Treating any of them as a keystroke would type
                    // garbage exactly when someone is typing fast.
                    if usage <= 3 {
                        continue;
                    }
                    // Press edge only -- a key still held from the
                    // previous report is not a new keystroke. See the
                    // module doc comment.
                    if previous.contains(&usage) {
                        continue;
                    }
                    let table = if shift { &SHIFTED } else { &UNSHIFTED };
                    let Some(&ascii) = table.get(usage as usize) else {
                        continue;
                    };
                    if ascii != 0 {
                        keyboard::push_decoded(ascii);
                        saw_key = true;
                    }
                }
                latest = Some(keys);
            });
        }

        if let Some(keys) = latest {
            usb.previous = keys;
        }
        if saw_key && !usb.first_key_logged {
            usb.first_key_logged = true;
            debugcon::write_str("[usb_hid] first keystroke received over USB\n");
        }
    });
}
