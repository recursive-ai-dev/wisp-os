//! Table-driven keyboard layouts (`us`, `uk`, `de`, `fr`, `es`, `dvorak`,
//! `colemak`) for both PS/2 Scancode Set 1 and USB HID Boot Protocol, with
//! Shift, Caps Lock, and AltGr (Right Alt) support.

use core::sync::atomic::{AtomicU8, Ordering};

const ESC: u8 = 0x1b;
const BACKSPACE: u8 = 0x08;
const TAB: u8 = 0x09;
const ENTER: u8 = 0x0d;

// Editing keys from `line_editor.rs` (kept as constants here so `keymap.rs`
// is self-contained and host-testable).
const KEY_UP: u8 = 0x80;
const KEY_DOWN: u8 = 0x81;
const KEY_LEFT: u8 = 0x82;
const KEY_RIGHT: u8 = 0x83;
const KEY_HOME: u8 = 0x84;
const KEY_END: u8 = 0x85;
const KEY_DELETE: u8 = 0x86;
const KEY_PAGE_UP: u8 = 0x87;
const KEY_PAGE_DOWN: u8 = 0x88;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LayoutId {
    Us = 0,
    Uk = 1,
    De = 2,
    Fr = 3,
    Es = 4,
    Dvorak = 5,
    Colemak = 6,
}

pub const ALL_LAYOUTS: [LayoutId; 7] = [
    LayoutId::Us,
    LayoutId::Uk,
    LayoutId::De,
    LayoutId::Fr,
    LayoutId::Es,
    LayoutId::Dvorak,
    LayoutId::Colemak,
];

impl LayoutId {
    pub const fn name(self) -> &'static str {
        match self {
            LayoutId::Us => "us",
            LayoutId::Uk => "uk",
            LayoutId::De => "de",
            LayoutId::Fr => "fr",
            LayoutId::Es => "es",
            LayoutId::Dvorak => "dvorak",
            LayoutId::Colemak => "colemak",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            LayoutId::Us => "US QWERTY",
            LayoutId::Uk => "UK QWERTY (ISO)",
            LayoutId::De => "German QWERTZ (AltGr for { [ ] } \\ @ ~ |)",
            LayoutId::Fr => "French AZERTY (AltGr for ~ # { [ | ` \\ ^ @ ] })",
            LayoutId::Es => "Spanish QWERTY (AltGr for | @ # ~ [ ] { } \\)",
            LayoutId::Dvorak => "US Dvorak",
            LayoutId::Colemak => "US Colemak",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let trimmed = name.trim();
        for id in ALL_LAYOUTS {
            if trimmed.eq_ignore_ascii_case(id.name()) {
                return Some(id);
            }
        }
        None
    }

    const fn from_u8(val: u8) -> Self {
        match val {
            1 => LayoutId::Uk,
            2 => LayoutId::De,
            3 => LayoutId::Fr,
            4 => LayoutId::Es,
            5 => LayoutId::Dvorak,
            6 => LayoutId::Colemak,
            _ => LayoutId::Us,
        }
    }
}

static ACTIVE_LAYOUT: AtomicU8 = AtomicU8::new(LayoutId::Us as u8);

pub fn active() -> LayoutId {
    LayoutId::from_u8(ACTIVE_LAYOUT.load(Ordering::Relaxed))
}

pub fn set_active(id: LayoutId) {
    ACTIVE_LAYOUT.store(id as u8, Ordering::Relaxed);
}

// ============================================================================
// PS/2 Scancode Set 1 Tables (0x00 .. 0x59, including 0x56 = ISO 102nd key)
// ============================================================================

#[rustfmt::skip]
const PS2_US_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6', // 0x00-0x07
    b'7', b'8', b'9', b'0', b'-', b'=', BACKSPACE, TAB, // 0x08-0x0F
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', // 0x10-0x17
    b'o', b'p', b'[', b']', ENTER, 0,   b'a', b's', // 0x18-0x1F
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', // 0x20-0x27
    b'\'', b'`', 0,   b'\\', b'z', b'x', b'c', b'v', // 0x28-0x2F
    b'b', b'n', b'm', b',', b'.', b'/', 0,    b'*', // 0x30-0x37
    0,    b' ', 0,    0,    0,    0,    0,    0,    // 0x38-0x3F
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x40-0x47
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x48-0x4F
    0,    0,    0,    0,    0,    0,    b'\\', 0,   // 0x50-0x57 (0x56=ISO)
    0,                                              // 0x58
];

#[rustfmt::skip]
const PS2_US_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', BACKSPACE, TAB,
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', ENTER, 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"', b'~', 0,    b'|', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

const PS2_EMPTY_ALTGR: [u8; 0x59] = [0; 0x59];

// UK QWERTY (ISO): Shift+2='"', Shift+'='@', 0x2B='#'/'~', 0x56='\'/'|', AltGr+3='#'
#[rustfmt::skip]
const PS2_UK_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', BACKSPACE, TAB,
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'[', b']', ENTER, 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'', b'`', 0,   b'#', b'z', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'/', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'\\', 0,
    0,
];

#[rustfmt::skip]
const PS2_UK_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'"', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', BACKSPACE, TAB,
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', ENTER, 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'@', b'~', 0,    b'~', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

#[rustfmt::skip]
const PS2_UK_ALTGR: [u8; 0x59] = [
    0,    0,    0,    b'@', b'#', 0,    0,    0,
    0,    0,    0,    0,    b'\\', 0,   0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    b'|', 0,    b'\\', 0,   0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

// German QWERTZ (ISO)
#[rustfmt::skip]
const PS2_DE_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'`', BACKSPACE, TAB,
    b'q', b'w', b'e', b'r', b't', b'z', b'u', b'i',
    b'o', b'p', b'[', b'+', ENTER, 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'', b'^', 0,   b'#', b'y', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'-', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'<', 0,
    0,
];

#[rustfmt::skip]
const PS2_DE_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'"', b'#', b'$', b'%', b'&',
    b'/', b'(', b')', b'=', b'?', b'`', BACKSPACE, TAB,
    b'Q', b'W', b'E', b'R', b'T', b'Z', b'U', b'I',
    b'O', b'P', b'{', b'*', ENTER, 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'\'', b'~', 0,   b'\'', b'Y', b'X', b'C', b'V',
    b'B', b'N', b'M', b';', b':', b'_', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'>', 0,
    0,
];

#[rustfmt::skip]
const PS2_DE_ALTGR: [u8; 0x59] = [
    0,    0,    0,    0,    0,    0,    0,    0,
    b'{', b'[', b']', b'}', b'\\', 0,   0,    0,
    b'@', 0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    b'~', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    b'|', 0,    0,    0,    0,
    0,    0,    0,    b'<', b'>', 0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

// French AZERTY
#[rustfmt::skip]
const PS2_FR_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'&', b'~', b'"', b'\'', b'(', b'-',
    b'`', b'_', b'^', b'@', b')', b'=', BACKSPACE, TAB,
    b'a', b'z', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'^', b'$', ENTER, 0,   b'q', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b'm',
    b'%', b'`', 0,    b'*', b'w', b'x', b'c', b'v',
    b'b', b'n', b',', b';', b':', b'!', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'<', 0,
    0,
];

#[rustfmt::skip]
const PS2_FR_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'[', b'+', BACKSPACE, TAB,
    b'A', b'Z', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', ENTER, 0,   b'Q', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b'M',
    b'%', b'~', 0,    b'|', b'W', b'X', b'C', b'V',
    b'B', b'N', b'?', b'.', b'/', b'\\', 0,   b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'>', 0,
    0,
];

#[rustfmt::skip]
const PS2_FR_ALTGR: [u8; 0x59] = [
    0,    0,    0,    b'~', b'#', b'{', b'[', b'|',
    b'`', b'\\', b'^', b'@', b']', b'}', 0,   0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    b'<', b'>', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

// Spanish QWERTY
#[rustfmt::skip]
const PS2_ES_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'\'', b'!', BACKSPACE, TAB,
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'`', b'+', ENTER, 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'{', b'\\', 0,   b'}', b'z', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'-', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'<', 0,
    0,
];

#[rustfmt::skip]
const PS2_ES_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'"', b'#', b'$', b'%', b'&',
    b'/', b'(', b')', b'=', b'?', b'^', BACKSPACE, TAB,
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'^', b'*', ENTER, 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'[', b'|', 0,    b']', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b';', b':', b'_', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'>', 0,
    0,
];

#[rustfmt::skip]
const PS2_ES_ALTGR: [u8; 0x59] = [
    0,    0,    b'|', b'@', b'#', b'~', 0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    b'[', b']', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    b'{', b'\\', 0,   b'}', 0,    0,    0,    0,
    0,    0,    0,    b'<', b'>', 0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,
];

// US Dvorak
#[rustfmt::skip]
const PS2_DVORAK_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'[', b']', BACKSPACE, TAB,
    b'\'', b',', b'.', b'p', b'y', b'f', b'g', b'c',
    b'r', b'l', b'/', b'=', ENTER, 0,   b'a', b'o',
    b'e', b'u', b'i', b'd', b'h', b't', b'n', b's',
    b'-', b'`', 0,    b'\\', b';', b'q', b'j', b'k',
    b'x', b'b', b'm', b'w', b'v', b'z', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'\\', 0,
    0,
];

#[rustfmt::skip]
const PS2_DVORAK_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'{', b'}', BACKSPACE, TAB,
    b'"', b'<', b'>', b'P', b'Y', b'F', b'G', b'C',
    b'R', b'L', b'?', b'+', ENTER, 0,   b'A', b'O',
    b'E', b'U', b'I', b'D', b'H', b'T', b'N', b'S',
    b'_', b'~', 0,    b'|', b':', b'Q', b'J', b'K',
    b'X', b'B', b'M', b'W', b'V', b'Z', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

// US Colemak
#[rustfmt::skip]
const PS2_COLEMAK_UNSHIFTED: [u8; 0x59] = [
    0,    ESC,  b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', BACKSPACE, TAB,
    b'q', b'w', b'f', b'p', b'g', b'j', b'l', b'u',
    b'y', b';', b'[', b']', ENTER, 0,   b'a', b'r',
    b's', b't', b'd', b'h', b'n', b'e', b'i', b'o',
    b'\'', b'`', 0,   b'\\', b'z', b'x', b'c', b'v',
    b'b', b'k', b'm', b',', b'.', b'/', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'\\', 0,
    0,
];

#[rustfmt::skip]
const PS2_COLEMAK_SHIFTED: [u8; 0x59] = [
    0,    ESC,  b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', BACKSPACE, TAB,
    b'Q', b'W', b'F', b'P', b'G', b'J', b'L', b'U',
    b'Y', b':', b'{', b'}', ENTER, 0,   b'A', b'R',
    b'S', b'T', b'D', b'H', b'N', b'E', b'I', b'O',
    b'"', b'~', 0,    b'|', b'Z', b'X', b'C', b'V',
    b'B', b'K', b'M', b'<', b'>', b'?', 0,    b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    b'|', 0,
    0,
];

// ============================================================================
// USB HID Boot Protocol Tables (0x00 .. 0x68, including 0x32 and 0x64 ISO keys)
// ============================================================================

#[rustfmt::skip]
const HID_US_UNSHIFTED: [u8; 0x68] = [
    0,    0,    0,    0,    b'a', b'b', b'c', b'd', // 0x00-0x07
    b'e', b'f', b'g', b'h', b'i', b'j', b'k', b'l', // 0x08-0x0F
    b'm', b'n', b'o', b'p', b'q', b'r', b's', b't', // 0x10-0x17
    b'u', b'v', b'w', b'x', b'y', b'z', b'1', b'2', // 0x18-0x1F
    b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', // 0x20-0x27
    ENTER, ESC, BACKSPACE, TAB, b' ', b'-', b'=', b'[', // 0x28-0x2F
    b']', b'\\', b'\\', b';', b'\'', b'`', b',', b'.', // 0x30-0x37 (0x32=non-US #)
    b'/', 0,    0,    0,    0,    0,    0,    0,    // 0x38-0x3F
    0,    0,    0,    0,    0,    0,    0,    0,    // 0x40-0x47
    0,    0,    KEY_HOME, KEY_PAGE_UP, KEY_DELETE, KEY_END, KEY_PAGE_DOWN, KEY_RIGHT, // 0x48-0x4F
    KEY_LEFT, KEY_DOWN, KEY_UP, 0, b'/', b'*', b'-', b'+', // 0x50-0x57
    ENTER, b'1', b'2', b'3', b'4', b'5', b'6', b'7', // 0x58-0x5F
    b'8', b'9', b'0', b'.', b'\\', 0,   0,    0,    // 0x60-0x67 (0x64=non-US \)
];

#[rustfmt::skip]
const HID_US_SHIFTED: [u8; 0x68] = [
    0,    0,    0,    0,    b'A', b'B', b'C', b'D',
    b'E', b'F', b'G', b'H', b'I', b'J', b'K', b'L',
    b'M', b'N', b'O', b'P', b'Q', b'R', b'S', b'T',
    b'U', b'V', b'W', b'X', b'Y', b'Z', b'!', b'@',
    b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')',
    ENTER, ESC, BACKSPACE, TAB, b' ', b'_', b'+', b'{',
    b'}', b'|', b'|', b':', b'"', b'~', b'<', b'>',
    b'?', 0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    KEY_HOME, KEY_PAGE_UP, KEY_DELETE, KEY_END, KEY_PAGE_DOWN, KEY_RIGHT,
    KEY_LEFT, KEY_DOWN, KEY_UP, 0, b'/', b'*', b'-', b'+',
    ENTER, b'1', b'2', b'3', b'4', b'5', b'6', b'7',
    b'8', b'9', b'0', b'.', b'|', 0,    0,    0,
];

const HID_EMPTY_ALTGR: [u8; 0x68] = [0; 0x68];

/// Map a USB HID usage code (`0x00..0x68`) to its corresponding PS/2 Set 1
/// scancode (`0x00..0x59`) for layouts whose non-US differences are defined
/// in the PS/2 tables above. Keypad/navigation keys (`0x48..0x63`) use the
/// shared HID navigation/keypad table directly.
fn hid_usage_to_ps2_scancode(usage: u8) -> Option<u8> {
    let sc = match usage {
        0x04 => 0x1E, // a
        0x05 => 0x30, // b
        0x06 => 0x2E, // c
        0x07 => 0x20, // d
        0x08 => 0x12, // e
        0x09 => 0x21, // f
        0x0A => 0x22, // g
        0x0B => 0x23, // h
        0x0C => 0x17, // i
        0x0D => 0x24, // j
        0x0E => 0x25, // k
        0x0F => 0x26, // l
        0x10 => 0x32, // m
        0x11 => 0x31, // n
        0x12 => 0x18, // o
        0x13 => 0x19, // p
        0x14 => 0x10, // q
        0x15 => 0x13, // r
        0x16 => 0x1F, // s
        0x17 => 0x14, // t
        0x18 => 0x16, // u
        0x19 => 0x2F, // v
        0x1A => 0x11, // w
        0x1B => 0x2D, // x
        0x1C => 0x15, // y
        0x1D => 0x2C, // z
        0x1E..=0x27 => 0x02 + (usage - 0x1E), // 1..0
        0x28 => 0x1C, // Enter
        0x29 => 0x01, // Esc
        0x2A => 0x0E, // Backspace
        0x2B => 0x0F, // Tab
        0x2C => 0x39, // Space
        0x2D => 0x0C, // -
        0x2E => 0x0D, // =
        0x2F => 0x1A, // [
        0x30 => 0x1B, // ]
        0x31 | 0x32 => 0x2B, // \ / ISO #
        0x33 => 0x27, // ;
        0x34 => 0x28, // '
        0x35 => 0x29, // `
        0x36 => 0x33, // ,
        0x37 => 0x34, // .
        0x38 => 0x35, // /
        0x64 => 0x56, // ISO 102nd key (< > |)
        _ => return None,
    };
    Some(sc)
}

fn tables_for_ps2(layout: LayoutId) -> (&'static [u8; 0x59], &'static [u8; 0x59], &'static [u8; 0x59]) {
    match layout {
        LayoutId::Us => (&PS2_US_UNSHIFTED, &PS2_US_SHIFTED, &PS2_EMPTY_ALTGR),
        LayoutId::Uk => (&PS2_UK_UNSHIFTED, &PS2_UK_SHIFTED, &PS2_UK_ALTGR),
        LayoutId::De => (&PS2_DE_UNSHIFTED, &PS2_DE_SHIFTED, &PS2_DE_ALTGR),
        LayoutId::Fr => (&PS2_FR_UNSHIFTED, &PS2_FR_SHIFTED, &PS2_FR_ALTGR),
        LayoutId::Es => (&PS2_ES_UNSHIFTED, &PS2_ES_SHIFTED, &PS2_ES_ALTGR),
        LayoutId::Dvorak => (&PS2_DVORAK_UNSHIFTED, &PS2_DVORAK_SHIFTED, &PS2_EMPTY_ALTGR),
        LayoutId::Colemak => (&PS2_COLEMAK_UNSHIFTED, &PS2_COLEMAK_SHIFTED, &PS2_EMPTY_ALTGR),
    }
}

/// Decode a PS/2 Scancode Set 1 make code (`0..0x59`) under `layout`.
pub fn translate_ps2(
    layout: LayoutId,
    scancode: u8,
    shift: bool,
    caps_lock: bool,
    altgr: bool,
) -> u8 {
    let idx = scancode as usize;
    if idx >= 0x59 {
        return 0;
    }
    let (unshifted, shifted, altgr_tbl) = tables_for_ps2(layout);
    if altgr {
        let ag = altgr_tbl[idx];
        if ag != 0 {
            return ag;
        }
    }
    let base = unshifted[idx];
    let is_letter = base.is_ascii_alphabetic();
    let effective_shift = if is_letter { shift ^ caps_lock } else { shift };
    if effective_shift {
        shifted[idx]
    } else {
        base
    }
}

/// Decode a USB HID usage code (`0..0x68`) under `layout`.
pub fn translate_hid(
    layout: LayoutId,
    usage: u8,
    shift: bool,
    caps_lock: bool,
    altgr: bool,
) -> u8 {
    let idx = usage as usize;
    if idx >= 0x68 {
        return 0;
    }
    if let Some(sc) = hid_usage_to_ps2_scancode(usage) {
        return translate_ps2(layout, sc, shift, caps_lock, altgr);
    }
    if altgr {
        let ag = HID_EMPTY_ALTGR[idx];
        if ag != 0 {
            return ag;
        }
    }
    if shift {
        HID_US_SHIFTED[idx]
    } else {
        HID_US_UNSHIFTED[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layout_maps_all_printable_ascii_on_both_ps2_and_hid() {
        for layout in ALL_LAYOUTS {
            let mut ps2_seen = [false; 128];
            for sc in 0..0x59u8 {
                for (shift, altgr) in [(false, false), (true, false), (false, true)] {
                    let b = translate_ps2(layout, sc, shift, false, altgr);
                    if (0x20..=0x7e).contains(&b) {
                        ps2_seen[b as usize] = true;
                    }
                }
            }
            let mut hid_seen = [false; 128];
            for usage in 0..0x68u8 {
                for (shift, altgr) in [(false, false), (true, false), (false, true)] {
                    let b = translate_hid(layout, usage, shift, false, altgr);
                    if (0x20..=0x7e).contains(&b) {
                        hid_seen[b as usize] = true;
                    }
                }
            }
            for ascii in 0x20u8..=0x7eu8 {
                let ch = char::from(ascii);
                assert!(
                    ps2_seen[ascii as usize],
                    "layout '{}' is missing PS/2 mapping for printable ASCII {:?} (0x{:02x})",
                    layout.name(),
                    ch,
                    ascii
                );
                assert!(
                    hid_seen[ascii as usize],
                    "layout '{}' is missing USB HID mapping for printable ASCII {:?} (0x{:02x})",
                    layout.name(),
                    ch,
                    ascii
                );
            }
        }
    }

    #[test]
    fn caps_lock_inverts_letters_without_shifting_digits_or_punctuation() {
        for layout in ALL_LAYOUTS {
            // Find the scancode for 'a' and '1' in this layout.
            let sc_a = (0..0x59u8)
                .find(|&sc| translate_ps2(layout, sc, false, false, false) == b'a')
                .unwrap();
            assert_eq!(translate_ps2(layout, sc_a, false, true, false), b'A');
            assert_eq!(translate_ps2(layout, sc_a, true, true, false), b'a');

            // Semicolon must not change when only Caps Lock is active.
            let sc_semi = (0..0x59u8)
                .find(|&sc| translate_ps2(layout, sc, false, false, false) == b';')
                .unwrap();
            assert_eq!(translate_ps2(layout, sc_semi, false, true, false), b';');
        }
    }
}
