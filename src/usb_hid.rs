//! USB HID boot-protocol keyboard decoding, and the one place the xHCI
//! controller this kernel found is kept alive and polled.
//!
//! Decodes the 8-byte boot-protocol report (modifier bitmap, reserved byte,
//! and up to 6 simultaneous usage codes) through `crate::keymap::translate_hid`,
//! tracking press edges, Caps Lock state, AltGr (Right Alt), and software key
//! repeat (500 ms initial delay, 50 ms repeat interval via `clock::ticks()`).

use crate::line_editor::KEY_F1;
use crate::sync::Locked;
use crate::xhci::Xhci;
use crate::{clock, debugcon, keyboard, keymap};

const MOD_LEFT_SHIFT: u8 = 1 << 1;
const MOD_RIGHT_SHIFT: u8 = 1 << 5;
const MOD_RIGHT_ALT: u8 = 1 << 6;
const USAGE_CAPSLOCK: u8 = 0x39;

/// Initial delay before a held USB key starts repeating (50 ticks = 500 ms).
const REPEAT_INITIAL_TICKS: u64 = 50;
/// Interval between subsequent repeats (5 ticks = 50 ms = 20 Hz).
const REPEAT_INTERVAL_TICKS: u64 = 5;

struct UsbInput {
    controller: Option<Xhci>,
    /// The six key usage codes present in the previous report.
    previous: [u8; 6],
    /// Modifier byte from the most recent report.
    last_modifiers: u8,
    caps_lock: bool,
    repeat_usage: u8,
    repeat_next_tick: u64,
    first_key_logged: bool,
}

static USB: Locked<UsbInput> = Locked::new(UsbInput {
    controller: None,
    previous: [0; 6],
    last_modifiers: 0,
    caps_lock: false,
    repeat_usage: 0,
    repeat_next_tick: 0,
    first_key_logged: false,
});

/// Adopt a brought-up controller with a live keyboard. Called once, from
/// `main.rs`'s boot sequence, if `xhci::init` found one.
pub fn install(controller: Xhci) {
    let has_keyboard = controller.has_keyboard();
    USB.with(|usb| {
        usb.controller = Some(controller);
        usb.previous = [0; 6];
        usb.last_modifiers = 0;
        usb.caps_lock = false;
        usb.repeat_usage = 0;
        usb.repeat_next_tick = 0;
        usb.first_key_logged = false;
    });
    if has_keyboard {
        debugcon::write_str("[usb_hid] USB keyboard installed as an input source\n");
    }
}

/// Temporarily borrow the controller on the mainline with interrupts enabled.
/// No IRQ handler accesses USB. Taking ownership out under the short lock
/// prevents nested polls from borrowing the controller while an I/O wait runs.
pub(crate) fn with_controller<R>(
    operation: impl FnOnce(&mut Xhci) -> R,
) -> Result<R, &'static str> {
    let mut controller = USB
        .with(|usb| usb.controller.take())
        .ok_or("usb-storage: USB controller unavailable")?;
    let result = operation(&mut controller);
    USB.with(|usb| usb.controller = Some(controller));
    Ok(result)
}

fn decode_usage(usage: u8, modifiers: u8, caps_lock: bool) -> u8 {
    if (0x3a..=0x43).contains(&usage) {
        return KEY_F1 + usage - 0x3a;
    }
    let shift = modifiers & (MOD_LEFT_SHIFT | MOD_RIGHT_SHIFT) != 0;
    let altgr = modifiers & MOD_RIGHT_ALT != 0;
    keymap::translate_hid(keymap::active(), usage, shift, caps_lock, altgr)
}

/// Drain the controller's event ring and enqueue a byte for every key
/// *press* (and software repeat) it reports.
pub fn poll() {
    USB.with(|usb| {
        let Some(controller) = usb.controller.as_mut() else {
            return;
        };
        let mut previous = usb.previous;
        let mut last_modifiers = usb.last_modifiers;
        let mut caps_lock = usb.caps_lock;
        let mut repeat_usage = usb.repeat_usage;
        let mut repeat_next_tick = usb.repeat_next_tick;
        let mut saw_key = false;
        let now = clock::ticks();

        unsafe {
            controller.poll(|report| {
                let modifiers = report[0];
                let keys: [u8; 6] = [
                    report[2], report[3], report[4], report[5], report[6], report[7],
                ];
                last_modifiers = modifiers;

                if repeat_usage != 0 && !keys.contains(&repeat_usage) {
                    repeat_usage = 0;
                }

                for &usage in &keys {
                    if usage <= 3 {
                        continue;
                    }
                    if previous.contains(&usage) {
                        continue;
                    }
                    if usage == USAGE_CAPSLOCK {
                        caps_lock = !caps_lock;
                        continue;
                    }
                    let decoded = decode_usage(usage, modifiers, caps_lock);
                    if decoded != 0 {
                        keyboard::push_decoded(decoded);
                        saw_key = true;
                        if decoded != keyboard::ESC {
                            repeat_usage = usage;
                            repeat_next_tick = now.saturating_add(REPEAT_INITIAL_TICKS);
                        }
                    }
                }
                previous = keys;
            });
        }

        if repeat_usage != 0 {
            if !previous.contains(&repeat_usage) {
                repeat_usage = 0;
            } else if now >= repeat_next_tick {
                let decoded = decode_usage(repeat_usage, last_modifiers, caps_lock);
                if decoded != 0 && decoded != keyboard::ESC {
                    keyboard::push_decoded(decoded);
                    saw_key = true;
                    repeat_next_tick = now.saturating_add(REPEAT_INTERVAL_TICKS);
                } else {
                    repeat_usage = 0;
                }
            }
        }

        usb.previous = previous;
        usb.last_modifiers = last_modifiers;
        usb.caps_lock = caps_lock;
        usb.repeat_usage = repeat_usage;
        usb.repeat_next_tick = repeat_next_tick;

        if saw_key && !usb.first_key_logged {
            usb.first_key_logged = true;
            debugcon::write_str("[usb_hid] first keystroke received over USB\n");
        }
    });
}
