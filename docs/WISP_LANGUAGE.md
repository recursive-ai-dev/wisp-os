# WISP Language Reference (v1)

WISP is the small scripting language embedded in this UEFI shell's `run`
command. It exists so that plain-text programs -- written by a person, or by
an LLM given this document -- can be dropped onto the boot USB's `APPS`
folder from any ordinary computer and executed directly from firmware, with
no compiler, no OS, and no network stack anywhere in the loop.

This document is the complete spec for what a WISP script can do today.
If a program only uses what's described here, it will run. The core
language (v1) is deliberately **text and logic only**; pixel/graphics
primitives -- the first piece of v2 to land -- are documented in their
own section below and, in full depth, in `docs/WISP_GRAPHICS.md`. See
"What's not in v1" at the end for what's still deliberately missing.

Looking to run a trained model (a tiny neural net doing fixed-point
forward-pass inference, not anything resembling an LLM) rather than
ordinary game/utility logic? See `docs/WISP_AI.md` -- it's a separate
document because it's a *pattern* built entirely on top of what's
described here (lists + plain integer arithmetic), not a language
extension, and `esp/APPS/xor_brain.wisp` / `esp/APPS/tamer.wisp` are its
worked examples.

## Quick example

```
# guess.wisp -- a number-guessing game
let secret = rand(1, 20);
let tries = 0;

print("I'm thinking of a number from 1 to 20.");

while true {
    let guess_text = input("Your guess: ");
    let guess = int(guess_text);
    tries = tries + 1;

    if guess == secret {
        print("Got it in " + str(tries) + " tries!");
        break;
    } elif guess < secret {
        print("Higher.");
    } else {
        print("Lower.");
    }
}
```

Run it from the shell with `run guess.wisp` (or just `run guess` -- the
`.wisp` extension is optional).

## Lexical structure

- **Comments**: `#` to end of line. No block comments.
- **Identifiers**: ASCII letter or `_`, followed by letters/digits/`_`.
  Case-sensitive.
- **Integer literals**: a run of ASCII digits, e.g. `0`, `42`, `1000`.
  64-bit signed (`i64`). No floats in v1. No negative literals as such --
  `-5` is the unary `-` operator applied to `5`.
- **String literals**: double-quoted, `"like this"`. Supported escapes:
  `\"`, `\\`, `\n`, `\t`. No multi-line strings.
- **Boolean literals**: `true`, `false`.
- **Nil literal**: `nil`.
- **Whitespace**: spaces, tabs, and newlines are insignificant except as
  token separators. Statements are terminated by `;`, not by newlines --
  you can format a program however you like.
- **Keywords** (reserved, cannot be used as identifiers): `let`, `if`,
  `elif`, `else`, `while`, `fn`, `return`, `break`, `continue`, `true`,
  `false`, `nil`, `and`, `or`, `not`.

## Values

Every value in WISP is one of exactly five types:

- `nil` -- the absence of a value. The default value of a variable that
  hasn't been assigned; also what a function returns if it falls off the
  end without a `return`.
- `bool` -- `true` or `false`.
- `int` -- a 64-bit signed integer.
- `string` -- an immutable UTF-8 text value.
- `list` -- a growable, shared, mutable sequence of values. See "Lists"
  below.

There is no map/struct/user-defined structured type in v1. Programs that
need key-value data should use two parallel lists (a list of keys, a list
of values, matched by index) or, for a small fixed set of fields, separate
variables.

## Lists

```
let a = [1, 2, 3];
let empty = [];
let mixed = [1, "two", true, nil];       # lists can mix types
let grid = [[1, 2], [3, 4]];             # and nest
```

- **Indexing** reads an element by position, 0-based: `a[0]` is `1`. The
  index must be an `int` and in bounds (`0 <= i < len(a)`); anything else
  (a negative index, an index `>= len(a)`, a non-`int` index, or indexing
  something that isn't a list at all) is a runtime error. There is no
  negative-indexing-from-the-end shorthand in v1.
- **Index assignment** writes an element in place: `a[0] = 99;`. Same
  bounds/type rules as reading. Chains work for nested lists:
  `grid[0][1] = 99;`.
- **`push(list, value)`** appends `value` to the end of `list` and
  returns `nil`.
- **`pop(list)`** removes and returns the last element of `list`.
  Calling it on an empty list is a runtime error.
- **`len(list)`** returns the element count (the same `len()` used for
  strings; it dispatches on whichever type you pass it).
- Lists are **shared and mutable, not copied**: assigning a list to a new
  variable, or passing it as a function argument, gives you another
  handle to the *same* underlying list, not an independent copy.
  ```
  fn add_one(lst) { push(lst, 1); }
  let a = [];
  let b = a;          # b is the same list as a, not a copy
  add_one(b);
  print(a);            # prints [1] -- a changed too
  ```
  This is what makes a function like `draw(bag)` in `oracle.wisp` able to
  actually shrink the caller's list, and is standard behavior for
  collection types in most scripting languages -- it's called out
  explicitly here because every *other* WISP value (`int`, `string`,
  `bool`) behaves the opposite way (copied on assignment) and it's easy
  to assume lists work the same until you need them not to.
- **Equality (`==`/`!=`) is deep/structural**, not identity: two
  different lists with equal elements in the same order are `==`, even
  though they're independent objects for mutation purposes.
  `[1, 2] == [1, 2]` is `true`. `<` `<=` `>` `>=` are not defined for
  lists (runtime error), the same as comparing any other non-`int`,
  non-`string` types.
- **Display**: `print()`/`str()` render a list as `[elem, elem, ...]`.
  Strings *inside* a list are shown quoted (`print([1, "a"])` prints
  `[1, "a"]`) so they aren't ambiguous with bare words, even though a
  bare top-level string prints unquoted (`print("a")` prints `a`).
- **Self-referential lists** (`let a = []; push(a, a);`) are not rejected
  -- WISP doesn't try to detect cycles when you build one -- but
  structural operations that would otherwise recurse forever into one
  (`==`, `print`/`str`) are depth-limited (64 levels) so a cyclic list
  degrades to a truncated `[...]` result instead of ever crashing the
  shell. This isn't a feature to rely on, just a documented safety net;
  there's no practical reason to build a self-referential list in a real
  script.
- No slicing (`a[1..3]`), no `+` concatenation between lists, no
  `insert`/`remove`-at-arbitrary-index, no `sort`/`map`/`filter` in v1.
  `oracle.wisp` (under `esp/APPS/`) shows the standard workaround for
  "remove an arbitrary index without a builtin for it": swap the target
  element with the last element, then `pop()`.

## Variables and scoping

```
let x = 10;
x = x + 1;      # reassignment: no `let`
```

- A variable must be declared with `let` before it's used or assigned.
  Assigning to an undeclared name is a runtime error (this is deliberate:
  it catches typos instead of silently creating a new global, which is a
  common source of bugs in dynamically-typed toy languages).
- **There are no closures and no nested function definitions.** A
  function body can see:
  - its own parameters and any `let` it declares itself (including
    inside its own nested blocks -- see below), and
  - every global (any `let` at the top level of the program, outside all
    functions and outside all blocks).
  It **cannot** see local variables of the function that called it. This
  is simpler than full lexical scoping and covers everything a small
  game or utility script needs; it's called out explicitly here because
  it's the one place WISP's scoping rules diverge most from languages
  you may already know.
- **Blocks (`if`/`while` bodies) each get their own nested scope,**
  entered when the block starts and discarded the moment it ends --
  whether it ends normally, or via `break`, `continue`, or `return`.
  Concretely:
  ```
  let tries = 0;
  while tries < 3 {
      let attempt = tries + 1;   # fresh `attempt` every single iteration
      print("attempt " + str(attempt));
      tries = tries + 1;
  }
  # `attempt` does not exist here -- it only lived inside the loop body.
  ```
  Because each pass through the loop body gets a brand-new scope, `let`
  inside a loop body declares cleanly on every iteration -- this is the
  normal, expected way to introduce a "this iteration's value" variable
  and is the pattern you should reach for by default (as `guess.wisp`
  and `adventure.wisp` both do, redeclaring their per-turn input with
  `let` inside the loop body every time round).
  A block *can* see and reassign (with plain `=`, not `let`) any
  variable declared by an enclosing block, its enclosing function, or
  the globals -- only introducing a *new* name with `let` is scoped to
  the innermost block. Shadowing is legal: a `let` inside a block may
  reuse a name already declared in an outer scope, creating a distinct
  inner variable for the rest of that block only; the outer one is
  unaffected and reappears once the block ends. The "already declared in
  this scope" error only fires for a second `let` of the same name
  within the *same* block (e.g. two `let x` statements back to back at
  the same nesting level) -- rebind with plain assignment instead if
  that's what you meant.

## Operators

From lowest to highest precedence:

| Level | Operators | Associativity |
|---|---|---|
| 1 | `or` | left |
| 2 | `and` | left |
| 3 | `==` `!=` | left |
| 4 | `<` `<=` `>` `>=` | left |
| 5 | `+` `-` | left |
| 6 | `*` `/` `%` | left |
| 7 | unary `not` `-` | right (prefix) |
| 8 | `( )` grouping, function calls, `[ ]` list literals/indexing | -- |

Semantics:

- `+ - * / %` operate on `int`s. Division and modulo by zero are runtime
  errors (not a crash, not a silently wrong answer).
- `+` on two `string`s concatenates them. `+` where exactly one side is a
  `string` is a runtime error -- use `str(x)` to convert explicitly rather
  than relying on implicit coercion, so a typo'd variable doesn't silently
  concatenate the word "nil" into your output.
- `==` `!=` compare same-typed values for equality (`int`/`int`,
  `string`/`string`, `bool`/`bool`, `nil`/`nil`). Comparing different types
  is always `false` for `==` and `true` for `!=` (never an error).
- `< <= > >=` work on two `int`s, or two `string`s (lexicographic byte
  comparison). Comparing other combinations is a runtime error.
- `and` / `or` / `not` operate on `bool`s only (no truthiness coercion of
  `int`/`string`/`nil` -- passing a non-`bool` to them is a runtime error).
  `and`/`or` short-circuit: the right side is not evaluated if the left
  side already determines the result.

## Statements

```
let NAME = EXPR;                 # declare + initialize
NAME = EXPR;                     # reassign
NAME[INDEX] = EXPR;              # list index assignment (chains: a[i][j] = EXPR;)

if EXPR { ... }
if EXPR { ... } else { ... }
if EXPR { ... } elif EXPR { ... } else { ... }   # any number of elif

while EXPR { ... }
break;                           # exit innermost while
continue;                        # skip to next iteration of innermost while

fn NAME(PARAM, PARAM, ...) {
    ...
    return EXPR;                 # or bare `return;` for nil, or fall off the end
}

EXPR;                            # expression statement (e.g. a bare call: print("hi");)
```

Function declarations may appear anywhere at the top level of a program
(not nested inside other functions, not inside `if`/`while` bodies) and
are all registered before the program's top-level statements start
running, so a function can call another function declared later in the
file. Calling a function with the wrong number of arguments is a runtime
error. Recursion is supported, with a call-depth limit (currently 256
nested calls) that produces a clean "stack overflow" runtime error instead
of crashing the shell -- if you hit it, you have an unbounded recursion
bug.

## Built-in functions

These are always available and cannot be shadowed by `let` or redefined
by `fn`:

- `print(a, b, ...)` -- converts every argument to text (as `str()`
  would) and writes them to the screen separated by a single space,
  followed by a newline. `print()` with no arguments prints a blank line.
- `input(prompt)` -- writes `prompt` to the screen (no trailing newline),
  then blocks (idling the CPU between keystrokes, same as the shell's own
  prompt) until the user presses Enter, and returns everything they typed
  as a `string`. Backspace works while typing, same as at the `>` prompt.
  Pressing Esc while `input()` is waiting stops the program instead of
  returning (see "Stopping a program").
- `len(v)` -- number of bytes in `v` if it's a `string`, or number of
  elements if it's a `list`. Runtime error for any other type.
- `str(v)` -- converts `v` (any type) to its `string` representation:
  ints print as decimal, `true`/`false` print as those words, `nil`
  prints as `nil`, a `string` is returned unchanged, and a `list` renders
  as `[elem, elem, ...]` (see "Lists").
- `push(list, value)` -- appends `value` to the end of `list`, in place.
  Returns `nil`. Runtime error if the first argument isn't a `list`.
- `pop(list)` -- removes and returns the last element of `list`, in
  place. Runtime error if `list` is empty or isn't a `list`.
- `int(v)` -- converts to `int`. If `v` is already an `int`, returned
  unchanged. If `v` is a `string`, it's parsed as a (possibly
  `-`-prefixed) decimal integer; a string that isn't a valid integer is a
  runtime error. `bool`/`nil` are not convertible (runtime error).
- `rand(lo, hi)` -- a pseudo-random `int` in the inclusive range
  `[lo, hi]`. Seeded once at shell startup from the CPU's cycle counter,
  so it varies run to run, but this is **not** cryptographically secure
  randomness -- fine for game mechanics (dice, encounters, loot), not for
  anything security-sensitive.
- `sleep_ms(n)` -- pause execution for at least `n` milliseconds, rounded
  up to the kernel's 10 ms timer tick (idles the CPU with `hlt` between
  ticks; does not spin). `sleep_ms(0)` returns immediately. Pressing Esc
  ends the sleep at once and stops the program (see "Stopping a program").
  Useful for pacing text output or animation frames. Runtime error for a
  negative or non-`int` argument.
- `time_ms()` -- milliseconds since the machine booted, as an `int`, from
  the kernel's monotonic clock (it advances in 10 ms steps). Subtract two
  readings to measure elapsed time; `esp/APPS/stopwatch.wisp` is the
  worked example.
- `clear()` -- clears the screen, same as the shell's `clear` command.
- `save(key, value)` -- persists `value` under the string `key` on the
  machine's AHCI save disk (a second disk, separate from the read-only
  boot USB stick -- see `CHANGES.md`'s Phase 2, Part B entry), surviving a
  reboot. Overwrites any value already saved under that key. `key` must be
  a `string` of 32 bytes or fewer; `value` may be any WISP value --
  `nil`/`bool`/`int`/`string` directly, or a `list` (nested lists are
  supported, to the same 64-level depth limit as everything else
  structural in this language). Returns `nil`. Runtime error if there's no
  save disk available this boot, `key` is empty or too long, `value` is
  too large to fit in one save slot, or every save slot is already in use
  (64 slots total).
- `load(key)` -- looks up the value previously saved under `key`. Returns
  `nil` if `key` was never saved -- WISP has no separate "undefined" value
  distinct from `nil`, so a script checks for "nothing saved yet" with
  `if load("x") == nil { ... }`. (A key saved with the value `nil` itself
  is indistinguishable from "never saved" -- a deliberate simplification,
  not a bug.) Runtime error if there's no save disk available this boot,
  `key` is empty or too long, or the save data itself is corrupt.
  `esp/APPS/counter.wisp` is the canonical example: it loads a boot
  counter (or starts at 0 if there isn't one yet), increments it, saves it
  back, and prints how many times the machine has booted -- the same
  program prints a higher number every time you reboot and run it again.
- `screen_width()` -- the framebuffer's width in pixels. Fixed for the
  life of the program (the display mode is set once at boot). See
  "Graphics" below.
- `screen_height()` -- the framebuffer's height in pixels. See
  `screen_width()` and "Graphics" below.
- `rgb(r, g, b)` -- packs three color channels (each `0..=255`) into the
  single `int` every drawing builtin below accepts as a color. Runtime
  error if any channel is outside `0..=255` or not an `int`. See
  "Graphics" below.
- `plot(x, y, color)` -- sets one pixel. `(x, y)` outside the visible
  screen is silently ignored, not an error. See "Graphics" below.
- `rect(x, y, w, h, color)` -- fills an axis-aligned rectangle, top-left
  corner `(x, y)`, `w` wide and `h` tall. A negative `w` or `h` is a
  runtime error; an off-screen position clips silently. See "Graphics"
  below.
- `line(x1, y1, x2, y2, color)` -- draws a straight line between the two
  points. Coordinates farther than 200,000 from the origin are a runtime
  error rather than a silent clip -- see "Graphics" below for why this
  one builtin is different from the rest.
- `fill_screen(color)` -- fills the entire framebuffer with one color.
  Does not move the text cursor; see "Graphics" below for how this
  differs from `clear()`.

## Graphics

The first piece of v2 to land: seven builtins (`screen_width`,
`screen_height`, `rgb`, `plot`, `rect`, `line`, `fill_screen`, all listed
above) that draw directly into the same framebuffer the text console
renders into. This section is the quick reference; the exact clipping
contract for each builtin, why `line()` validates its coordinates while
every other drawing builtin clips silently instead, and how text and
graphics share one framebuffer are covered in full in
`docs/WISP_GRAPHICS.md`.

- **Coordinates**: `(0, 0)` is the top-left corner; `x` increases to the
  right, `y` increases downward. Both are plain `int`s -- there is no
  separate pixel-coordinate type.
- **Color**: a plain `int`, packed as `0xRRGGBB` (`rgb(255, 0, 0)` and
  the equivalent literal `16711680` both mean pure red). Build one with
  `rgb(r, g, b)`, or write the packed literal directly if you already
  know it. WISP has no bitwise operators, so `rgb()` is the only way to
  build a color from its channels at the language level.
- **`fill_screen()` vs `clear()`**: `fill_screen(color)` repaints every
  pixel but never touches the text cursor; `clear()` repaints the screen
  to the console's own background color *and* resets the cursor to the
  top-left. Printing text and drawing graphics write into the *same*
  framebuffer, so whichever happened most recently wins on any pixel the
  two overlap -- in practice, this means: draw your background *before*
  printing text you want to stay visible, never after.

```
# Draws a filled red square in the middle of the screen.
let color = rgb(220, 40, 40);
let size = 100;
let x = screen_width() / 2 - size / 2;
let y = screen_height() / 2 - size / 2;
rect(x, y, size, size, color);
```

`esp/APPS/bounce.wisp` (a short automatic animation) and
`esp/APPS/canvas.wisp` (an interactive drawing tool exercising every one
of these builtins) are the worked examples.

## Errors

WISP never crashes the shell. Every parse error and every runtime error
(type mismatch, undefined variable, division by zero, wrong argument
count, stack overflow, and so on) is caught, reported to the screen as a
single line in the form:

```
wisp: line N: <message>
```

and control returns to the `>` prompt exactly as if the `run` command had
been any other shell command. A buggy script cannot crash or lock up the
machine: even an infinite `while true { }` loop can be stopped with Esc
(next section).

## Stopping a program

Press **Esc** at any time to stop the running program. This works on both
the PS/2 and USB keyboards, whatever the program is doing:

- in a loop (including a pure compute loop with no I/O at all) -- the
  interpreter checks before every `while` iteration and every function
  call;
- inside `sleep_ms()` -- the sleep wakes within one 10 ms tick;
- waiting in `input()` -- the half-typed line is discarded.

The program ends with an ordinary runtime error naming the line it was
on, which doubles as a cheap profiler for "where was it stuck?":

```
wisp: line 31: stopped (Esc pressed)
```

Anything typed into the program after it stopped is discarded, so it
never leaks onto the shell's `>` prompt; and an Esc pressed at the prompt
itself (where it clears the half-typed command line) never stops the next
program you run. A program cannot catch or ignore Esc -- WISP has no
exception handling, by design, so stopping is always available.
`esp/APPS/primes.wisp` (a never-ending compute loop) and
`esp/APPS/stopwatch.wisp` (a never-ending `sleep_ms()` loop) exist to be
stopped this way.

## What's not in v1

Intentionally out of scope for the original release, to keep the
interpreter small and solid before growing it. v2 is landing
incrementally -- pixel/graphics primitives (see "Graphics" above) are the
first piece; everything below is still missing as of this writing:

- No maps, sets, or user-defined structured types (see "Lists" for the
  parallel-lists workaround).
- No list slicing, `+` concatenation, sorting, or `map`/`filter`.
- No closures or nested function definitions.
- No floating point.
- No string indexing/slicing, splitting, or formatting beyond `+`
  concatenation and `str()`/`int()` conversion.
- No file I/O from within a script (scripts can't read/write other files
  on the volume; only the shell's `run` command reads the script file
  itself).
- No `import`/multi-file programs -- one `.wisp` file is one whole program.

None of these are hard blockers for text adventures, quiz games, dice
games, calculators, or similar small text-mode programs; they're the
first things to reach for if you outgrow v1.
