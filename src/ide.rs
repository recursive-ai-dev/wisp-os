//! Wisp Studio: a single-buffer native editor over the framebuffer console.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::editor::Editor;
use crate::gfx::{self, Color, Console};
use crate::keyboard;
use crate::line_editor::{self, Screen, KEY_F1, KEY_PAGE_DOWN, KEY_PAGE_UP};
use crate::projects;
use crate::save_format::SaveDisk;
use crate::wisp;

const TEXT: u8 = 0;
const DIM: u8 = 1;
const ACCENT: u8 = 2;
const STRING: u8 = 3;
const NUMBER: u8 = 4;
const KEYWORD: u8 = 5;
const ERROR: u8 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    byte: u8,
    style: u8,
    cursor: bool,
}

const BLANK: Cell = Cell {
    byte: b' ',
    style: TEXT,
    cursor: false,
};

struct View {
    cols: usize,
    rows: usize,
    top: usize,
    left: usize,
    previous: Vec<Cell>,
}

impl View {
    fn new(console: &Console) -> Self {
        Self {
            cols: console.cols(),
            rows: console.rows(),
            top: 0,
            left: 0,
            previous: Vec::new(),
        }
    }

    fn invalidate(&mut self, console: &mut Console) {
        console.set_fg(gfx::FOREGROUND);
        console.clear();
        self.previous.clear();
    }

    fn text(&self, cells: &mut [Cell], col: usize, row: usize, text: &str, style: u8) {
        if row >= self.rows {
            return;
        }
        for (x, ch) in text.chars().take(self.cols.saturating_sub(col)).enumerate() {
            cells[row * self.cols + col + x] = Cell {
                byte: display_char(ch),
                style,
                cursor: false,
            };
        }
    }

    fn draw(&mut self, console: &mut Console, cells: Vec<Cell>) {
        for (index, cell) in cells.iter().enumerate() {
            if self.previous.get(index) != Some(cell) {
                console.set_fg(color(cell.style));
                console.mark(index % self.cols, index / self.cols, cell.byte, cell.cursor);
            }
        }
        console.set_fg(gfx::FOREGROUND);
        self.previous = cells;
    }

    fn editor(
        &mut self,
        console: &mut Console,
        editor: &Editor,
        name: &str,
        status: &str,
        error: bool,
        confirming: bool,
    ) {
        let mut cells = vec![BLANK; self.cols * self.rows];
        let (line, column) = editor.position();
        let height = self.rows - 6;
        let width = self.cols - 7;
        if line < self.top {
            self.top = line;
        }
        if line >= self.top + height {
            self.top = line + 1 - height;
        }
        if column < self.left {
            self.left = column;
        }
        if column >= self.left + width {
            self.left = column + 1 - width;
        }
        self.text(
            &mut cells,
            1,
            0,
            &format!(
                "WISP STUDIO / {name}.wisp{}",
                if editor.is_dirty() { " *" } else { "" }
            ),
            ACCENT,
        );
        self.text(
            &mut cells,
            1,
            1,
            &format!(
                "Ln {}  Col {}    {} lines    {} bytes / {} saved max",
                line + 1,
                column + 1,
                editor.line_count(),
                editor.source().len(),
                projects::MAX_SAVED_SOURCE_BYTES
            ),
            DIM,
        );
        self.text(&mut cells, 0, 2, &"-".repeat(self.cols), DIM);
        for (offset, text) in editor
            .source()
            .split('\n')
            .skip(self.top)
            .take(height)
            .enumerate()
        {
            let row = offset + 3;
            self.text(
                &mut cells,
                0,
                row,
                &format!("{:>5} |", self.top + offset + 1),
                DIM,
            );
            let styles = highlight(text);
            for (x, (ch, style)) in text
                .chars()
                .zip(styles)
                .skip(self.left)
                .take(width)
                .enumerate()
            {
                cells[row * self.cols + 7 + x] = Cell {
                    byte: display_char(ch),
                    style,
                    cursor: false,
                };
            }
        }
        cells[(line - self.top + 3) * self.cols + column - self.left + 7].cursor = true;
        self.text(
            &mut cells,
            1,
            self.rows - 3,
            status,
            if error { ERROR } else { ACCENT },
        );
        self.text(&mut cells, 0, self.rows - 2, &"-".repeat(self.cols), DIM);
        self.text(
            &mut cells,
            1,
            self.rows - 1,
            if confirming {
                "Unsaved changes: [S]ave  [D]iscard  [Esc] Cancel"
            } else {
                "F1 Help  F2 Save  F3 Open  F4 Find  F5 Run  F6 Check  F7 Line  F10 Quit"
            },
            TEXT,
        );
        self.draw(console, cells);
    }

    fn prompt_bottom(&mut self, console: &mut Console, prompt: &str) -> Option<String> {
        console.set_position(1, self.rows - 1);
        console.set_fg(gfx::FOREGROUND);
        console.write_str(&" ".repeat(self.cols.saturating_sub(2)));
        console.set_position(1, self.rows - 1);
        console.set_fg(gfx::ACCENT);
        console.write_str(prompt);
        console.set_fg(gfx::FOREGROUND);
        keyboard::clear_stop_request();
        let result = line_editor::read_line(console, &mut keyboard::wait_for_key, None, false);
        keyboard::clear_stop_request();
        self.invalidate(console);
        match result {
            line_editor::Outcome::Line(text) => Some(text),
            line_editor::Outcome::Escaped(_) => None,
        }
    }

    fn help(&mut self, console: &mut Console) {
        let mut cells = vec![BLANK; self.cols * self.rows];
        for (row, text) in [
            " WISP STUDIO / HELP",
            "",
            " F2           Save to prog.<name> on the save disk",
            " F3           Open a saved program or bundled example; new names start empty",
            " F4           Search forward for text (wraps around; Enter repeats last search)",
            " F5           Run this buffer in a fresh WISP session; Esc stops the program",
            " F6           Check syntax without running; errors jump to their line",
            " F7           Jump directly to a 1-based line number",
            " F10 / Esc    Leave the editor; unsaved changes require a choice",
            "",
            " Arrows       Move through the source",
            " Home / End   Start / end of the current line",
            " PgUp / PgDn  Move a screen at a time",
            " Enter        New line with automatic indentation",
            " Tab          Insert spaces to the next four-column stop",
            " Backspace / Delete remove text, including line breaks",
            "",
            " Saved programs override bundled examples with the same name.",
            " Names: 1-27 letters, digits, underscores or hyphens; .wisp is optional.",
            " The editor and multi-slot save store hold up to 64 KiB (65536 bytes).",
            " Without a formatted save disk, editing and running still work in memory.",
            " Use the shell's format command to provision a blank save disk.",
            " The font displays ASCII; other characters are preserved and shown as ?.",
            "",
            " Press any key to return.",
        ]
        .iter()
        .enumerate()
        .take(self.rows)
        {
            self.text(
                &mut cells,
                0,
                row,
                text,
                if row == 0 { ACCENT } else { TEXT },
            );
        }
        self.draw(console, cells);
        keyboard::wait_for_key();
        keyboard::clear_stop_request();
    }

    fn open_name(
        &mut self,
        console: &mut Console,
        apps: &[(String, String)],
        disk: Option<&SaveDisk>,
    ) -> Result<Option<String>, &'static str> {
        let saved = projects::list(disk)?;
        let mut entries: Vec<String> = saved
            .iter()
            .map(|name| format!("{name}.wisp  [saved]"))
            .collect();
        entries.extend(apps.iter().map(|(name, _)| format!("{name}  [bundled]")));
        self.invalidate(console);
        let mut cells = vec![BLANK; self.cols * self.rows];
        self.text(&mut cells, 1, 0, "WISP STUDIO / OPEN OR CREATE", ACCENT);
        self.text(
            &mut cells,
            1,
            1,
            "Type a name to open it, or a new name to create it. Esc cancels.",
            TEXT,
        );
        for (row, entry) in entries.iter().take(self.rows - 6).enumerate() {
            self.text(&mut cells, 2, row + 3, entry, TEXT);
        }
        if entries.len() > self.rows - 6 {
            self.text(
                &mut cells,
                1,
                self.rows - 3,
                "More programs available; type any name below.",
                DIM,
            );
        }
        self.draw(console, cells);
        console.set_position(1, self.rows - 1);
        console.write_str("Name: ");
        keyboard::clear_stop_request();
        let result = line_editor::read_line(console, &mut keyboard::wait_for_key, None, false);
        keyboard::clear_stop_request();
        self.invalidate(console);
        match result {
            line_editor::Outcome::Line(name) if !name.trim().is_empty() => {
                projects::normalize_name(&name).map(Some)
            }
            _ => Ok(None),
        }
    }
}

fn display_char(ch: char) -> u8 {
    match ch {
        ' '..='~' => ch as u8,
        '\t' | '\r' => b' ',
        _ => b'?',
    }
}

fn color(style: u8) -> Color {
    match style {
        DIM => Color::rgb(0x70, 0x88, 0x80),
        ACCENT => gfx::ACCENT,
        STRING => Color::rgb(0xe5, 0xc0, 0x7b),
        NUMBER => Color::rgb(0xd1, 0x9a, 0x66),
        KEYWORD => Color::rgb(0x9d, 0x9f, 0xf2),
        ERROR => gfx::ERROR,
        _ => gfx::FOREGROUND,
    }
}

/// Best-effort coloring of incomplete lines; parsing always uses the real lexer.
fn highlight(text: &str) -> Vec<u8> {
    let chars: Vec<char> = text.chars().collect();
    let mut styles = vec![TEXT; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            styles[i..].fill(DIM);
            break;
        }
        if chars[i] == '"' {
            styles[i] = STRING;
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                styles[i] = STRING;
                let ch = chars[i];
                i += 1;
                if ch == '"' && !escaped {
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
        } else if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if matches!(
                word.as_str(),
                "let"
                    | "if"
                    | "elif"
                    | "else"
                    | "while"
                    | "for"
                    | "in"
                    | "fn"
                    | "return"
                    | "break"
                    | "continue"
                    | "and"
                    | "or"
                    | "not"
                    | "true"
                    | "false"
                    | "nil"
            ) {
                styles[start..i].fill(KEYWORD);
            } else if wisp::builtins::is_builtin(word.as_str()) {
                styles[start..i].fill(ACCENT);
            }
        } else {
            if chars[i].is_ascii_digit() {
                styles[i] = NUMBER;
            }
            i += 1;
        }
    }
    styles
}

fn save(editor: &mut Editor, name: &str, disk: Option<&SaveDisk>) -> Result<String, &'static str> {
    projects::save_source(name, editor.source(), disk)?;
    editor.mark_saved();
    Ok(format!("Saved prog.{name}"))
}

fn may_replace(
    view: &mut View,
    console: &mut Console,
    editor: &mut Editor,
    name: &str,
    disk: Option<&SaveDisk>,
    status: &mut String,
) -> bool {
    if !editor.is_dirty() {
        return true;
    }
    let mut error = false;
    loop {
        view.editor(console, editor, name, status, error, true);
        match keyboard::wait_for_key() {
            b's' | b'S' => match save(editor, name, disk) {
                Ok(message) => {
                    *status = message;
                    return true;
                }
                Err(message) => {
                    *status = format!("Save failed: {message}");
                    error = true;
                }
            },
            b'd' | b'D' => return true,
            keyboard::ESC => {
                keyboard::clear_stop_request();
                return false;
            }
            _ => {}
        }
    }
}

pub fn run(
    console: &mut Console,
    apps: &[(String, String)],
    rng: &mut u64,
    disk: Option<&SaveDisk>,
    initial_name: &str,
) -> Result<(), &'static str> {
    if console.cols() < 60 || console.rows() < 24 {
        return Err("Wisp Studio needs at least 60 columns and 24 rows");
    }
    let mut name = projects::normalize_name(if initial_name.trim().is_empty() {
        "main"
    } else {
        initial_name
    })?;
    let source = projects::load_source(&name, apps, disk)?;
    let mut editor = Editor::new(source.as_deref().unwrap_or(""))?;
    let mut status = if source.is_some() {
        "Opened program"
    } else {
        "New program; F1 for help"
    }
    .to_string();
    if !disk.is_some_and(SaveDisk::is_mounted) {
        status = "Memory only: no formatted save disk. Use 'disks' at the shell; F1 for help.".to_string();
    }
    let mut error = false;
    let mut last_search = String::new();
    let mut view = View::new(console);
    view.invalidate(console);
    keyboard::clear_stop_request();
    crate::debugcon::write_str("[ide] ready\n");
    loop {
        view.editor(console, &editor, &name, &status, error, false);
        let key = keyboard::wait_for_key();
        match key {
            KEY_F1 => view.help(console),
            k if k == KEY_F1 + 1 => match save(&mut editor, &name, disk) {
                Ok(message) => {
                    status = message;
                    error = false;
                }
                Err(message) => {
                    status = format!("Save failed: {message}");
                    error = true;
                }
            },
            k if k == KEY_F1 + 2 => {
                if may_replace(&mut view, console, &mut editor, &name, disk, &mut status) {
                    match view.open_name(console, apps, disk).and_then(|next| {
                        let Some(next) = next else {
                            return Ok(None);
                        };
                        let source = projects::load_source(&next, apps, disk)?.unwrap_or_default();
                        Editor::new(&source).map(|editor| Some((next, editor)))
                    }) {
                        Ok(Some((next, buffer))) => {
                            name = next;
                            editor = buffer;
                            view.top = 0;
                            view.left = 0;
                            status = "Opened program".to_string();
                            error = false;
                        }
                        Ok(None) => {}
                        Err(message) => {
                            status = message.to_string();
                            error = true;
                        }
                    }
                }
            }
            k if k == KEY_F1 + 3 => {
                let prompt = if last_search.is_empty() {
                    String::from("Find: ")
                } else {
                    format!("Find [{last_search}]: ")
                };
                if let Some(typed) = view.prompt_bottom(console, &prompt) {
                    let needle = if typed.is_empty() {
                        last_search.clone()
                    } else {
                        last_search.clone_from(&typed);
                        typed
                    };
                    if !needle.is_empty() {
                        if editor.search_next(&needle) {
                            status = format!("Found '{needle}'");
                            error = false;
                        } else {
                            status = format!("'{needle}' not found");
                            error = true;
                        }
                    }
                }
            }
            k if k == KEY_F1 + 4 => {
                view.invalidate(console);
                console.write_str(&format!("Running {name}.wisp -- Esc stops the program\n\n"));
                let result = crate::shell::run_program_source(editor.source(), console, rng, disk);
                match result {
                    Ok(()) => {
                        status = "Run finished".to_string();
                        error = false;
                    }
                    Err(problem) => {
                        editor.go_to_line(problem.line.saturating_sub(1) as usize);
                        status = problem.to_display_string();
                        console.set_fg(gfx::ERROR);
                        console.write_str(&format!("\n{status}\n"));
                        error = true;
                    }
                }
                console.set_fg(gfx::FOREGROUND);
                console.write_str("\nPress any key to return to Wisp Studio.\n");
                crate::debugcon::write_str("[ide] run finished\n");
                keyboard::wait_for_key();
                keyboard::clear_stop_request();
                view.invalidate(console);
            }
            k if k == KEY_F1 + 5 => {
                match wisp::lexer::lex(editor.source())
                    .and_then(|tokens| wisp::parser::parse(&tokens))
                {
                    Ok(_) => {
                        status = "Syntax OK".to_string();
                        error = false;
                    }
                    Err(problem) => {
                        editor.go_to_line(problem.line.saturating_sub(1) as usize);
                        status = problem.to_display_string();
                        error = true;
                    }
                }
            }
            k if k == KEY_F1 + 6 => {
                if let Some(typed) = view.prompt_bottom(console, "Go to line: ") {
                    let trimmed = typed.trim();
                    if !trimmed.is_empty() {
                        match trimmed.parse::<usize>() {
                            Ok(n) if n >= 1 => {
                                editor.go_to_line(n - 1);
                                status = format!("Jumped to line {}", editor.position().0 + 1);
                                error = false;
                            }
                            _ => {
                                status = String::from("Expected a positive line number");
                                error = true;
                            }
                        }
                    }
                }
            }
            k if k == KEY_F1 + 9 || k == keyboard::ESC => {
                keyboard::clear_stop_request();
                if may_replace(&mut view, console, &mut editor, &name, disk, &mut status) {
                    view.invalidate(console);
                    crate::debugcon::write_str("[ide] closed\n");
                    return Ok(());
                }
            }
            KEY_PAGE_UP => editor.move_vertical(-((view.rows - 6) as isize)),
            KEY_PAGE_DOWN => editor.move_vertical((view.rows - 6) as isize),
            _ => {
                let previous_len = editor.source().len();
                if let Err(message) = editor.handle_key(key) {
                    status = message.to_string();
                    error = true;
                } else if editor.source().len() != previous_len {
                    status = "Editing; F2 saves, F5 runs".to_string();
                    error = false;
                }
            }
        }
    }
}
