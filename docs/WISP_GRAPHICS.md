# Pixel graphics in WISP

This document is the deep reference for the seven graphics builtins
(`screen_width`, `screen_height`, `rgb`, `plot`, `rect`, `line`,
`fill_screen`) -- the first piece of WISP v2 to land. `docs/WISP_LANGUAGE.md`
has the quick version in its own "Graphics" section; read this one for the
exact clipping/error contract of each builtin, why two of them behave
differently from the rest on purpose, and how text and graphics interact
now that they share one framebuffer.

If you're an LLM being handed this document to write a new WISP program
that draws something: read this file plus `docs/WISP_LANGUAGE.md` in full,
then look at `esp/APPS/canvas.wisp` (exercises every builtin below,
interactively) and `esp/APPS/bounce.wisp` (a short automatic animation)
as worked examples before writing anything.

## The pixel model

`(0, 0)` is the top-left corner of the screen. `x` increases to the
right, `y` increases downward. Both are plain WISP `int`s -- there is no
separate pixel-coordinate or vector type, and none of the arithmetic
operators gained any new behavior for this feature; `x + 1`, `y * 2`, and
so on are exactly the same `int` arithmetic `docs/WISP_LANGUAGE.md`
already documents.

`screen_width()` and `screen_height()` report the framebuffer's actual
pixel dimensions, fixed for the life of the program (the display mode is
negotiated once with the firmware at boot and never changed afterward).
There is no way to change resolution, or to open more than one drawing
surface -- there is exactly one framebuffer, the same one the text
console renders into (see "Text and graphics share one framebuffer"
below).

## Color: a packed `int`, not a new type

WISP still has exactly the five value types `docs/WISP_LANGUAGE.md`
describes -- this feature did not add a sixth `color` type. A color is a
plain `int`, packed as `0xRRGGBB`: the red channel in bits 16-23, green
in bits 8-15, blue in bits 0-7, each `0..=255`. `rgb(255, 0, 0)` and the
equivalent literal `16711680` both mean pure red; nothing distinguishes
"a color" from "a plain number that happens to be used as one" at the
language level.

This is a deliberate fit within the existing language rather than a
workaround: WISP has no bitwise operators (see `docs/WISP_LANGUAGE.md`'s
operator table -- there is no `&`, `|`, `<<`, `>>`, or anything like
them), so a script cannot pack or unpack a color's channels by hand even
if it wanted to. `rgb(r, g, b)` is the only way to build a color value
from its channels; the bit-packing itself happens entirely on the native
Rust side (`wisp::builtins::builtin_rgb`), invisible to the script. If
you already know the packed hex value you want, writing the literal
integer directly (as `bounce.wisp` does for its two colors) works exactly
as well as calling `rgb()` -- they produce identical values.

`rgb(r, g, b)` requires each channel to be an `int` in `0..=255`;
anything outside that range, or a non-`int` argument, is a runtime error
citing which channel (`r`, `g`, or `b`) was the problem. Every drawing
builtin below applies the same range check to whatever `int` it receives
as a color argument, whether it came from `rgb()` or a hand-written
literal -- a color argument outside `0..=0xFFFFFF` (or negative) is
always a runtime error, never a value that gets silently masked down to
24 bits.

On the native side, this packed `0xRRGGBB` value is unpacked into a
`gfx::Color` (`Color::from_rgb24`) and re-encoded into whatever raw byte
order the emulated/real graphics hardware actually wants (`Bgr` in every
environment this project has been run in, `Rgb` also supported --
`Console::encode` handles the difference). A WISP script never sees or
needs to know which one is in effect; `0xRRGGBB` always means the same
logical color regardless of the underlying hardware's native pixel
layout, exactly the same guarantee the pre-existing `gfx::Color` type
already gave the text console's own colors (`gfx::FOREGROUND`,
`gfx::ACCENT`, `gfx::ERROR`).

## The four drawing builtins

### `plot(x, y, color)`

Sets one pixel. If `(x, y)` falls outside the visible screen, this is a
silent no-op -- not an error. That's deliberate: a drawing position that
was computed (the midpoint of a moving object, say) landing briefly
off-screen is completely ordinary and not a sign of a script bug, unlike
some of the conditions below that *are* treated as errors.

### `rect(x, y, w, h, color)`

Fills an axis-aligned rectangle: top-left corner `(x, y)`, `w` pixels
wide, `h` pixels tall. Its *position* clips exactly like `plot()` --
a rectangle straddling an edge draws only its on-screen portion, and one
entirely off-screen draws nothing at all. `w == 0` or `h == 0` is a
legal, silent no-op (an empty rectangle covers no pixels).

A **negative** `w` or `h`, however, is a runtime error:

```
wisp: line N: rect() width and height must not be negative, found w=-5, h=20
```

This is the first place the "off-screen is fine, but this is a mistake"
line gets drawn, and it's worth understanding why the line falls where
it does: an off-screen *position* is a completely ordinary thing to want
(a HUD element animating in from the edge, an object that's briefly out
of view), but there is no sensible rectangle a *negative size* could mean
-- it's essentially always a swapped-argument or bad-arithmetic bug (e.g.
`rect(x2, y2, x1 - x2, y1 - y2, c)` where the caller forgot `x1` might be
larger than `x2`), and silently doing nothing with it would just hide
that bug instead of surfacing it. `esp/APPS/canvas.wisp`'s "rect" command
demonstrates hitting this error live and recovering cleanly back to a
working shell prompt.

### `line(x1, y1, x2, y2, color)`

Draws a straight, one-pixel-wide line between the two points, using
Bresenham's integer algorithm (there's no floating point anywhere in
WISP, and none was introduced into the kernel to implement this -- see
"Implementation notes" below). Each point along the line clips
independently, exactly like `plot()`, so a line that runs off one edge of
the screen and back on renders correctly instead of being rejected or
truncated wholesale.

Unlike every other drawing builtin here, `line()` rejects coordinates
that are merely *far* from the origin -- not just wildly invalid ones --
with a runtime error:

```
wisp: line N: line() coordinates must be within +/-200000 of the origin, found x2=5000000
```

This is the one place this feature's clipping story is asymmetric, and
it's for a real, mechanical reason rather than an arbitrary inconsistency:
`plot()` touches exactly one pixel no matter what `(x, y)` is, and
`rect()` clips its extent *before* it starts filling, so its cost is
always bounded by the screen's own size no matter how large `w`/`h` are.
`line()` has no equivalent shortcut -- Bresenham's cost is proportional
to `max(|x2-x1|, |y2-y1|)`, a property of the endpoints themselves,
before any clipping against the screen even happens. A coordinate a
million pixels from a screen that's at most a few thousand pixels across
is never a legitimate on-screen (or plausibly-just-off-screen) value --
it's essentially always a script bug, the same class of mistake
`rect()`'s negative-size check catches -- so `line()` catches it
immediately with a clear error instead of either silently doing nothing
or (if it clipped instead of rejecting) walking an enormous number of
pixels to find out the line never touched the screen at all.

### `fill_screen(color)`

Fills the entire visible framebuffer with `color`. See the next section
for how this interacts with the text console.

## Text and graphics share one framebuffer

There is exactly one framebuffer. The text console (`print()`, the shell
prompt, every line of output WISP programs have always produced) and
these five new drawing builtins draw into the *same* pixels -- there is
no separate graphics layer, no compositing, and no z-order. Whichever one
drew a given pixel most recently is what's on screen there, permanently,
until something draws over it again.

This has one very concrete consequence worth internalizing:
**`fill_screen()` erases previously-printed text just as completely as it
erases previously-drawn graphics.** This surfaced as a real bug while
writing `canvas.wisp` for this feature: an early draft printed its
startup banner (`"canvas.wisp -- screen is ..."`, the command list) and
*then* called `fill_screen()` to paint a background before entering its
main loop -- and the fill immediately painted over every pixel of the
banner that had just been printed, so it never appeared on screen at all
(the automated test that caught this saw only the empty `canvas>` prompt
where three lines of instructions should have been). The fix was to
reorder the two calls -- fill the background *first*, then print the
banner on top of it -- which is now what the shipped `canvas.wisp` does.
The general rule this bug illustrates: **draw your background before
printing text you want to stay visible, never after.**

`clear()` -- the plain-text builtin that already existed before this
feature, and the shell's own `clear` command -- is not the same
operation as `fill_screen()`, and the difference matters for exactly this
reason. `clear()` repaints the whole screen to the console's own
background color (always black; there's no builtin to change it) *and*
resets the text cursor to the top-left, which is what makes it safe to
call right before printing something you want to appear starting at a
predictable, known position. `fill_screen()` is a pure graphics
primitive: it never touches the cursor, so text printed afterward
continues exactly where the cursor already was -- which could be
anywhere, depending on what was printed (by this script or an earlier
command) before it ran. Both `bounce.wisp` and `canvas.wisp` call
`clear()`, not `fill_screen()`, right before their final closing message,
specifically to land it at a clean, predictable spot rather than
wherever the cursor happened to be left.

## Implementation notes

For whoever (human or LLM) next touches `src/gfx.rs`, `src/wisp/interp.rs`,
`src/wisp/builtins.rs`, or `src/shell.rs`'s graphics code:

- **Every coordinate reaching these primitives is untrusted, script-
  controlled input**, and this project's release profile
  (`Cargo.toml`'s `[profile.release]`) sets neither `debug-assertions`
  nor `overflow-checks`, so it gets the standard release defaults for
  both: `debug_assert!` compiles to nothing at all, and integer overflow
  silently wraps instead of panicking. The pre-existing `put_pixel`
  (used only by the text renderer, whose coordinates are always derived
  from its own character grid and can never actually be out of range)
  only `debug_assert!`s its bounds for exactly that reason -- it's a
  trusted-caller invariant check, not a defense. Every *new* method this
  feature added (`plot_pixel`, `draw_rect_filled`, `draw_line`,
  `fill_screen`) does a real, always-compiled `if` bounds check instead,
  and none of them uses plain `.abs()` on a script-supplied `i64`
  (`i64::MIN.abs()` cannot be represented and either panics or, under
  this release profile, silently wraps back to a *negative* result --
  `.unsigned_abs()` is correct for every possible `i64` input and is used
  throughout instead).
- **`draw_line` carries its own independent backstop** (`MAX_LINE_STEPS`,
  currently 500,000) below `builtins::builtin_line`'s much tighter
  rejection threshold (`MAX_LINE_COORD`, 200,000), so the Bresenham walk
  itself can never be forced to run for an unbounded number of steps even
  if some future caller reaches it without going through
  `builtin_line`'s validation first.
- **No floating point was introduced anywhere in the kernel to implement
  this.** `draw_line`'s Bresenham walk is integer-only, matching the
  language it serves (WISP itself has no float type -- see
  `docs/WISP_LANGUAGE.md`). This was a deliberate choice, not just a
  convenience: this project has never exercised FPU/SSE exception
  handling end to end (unlike every CPU exception `idt.rs` does handle,
  proven by `tests/fault_test.py`), so introducing floating-point
  arithmetic as a hidden implementation detail here -- even purely
  internally, never surfaced to a script -- would have been jumping ahead
  of infrastructure this project hasn't actually built or verified yet.
  If/when floating point lands as its own, separate v2 feature, it should
  bring that verification with it; this feature deliberately did not
  borrow against it.
- **Verification**: `tests/acceptance_test.py`'s steps 14 and 15 boot the
  real kernel under QEMU/OVMF, run `bounce.wisp` and `canvas.wisp`
  against the live interpreter and the live GOP framebuffer, and assert
  actual sampled pixel colors (not just OCR'd text) against what each
  drawing call was expected to produce -- including the `rect()`
  negative-size error path and confirming the shell prompt is still
  fully responsive afterward. See `assert_colors_present` and
  `assert_pixel_near` in that file.
