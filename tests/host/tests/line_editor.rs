//! The line editor (`src/line_editor.rs`), driven against a simulated
//! character screen with `gfx::Console`'s exact wrap-and-scroll rules.
//!
//! A reference model keeps the line and cursor the editor *should* have.
//! After every keystroke the screen is checked against it: every visible
//! cell of the line shows the model's character (or a blank past its end,
//! out to the longest the line has ever been, so nothing stale survives an
//! edit), the prompt before the line is untouched, and exactly one cell
//! carries the cursor mark -- the model cursor's. Random key sequences run
//! on screens small enough that lines wrap and scroll constantly.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use wisp_host_tests::line_editor::{
    self, History, Outcome, Screen, KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT, KEY_UP, MAX_LINE,
};

const ENTER: u8 = 0x0d;
const BACKSPACE: u8 = 0x08;
const ESC: u8 = 0x1b;

// ---- a simulated console ---------------------------------------------------

struct FakeScreen {
    cols: usize,
    rows: usize,
    grid: Vec<Vec<u8>>,
    col: usize,
    row: usize,
    scrolls: u64,
    marks: BTreeSet<(usize, usize)>,
}

impl FakeScreen {
    fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows, grid: vec![vec![b' '; cols]; rows], col: 0, row: 0, scrolls: 0, marks: BTreeSet::new() }
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= self.rows {
            self.scroll_up();
            self.row = self.rows - 1;
        }
    }

    /// `Console::write_str` for printable ASCII and `\n`.
    fn write(&mut self, text: &str) {
        for b in text.bytes() {
            if b == b'\n' {
                self.newline();
            } else {
                self.put(b);
            }
        }
    }
}

impl Screen for FakeScreen {
    fn cols(&self) -> usize {
        self.cols
    }
    fn rows(&self) -> usize {
        self.rows
    }
    fn position(&self) -> (usize, usize) {
        (self.col, self.row)
    }
    fn set_position(&mut self, col: usize, row: usize) {
        assert!(col <= self.cols && row < self.rows, "set_position({col}, {row}) outside {}x{}", self.cols, self.rows);
        self.col = col;
        self.row = row;
    }
    fn scrolls(&self) -> u64 {
        self.scrolls
    }
    fn put(&mut self, byte: u8) {
        assert!((0x20..=0x7e).contains(&byte), "put of non-printable {byte:#x}");
        if self.col >= self.cols {
            self.newline();
        }
        self.grid[self.row][self.col] = byte;
        self.col += 1;
    }
    fn scroll_up(&mut self) {
        self.grid.remove(0);
        self.grid.push(vec![b' '; self.cols]);
        self.scrolls += 1;
        // Marks are pixels: they scroll with everything else.
        self.marks = self.marks.iter().filter(|&&(_, r)| r > 0).map(|&(c, r)| (c, r - 1)).collect();
    }
    fn mark(&mut self, col: usize, row: usize, byte: u8, on: bool) {
        assert!(col < self.cols && row < self.rows, "mark({col}, {row}) outside the screen");
        self.grid[row][col] = byte;
        if on {
            self.marks.insert((col, row));
        } else {
            self.marks.remove(&(col, row));
        }
    }
}

/// The editor draws through this; the harness keeps another handle to the
/// same screen and inspects it between keystrokes, while the editor is
/// waiting for a key and holds no borrow of it.
struct Shared(Rc<RefCell<FakeScreen>>);

impl Screen for Shared {
    fn cols(&self) -> usize {
        self.0.borrow().cols()
    }
    fn rows(&self) -> usize {
        self.0.borrow().rows()
    }
    fn position(&self) -> (usize, usize) {
        self.0.borrow().position()
    }
    fn set_position(&mut self, col: usize, row: usize) {
        self.0.borrow_mut().set_position(col, row)
    }
    fn scrolls(&self) -> u64 {
        self.0.borrow().scrolls()
    }
    fn put(&mut self, byte: u8) {
        self.0.borrow_mut().put(byte)
    }
    fn scroll_up(&mut self) {
        self.0.borrow_mut().scroll_up()
    }
    fn mark(&mut self, col: usize, row: usize, byte: u8, on: bool) {
        self.0.borrow_mut().mark(col, row, byte, on)
    }
}

// ---- the reference model ---------------------------------------------------

#[derive(Clone)]
struct Model {
    buf: Vec<u8>,
    cursor: usize,
    browsing: Option<usize>,
    draft: Vec<u8>,
    history: Vec<String>,
    esc_clears: bool,
}

impl Model {
    fn recent(&self, back: usize) -> Option<Vec<u8>> {
        self.history.len().checked_sub(back + 1).map(|i| self.history[i].as_bytes().to_vec())
    }

    fn apply(&mut self, key: u8) {
        match key {
            0x20..=0x7e => {
                if self.buf.len() < MAX_LINE {
                    self.buf.insert(self.cursor, key);
                    self.cursor += 1;
                }
            }
            BACKSPACE if self.cursor > 0 => {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
            }
            KEY_DELETE if self.cursor < self.buf.len() => {
                self.buf.remove(self.cursor);
            }
            KEY_LEFT => self.cursor = self.cursor.saturating_sub(1),
            KEY_RIGHT => self.cursor = (self.cursor + 1).min(self.buf.len()),
            KEY_HOME => self.cursor = 0,
            KEY_END => self.cursor = self.buf.len(),
            ESC if self.esc_clears => {
                self.browsing = None;
                self.buf.clear();
                self.cursor = 0;
            }
            KEY_UP => {
                let back = self.browsing.map_or(0, |b| b + 1);
                if let Some(line) = self.recent(back) {
                    if self.browsing.is_none() {
                        self.draft = self.buf.clone();
                    }
                    self.browsing = Some(back);
                    self.buf = line;
                    self.cursor = self.buf.len();
                }
            }
            KEY_DOWN => match self.browsing {
                None => {}
                Some(0) => {
                    self.browsing = None;
                    self.buf = std::mem::take(&mut self.draft);
                    self.cursor = self.buf.len();
                }
                Some(b) => {
                    self.browsing = Some(b - 1);
                    self.buf = self.recent(b - 1).unwrap();
                    self.cursor = self.buf.len();
                }
            },
            _ => {}
        }
    }
}

// ---- the harness -----------------------------------------------------------

/// Where character `index` of the line should be drawn, given where the
/// line started and how far the screen has scrolled since.
fn cell(screen: &FakeScreen, origin: (usize, i64), origin_scrolls: u64, index: usize) -> (usize, i64) {
    let linear = origin.0 + index;
    let scrolled = (screen.scrolls - origin_scrolls) as i64;
    (linear % screen.cols, origin.1 - scrolled + (linear / screen.cols) as i64)
}

struct Run {
    screen: FakeScreen,
    model: Model,
    history: History,
}

/// Type `prompt` on a `cols` x `rows` screen whose cursor starts on
/// `start_row`, then feed `keys` to the editor (ENTER is appended),
/// checking the screen against the model after every key. Returns the
/// outcome and the final state.
fn run(cols: usize, rows: usize, start_row: usize, prompt: &str, keys: &[u8], history_lines: &[&str], esc_clears: bool) -> (Outcome, Run) {
    let mut screen = FakeScreen::new(cols, rows);
    for _ in 0..start_row {
        screen.write("x\n");
    }
    screen.write(prompt);
    let prompt_end = screen.position();
    // Linear cell arithmetic: a prompt that filled its row leaves the
    // cursor at column `cols`, which is the next row's first cell.
    let origin = (prompt_end.0, prompt_end.1 as i64);
    let origin_scrolls = screen.scrolls;
    // Where the prompt's characters are, to check they survive.
    let prompt_len = prompt.len();

    let mut history = History::new();
    for l in history_lines {
        history.push(l);
    }
    let mut model = Model {
        buf: Vec::new(),
        cursor: 0,
        browsing: None,
        draft: Vec::new(),
        // What `History::push` keeps: no blank lines, no immediate repeats.
        history: history_lines.iter().fold(Vec::new(), |mut kept: Vec<String>, l| {
            if !l.trim().is_empty() && kept.last().map(String::as_str) != Some(*l) {
                kept.push(l.to_string());
            }
            kept
        }),
        esc_clears,
    };

    let mut queue: Vec<u8> = keys.to_vec();
    queue.push(ENTER);
    queue.reverse();
    let mut extent = 0usize; // longest the line has been
    let mut steps = 0usize;

    // `check` runs before each key is read: the screen must already show
    // the effect of every key before it.
    let check = |screen: &FakeScreen, model: &Model, extent: usize, steps: usize| {
        let ctx = format!("{cols}x{rows}, prompt {prompt:?}, after {steps} keys, line {:?}", String::from_utf8_lossy(&model.buf));
        for index in 0..extent.max(model.buf.len()) {
            let (c, r) = cell(screen, origin, origin_scrolls, index);
            if r < 0 {
                continue;
            }
            let want = model.buf.get(index).copied().unwrap_or(b' ');
            let got = screen.grid[r as usize][c];
            assert_eq!(got as char, want as char, "{ctx}: cell of index {index} at ({c}, {r})");
        }
        // The prompt's last characters (those still on screen) are intact.
        for k in 0..prompt_len {
            let linear = origin.0 as i64 - (prompt_len - k) as i64 + (origin.1 * cols as i64);
            let scrolled = (screen.scrolls - origin_scrolls) as i64;
            let row = linear.div_euclid(cols as i64) - scrolled;
            let col = linear.rem_euclid(cols as i64) as usize;
            if row >= 0 && row < rows as i64 {
                assert_eq!(screen.grid[row as usize][col], prompt.as_bytes()[k], "{ctx}: prompt damaged");
            }
        }
        let (c, r) = cell(screen, origin, origin_scrolls, model.cursor);
        let expected: BTreeSet<(usize, usize)> =
            if r >= 0 && r < rows as i64 { [(c, r as usize)].into() } else { BTreeSet::new() };
        assert_eq!(screen.marks, expected, "{ctx}: cursor mark");
    };

    let shared = Rc::new(RefCell::new(screen));
    let outcome = {
        let watch = Rc::clone(&shared);
        let model = &mut model;
        let mut next_key = || {
            if steps > 0 {
                check(&watch.borrow(), model, extent, steps);
            }
            let key = queue.pop().expect("editor read past the end of the keys");
            if key != ENTER && !(key == ESC && !esc_clears) {
                model.apply(key);
                extent = extent.max(model.buf.len());
            }
            steps += 1;
            key
        };
        line_editor::read_line(&mut Shared(Rc::clone(&shared)), &mut next_key, Some(&mut history), esc_clears)
    };
    let screen = Rc::try_unwrap(shared).ok().expect("screen still shared").into_inner();
    // Enter (or Esc) leaves no mark, and the cursor where plain echoing
    // would have: just past the text, or -- for an empty line -- where
    // the prompt ended, even if that is past the last column.
    assert!(screen.marks.is_empty(), "a cursor mark was left behind");
    if !model.buf.is_empty() {
        let (c, r) = cell(&screen, origin, origin_scrolls, model.buf.len() - 1);
        if r >= 0 {
            assert_eq!(screen.position(), (c + 1, r as usize), "cursor not parked after the text");
        }
    } else {
        let row = prompt_end.1 as i64 - (screen.scrolls - origin_scrolls) as i64;
        if row >= 0 {
            assert_eq!(screen.position(), (prompt_end.0, row as usize), "an empty line moved the cursor");
        }
    }
    (outcome, Run { screen, model, history })
}

fn line_of(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Line(l) | Outcome::Escaped(l) => l,
    }
}

// ---- deterministic cases ---------------------------------------------------

#[test]
fn typing_and_editing_mid_line() {
    let keys: Vec<u8> = b"helo".iter().copied()
        .chain([KEY_LEFT, b'l'])          // hel|o -> hell|o
        .chain([KEY_HOME, b'>', b' '])    // "> hello"
        .chain([KEY_END, b'!'])
        .chain([KEY_LEFT, KEY_LEFT, KEY_DELETE]) // remove the 'o'
        .chain([BACKSPACE])                      // and the 'l' before it
        .collect();
    let (out, _) = run(40, 5, 0, "> ", &keys, &[], true);
    assert_eq!(line_of(&out), "> hel!");
}

#[test]
fn history_recalls_and_restores_the_draft() {
    let hist = ["run calc", "ls", "echo hi"];
    let (out, r) = run(40, 5, 0, "> ", &[KEY_UP], &hist, true);
    assert_eq!(line_of(&out), "echo hi");
    assert_eq!(r.model.buf, b"echo hi");
    let (out, _) = run(40, 5, 0, "> ", &[KEY_UP, KEY_UP, KEY_UP, KEY_UP], &hist, true);
    assert_eq!(line_of(&out), "run calc", "Up stops at the oldest entry");
    let mut keys = b"dra".to_vec();
    keys.extend([KEY_UP, KEY_UP, KEY_DOWN, KEY_DOWN, b'f', b't']);
    let (out, _) = run(40, 5, 0, "> ", &keys, &hist, true);
    assert_eq!(line_of(&out), "draft", "Down past the newest entry restores what was typed");
    let (out, _) = run(40, 5, 0, "> ", &[KEY_UP, KEY_DOWN, KEY_DOWN], &hist, true);
    assert_eq!(line_of(&out), "");
}

#[test]
fn the_finished_line_joins_the_history() {
    let (_, r) = run(40, 5, 0, "> ", b"mem", &["ls"], true);
    let mut history = r.history;
    // Recall it, and the entry before it, through later reads.
    for (keys, want) in [(vec![ENTER, KEY_UP], "mem"), (vec![ENTER, KEY_UP, KEY_UP], "ls")] {
        let mut screen = FakeScreen::new(40, 5);
        let mut keys = keys.into_iter();
        let out = line_editor::read_line(&mut screen, &mut || keys.next_back().unwrap(), Some(&mut history), true);
        assert_eq!(line_of(&out), want);
    }
    // Blank lines and an immediate repeat are not added: one Up still
    // reaches "mem", two reach "ls".
    for line in ["   ", "mem"] {
        let mut screen = FakeScreen::new(40, 5);
        let mut keys: Vec<u8> = line.bytes().collect();
        keys.push(ENTER);
        keys.reverse();
        line_editor::read_line(&mut screen, &mut || keys.pop().unwrap(), Some(&mut history), true);
    }
    let mut screen = FakeScreen::new(40, 5);
    let mut keys = vec![ENTER, KEY_UP, KEY_UP].into_iter();
    let out = line_editor::read_line(&mut screen, &mut || keys.next_back().unwrap(), Some(&mut history), true);
    assert_eq!(line_of(&out), "ls");
}

#[test]
fn esc_clears_at_a_prompt_and_ends_a_program_read() {
    let mut keys = b"oops".to_vec();
    keys.push(ESC);
    keys.extend(b"ok");
    let (out, _) = run(40, 5, 0, "> ", &keys, &[], true);
    assert!(matches!(&out, Outcome::Line(l) if l == "ok"));

    let mut screen = FakeScreen::new(40, 5);
    let mut keys = vec![ESC, b'f', b'a', b'h'].into_iter();
    let out = line_editor::read_line(&mut screen, &mut || keys.next_back().unwrap(), None, false);
    assert!(matches!(&out, Outcome::Escaped(l) if l == "haf"));
    assert!(screen.marks.is_empty());
}

#[test]
fn lines_are_capped() {
    let keys = vec![b'z'; MAX_LINE + 50];
    let (out, _) = run(200, 40, 0, "", &keys, &[], true);
    assert_eq!(line_of(&out).len(), MAX_LINE);
}

#[test]
fn a_prompt_that_fills_its_row_exactly() {
    // The console's cursor sits past the last column (wrap pending); the
    // line must start at the next row's first cell -- including on the
    // bottom row, where that needs a scroll.
    for start_row in [0, 3] {
        let (out, r) = run(6, 4, start_row, "wisp> ", b"abc", &[], true);
        assert_eq!(line_of(&out), "abc");
        assert!(r.screen.grid.iter().any(|row| row.starts_with(b"abc")), "{start_row}");
        // Enter on an empty line must leave the cursor at the end of the
        // prompt's row, as plain echoing would, or the shell's newline
        // after it would add a blank line.
        let (out, r) = run(6, 4, start_row, "wisp> ", &[], &[], true);
        assert_eq!(line_of(&out), "");
        assert_eq!(r.screen.position().0, 6, "{start_row}");
        let (out, _) = run(6, 4, start_row, "wisp> ", &[b'x', BACKSPACE], &[], true);
        assert_eq!(line_of(&out), "");
    }
}

#[test]
fn editing_a_wrapped_line_at_the_bottom_of_the_screen() {
    // 10 columns, prompt on the last row: the line wraps and scrolls as it
    // is typed, then is edited back and forth across the wraps.
    let mut keys: Vec<u8> = b"0123456789abcdefghijklmnopqrstuvwxyz".to_vec();
    keys.extend([KEY_HOME, b'<', KEY_END, b'>']);
    keys.extend(std::iter::repeat_n(KEY_LEFT, 15));
    keys.extend([KEY_DELETE, KEY_DELETE, BACKSPACE, b'*']);
    keys.extend(std::iter::repeat_n(BACKSPACE, 20));
    let (out, r) = run(10, 4, 3, "> ", &keys, &[], true);
    // "<" + digits + letters + ">", then: delete "mn", backspace "l",
    // type "*", and 20 backspaces take out "23456789abcdefghijk*".
    assert_eq!(line_of(&out), "<01opqrstuvwxyz>");
    assert!(r.screen.scrolls > 0);
}

// ---- randomized ------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn random_keys(rng: &mut Rng, n: usize) -> Vec<u8> {
    const SPECIAL: [u8; 10] = [BACKSPACE, KEY_DELETE, KEY_LEFT, KEY_RIGHT, KEY_HOME, KEY_END, KEY_UP, KEY_DOWN, ESC, 0x07];
    (0..n)
        .map(|_| {
            if rng.below(3) == 0 {
                SPECIAL[rng.below(SPECIAL.len())]
            } else {
                b"abcdefghij (){}+-"[rng.below(17)]
            }
        })
        .collect()
}

#[test]
fn random_editing_keeps_the_screen_exact() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let history = ["fn f(x) { return x * 2; }", "let a = 1", "", "print(f(a))", "a long line that wraps around a narrow screen several times over"];
    let mut runs = 0;
    for (cols, rows) in [(10, 4), (7, 3), (13, 6), (80, 25), (1, 5), (3, 2)] {
        for prompt in ["", "> ", "wisp> ", "...   "] {
            for start_row in [0, rows - 1] {
                for _ in 0..12 {
                    let n = 20 + rng.below(140);
                    let keys = random_keys(&mut rng, n);
                    let esc_clears = rng.below(4) != 0;
                    // Esc ends a non-clearing read early; cut the keys there.
                    let keys: Vec<u8> = if esc_clears {
                        keys
                    } else {
                        keys.into_iter().take_while(|&k| k != ESC).collect()
                    };
                    let (out, r) = run(cols, rows, start_row, prompt, &keys, &history, esc_clears);
                    assert_eq!(line_of(&out).as_bytes(), r.model.buf.as_slice());
                    runs += 1;
                }
            }
        }
    }
    assert_eq!(runs, 6 * 4 * 2 * 12);
}
