//! Bounded text buffer for the native WISP editor. Cursor offsets are
//! always UTF-8 boundaries; screen columns count characters, not bytes.

use alloc::string::String;

use crate::line_editor::{KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT, KEY_UP};

pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
const FULL_MESSAGE: &str = "editor source limit is 64 KiB";
const INDENT: usize = 4;

pub struct Editor {
    source: String,
    saved: String,
    cursor: usize,
    preferred_column: Option<usize>,
}

impl Editor {
    pub fn new(source: &str) -> Result<Self, &'static str> {
        if source.len() > MAX_SOURCE_BYTES {
            return Err(FULL_MESSAGE);
        }
        Ok(Self {
            source: String::from(source),
            saved: String::from(source),
            cursor: 0,
            preferred_column: None,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_dirty(&self) -> bool {
        self.source != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved.clone_from(&self.source);
    }

    /// Zero-based `(line, column)`; existing tabs occupy one character.
    pub fn position(&self) -> (usize, usize) {
        let before = &self.source[..self.cursor];
        let line = before.bytes().filter(|&b| b == b'\n').count();
        let column = before[self.line_start()..].chars().count();
        (line, column)
    }

    pub fn line_count(&self) -> usize {
        self.source.bytes().filter(|&b| b == b'\n').count() + 1
    }

    /// Select the first column of a line, clamping to the last line.
    pub fn go_to_line(&mut self, line: usize) {
        self.cursor = self.offset_at(line.min(self.line_count() - 1), 0);
        self.preferred_column = None;
    }

    /// Find the next occurrence of `needle` after the cursor, wrapping around
    /// to the beginning of the buffer if needed.
    pub fn search_next(&mut self, needle: &str) -> bool {
        if needle.is_empty() || self.source.is_empty() {
            return false;
        }
        let search_from = self.source[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |ch| self.cursor + ch.len_utf8());
        if search_from <= self.source.len() {
            if let Some(rel) = self.source[search_from..].find(needle) {
                self.cursor = search_from + rel;
                self.preferred_column = None;
                return true;
            }
        }
        if let Some(pos) = self.source.find(needle) {
            self.cursor = pos;
            self.preferred_column = None;
            return true;
        }
        false
    }

    /// Keep the desired column while crossing shorter lines. Page keys
    /// use this with the actual viewport height supplied by the UI.
    pub fn move_vertical(&mut self, delta: isize) {
        let (line, column) = self.position();
        let target_column = *self.preferred_column.get_or_insert(column);
        let target_line = line.saturating_add_signed(delta).min(self.line_count() - 1);
        self.cursor = self.offset_at(target_line, target_column);
    }

    /// Apply a decoded keyboard byte. Commands and unsupported keys are
    /// ignored. A failed insertion leaves both text and cursor intact.
    pub fn handle_key(&mut self, key: u8) -> Result<(), &'static str> {
        match key {
            KEY_UP => self.move_vertical(-1),
            KEY_DOWN => self.move_vertical(1),
            KEY_LEFT => {
                self.preferred_column = None;
                if let Some((offset, _)) = self.source[..self.cursor].char_indices().next_back() {
                    self.cursor = offset;
                }
            }
            KEY_RIGHT => {
                self.preferred_column = None;
                if let Some(ch) = self.source[self.cursor..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            KEY_HOME => {
                self.cursor = self.line_start();
                self.preferred_column = None;
            }
            KEY_END => {
                self.cursor = self.line_end();
                self.preferred_column = None;
            }
            0x08 => {
                self.preferred_column = None;
                if let Some((offset, _)) = self.source[..self.cursor].char_indices().next_back() {
                    self.source.drain(offset..self.cursor);
                    self.cursor = offset;
                }
            }
            KEY_DELETE => {
                self.preferred_column = None;
                if let Some(ch) = self.source[self.cursor..].chars().next() {
                    self.source.drain(self.cursor..self.cursor + ch.len_utf8());
                }
            }
            0x0d => {
                let prefix = &self.source[self.line_start()..self.cursor];
                let mut spaces = prefix.bytes().take_while(|&b| b == b' ').count();
                if prefix.trim_end().ends_with('{') {
                    spaces += INDENT;
                }
                // Check before allocating or inserting the newline itself.
                self.check_room(1 + spaces)?;
                let mut text = String::with_capacity(1 + spaces);
                text.push('\n');
                for _ in 0..spaces {
                    text.push(' ');
                }
                self.insert(&text)?;
            }
            0x09 => {
                let spaces = INDENT - self.position().1 % INDENT;
                self.insert(&"    "[..spaces])?;
            }
            0x20..=0x7e => {
                let byte = [key];
                // Printable ASCII is a valid single-character string.
                self.insert(core::str::from_utf8(&byte).unwrap())?;
            }
            _ => {}
        }
        Ok(())
    }

    fn check_room(&self, bytes: usize) -> Result<(), &'static str> {
        if bytes > MAX_SOURCE_BYTES - self.source.len() {
            Err(FULL_MESSAGE)
        } else {
            Ok(())
        }
    }

    fn insert(&mut self, text: &str) -> Result<(), &'static str> {
        self.check_room(text.len())?;
        self.source.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.preferred_column = None;
        Ok(())
    }

    fn line_start(&self) -> usize {
        self.source[..self.cursor]
            .rfind('\n')
            .map_or(0, |offset| offset + 1)
    }

    fn line_end(&self) -> usize {
        self.source[self.cursor..]
            .find('\n')
            .map_or(self.source.len(), |offset| self.cursor + offset)
    }

    fn offset_at(&self, line: usize, column: usize) -> usize {
        let mut start = 0;
        for (index, text) in self.source.split('\n').enumerate() {
            if index == line {
                return start
                    + text
                        .char_indices()
                        .nth(column)
                        .map_or(text.len(), |(offset, _)| offset);
            }
            start += text.len() + 1;
        }
        self.source.len()
    }
}
