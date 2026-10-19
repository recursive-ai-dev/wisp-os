//! The fixed set of built-in functions described in `docs/WISP_LANGUAGE.md`.
//! These names are reserved: `interp::Interp::run` refuses to register a
//! user `fn` with one of these names before the program ever runs.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::interp::{Host, OUT_OF_MEMORY_MESSAGE, STOPPED_MESSAGE};
use super::value::{Value, MAX_DISPLAY_LEN, MAX_LIST_LEN};
use super::WispError;

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "input"
            | "len"
            | "str"
            | "int"
            | "rand"
            | "sleep_ms"
            | "time_ms"
            | "clear"
            | "push"
            | "pop"
            | "save"
            | "load"
            | "screen_width"
            | "screen_height"
            | "rgb"
            | "plot"
            | "rect"
            | "line"
            | "fill_screen"
    )
}

// Never inlined into the interpreter: its frame (every builtin's locals)
// would otherwise be paid at every level of WISP function recursion.
#[inline(never)]
pub fn call(
    host: &mut dyn Host,
    name: &str,
    args: Vec<Value>,
    line: u32,
) -> Result<Value, WispError> {
    match name {
        "print" => {
            builtin_print(host, &args);
            Ok(Value::Nil)
        }
        "input" => builtin_input(host, args, line),
        "len" => builtin_len(&args, line),
        "str" => builtin_str(&args, line),
        "int" => builtin_int(&args, line),
        "rand" => builtin_rand(host, &args, line),
        "sleep_ms" => builtin_sleep_ms(host, &args, line),
        "time_ms" => {
            expect_arity(&args, 0, "time_ms", line)?;
            // Milliseconds since boot comfortably fit an i64 for the next
            // ~292 million years; saturate rather than wrap regardless.
            Ok(Value::Int(i64::try_from(host.time_ms()).unwrap_or(i64::MAX)))
        }
        "clear" => {
            expect_arity(&args, 0, "clear", line)?;
            host.clear();
            Ok(Value::Nil)
        }
        "push" => builtin_push(&args, line),
        "pop" => builtin_pop(&args, line),
        "save" => builtin_save(host, &args, line),
        "load" => builtin_load(host, &args, line),
        "screen_width" => builtin_screen_width(host, &args, line),
        "screen_height" => builtin_screen_height(host, &args, line),
        "rgb" => builtin_rgb(&args, line),
        "plot" => builtin_plot(host, &args, line),
        "rect" => builtin_rect(host, &args, line),
        "line" => builtin_line(host, &args, line),
        "fill_screen" => builtin_fill_screen(host, &args, line),
        _ => unreachable!("is_builtin() must be checked before call()"),
    }
}

/// Coordinates further than this from the origin make `line()` a clear
/// WISP-level runtime error instead of a silent clip or an expensive draw.
/// `line()`'s cost is proportional to the distance between its endpoints
/// (`Console::draw_line` walks every pixel from one to the other), unlike
/// every other drawing builtin here, whose cost is bounded by the
/// screen's own size no matter what coordinates a script passes in
/// (`rect()` clips *before* it starts filling; `plot()` touches exactly
/// one pixel regardless of how far off-screen it is). A coordinate this
/// far from a screen that is at most a few thousand pixels across is
/// never a legitimate on-screen (or plausibly-just-off-screen) value --
/// it is essentially always a script bug (an uninitialized variable, a
/// sign error) -- so `builtin_line` catches it immediately with a clear
/// error rather than letting it through to either an invisible no-op or
/// an unboundedly expensive draw. `Console::draw_line` also enforces its
/// own, much larger backstop bound independently -- see its doc comment.
const MAX_LINE_COORD: i64 = 200_000;

fn expect_arity(args: &[Value], want: usize, name: &str, line: u32) -> Result<(), WispError> {
    if args.len() != want {
        return Err(WispError::new(
            line,
            format!(
                "{name}() expects {want} argument{}, got {}",
                if want == 1 { "" } else { "s" },
                args.len()
            ),
        ));
    }
    Ok(())
}

/// Shared "this argument must be an int" check for the graphics
/// builtins' coordinate/size arguments, parameterized the same way
/// `expect_arity` is above: `fn_name` and `arg_desc` are spliced into the
/// message so every call site still reads as if it were hand-written for
/// that one argument, e.g. `plot() expects an int for x, found string`.
fn expect_int(v: &Value, fn_name: &str, arg_desc: &str, line: u32) -> Result<i64, WispError> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(WispError::new(
            line,
            format!(
                "{fn_name}() expects an int for {arg_desc}, found {}",
                other.type_name()
            ),
        )),
    }
}

/// Shared color-argument validation for every drawing builtin (`plot`,
/// `rect`, `line`, `fill_screen`): `v` must be an `int` in `0..=0xFFFFFF`
/// -- the packed `0xRRGGBB` format `rgb()` produces, see
/// `docs/WISP_GRAPHICS.md`. `fn_name` is spliced into the message the
/// same way `expect_int` above does it.
fn expect_color(v: &Value, fn_name: &str, line: u32) -> Result<u32, WispError> {
    match v {
        Value::Int(i) if (0..=0x00FF_FFFF).contains(i) => Ok(*i as u32),
        Value::Int(i) => Err(WispError::new(
            line,
            format!("{fn_name}() expects a color in 0..=16777215 (see rgb()), found {i}"),
        )),
        other => Err(WispError::new(
            line,
            format!(
                "{fn_name}() expects a color (int), found {}",
                other.type_name()
            ),
        )),
    }
}

fn builtin_print(host: &mut dyn Host, args: &[Value]) {
    // Each argument is rendered (bounded -- see `Value::to_display_string`)
    // and handed to the host on its own, rather than first concatenated
    // into one more full-size copy of all of them.
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            host.print(" ");
        }
        match a {
            Value::Str(s) => host.print(s),
            other => host.print(&other.to_display_string()),
        }
    }
    host.print("\n");
}

fn builtin_input(host: &mut dyn Host, args: Vec<Value>, line: u32) -> Result<Value, WispError> {
    if args.len() > 1 {
        return Err(WispError::new(
            line,
            format!("input() expects 0 or 1 argument(s), got {}", args.len()),
        ));
    }
    let prompt = match args.first() {
        None => String::new(),
        Some(Value::Str(s)) => (**s).clone(),
        Some(other) => {
            return Err(WispError::new(
                line,
                format!("input() expects a string prompt, found {}", other.type_name()),
            ))
        }
    };
    let text = host.input(&prompt);
    // Esc ends `input()` early (the host's line reader returns on it); the
    // partial line is discarded and the program stops right here, rather
    // than continuing with input the user never confirmed.
    if host.stop_requested() {
        return Err(WispError::new(line, STOPPED_MESSAGE));
    }
    Ok(Value::Str(Rc::new(text)))
}

fn builtin_len(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "len", line)?;
    match &args[0] {
        Value::Str(s) => Ok(Value::Int(s.len() as i64)),
        Value::List(l) => Ok(Value::Int(l.borrow().len() as i64)),
        other => Err(WispError::new(
            line,
            format!("len() expects a string or list, found {}", other.type_name()),
        )),
    }
}

fn builtin_push(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "push", line)?;
    match &args[0] {
        Value::List(l) => {
            let mut items = l.borrow_mut();
            if items.len() >= MAX_LIST_LEN {
                return Err(WispError::new(
                    line,
                    format!("push(): list is full ({MAX_LIST_LEN} elements is the limit)"),
                ));
            }
            items
                .try_reserve(1)
                .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
            items.push(args[1].clone());
            Ok(Value::Nil)
        }
        other => Err(WispError::new(
            line,
            format!(
                "push() expects a list as its first argument, found {}",
                other.type_name()
            ),
        )),
    }
}

fn builtin_pop(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "pop", line)?;
    match &args[0] {
        Value::List(l) => l
            .borrow_mut()
            .pop()
            .ok_or_else(|| WispError::new(line, "pop() called on an empty list")),
        other => Err(WispError::new(
            line,
            format!("pop() expects a list, found {}", other.type_name()),
        )),
    }
}

fn builtin_str(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "str", line)?;
    match &args[0] {
        // Already a string: share the buffer instead of reallocating.
        Value::Str(s) => Ok(Value::Str(s.clone())),
        other => other.try_display_string().map(|s| Value::Str(Rc::new(s))).ok_or_else(|| {
            WispError::new(
                line,
                format!("str(): the result would be longer than {MAX_DISPLAY_LEN} bytes"),
            )
        }),
    }
}

fn builtin_int(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "int", line)?;
    match &args[0] {
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::Str(s) => s
            .trim()
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| WispError::new(line, format!("int() could not parse \"{s}\" as an integer"))),
        other => Err(WispError::new(
            line,
            format!("int() cannot convert {} to int", other.type_name()),
        )),
    }
}

fn builtin_rand(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "rand", line)?;
    let lo = match &args[0] {
        Value::Int(i) => *i,
        other => {
            return Err(WispError::new(
                line,
                format!("rand() expects two ints, found {} as the first argument", other.type_name()),
            ))
        }
    };
    let hi = match &args[1] {
        Value::Int(i) => *i,
        other => {
            return Err(WispError::new(
                line,
                format!("rand() expects two ints, found {} as the second argument", other.type_name()),
            ))
        }
    };
    if lo > hi {
        return Err(WispError::new(
            line,
            format!("rand({lo}, {hi}): lower bound is greater than the upper bound"),
        ));
    }
    // i128/u128 math sidesteps any overflow when the caller passes an
    // extreme range (e.g. close to the full i64 span) -- correctness
    // here matters more than the handful of extra cycles for a builtin
    // that's typically called with tiny dice-roll-sized ranges.
    let span = (hi as i128 - lo as i128 + 1) as u128;
    let offset = (host.rand_u64() as u128 % span) as i64;
    Ok(Value::Int(lo + offset))
}

fn builtin_sleep_ms(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "sleep_ms", line)?;
    match &args[0] {
        Value::Int(ms) if *ms >= 0 => {
            host.sleep_ms(*ms as u64);
            // The host's sleep wakes early on Esc; report it here, at the
            // `sleep_ms()` call, rather than at whatever loop runs next.
            if host.stop_requested() {
                return Err(WispError::new(line, STOPPED_MESSAGE));
            }
            Ok(Value::Nil)
        }
        Value::Int(_) => Err(WispError::new(line, "sleep_ms() expects a non-negative int")),
        other => Err(WispError::new(
            line,
            format!("sleep_ms() expects an int, found {}", other.type_name()),
        )),
    }
}

/// `save(key, value)`: persist `value` under `key` on the AHCI save disk,
/// surviving a reboot (Phase 2, Part B). Always returns `nil` on success;
/// the interesting failure modes (no save disk, key/value too large, out
/// of slots) all surface as a normal WISP runtime error via `Err`, the
/// same way any other builtin reports a problem.
fn builtin_save(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "save", line)?;
    let key = match &args[0] {
        Value::Str(s) => s,
        other => {
            return Err(WispError::new(
                line,
                format!("save() expects a string key, found {}", other.type_name()),
            ))
        }
    };
    host.save(key, &args[1])
        .map(|()| Value::Nil)
        .map_err(|msg| WispError::new(line, format!("save(): {msg}")))
}

/// `load(key)`: look up the value previously saved under `key`. Returns
/// `nil` if `key` was never saved -- WISP has no separate "undefined"
/// value distinct from `nil` (see `docs/WISP_LANGUAGE.md`), so this
/// mirrors that: a script checks for "no saved value" with
/// `if load("x") == nil { ... }`, the same pattern any other "might not
/// have a value" case in this language already uses. A key saved with the
/// value `nil` itself is indistinguishable from "never saved" -- a
/// documented, deliberate simplification, not an oversight. Real on-disk
/// corruption (a failed checksum, malformed stored data) is a genuine
/// `Err`, never silently reported as `nil`.
fn builtin_load(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "load", line)?;
    let key = match &args[0] {
        Value::Str(s) => s,
        other => {
            return Err(WispError::new(
                line,
                format!("load() expects a string key, found {}", other.type_name()),
            ))
        }
    };
    match host.load(key) {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Ok(Value::Nil),
        Err(msg) => Err(WispError::new(line, format!("load(): {msg}"))),
    }
}

// ---- WISP v2: pixel/graphics primitives --------------------------------
//
// See `docs/WISP_GRAPHICS.md` for the full language-level contract (the
// pixel model, the `0xRRGGBB` color convention, and each builtin's exact
// clipping/error semantics) and `gfx::Console`'s doc comments for the
// bounds-safety reasoning on the host side these all delegate to.

fn builtin_screen_width(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 0, "screen_width", line)?;
    Ok(Value::Int(host.screen_width() as i64))
}

fn builtin_screen_height(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 0, "screen_height", line)?;
    Ok(Value::Int(host.screen_height() as i64))
}

/// `rgb(r, g, b)`: pack three `0..=255` color channels into the single
/// `0xRRGGBB` integer every drawing builtin (`plot`, `rect`, `line`,
/// `fill_screen`) accepts as a color -- see `docs/WISP_GRAPHICS.md`. WISP
/// has no dedicated color type, and no bitwise operators to build one by
/// hand (see `docs/WISP_LANGUAGE.md`'s operator table), so an ordinary
/// `int` doing double duty as a packed color -- `rgb(255, 0, 0)` and the
/// equivalent literal `16711680` both read unambiguously as pure red -- is
/// the natural fit within the existing five-value-type language, rather
/// than growing a sixth `Value` variant for this one feature.
fn builtin_rgb(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 3, "rgb", line)?;
    let channel = |v: &Value, which: &str| -> Result<i64, WispError> {
        match v {
            Value::Int(i) if (0..=255).contains(i) => Ok(*i),
            Value::Int(i) => Err(WispError::new(
                line,
                format!("rgb() expects {which} in 0..=255, found {i}"),
            )),
            other => Err(WispError::new(
                line,
                format!("rgb() expects three ints, found {} as {which}", other.type_name()),
            )),
        }
    };
    let r = channel(&args[0], "r")?;
    let g = channel(&args[1], "g")?;
    let b = channel(&args[2], "b")?;
    Ok(Value::Int((r << 16) | (g << 8) | b))
}

/// `plot(x, y, color)`: set one pixel. `(x, y)` outside the visible
/// framebuffer is silently ignored rather than an error -- deliberately
/// forgiving, since a drawing position that's computed (e.g.
/// mid-animation) landing briefly off-screen is normal and not a script
/// bug the way, say, a negative `rect()` size would be. See
/// `docs/WISP_GRAPHICS.md`.
fn builtin_plot(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 3, "plot", line)?;
    let x = expect_int(&args[0], "plot", "x", line)?;
    let y = expect_int(&args[1], "plot", "y", line)?;
    let color = expect_color(&args[2], "plot", line)?;
    host.plot(x, y, color);
    Ok(Value::Nil)
}

/// `rect(x, y, w, h, color)`: fill an axis-aligned rectangle, top-left
/// corner `(x, y)`, `w` wide and `h` tall. A negative `w` or `h` is a
/// runtime error -- almost certainly a swapped-argument or bad-arithmetic
/// bug, since there's no sensible drawing it could mean -- unlike an
/// off-screen *position*, which just clips silently: a rectangle sliding
/// in from off-screen (a HUD element animating into view, say) is a
/// completely ordinary thing to draw. See `docs/WISP_GRAPHICS.md` for the
/// full clipping contract.
fn builtin_rect(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 5, "rect", line)?;
    let x = expect_int(&args[0], "rect", "x", line)?;
    let y = expect_int(&args[1], "rect", "y", line)?;
    let w = expect_int(&args[2], "rect", "w", line)?;
    let h = expect_int(&args[3], "rect", "h", line)?;
    if w < 0 || h < 0 {
        return Err(WispError::new(
            line,
            format!("rect() width and height must not be negative, found w={w}, h={h}"),
        ));
    }
    let color = expect_color(&args[4], "rect", line)?;
    host.draw_rect(x, y, w as usize, h as usize, color);
    Ok(Value::Nil)
}

/// `line(x1, y1, x2, y2, color)`: draw a straight line between the two
/// points (Bresenham's algorithm; see `Console::draw_line`). Each point
/// along it is clipped independently, so a line that crosses off one edge
/// of the screen and back on renders correctly. See `MAX_LINE_COORD`'s
/// doc comment above for why extreme coordinates are rejected outright
/// here rather than silently clipped like every other drawing builtin's.
fn builtin_line(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 5, "line", line)?;
    let x1 = expect_int(&args[0], "line", "x1", line)?;
    let y1 = expect_int(&args[1], "line", "y1", line)?;
    let x2 = expect_int(&args[2], "line", "x2", line)?;
    let y2 = expect_int(&args[3], "line", "y2", line)?;
    // `.unsigned_abs()`, not `.abs()`: see `Console::draw_line`'s doc
    // comment for why `.abs()` is unsafe to use on a script-controlled
    // `i64` here (it cannot represent `-i64::MIN` and silently produces a
    // *negative* result for that one input under this project's release
    // profile, which would let `i64::MIN` slip past this exact check).
    for (value, name) in [(x1, "x1"), (y1, "y1"), (x2, "x2"), (y2, "y2")] {
        if value.unsigned_abs() > MAX_LINE_COORD as u64 {
            return Err(WispError::new(
                line,
                format!(
                    "line() coordinates must be within +/-{MAX_LINE_COORD} of the origin, \
                     found {name}={value}"
                ),
            ));
        }
    }
    let color = expect_color(&args[4], "line", line)?;
    host.draw_line(x1, y1, x2, y2, color);
    Ok(Value::Nil)
}

/// `fill_screen(color)`: fill the entire visible framebuffer with
/// `color`. Does not move the text cursor or otherwise touch console
/// state -- text and graphics share one framebuffer (there is only the
/// one), so this is a peer to `print()`/`clear()`, not a replacement for
/// either. See `docs/WISP_GRAPHICS.md`'s note on how the two layers
/// interact.
fn builtin_fill_screen(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "fill_screen", line)?;
    let color = expect_color(&args[0], "fill_screen", line)?;
    host.fill_screen(color);
    Ok(Value::Nil)
}
