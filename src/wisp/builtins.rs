//! The fixed set of built-in functions described in `docs/WISP_LANGUAGE.md`.
//! These names are reserved: `interp::Interp::run` refuses to register a
//! user `fn` with one of these names before the program ever runs.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::interp::{Host, OUT_OF_MEMORY_MESSAGE, STOPPED_MESSAGE};
use super::value::{self, List, Map, MapKey, ObjectRegistry, Value, MAX_DISPLAY_LEN, MAX_LIST_LEN, MAX_STR_LEN};
use super::WispError;

/// The built-ins that call back into the interpreter, so they live in
/// `interp.rs` (`Interp::call_higher_order`) rather than in [`call`].
pub fn is_higher_order(name: &str) -> bool {
    matches!(name, "map" | "filter" | "reduce" | "sort_by")
}

pub fn is_builtin(name: &str) -> bool {
    is_higher_order(name)
        || matches!(
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
            | "split"
            | "join"
            | "find"
            | "contains"
            | "replace"
            | "upper"
            | "lower"
            | "trim"
            | "starts_with"
            | "ends_with"
            | "chr"
            | "ord"
            | "chars"
            | "repeat"
            | "insert"
            | "remove"
            | "sort"
            | "reverse"
            | "range"
            | "abs"
            | "min"
            | "max"
            | "type"
            | "keys"
            | "values"
            | "get"
    )
}

// Never inlined into the interpreter: its frame (every builtin's locals)
// would otherwise be paid at every level of WISP function recursion.
//
// `objects` is the running session's registry: every list or map a builtin
// creates must be made through it, so that a cycle a script later builds
// out of it is still reclaimed when the session ends (see `ObjectRegistry`).
#[inline(never)]
pub fn call(
    host: &mut dyn Host,
    objects: &mut ObjectRegistry,
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
        "split" => builtin_split(host, objects, &args, line),
        "join" => builtin_join(&args, line),
        "find" => builtin_find(host, &args, line),
        "contains" => builtin_contains(host, &args, line),
        "replace" => builtin_replace(&args, line),
        "upper" => builtin_case(&args, line, "upper", str::make_ascii_uppercase),
        "lower" => builtin_case(&args, line, "lower", str::make_ascii_lowercase),
        "trim" => builtin_trim(&args, line),
        "starts_with" => builtin_affix(&args, line, "starts_with", |text, part| text.starts_with(part)),
        "ends_with" => builtin_affix(&args, line, "ends_with", |text, part| text.ends_with(part)),
        "chr" => builtin_chr(&args, line),
        "ord" => builtin_ord(&args, line),
        "chars" => builtin_chars(host, objects, &args, line),
        "repeat" => builtin_repeat(&args, line),
        "insert" => builtin_insert(&args, line),
        "remove" => builtin_remove(&args, line),
        "sort" => builtin_sort(&args, line),
        "reverse" => builtin_reverse(&args, line),
        "range" => builtin_range(objects, &args, line),
        "abs" => builtin_abs(&args, line),
        "min" => builtin_min_max(&args, line, "min"),
        "max" => builtin_min_max(&args, line, "max"),
        "type" => {
            expect_arity(&args, 1, "type", line)?;
            Ok(Value::Str(Rc::new(String::from(args[0].type_name()))))
        }
        "keys" => builtin_keys_values(objects, &args, line, "keys"),
        "values" => builtin_keys_values(objects, &args, line, "values"),
        "get" => builtin_get(&args, line),
        _ => unreachable!("is_builtin() must be checked, and higher-order built-ins routed to the interpreter, before call()"),
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
        Value::Map(m) => Ok(Value::Int(m.borrow().len() as i64)),
        other => Err(WispError::new(
            line,
            format!("len() expects a string, list or map, found {}", other.type_name()),
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
/// surviving a reboot (Phase 2, Part B); `save(key, nil)` deletes the
/// key. Always returns `nil` on success; the interesting failure modes (no
/// save disk, key/value too large, store full) all surface as a normal
/// WISP runtime error via `Err`, the same way any other builtin reports a
/// problem.
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
/// the natural fit within the existing value types, rather than growing
/// another `Value` variant for this one feature.
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

// ---- strings ---------------------------------------------------------------
//
// String offsets are byte offsets, the same unit `len()` counts in: they
// index the exact UTF-8 text a script holds, whatever it contains. Every
// builtin that builds a string refuses results over `MAX_STR_LEN` bytes
// and allocates them fallibly, so a script that asks for too much gets a
// WISP error, never a kernel halt -- the same contract as `+`.

/// How many items `split()`/`chars()` build between checks of the host's
/// heap reserve. Each item is a small string; checking every so often
/// bounds what one call can allocate past the reserve to a few KiB, the
/// same guarantee the interpreter's own per-iteration check gives a loop.
const MEMORY_CHECK_INTERVAL: usize = 256;

fn expect_str<'a>(v: &'a Value, fn_name: &str, arg_desc: &str, line: u32) -> Result<&'a Rc<String>, WispError> {
    match v {
        Value::Str(s) => Ok(s),
        other => Err(WispError::new(
            line,
            format!("{fn_name}() expects a string for {arg_desc}, found {}", other.type_name()),
        )),
    }
}

fn expect_list<'a>(v: &'a Value, fn_name: &str, line: u32) -> Result<&'a List, WispError> {
    match v {
        Value::List(l) => Ok(l),
        other => Err(WispError::new(
            line,
            format!("{fn_name}() expects a list as its first argument, found {}", other.type_name()),
        )),
    }
}

/// A new, empty string with room for `len` bytes -- or the "too long" /
/// "out of memory" error a script should see instead.
fn string_with_capacity(len: usize, fn_name: &str, line: u32) -> Result<String, WispError> {
    if len > MAX_STR_LEN {
        return Err(WispError::new(
            line,
            format!("{fn_name}(): the result would be {len} bytes; the limit is {MAX_STR_LEN}"),
        ));
    }
    let mut out = String::new();
    out.try_reserve_exact(len)
        .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
    Ok(out)
}

fn new_str(text: String) -> Value {
    Value::Str(Rc::new(text))
}

/// Build a list of strings from `pieces` for `split()`/`chars()`: at most
/// `MAX_LIST_LEN` of them, with the heap reserve checked as it grows.
fn string_list<'a>(
    host: &mut dyn Host,
    objects: &mut ObjectRegistry,
    pieces: impl Iterator<Item = &'a str>,
    fn_name: &str,
    line: u32,
) -> Result<Value, WispError> {
    let mut items: Vec<Value> = Vec::new();
    for piece in pieces {
        if items.len() >= MAX_LIST_LEN {
            return Err(WispError::new(
                line,
                format!("{fn_name}(): the result would have more than {MAX_LIST_LEN} elements"),
            ));
        }
        if items.len() % MEMORY_CHECK_INTERVAL == 0 && host.memory_low() {
            return Err(WispError::new(line, OUT_OF_MEMORY_MESSAGE));
        }
        items
            .try_reserve(1)
            .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
        let mut text = String::new();
        text.try_reserve_exact(piece.len())
            .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
        text.push_str(piece);
        items.push(new_str(text));
    }
    Ok(objects.new_list(items))
}

/// `split(text)` splits on runs of whitespace, dropping empty pieces --
/// the natural reading of a typed command line. `split(text, sep)` splits
/// on every occurrence of `sep`, keeping the empty pieces between adjacent
/// separators, so `join(split(t, sep), sep)` always gives `t` back.
fn builtin_split(host: &mut dyn Host, objects: &mut ObjectRegistry, args: &[Value], line: u32) -> Result<Value, WispError> {
    if args.is_empty() || args.len() > 2 {
        return Err(WispError::new(line, format!("split() expects 1 or 2 arguments, got {}", args.len())));
    }
    let text = expect_str(&args[0], "split", "the text", line)?;
    match args.get(1) {
        None => string_list(host, objects, text.split_whitespace(), "split", line),
        Some(sep) => {
            let sep = expect_str(sep, "split", "the separator", line)?;
            if sep.is_empty() {
                return Err(WispError::new(line, "split(): the separator must not be empty"));
            }
            string_list(host, objects, text.split(sep.as_str()), "split", line)
        }
    }
}

fn builtin_join(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "join", line)?;
    let list = expect_list(&args[0], "join", line)?;
    let sep = expect_str(&args[1], "join", "the separator", line)?;
    let items = list.borrow();
    let mut total = sep.len().saturating_mul(items.len().saturating_sub(1));
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(s) => total = total.saturating_add(s.len()),
            other => {
                return Err(WispError::new(
                    line,
                    format!("join() expects a list of strings, but element {i} is {} (use str() first)", other.type_name()),
                ))
            }
        }
    }
    let mut out = string_with_capacity(total, "join", line)?;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        if let Value::Str(s) = item {
            out.push_str(s);
        }
    }
    Ok(new_str(out))
}

/// `find(text, part)`: the byte offset of the first occurrence of `part`
/// in `text`; `find(list, value)`: the index of the first element equal
/// to `value` (`==`'s structural equality). `-1` when there is none.
fn builtin_find(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "find", line)?;
    position(host, &args[0], &args[1], "find", line).map(|p| Value::Int(p.map_or(-1, |p| p as i64)))
}

/// `contains(text, part)` / `contains(list, value)`: whether `find` would
/// find it. `contains(map, key)`: whether `map` has an entry for `key`.
fn builtin_contains(host: &mut dyn Host, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "contains", line)?;
    if let Value::Map(map) = &args[0] {
        let key = expect_key(&args[1], "contains", line)?;
        return Ok(Value::Bool(map.borrow().contains_key(&key)));
    }
    position(host, &args[0], &args[1], "contains", line).map(|p| Value::Bool(p.is_some()))
}

fn position(host: &mut dyn Host, haystack: &Value, needle: &Value, fn_name: &str, line: u32) -> Result<Option<usize>, WispError> {
    match haystack {
        Value::Str(text) => {
            let part = expect_str(needle, fn_name, "the text to look for", line)?;
            Ok(text.find(part.as_str()))
        }
        Value::List(list) => {
            for (i, item) in list.borrow().iter().enumerate() {
                // Each comparison may take up to `MAX_EQ_WORK` steps, and a
                // list holds up to `MAX_LIST_LEN` elements: far too long to
                // go without honouring Esc, so check it between elements as
                // a loop would between iterations.
                if host.stop_requested() {
                    return Err(WispError::new(line, STOPPED_MESSAGE));
                }
                if value::try_equal(item, needle).map_err(|msg| WispError::new(line, msg))? {
                    return Ok(Some(i));
                }
            }
            Ok(None)
        }
        other => Err(WispError::new(
            line,
            format!("{fn_name}() expects a string or a list to search, found {}", other.type_name()),
        )),
    }
}

fn builtin_replace(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 3, "replace", line)?;
    let text = expect_str(&args[0], "replace", "the text", line)?;
    let from = expect_str(&args[1], "replace", "the text to replace", line)?;
    let to = expect_str(&args[2], "replace", "the replacement", line)?;
    if from.is_empty() {
        return Err(WispError::new(line, "replace(): the text to replace must not be empty"));
    }
    let count = text.matches(from.as_str()).count();
    let total = (text.len() - count * from.len()).saturating_add(count.saturating_mul(to.len()));
    let mut out = string_with_capacity(total, "replace", line)?;
    let mut last = 0;
    for (at, _) in text.match_indices(from.as_str()) {
        out.push_str(&text[last..at]);
        out.push_str(to);
        last = at + from.len();
    }
    out.push_str(&text[last..]);
    Ok(new_str(out))
}

/// `upper(text)` / `lower(text)`: ASCII letters only -- the console font is
/// ASCII, and every other character comes back unchanged, byte for byte.
fn builtin_case(args: &[Value], line: u32, fn_name: &str, convert: fn(&mut str)) -> Result<Value, WispError> {
    expect_arity(args, 1, fn_name, line)?;
    let text = expect_str(&args[0], fn_name, "the text", line)?;
    let mut out = string_with_capacity(text.len(), fn_name, line)?;
    out.push_str(text);
    convert(&mut out);
    Ok(new_str(out))
}

fn builtin_trim(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "trim", line)?;
    let text = expect_str(&args[0], "trim", "the text", line)?;
    let trimmed = text.trim();
    if trimmed.len() == text.len() {
        return Ok(Value::Str(text.clone()));
    }
    let mut out = string_with_capacity(trimmed.len(), "trim", line)?;
    out.push_str(trimmed);
    Ok(new_str(out))
}

fn builtin_affix(args: &[Value], line: u32, fn_name: &str, test: fn(&str, &str) -> bool) -> Result<Value, WispError> {
    expect_arity(args, 2, fn_name, line)?;
    let text = expect_str(&args[0], fn_name, "the text", line)?;
    let part = expect_str(&args[1], fn_name, "the text to look for", line)?;
    Ok(Value::Bool(test(text, part)))
}

fn builtin_chr(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "chr", line)?;
    let code = expect_int(&args[0], "chr", "the character code", line)?;
    let ch = u32::try_from(code).ok().and_then(char::from_u32).ok_or_else(|| {
        WispError::new(
            line,
            format!("chr() expects a Unicode code point (0 to 1114111, excluding 55296 to 57343), found {code}"),
        )
    })?;
    Ok(new_str(ch.to_string()))
}

fn builtin_ord(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "ord", line)?;
    let text = expect_str(&args[0], "ord", "the character", line)?;
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(Value::Int(i64::from(u32::from(ch)))),
        _ => Err(WispError::new(
            line,
            format!("ord() expects a string of exactly one character, found {} bytes", text.len()),
        )),
    }
}

/// `chars(text)`: a list of `text`'s characters, each a one-character
/// string (one to four bytes).
fn builtin_chars(host: &mut dyn Host, objects: &mut ObjectRegistry, args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "chars", line)?;
    let text = expect_str(&args[0], "chars", "the text", line)?;
    let pieces = text.char_indices().map(|(i, ch)| &text[i..i + ch.len_utf8()]);
    string_list(host, objects, pieces, "chars", line)
}

fn builtin_repeat(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "repeat", line)?;
    let text = expect_str(&args[0], "repeat", "the text", line)?;
    let times = expect_int(&args[1], "repeat", "the count", line)?;
    if times < 0 {
        return Err(WispError::new(line, format!("repeat() expects a count of 0 or more, found {times}")));
    }
    // The size check below bounds the work only for a non-empty string:
    // `repeat("", n)` is empty for every `n`, and must not loop `n` times.
    if text.is_empty() {
        return Ok(Value::Str(text.clone()));
    }
    let total = usize::try_from(times).unwrap_or(usize::MAX).saturating_mul(text.len());
    let mut out = string_with_capacity(total, "repeat", line)?;
    for _ in 0..times {
        out.push_str(text);
    }
    Ok(new_str(out))
}

// ---- lists -----------------------------------------------------------------

/// A position argument for `insert`/`remove`: an `int` in `0..=max`.
fn expect_position(v: &Value, fn_name: &str, max: usize, len: usize, line: u32) -> Result<usize, WispError> {
    let i = expect_int(v, fn_name, "the index", line)?;
    if i < 0 || i as u64 > max as u64 {
        return Err(WispError::new(line, format!("{fn_name}(): index {i} out of bounds (length {len})")));
    }
    Ok(i as usize)
}

/// `insert(list, index, value)`: `value` becomes element `index`, and the
/// elements from there on move up one; `index` may be `len(list)`, which
/// appends.
fn builtin_insert(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 3, "insert", line)?;
    let list = expect_list(&args[0], "insert", line)?;
    let mut items = list.borrow_mut();
    let at = expect_position(&args[1], "insert", items.len(), items.len(), line)?;
    if items.len() >= MAX_LIST_LEN {
        return Err(WispError::new(
            line,
            format!("insert(): list is full ({MAX_LIST_LEN} elements is the limit)"),
        ));
    }
    items
        .try_reserve(1)
        .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
    items.insert(at, args[2].clone());
    Ok(Value::Nil)
}

/// `remove(list, index)`: takes element `index` out of the list and
/// returns it; the elements after it move down one. `remove(map, key)`:
/// takes `key`'s entry out of the map and returns its value.
fn builtin_remove(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 2, "remove", line)?;
    if let Value::Map(map) = &args[0] {
        let key = expect_key(&args[1], "remove", line)?;
        let removed = map.borrow_mut().remove(&key);
        return removed.ok_or_else(|| {
            WispError::new(line, format!("remove(): key {} is not in the map", key.describe()))
        });
    }
    let list = expect_list(&args[0], "remove", line)?;
    let mut items = list.borrow_mut();
    if items.is_empty() {
        return Err(WispError::new(line, "remove() called on an empty list"));
    }
    let at = expect_position(&args[1], "remove", items.len() - 1, items.len(), line)?;
    Ok(items.remove(at))
}

/// `sort(list)`: in place, ascending -- ints numerically, strings by byte
/// (the same order `<` uses). Every element must be an int, or every
/// element a string: there is no order between the two.
fn builtin_sort(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "sort", line)?;
    let list = expect_list(&args[0], "sort", line)?;
    let mut items = list.borrow_mut();
    if let Some(first) = items.first() {
        if !matches!(first, Value::Int(_) | Value::Str(_)) {
            return Err(WispError::new(
                line,
                format!("sort() needs a list of all ints or all strings, but element 0 is {}", first.type_name()),
            ));
        }
        for (i, item) in items.iter().enumerate().skip(1) {
            if !matches!((item, first), (Value::Int(_), Value::Int(_)) | (Value::Str(_), Value::Str(_))) {
                return Err(WispError::new(
                    line,
                    format!(
                        "sort() needs a list of all ints or all strings, but element 0 is {} and element {i} is {}",
                        first.type_name(),
                        item.type_name()
                    ),
                ));
            }
        }
    }
    // Unstable is indistinguishable here (equal ints or equal strings are
    // equal values), and it sorts in place without allocating.
    items.sort_unstable_by(|a, b| match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.as_str().cmp(y.as_str()),
        _ => core::cmp::Ordering::Equal,
    });
    Ok(Value::Nil)
}

fn builtin_reverse(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "reverse", line)?;
    expect_list(&args[0], "reverse", line)?.borrow_mut().reverse();
    Ok(Value::Nil)
}

/// `range(n)`: `[0, 1, ..., n - 1]`; `range(a, b)`: `[a, ..., b - 1]`.
/// Empty when the end is not past the start.
fn builtin_range(objects: &mut ObjectRegistry, args: &[Value], line: u32) -> Result<Value, WispError> {
    let (start, end) = match args {
        [n] => (0, expect_int(n, "range", "the count", line)?),
        [a, b] => (expect_int(a, "range", "the start", line)?, expect_int(b, "range", "the end", line)?),
        _ => return Err(WispError::new(line, format!("range() expects 1 or 2 arguments, got {}", args.len()))),
    };
    let span = (i128::from(end) - i128::from(start)).max(0);
    if span > MAX_LIST_LEN as i128 {
        return Err(WispError::new(
            line,
            format!("range() would make {span} elements; the limit is {MAX_LIST_LEN}"),
        ));
    }
    let mut items = Vec::new();
    items
        .try_reserve_exact(span as usize)
        .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
    items.extend((0..span as i64).map(|k| Value::Int(start + k)));
    Ok(objects.new_list(items))
}

// ---- maps ------------------------------------------------------------------
//
// `len`, `contains` and `remove` also take a map (above); these three are
// for maps only. A map keeps its entries in key order -- ints ascending,
// then strings in the order `<` uses -- so every list built here is in
// that order, the same one `for` and `print` use.

fn expect_map<'a>(v: &'a Value, fn_name: &str, line: u32) -> Result<&'a Map, WispError> {
    match v {
        Value::Map(m) => Ok(m),
        other => Err(WispError::new(
            line,
            format!("{fn_name}() expects a map as its first argument, found {}", other.type_name()),
        )),
    }
}

fn expect_key(v: &Value, fn_name: &str, line: u32) -> Result<MapKey, WispError> {
    MapKey::from_value(v).ok_or_else(|| {
        WispError::new(
            line,
            format!("{fn_name}() expects an int or string key, found {}", v.type_name()),
        )
    })
}

/// `keys(map)` / `values(map)`: a new list of the map's keys, or of its
/// values, in key order.
fn builtin_keys_values(objects: &mut ObjectRegistry, args: &[Value], line: u32, fn_name: &str) -> Result<Value, WispError> {
    expect_arity(args, 1, fn_name, line)?;
    let map = expect_map(&args[0], fn_name, line)?.borrow();
    let mut items = Vec::new();
    items
        .try_reserve_exact(map.len())
        .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
    if fn_name == "keys" {
        items.extend(map.keys().map(MapKey::to_value));
    } else {
        items.extend(map.values().cloned());
    }
    drop(map);
    Ok(objects.new_list(items))
}

/// `get(map, key)`: the value under `key`, or `nil` if there is none;
/// `get(map, key, default)`: `default` instead of `nil`. The way to read a
/// key that may be missing, where `map[key]` would be an error.
fn builtin_get(args: &[Value], line: u32) -> Result<Value, WispError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(WispError::new(line, format!("get() expects 2 or 3 arguments, got {}", args.len())));
    }
    let map = expect_map(&args[0], "get", line)?;
    let key = expect_key(&args[1], "get", line)?;
    let found = map.borrow().get(&key).cloned();
    Ok(found.unwrap_or_else(|| args.get(2).cloned().unwrap_or(Value::Nil)))
}

// ---- numbers ---------------------------------------------------------------

fn builtin_abs(args: &[Value], line: u32) -> Result<Value, WispError> {
    expect_arity(args, 1, "abs", line)?;
    let n = expect_int(&args[0], "abs", "the number", line)?;
    n.checked_abs()
        .map(Value::Int)
        .ok_or_else(|| WispError::new(line, format!("integer overflow in 'abs({n})'")))
}

/// `min(a, b)` / `max(a, b)`: two ints, or two strings (in the order `<`
/// uses). On a tie, the first argument.
fn builtin_min_max(args: &[Value], line: u32, fn_name: &str) -> Result<Value, WispError> {
    expect_arity(args, 2, fn_name, line)?;
    let ordering = match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.as_str().cmp(b.as_str()),
        (a, b) => {
            return Err(WispError::new(
                line,
                format!("{fn_name}() expects two ints or two strings, found {} and {}", a.type_name(), b.type_name()),
            ))
        }
    };
    let pick_second = if fn_name == "min" { ordering.is_gt() } else { ordering.is_lt() };
    Ok(args[usize::from(pick_second)].clone())
}
