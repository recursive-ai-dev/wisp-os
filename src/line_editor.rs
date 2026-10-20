//! The line editor behind every line of typed input: the shell's `>`
//! prompt, the `wisp>` prompt, and a WISP program's `input()`.
//!
//! Left/Right, Home/End, Backspace and Delete edit anywhere in the line,
//! with a visible cursor; Up/Down recall earlier lines when the caller
//! keeps a [`History`]. The editor never assumes the line fits on one
//! row: it remembers where its text starts and, because typing near the
//! bottom of the screen scrolls everything up, how many times the screen
//! had scrolled when it last looked -- so it can always find the cell any
//! character of the line is drawn in, and redraw exactly the part that
//! changed.
//!
//! It draws through the small [`Screen`] trait rather than `gfx::Console`
//! directly, so the host tests can run it against a simulated character
//! grid and check the screen after every keystroke (`tests/host`).

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Keys that have no ASCII byte, as the keyboard drivers deliver them.
/// Above 0x7F, so they can never be mistaken for typed text, and outside
/// anything either keyboard decodes to a character.
pub const KEY_UP: u8 = 0x80;
pub const KEY_DOWN: u8 = 0x81;
pub const KEY_LEFT: u8 = 0x82;
pub const KEY_RIGHT: u8 = 0x83;
pub const KEY_HOME: u8 = 0x84;
pub const KEY_END: u8 = 0x85;
pub const KEY_DELETE: u8 = 0x86;

const ENTER: u8 = 0x0d;
const BACKSPACE: u8 = 0x08;
const ESC: u8 = 0x1b;

/// Longest line the editor accepts; further keystrokes are ignored. Far
/// more than anyone types at a prompt, and small enough that a line always
/// fits on a screen of any plausible size, so its start never scrolls out
/// of reach of the cursor.
pub const MAX_LINE: usize = 1024;

/// How many earlier lines a [`History`] keeps.
pub const HISTORY_LEN: usize = 32;

/// What the editor draws on: a grid of character cells with a cursor
/// that `put` advances, wrapping at the end of a row and scrolling at the
/// bottom of the screen -- `gfx::Console`'s model exactly.
pub trait Screen {
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;
    /// The cursor as `(col, row)`. `col` may equal `cols()`: the row is
    /// full and the next `put` wraps first.
    fn position(&self) -> (usize, usize);
    /// Move the cursor; `col <= cols()` and `row < rows()`.
    fn set_position(&mut self, col: usize, row: usize);
    /// How many times the screen has scrolled up a row, ever.
    fn scrolls(&self) -> u64;
    /// Draw one printable ASCII byte at the cursor and advance it.
    fn put(&mut self, byte: u8);
    /// Scroll the screen up one row; the cursor stays where it is.
    fn scroll_up(&mut self);
    /// Redraw cell `(col, row)` as `byte`, with the cursor marked on it
    /// (`on`) or not. Does not move the cursor.
    fn mark(&mut self, col: usize, row: usize, byte: u8, on: bool);
}

/// Earlier lines, newest last, for Up/Down to recall.
pub struct History {
    entries: VecDeque<String>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const fn new() -> Self {
        Self { entries: VecDeque::new() }
    }

    /// Remember `line`, unless it is blank or repeats the newest entry.
    pub fn push(&mut self, line: &str) {
        if line.trim().is_empty() || self.entries.back().is_some_and(|last| last == line) {
            return;
        }
        if self.entries.len() == HISTORY_LEN {
            self.entries.pop_front();
        }
        self.entries.push_back(String::from(line));
    }

    /// The `back`-th most recent line (0 = newest).
    fn recent(&self, back: usize) -> Option<&String> {
        self.entries.len().checked_sub(back + 1).and_then(|i| self.entries.get(i))
    }
}

/// How a read ended.
pub enum Outcome {
    /// Enter: the finished line.
    Line(String),
    /// Esc, when the caller asked for Esc to end the read (see
    /// [`read_line`]): whatever had been typed.
    Escaped(String),
}

/// Read one line, echoing and editing it on `screen`, taking keystrokes
/// from `next_key`. With `history`, Up/Down recall earlier lines and the
/// finished line is added to it. `esc_clears` decides what Esc does: erase
/// the line and keep reading (a prompt), or end the read (a program's
/// `input()`, which the caller then stops).
///
/// On return the screen's cursor is just past the line's last character,
/// exactly where plain echoing would have left it, and no cursor mark is
/// left on screen.
pub fn read_line(
    screen: &mut dyn Screen,
    next_key: &mut dyn FnMut() -> u8,
    mut history: Option<&mut History>,
    esc_clears: bool,
) -> Outcome {
    let mut ed = Editor::new(screen);
    ed.show_cursor(screen);
    loop {
        let key = next_key();
        match key {
            ENTER => {
                ed.hide_cursor(screen);
                ed.park_after_text(screen);
                let line = ed.text();
                if let Some(h) = history.as_deref_mut() {
                    h.push(&line);
                }
                return Outcome::Line(line);
            }
            ESC if !esc_clears => {
                ed.hide_cursor(screen);
                ed.park_after_text(screen);
                return Outcome::Escaped(ed.text());
            }
            ESC => {
                ed.browsing = None;
                ed.replace(screen, Vec::new());
            }
            BACKSPACE => ed.backspace(screen),
            KEY_DELETE => ed.delete(screen),
            KEY_LEFT => ed.move_to(screen, ed.cursor.saturating_sub(1)),
            KEY_RIGHT => ed.move_to(screen, (ed.cursor + 1).min(ed.buf.len())),
            KEY_HOME => ed.move_to(screen, 0),
            KEY_END => ed.move_to(screen, ed.buf.len()),
            KEY_UP => {
                if let Some(h) = history.as_deref() {
                    ed.recall_older(screen, h);
                }
            }
            KEY_DOWN => {
                if let Some(h) = history.as_deref() {
                    ed.recall_newer(screen, h);
                }
            }
            0x20..=0x7e => ed.insert(screen, key),
            _ => {}
        }
    }
}

struct Editor {
    /// The line: printable ASCII only, so a byte is a cell.
    buf: Vec<u8>,
    /// Insertion point, `0..=buf.len()`.
    cursor: usize,
    /// The screen position the read began at -- where the line's first
    /// character is drawn -- as it was when the screen had scrolled
    /// `origin_scrolls` times. Every scroll since moved it up a row; it may
    /// now be above the top (negative). `origin_col` may be `cols`: a
    /// prompt that filled its row exactly, so the line starts on the next
    /// one. [`Editor::cell`]'s arithmetic wraps that by itself.
    origin_col: usize,
    origin_row: i64,
    origin_scrolls: u64,
    /// While Up/Down are browsing history: how far back (0 = newest), and
    /// the line that was being typed before browsing began.
    browsing: Option<usize>,
    draft: Vec<u8>,
}

impl Editor {
    fn new(screen: &mut dyn Screen) -> Self {
        let (col, row) = screen.position();
        Self {
            buf: Vec::new(),
            cursor: 0,
            origin_col: col,
            origin_row: row as i64,
            origin_scrolls: screen.scrolls(),
            browsing: None,
            draft: Vec::new(),
        }
    }

    fn text(&self) -> String {
        self.buf.iter().map(|&b| char::from(b)).collect()
    }

    /// The cell character `index` of the line is drawn in, as of now:
    /// `(col, row)`, `row` negative if it has scrolled off the top.
    fn cell(&self, screen: &dyn Screen, index: usize) -> (usize, i64) {
        let cols = screen.cols();
        let scrolled = screen.scrolls().wrapping_sub(self.origin_scrolls) as i64;
        let linear = self.origin_col + index;
        (linear % cols, self.origin_row - scrolled + (linear / cols) as i64)
    }

    /// The on-screen cell for `index`, scrolling the screen first if that
    /// cell is just below the bottom (the end of a line that exactly fills
    /// its last row). `None` if it has scrolled off the top.
    fn visible_cell(&self, screen: &mut dyn Screen, index: usize) -> Option<(usize, usize)> {
        let (col, mut row) = self.cell(screen, index);
        while row >= screen.rows() as i64 {
            screen.scroll_up();
            row -= 1;
        }
        usize::try_from(row).ok().map(|row| (col, row))
    }

    fn byte_at(&self, index: usize) -> u8 {
        self.buf.get(index).copied().unwrap_or(b' ')
    }

    fn show_cursor(&self, screen: &mut dyn Screen) {
        if let Some((col, row)) = self.visible_cell(screen, self.cursor) {
            screen.mark(col, row, self.byte_at(self.cursor), true);
        }
    }

    fn hide_cursor(&self, screen: &mut dyn Screen) {
        let (col, row) = self.cell(screen, self.cursor);
        if let Ok(row) = usize::try_from(row) {
            if row < screen.rows() {
                screen.mark(col, row, self.byte_at(self.cursor), false);
            }
        }
    }

    /// Redraw the line from `from` to its end, then blank `erase` more
    /// cells after it (what a shorter line no longer covers).
    fn redraw_from(&self, screen: &mut dyn Screen, from: usize, erase: usize) {
        let end = self.buf.len() + erase;
        // Cells that scrolled off the top cannot be drawn; start at the
        // first one still on screen.
        let mut start = from;
        while start < end && self.cell(screen, start).1 < 0 {
            start += 1;
        }
        if start >= end {
            return;
        }
        let Some((col, row)) = self.visible_cell(screen, start) else {
            return;
        };
        screen.set_position(col, row);
        for index in start..end {
            screen.put(self.byte_at(index));
        }
    }

    /// Leave the screen's cursor just past the last character, as plain
    /// echoing would have: on a full row, the wrap still pending. For an
    /// empty line that is exactly where the read began -- which, after a
    /// prompt that filled its row, is the end of that row, not the start
    /// of the next.
    fn park_after_text(&self, screen: &mut dyn Screen) {
        match self.buf.len().checked_sub(1) {
            None => {
                let scrolled = screen.scrolls().wrapping_sub(self.origin_scrolls) as i64;
                if let Ok(row) = usize::try_from(self.origin_row - scrolled) {
                    screen.set_position(self.origin_col, row.min(screen.rows() - 1));
                }
            }
            Some(last) => {
                if let Some((col, row)) = self.visible_cell(screen, last) {
                    screen.set_position(col + 1, row);
                }
            }
        }
    }

    fn move_to(&mut self, screen: &mut dyn Screen, index: usize) {
        self.hide_cursor(screen);
        self.cursor = index;
        self.show_cursor(screen);
    }

    fn insert(&mut self, screen: &mut dyn Screen, byte: u8) {
        if self.buf.len() >= MAX_LINE {
            return;
        }
        self.hide_cursor(screen);
        self.buf.insert(self.cursor, byte);
        self.redraw_from(screen, self.cursor, 0);
        self.cursor += 1;
        self.show_cursor(screen);
    }

    fn backspace(&mut self, screen: &mut dyn Screen) {
        if self.cursor == 0 {
            return;
        }
        self.hide_cursor(screen);
        self.cursor -= 1;
        self.buf.remove(self.cursor);
        self.redraw_from(screen, self.cursor, 1);
        self.show_cursor(screen);
    }

    fn delete(&mut self, screen: &mut dyn Screen) {
        if self.cursor == self.buf.len() {
            return;
        }
        self.hide_cursor(screen);
        self.buf.remove(self.cursor);
        self.redraw_from(screen, self.cursor, 1);
        self.show_cursor(screen);
    }

    /// Replace the whole line (Esc at a prompt, or recalling history),
    /// with the cursor at its end.
    fn replace(&mut self, screen: &mut dyn Screen, line: Vec<u8>) {
        self.hide_cursor(screen);
        let erase = self.buf.len().saturating_sub(line.len());
        self.buf = line;
        self.buf.truncate(MAX_LINE);
        self.cursor = self.buf.len();
        self.redraw_from(screen, 0, erase);
        self.show_cursor(screen);
    }

    fn recall_older(&mut self, screen: &mut dyn Screen, history: &History) {
        let back = match self.browsing {
            None => 0,
            Some(back) => back + 1,
        };
        let Some(line) = history.recent(back) else {
            return;
        };
        if self.browsing.is_none() {
            self.draft = self.buf.clone();
        }
        self.browsing = Some(back);
        let line = line.as_bytes().to_vec();
        self.replace(screen, line);
    }

    fn recall_newer(&mut self, screen: &mut dyn Screen, history: &History) {
        match self.browsing {
            None => {}
            Some(0) => {
                self.browsing = None;
                let draft = core::mem::take(&mut self.draft);
                self.replace(screen, draft);
            }
            Some(back) => {
                self.browsing = Some(back - 1);
                let line = history.recent(back - 1).map(|l| l.as_bytes().to_vec()).unwrap_or_default();
                self.replace(screen, line);
            }
        }
    }
}
