//! Raw Graphics Output Protocol (GOP) framebuffer text console.
//!
//! This module owns the pixels: it writes glyphs directly into the linear
//! framebuffer that the firmware hands us, using the embedded font in
//! [`crate::font`]. It deliberately does not use the UEFI Simple Text
//! Output protocol for on-screen rendering.

use core::fmt;

use uefi::proto::console::gop::PixelFormat;

use crate::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

/// An RGB color, channel order independent of the framebuffer's native
/// byte layout -- [`Console`] re-encodes it correctly for whichever
/// [`PixelFormat`] the firmware reports.
#[derive(Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build a `Color` from a packed `0xRRGGBB` value -- the format WISP's
    /// `rgb()` builtin produces and every drawing builtin (`plot`/`rect`/
    /// `line`/`fill_screen`) accepts as a color argument; see
    /// `docs/WISP_GRAPHICS.md`. Only the low 24 bits are read. This never
    /// needs to fail: `wisp::builtins::expect_color` has already validated
    /// the value fits in `0..=0xFFFFFF` before it ever reaches here (the
    /// sole caller is `shell.rs`'s `Host` impl), so an out-of-range packed
    /// value is a WISP-level runtime error long before it could reach this
    /// constructor, not something this constructor itself needs to guard
    /// against.
    pub const fn from_rgb24(packed: u32) -> Self {
        Self {
            r: ((packed >> 16) & 0xFF) as u8,
            g: ((packed >> 8) & 0xFF) as u8,
            b: (packed & 0xFF) as u8,
        }
    }
}

pub const BLACK: Color = Color::rgb(0x00, 0x00, 0x00);
pub const FOREGROUND: Color = Color::rgb(0xd0, 0xe8, 0xd0);
pub const ACCENT: Color = Color::rgb(0x33, 0xff, 0x88);
pub const ERROR: Color = Color::rgb(0xff, 0x55, 0x55);

/// A text console rendered directly onto a GOP linear framebuffer.
///
/// `Console` is built from a raw pointer/length pair obtained once from the
/// `GraphicsOutput` protocol's `FrameBuffer`, rather than holding onto the
/// `FrameBuffer` borrow itself. The framebuffer is a physical MMIO window
/// that firmware hands over, not a Boot Services allocation, so it stays
/// valid across `exit_boot_services`; `paging.rs` identity-maps it
/// (cache-disabled, non-executable) in the kernel's own page tables, so the
/// same pointer is valid for the life of the program.
pub struct Console {
    fb: *mut u8,
    fb_len: usize,
    /// Visible framebuffer width, in pixels.
    width: usize,
    /// Visible framebuffer height, in pixels.
    height: usize,
    /// Pixels per scanline (may exceed `width` due to hardware padding).
    stride: usize,
    /// True if the native pixel layout is byte-order B,G,R,(pad); false if
    /// it is R,G,B,(pad). These are the only two direct-pixel-access
    /// layouts UEFI defines.
    bgr: bool,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    fg: Color,
    bg: Color,
    /// How many times the contents have scrolled up a row, ever -- what
    /// lets `line_editor` find a line that scrolled while being typed.
    scrolls: u64,
}

impl Console {
    /// Build a console from a raw framebuffer pointer and its reported
    /// mode parameters.
    ///
    /// # Safety
    /// `fb` must point to a mapped, writable region of at least `fb_len`
    /// bytes, valid for the entire remaining lifetime of the program (true
    /// here because Boot Services is never exited). `width`, `height`, and
    /// `stride` must match the values reported by the active GOP mode.
    ///
    /// # Errors
    /// Returns `Err` if `format` is `Bitmask` or `BltOnly`. Those formats
    /// require going through the protocol's `blt()` method instead of
    /// direct pixel writes, which this text-mode-only console does not
    /// implement. Every GOP implementation this project targets (QEMU's
    /// stdvga under OVMF) reports `Bgr`, so this is a documented
    /// environmental precondition, not a missing feature.
    pub unsafe fn new(
        fb: *mut u8,
        fb_len: usize,
        width: usize,
        height: usize,
        stride: usize,
        format: PixelFormat,
    ) -> Result<Self, &'static str> {
        let bgr = match format {
            PixelFormat::Bgr => true,
            PixelFormat::Rgb => false,
            PixelFormat::Bitmask | PixelFormat::BltOnly => {
                return Err(
                    "unsupported GOP pixel format: this console requires direct \
                     pixel access (Rgb or Bgr); Bitmask/BltOnly framebuffers are \
                     not supported",
                );
            }
        };

        let cols = width / GLYPH_WIDTH;
        let rows = height / GLYPH_HEIGHT;
        if cols == 0 || rows == 0 {
            return Err("display resolution is too small for an 8x16 text console");
        }

        let mut console = Self {
            fb,
            fb_len,
            width,
            height,
            stride,
            bgr,
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            fg: FOREGROUND,
            bg: BLACK,
            scrolls: 0,
        };
        console.clear();
        Ok(console)
    }

    /// Total pixel-rows actually covered by whole character cells.
    fn usable_height(&self) -> usize {
        self.rows * GLYPH_HEIGHT
    }

    fn encode(&self, c: Color) -> u32 {
        if self.bgr {
            (c.b as u32) | ((c.g as u32) << 8) | ((c.r as u32) << 16)
        } else {
            (c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16)
        }
    }

    /// Write one pixel. `x` and `y` must be within `(width, height)`.
    #[inline]
    fn put_pixel(&mut self, x: usize, y: usize, value: u32) {
        debug_assert!(x < self.width && y < self.height);
        let byte_offset = (y * self.stride + x) * 4;
        debug_assert!(byte_offset + 4 <= self.fb_len);
        unsafe {
            let ptr = self.fb.add(byte_offset).cast::<u32>();
            ptr.write_volatile(value);
        }
    }

    /// Fill the entire framebuffer (including any stride padding) with the
    /// current background color and reset the cursor to the top-left.
    pub fn clear(&mut self) {
        let bg = self.encode(self.bg);
        let total_pixels = self.stride * self.height;
        for i in 0..total_pixels {
            let byte_offset = i * 4;
            unsafe {
                let ptr = self.fb.add(byte_offset).cast::<u32>();
                ptr.write_volatile(bg);
            }
        }
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    /// Draw one glyph cell at character coordinates `(col, row)`.
    fn draw_glyph(&mut self, col: usize, row: usize, ch: u8, fg: Color, bg: Color) {
        let bitmap = *font::glyph_for(ch);
        let fg_px = self.encode(fg);
        let bg_px = self.encode(bg);
        let base_x = col * GLYPH_WIDTH;
        let base_y = row * GLYPH_HEIGHT;
        for (row_idx, byte) in bitmap.iter().enumerate() {
            let y = base_y + row_idx;
            for bit in 0..GLYPH_WIDTH {
                let x = base_x + bit;
                let set = (byte >> (7 - bit)) & 1 != 0;
                self.put_pixel(x, y, if set { fg_px } else { bg_px });
            }
        }
    }

    /// Move the contents up by one character row, revealing a blank row at
    /// the bottom. Used when output reaches the last row.
    fn scroll_up_one_row(&mut self) {
        let usable_height = self.usable_height();
        let row_bytes = self.stride * 4;
        let move_bytes = (usable_height - GLYPH_HEIGHT) * row_bytes;
        let src_offset = GLYPH_HEIGHT * row_bytes;

        unsafe {
            let dst = self.fb;
            let src = self.fb.add(src_offset);
            // The source and destination regions overlap (this is a
            // shift-up-in-place), so this must be a memmove, not memcpy.
            // `core::ptr::copy` is documented to handle overlap correctly.
            core::ptr::copy(src, dst, move_bytes);
        }

        self.scrolls += 1;

        // Blank the newly-exposed last row.
        let bg = self.encode(self.bg);
        let last_row_y0 = usable_height - GLYPH_HEIGHT;
        for y in last_row_y0..usable_height {
            for x in 0..self.width {
                self.put_pixel(x, y, bg);
            }
        }
    }

    /// Advance the cursor to the start of the next line, scrolling if
    /// necessary.
    pub fn newline(&mut self) {
        self.cursor_col = 0;
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.scroll_up_one_row();
            self.cursor_row = self.rows - 1;
        }
    }

    /// Write one printable ASCII byte at the cursor and advance it,
    /// wrapping to a new line if the current row is full.
    pub fn put_ascii(&mut self, byte: u8) {
        if self.cursor_col >= self.cols {
            self.newline();
        }
        let fg = self.fg;
        let bg = self.bg;
        self.draw_glyph(self.cursor_col, self.cursor_row, byte, fg, bg);
        self.cursor_col += 1;
    }

    /// Write a string, honoring `\n` as a line break and rendering every
    /// other byte in the printable ASCII range directly. Any byte outside
    /// that range is substituted with `?` rather than silently dropped or
    /// panicking, so unexpected input (e.g. stray control bytes from a
    /// misbehaving command) can never corrupt console state.
    pub fn write_str(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                b'\n' => self.newline(),
                0x20..=0x7e => self.put_ascii(byte),
                _ => self.put_ascii(b'?'),
            }
        }
    }

    /// Set the foreground color used for subsequent writes.
    pub fn set_fg(&mut self, color: Color) {
        self.fg = color;
    }

    /// Return the raw framebuffer pointer, length, and mode geometry
    /// captured at [`Console::new`] time, as `(ptr, len, width, height,
    /// stride, bgr)` -- exactly the field order [`crate::panic::register`]
    /// expects, and exactly the field order `main.rs` destructures at both
    /// of its call sites. A plain, side-effect-free read of fields already
    /// captured once at construction, so it's safe to call as many times
    /// as needed (before *and* after `exit_boot_services`) without
    /// re-touching Boot Services or the GOP protocol at all.
    ///
    /// Two call sites depend on this returning the exact same bytes
    /// `Console` itself draws through, not a re-derived or re-queried
    /// copy: `panic::register` so a fault screen can draw directly into
    /// this same framebuffer independent of whatever state the
    /// interactive console was left in, and `paging::init`'s
    /// `extra_ranges` so the GOP framebuffer -- an MMIO window firmware
    /// does not reliably describe as its own memory-map entry -- gets
    /// identity-mapped under this kernel's own page tables before the
    /// live `CR3` switch away from firmware's.
    pub fn raw_parts(&self) -> (*mut u8, usize, usize, usize, usize, bool) {
        (self.fb, self.fb_len, self.width, self.height, self.stride, self.bgr)
    }

    // ---- WISP v2 graphics primitives (plot/rect/line/fill_screen) -----
    //
    // Every method below is a bridge target for `wisp::interp::Host`'s
    // graphics methods (implemented by `ShellHost` in `shell.rs`), which
    // means every coordinate reaching them originates from a WISP
    // script's `plot()`/`rect()`/`line()`/`fill_screen()` call and must be
    // treated as fully untrusted input -- unlike this console's *existing*
    // pixel-writing code above (`draw_glyph`, `scroll_up_one_row`), whose
    // coordinates are always derived from this console's own character
    // grid and therefore can never actually be out of range. That
    // distinction is exactly why `put_pixel` above only `debug_assert!`s
    // its bounds (a real, if-never-actually-hit invariant check on a
    // trusted caller) while every method below does a real, always-
    // compiled `if` bounds check instead: this project's release profile
    // (`Cargo.toml`'s `[profile.release]`) sets neither
    // `debug-assertions` nor `overflow-checks`, so it gets the standard
    // release defaults for both -- `debug_assert!` compiles to nothing at
    // all, and integer overflow silently wraps instead of panicking -- and
    // a script-controlled out-of-bounds pixel write would corrupt
    // arbitrary memory beyond the framebuffer rather than merely fail an
    // assertion. Every method below is written to be safe under both of
    // those release-profile defaults, not just "correct when debug
    // assertions happen to be compiled in."

    /// Pixel width of the visible framebuffer -- the same `width` this
    /// console renders its own text into, and the same value
    /// `raw_parts()` reports, just without the other four fields that
    /// method's two very specific callers (`panic::register`,
    /// `paging::init`) need and a graphics builtin does not.
    pub fn width_px(&self) -> usize {
        self.width
    }

    /// Pixel height of the visible framebuffer. See `width_px`.
    pub fn height_px(&self) -> usize {
        self.height
    }

    /// Set one pixel to `color`, doing nothing if `(x, y)` falls outside
    /// the visible framebuffer -- WISP's `plot()` documents this as a
    /// silent clip, not an error (see `docs/WISP_GRAPHICS.md`), since a
    /// drawing position that's computed (e.g. mid-animation) landing
    /// briefly off-screen is completely ordinary.
    ///
    /// `x`/`y` are `i64` (not `usize`) specifically so a negative
    /// coordinate is representable at all here rather than already
    /// rejected by the type before this method ever gets to apply that
    /// "silently clip" policy.
    pub fn plot_pixel(&mut self, x: i64, y: i64, color: Color) {
        if x < 0 || y < 0 {
            return;
        }
        // Lossless: x86_64 is this project's only target (`Cargo.toml`'s
        // `[[bin]]` builds solely for `x86_64-unknown-uefi`), so `usize`
        // is 64 bits wide here and every non-negative `i64` fits exactly.
        let (x, y) = (x as usize, y as usize);
        if x >= self.width || y >= self.height {
            return;
        }
        let value = self.encode(color);
        self.put_pixel(x, y, value);
    }

    /// Fill an axis-aligned rectangle: top-left corner `(x, y)`, `w` wide
    /// and `h` tall, clipped to the visible framebuffer -- a rectangle
    /// entirely off-screen draws nothing, and one that only partially
    /// overlaps the screen draws just the overlapping part. `w == 0` or
    /// `h == 0` is a legal, silent no-op (an empty rectangle covers no
    /// pixels), not an error.
    ///
    /// `w`/`h` are `usize`, not `i64`: a negative size cannot be
    /// represented at all, by construction, because it isn't a clipping
    /// question the way an off-screen *position* is -- it's essentially
    /// always a script bug (a swapped argument, bad arithmetic), so
    /// `wisp::builtins::builtin_rect` rejects it with a clear WISP-level
    /// error before ever calling this method, rather than this method
    /// having to decide what a "negative-sized rectangle" should even
    /// mean.
    pub fn draw_rect_filled(&mut self, x: i64, y: i64, w: usize, h: usize, color: Color) {
        if w == 0 || h == 0 {
            return;
        }
        let value = self.encode(color);

        // All the arithmetic below is `i64`, including the far edge
        // (`x + w`, `y + h`), so a huge `w`/`h` added to an extreme `x`/`y`
        // saturates at `i64::MAX` instead of silently wrapping past it
        // and back around to a small (very wrong) coordinate -- a script
        // can set `x`/`y`/`w`/`h` to whatever it likes, independently, so
        // this must hold even for combinations no legitimate script would
        // ever construct on purpose.
        let x1 = x.saturating_add(w as i64);
        let y1 = y.saturating_add(h as i64);

        // Clip to `[0, width) x [0, height)`. `.max(0)` folds a negative
        // edge up to 0 before the `as usize` cast (never cast a negative
        // i64 to usize directly -- that's a huge, wrong value, not a
        // clamp); the width/height on the `x86_64-unknown-uefi` target is
        // always representable, so the cast itself is exact per
        // `plot_pixel`'s doc comment above.
        let clip_x0 = x.max(0) as usize;
        let clip_y0 = y.max(0) as usize;
        let clip_x1 = (x1.max(0) as usize).min(self.width);
        let clip_y1 = (y1.max(0) as usize).min(self.height);
        if clip_x0 >= clip_x1 || clip_y0 >= clip_y1 {
            return; // entirely off-screen in at least one axis
        }

        for py in clip_y0..clip_y1 {
            for px in clip_x0..clip_x1 {
                self.put_pixel(px, py, value);
            }
        }
    }

    /// Coordinates further than this from the origin cause `draw_line` to
    /// draw nothing at all, as a defense-in-depth backstop -- see the
    /// method's own doc comment for why. `wisp::builtins::builtin_line`
    /// already rejects anything anywhere near this with a clear WISP-level
    /// error (its own, much tighter `MAX_LINE_COORD`) before ever calling
    /// this method, so in ordinary operation this backstop is never
    /// actually reached.
    const MAX_LINE_STEPS: u64 = 500_000;

    /// Draw a straight, one-pixel-wide line from `(x1, y1)` to `(x2, y2)`
    /// inclusive, using Bresenham's integer algorithm -- WISP has no
    /// floating-point type (see `docs/WISP_LANGUAGE.md`), and this project
    /// deliberately avoids introducing floating-point arithmetic into the
    /// kernel binary anywhere before that's a real, considered feature
    /// (FPU/SSE exception handling has never been exercised end to end
    /// here the way `idt.rs`'s other fault paths have), so this stays
    /// integer-only throughout, matching the language it serves. Each
    /// point along the line is clipped independently via `plot_pixel`, so
    /// a line that runs off one edge of the screen and back on renders
    /// correctly instead of being rejected or truncated wholesale.
    ///
    /// # Why the step count must be bounded before the walk starts
    /// Unlike `plot_pixel` (always exactly one pixel) and
    /// `draw_rect_filled` (already clipped to at most `width * height`
    /// pixels *before* it starts filling, regardless of how large `w`/`h`
    /// are), this method's cost is proportional to `max(|x2-x1|, |y2-y1|)`
    /// -- a property of the endpoints themselves, before any clipping
    /// against the screen even happens. A script that (by bug, not
    /// malice -- an uninitialized variable, a sign error) passes two
    /// far-apart extreme coordinates could otherwise make this walk for
    /// an astronomically long time, or make the `dx`/`dy` arithmetic
    /// overflow along the way. `wisp::builtins::builtin_line` is the
    /// primary defense (a clear, immediate WISP-level error long before
    /// either problem could occur); `MAX_LINE_STEPS` below is a second,
    /// always-on backstop in this method itself, so `draw_line` can never
    /// be walked for an unbounded number of steps regardless of what any
    /// future caller passes it.
    pub fn draw_line(&mut self, x1: i64, y1: i64, x2: i64, y2: i64, color: Color) {
        // `.wrapping_sub()`, not `-`: with overflow-checks off (this
        // project's release default -- see this section's module-level
        // note above), plain `-` would silently wrap on an adversarial
        // input anyway, but `wrapping_sub` says so explicitly rather than
        // relying on that default. `.unsigned_abs()`, not `.abs()`: unlike
        // `.abs()`, which cannot represent `-i64::MIN` and either panics
        // (debug) or wraps back to a *negative* `i64::MIN` (release) --
        // silently defeating a magnitude check like the one right below
        // -- `.unsigned_abs()` returns a `u64` and is correct for every
        // possible `i64` input, including `i64::MIN`, with no panic and no
        // wraparound-to-negative ever.
        let raw_dx = x2.wrapping_sub(x1);
        let raw_dy = y2.wrapping_sub(y1);
        if raw_dx.unsigned_abs().max(raw_dy.unsigned_abs()) > Self::MAX_LINE_STEPS {
            return;
        }

        // Bresenham's algorithm, integer-only, exactly the well-known
        // all-octants form (e.g. Wikipedia's "Bresenham's line
        // algorithm"): `dy` here is *negative* -- that sign choice, not a
        // positive `dy`, is what lets the single `error` accumulator below
        // handle all eight octants/directions correctly with the same two
        // comparisons every iteration, rather than needing a
        // slope-dependent branch. Verified by hand against horizontal,
        // vertical, diagonal, shallow, steep, and single-point (`x1==x2`
        // and `y1==y2`) cases before landing on this exact form.
        let dx = raw_dx.unsigned_abs() as i64;
        let sx: i64 = if x1 < x2 { 1 } else { -1 };
        let dy = -(raw_dy.unsigned_abs() as i64);
        let sy: i64 = if y1 < y2 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut px, mut py) = (x1, y1);

        loop {
            self.plot_pixel(px, py, color);
            if px == x2 && py == y2 {
                break;
            }
            let e2 = 2 * error;
            if e2 >= dy {
                error += dy;
                px += sx;
            }
            if e2 <= dx {
                error += dx;
                py += sy;
            }
        }
    }

    /// Fill the entire visible framebuffer with `color`. Does not move or
    /// otherwise touch the text cursor (`cursor_col`/`cursor_row`) --
    /// unlike `clear()`, this is a pure graphics primitive, not a console
    /// operation, so `print()`-family output continues exactly where it
    /// left off afterward. Text and graphics are drawn into the same one
    /// framebuffer (there is only the one), so whichever was drawn most
    /// recently wins in any pixel the two happen to overlap -- a
    /// deliberate, documented simplification (see `docs/WISP_GRAPHICS.md`)
    /// rather than maintaining two separate surfaces and compositing them,
    /// which this project has no need for.
    pub fn fill_screen(&mut self, color: Color) {
        let value = self.encode(color);
        let total_pixels = self.stride * self.height;
        for i in 0..total_pixels {
            let byte_offset = i * 4;
            unsafe {
                let ptr = self.fb.add(byte_offset).cast::<u32>();
                ptr.write_volatile(value);
            }
        }
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Console::write_str(self, s);
        Ok(())
    }
}

/// The line editor's view of the console (see `line_editor::Screen`).
/// Positions are character cells, exactly the console's own cursor.
impl crate::line_editor::Screen for Console {
    fn cols(&self) -> usize {
        self.cols
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn position(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    fn set_position(&mut self, col: usize, row: usize) {
        self.cursor_col = col.min(self.cols);
        self.cursor_row = row.min(self.rows - 1);
    }

    fn scrolls(&self) -> u64 {
        self.scrolls
    }

    fn put(&mut self, byte: u8) {
        self.put_ascii(byte);
    }

    fn scroll_up(&mut self) {
        self.scroll_up_one_row();
    }

    /// The cursor is a two-pixel underline across the cell's bottom rows,
    /// in the text color: visible on any character, and clear of the
    /// glyph itself, which a block cursor would cover.
    fn mark(&mut self, col: usize, row: usize, byte: u8, on: bool) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let (fg, bg) = (self.fg, self.bg);
        self.draw_glyph(col, row, byte, fg, bg);
        if on {
            let px = self.encode(fg);
            let y0 = row * GLYPH_HEIGHT + GLYPH_HEIGHT - 2;
            for y in y0..y0 + 2 {
                for x in col * GLYPH_WIDTH..(col + 1) * GLYPH_WIDTH {
                    self.put_pixel(x, y, px);
                }
            }
        }
    }
}
