# WISP Language Reference

WISP is the small scripting language embedded in this UEFI shell's `run`
command. It exists so that plain-text programs -- written by a person, or by
an LLM given this document -- can be dropped onto the boot USB's `APPS`
folder from any ordinary computer and executed directly from firmware, with
no compiler, no OS, and no network stack anywhere in the loop.

This document is the complete spec for what a WISP script can do today.
If a program only uses what's described here, it will run. The core
language (v1) was deliberately **text and logic only**; v2 is landing in
pieces: pixel/graphics primitives (their own section below, and in full
depth `docs/WISP_GRAPHICS.md`), then `for` loops, slicing, string
indexing and the string and list builtins, then maps, then functions as
values and closures. See "What's still not in WISP" at the end for what's
deliberately missing.

To try things out without writing a file, type `wisp` at the shell
prompt: an interactive prompt that runs each line as you type it (see
"The interactive prompt" below).

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
  Any UTF-8 text may appear between the quotes and is kept exactly as
  written: `len("é")` is `2`, because string lengths are in bytes. (The
  console font only draws ASCII, so other characters may *display* as a
  placeholder, but the value itself is unchanged.)
- **Boolean literals**: `true`, `false`.
- **Nil literal**: `nil`.
- **Whitespace**: spaces, tabs, and newlines are insignificant except as
  token separators. Statements are terminated by `;`, not by newlines --
  you can format a program however you like.
- **Keywords** (reserved, cannot be used as identifiers): `let`, `if`,
  `elif`, `else`, `while`, `for`, `in`, `fn`, `return`, `break`,
  `continue`, `true`, `false`, `nil`, `and`, `or`, `not`.

## Values

Every value in WISP is one of exactly seven types:

- `nil` -- the absence of a value. The default value of a variable that
  hasn't been assigned; also what a function returns if it falls off the
  end without a `return`.
- `bool` -- `true` or `false`.
- `int` -- a 64-bit signed integer.
- `string` -- an immutable UTF-8 text value. See "Strings" below.
- `list` -- a growable, shared, mutable sequence of values. See "Lists"
  below.
- `map` -- a shared, mutable table from keys (ints or strings) to
  values. See "Maps" below.
- `func` -- a function, as a value: a named function, a built-in, or a
  closure. See "Functions as values" and "Closures" below.

There are no user-defined types: a map with fixed keys
(`{"name": "ada", "hp": 12}`) is the way to group named fields.

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
  something that isn't a list or a string) is a runtime error. There is
  no negative-indexing-from-the-end shorthand.
- **Slicing** copies a run of elements into a new list: `a[1:3]` is the
  elements at 1 and 2; either bound may be left out (`a[2:]`, `a[:2]`,
  and `a[:]`, a copy of the whole list). Bounds are `int`s with
  `0 <= start <= end <= len(a)` -- anything else is a runtime error, not
  a silently shortened slice. The new list holds the same element values
  (a nested list in it is the *same* nested list, as with any copy of an
  element). You cannot assign to a slice.
- **`+`** on two lists makes a new list: the first's elements, then the
  second's. `[1] + [2, 3]` is `[1, 2, 3]`.
- **Index assignment** writes an element in place: `a[0] = 99;`. Same
  bounds/type rules as reading. Chains work for nested lists:
  `grid[0][1] = 99;`.
- **`push(list, value)`** appends `value` to the end of `list` and
  returns `nil`.
- **`pop(list)`** removes and returns the last element of `list`.
  Calling it on an empty list is a runtime error.
- **`insert(list, i, value)`** / **`remove(list, i)`** add or take out
  the element at any position; **`sort(list)`** and **`reverse(list)`**
  reorder in place; **`find`/`contains`** search; **`range(n)`** builds
  `[0, 1, ..., n - 1]`. See "Built-in functions".
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
- **Self-referential lists** (`let a = []; push(a, a);`) are allowed.
  `==` compares them by structure (two cyclic lists are equal when no
  path through them finds a difference); `print`/`str` stop descending
  after 64 levels and show `[...]`. Every list a program creates is
  freed when the program ends, cycles included.
- **Size limits.** Because lists are shared, a few lists can describe
  an enormous structure (`v = [v, v]` repeated 22 times is 23 lists but
  tens of megabytes when written out). So: `print` output for one value
  stops after 16 KiB and ends with `...(truncated)`; `str()` of a value
  longer than that is a runtime error; `==` gives up with an error after
  visiting 100,000 elements; `save()` rejects anything whose encoding
  exceeds one slot (4055 bytes) as soon as it gets that far. A list
  holds at most 32,768 elements and a string at most 256 KiB. If a
  program runs the kernel heap down to its reserve it is stopped with an
  "out of memory" error.
- `for x in list { ... }` visits each element in order; see
  "Statements".
- There is no `map`/`filter`: write the loop. (`oracle.wisp` predates
  `remove()` and still shows the older swap-with-last-then-`pop()` way
  of removing an arbitrary element, which is also the fastest.)

## Maps

```
let hero = {"name": "ada", "hp": 12, "items": ["lamp"]};
let empty = {};
print(hero["name"]);          # ada
hero["hp"] = hero["hp"] - 3;  # change an entry
hero["gold"] = 5;             # add one
for key in hero {
    print(key, hero[key]);    # gold 5 / hp 9 / items ["lamp"] / name ada
}
```

- **Keys** are ints or strings -- nothing else (a list or map can change
  after it is put in, which a key must never do). `1` and `"1"` are
  different keys. Values may be anything, including lists and other maps.
- **Order.** A map keeps its entries sorted by key: all int keys first,
  in numeric order, then all string keys, in the order `<` uses (byte
  order, so `"B"` comes before `"a"`). That is the order `print`, `for`,
  `keys()` and `values()` all use, whatever order the entries were added
  in -- so a program's output never depends on its insertion history.
- **A literal** `{key: value, ...}` builds a new map every time it runs,
  evaluating each key and then its value, entry by entry, left to right.
  The same key twice in one literal is a runtime error. There is no
  trailing comma.
- **Reading** `map[key]` gives the value stored under `key`. A key that is
  not in the map is a runtime error, not `nil`: use `contains(map, key)`
  to ask first, or `get(map, key)` / `get(map, key, default)`, which
  answer `nil` / `default` for a missing key. A key of any other type
  than int or string is a runtime error.
- **Writing** `map[key] = value;` replaces the value under `key`, or adds
  the entry if `key` is new. Chains mix freely with lists:
  `world["rooms"][0]["exits"]["n"] = 1;`.
- **`remove(map, key)`** takes `key`'s entry out and returns its value
  (runtime error if there is none). **`len(map)`** is the number of
  entries. **`keys(map)`** and **`values(map)`** are new lists, in key
  order. See "Built-in functions".
- Maps are **shared and mutable, not copied**, exactly like lists:
  `let b = a;` and passing a map to a function give another handle to
  the same map.
- **Equality** is structural: two maps are `==` when they hold the same
  keys with `==` values. Order cannot differ, since both are kept sorted.
  `<` and the other comparisons are not defined for maps, and `+` does
  not combine them.
- `for key in map { ... }` visits the keys, in order, as they were when
  the loop started: adding or removing entries inside the body does not
  change what the loop visits. Read the value with `map[key]`.
- **Display**: `print({"a": 1, 2: [3]})` prints `{2: [3], "a": 1}`, string
  keys and values quoted as inside a list.
- **Limits** are the same as lists': at most 32,768 entries in one map;
  self-referential maps (`m["me"] = m;`) are allowed, compare by
  structure, display as `{...}` past 64 levels, and are freed when the
  program ends; `save()` stores a map like any other value, within the
  same one-slot size limit.
- Slicing (`m[1:2]`) is not defined for maps.

`esp/APPS/words.wisp` -- a word counter that keeps every tally in one
map -- is the worked example.

## Strings

```
let s = "hello, world";
print(s[0]);             # h
print(s[7:]);            # world
print(upper(s[:5]));     # HELLO
for word in split("go north now") {
    print(word);         # go / north / now
}
```

Strings are immutable: every operation on one makes a new string, and
`s[0] = "x";` is a runtime error.

**Offsets are bytes**, the same unit `len()` counts in. For ASCII text --
everything the console font can draw -- a byte is a character and this
is all you need to know. For other characters, which are 2-4 bytes in
UTF-8, an offset must land on the first byte of a character:

- `s[i]` is the character *starting at* byte `i`, as a new one-character
  string; `"héllo"[1]` is `"é"`, and `"héllo"[2]` -- the middle of that
  `é` -- is a runtime error rather than half a character.
- `s[a:b]` is the text between byte offsets `a` and `b`, with the same
  rules as list slices plus the same character-boundary check at both
  ends.
- `find(s, part)` answers in bytes too, so its result can be used to
  slice: `s[find(s, ","):]`.
- `for c in s { ... }` and `chars(s)` go one *character* at a time, so
  they are the way to walk text that may not be ASCII.

The string builtins (details under "Built-in functions"): `split`,
`join`, `find`, `contains`, `replace`, `upper`, `lower`, `trim`,
`starts_with`, `ends_with`, `chars`, `repeat`, `chr`, `ord`, plus `len`,
`str` and `int`. Like `+`, none of them builds a string over 256 KiB.

`esp/APPS/calc.wisp` -- an arithmetic calculator whose expression parser
is written in WISP -- is the worked example.

## Functions as values

A function's name, used where a value is expected, is that function. It
can go in a variable, a list or a map, be passed to another function, or
be returned from one:

```
fn double(x) { return x * 2; }
fn twice(f, x) { return f(f(x)); }

let f = double;
print(f(21));            # 42
print(twice(double, 5)); # 20
print(f, type(f));       # <fn double> func

let table = {"d": double, "len": len};
print(table["d"](4));    # 8
```

- **Any built-in is a function value too** (`let n = len;`,
  `map(upper, words)`), as are `map`, `filter`, `reduce` and `sort_by`.
- **Calling.** `name(args)` calls the function a variable called `name`
  holds, if there is one, else the function called `name`. `callee(args)`
  calls any expression that evaluates to a function: `table[word](rest)`,
  `pick()(3)`, `(f)(3)`. Calling anything that is not a function is a
  runtime error naming what it was. The number of arguments is checked
  against the function actually called, so a wrong count says which
  function was meant.
- **Variables and functions share one lookup, in this order:** the
  nearest variable, then the function of that name. A parameter `f`
  holding a function is therefore what `f(x)` calls, even if a function
  named `f` also exists. A variable holding something *else* (`let f = 5;`)
  does not hide a function of the same name when called: `f()` still
  calls the function, as it did before functions were values.
- **A named function's value is its name and nothing else.** Its body
  still sees only its parameters, its own locals and the globals. It is
  looked up when it is called, so redefining a function at the `wisp>`
  prompt changes what every stored copy of it does. (A function that
  should remember variables is a closure; see "Closures".)
- **Comparing and showing.** Two function values are `==` if they name the
  same function. `print` and `str` show `<fn name>`; `type` says `"func"`.
- **Not keys, and not saved.** A function cannot be a map key, and
  `save()` refuses any value that holds one (a list or map with a
  function inside included): the name would mean whatever happens to
  carry it after a reboot.

### Calling a function on every element

These four built-ins take a function as an argument. Each works from a
snapshot of the list as it was when the call began, so a function that
grows the list cannot make the call run on. Pressing Esc stops them like
any loop. An error inside the function you pass is reported at its own
line.

- `map(f, list)` -- a new list of `f(element)` for each element, in
  order. The original is unchanged.
- `filter(f, list)` -- a new list of the elements for which `f(element)`
  is `true`, in their original order. `f` must return a `bool`.
- `reduce(f, list, start)` -- folds the list from the left: the result is
  `start`, then `f(result, element)` for each element in turn. An empty
  list gives `start`.
- `sort_by(list, f)` -- reorders `list` **in place**, ascending by the key
  `f(element)`; `f` runs once per element, and every key must be an int,
  or every key a string. The sort is stable: elements with equal keys stay
  in their original order, so sorting by a second key first and the main
  key last works as expected. If `f` changes the size of the list, or
  returns an unusable key, the call is a runtime error and the list is
  left as it was. Returns `nil`, as `sort` does.

```
fn square(x) { return x * x; }
fn is_even(x) { return x % 2 == 0; }
fn add(a, b) { return a + b; }
print(reduce(add, filter(is_even, map(square, range(6))), 0));   # 0+4+16 = 20
```

`esp/APPS/tasks.wisp` is the worked example: a to-do list whose commands
are a map from what you type to the function that handles it.

## Closures

`fn (parameters) { body }`, written where an expression is expected, is a
function with no name -- and it remembers the variables that were in
scope where it was written:

```
let double = fn (x) { return x * 2; };
print(double(21));                  # 42
print(double, type(double));        # <fn (x)> func
print(fn (a, b) { return a + b; }(1, 2));   # 3

fn make_counter() {
    let n = 0;
    return fn () {
        n = n + 1;
        return n;
    };
}
let a = make_counter();
let b = make_counter();
print(a(), a(), a());   # 1 2 3
print(b());             # 1 -- b has its own n
```

A closure is a value like any other function value (type `func`): it can
go in a variable, a list or a map, be passed to `map`, `filter`, `reduce`
and `sort_by`, be returned, and be called with `f(x)` or `callee(x)`.

- **What it sees.** The body runs with the parameters first, then every
  variable of every block and call that enclosed the `fn` expression,
  innermost first, then the globals. This is ordinary lexical scoping.
  Variables are **shared, not copied**: a closure sees what the code
  around it assigns afterwards, and the code around it sees what the
  closure assigns (`n = n + 1` above changes the one `n` that
  `make_counter`'s call made). Two closures made in the same scope share
  its variables, which is how a getter and a setter can work on one
  value. A name is looked up when the closure is *called*, so it may even
  be declared after the closure was made.
- **Loops.** Every pass of a `while` or `for` body is a fresh scope, so
  closures made in different passes each keep that pass's variables:
  `for i in range(3) { push(fs, fn () { return i; }); }` gives three
  closures returning 0, 1 and 2.
- **Recursion.** A closure calls itself through any variable that holds
  it (`let fact = fn (n) { ... fact(n - 1) ... };`) or through a
  parameter. The ordinary limits apply: 256 nested calls, and the
  combined nesting budget.
- **Declarations are separate.** `fn NAME(...) { }` is still a declaration
  that is only allowed at the top level, hoisted, and sees only its
  parameters, locals and globals. `fn (` with no name is the expression.
  At the start of a statement `fn (` begins an expression statement, so
  `fn (x) { print(x); }(7);` runs. A named `fn` inside a block or an
  expression is a parse error that says so.
- **Calling.** The rules of "Functions as values" apply unchanged: a
  variable holding a closure is what `name(args)` calls, a wrong argument
  count is an error (`the anonymous function expects 2 argument(s), got
  1`), and the higher-order built-ins name it the same way in their own
  errors.
- **Comparing and showing.** A closure is `==` only to itself (and to
  copies of it, in a variable or a list): two `fn` expressions are
  different functions even with the same text. `print` and `str` show
  `<fn (x, y)>`, its parameters. Like any function it cannot be a map key
  or be saved.
- **Memory.** A closure keeps the variables it can see alive for as long
  as it is. A closure stored in a variable that it can itself see (as in
  the recursive example) is a cycle; like cyclic lists and maps, it is
  reclaimed when the program ends. A very long chain of closures (each
  holding the one before) is freed without deep recursion.

`esp/APPS/undo.wisp` is the worked example: a notebook of numbers where
each edit is a pair of closures that remember what that edit needs to be
undone and redone.

## Variables and scoping

```
let x = 10;
x = x + 1;      # reassignment: no `let`
```

- A variable must be declared with `let` before it's used or assigned.
  Assigning to an undeclared name is a runtime error (this is deliberate:
  it catches typos instead of silently creating a new global, which is a
  common source of bugs in dynamically-typed toy languages).
- **A `fn NAME(...)` declaration is top-level only** (not nested inside
  other functions or blocks). Its body can see:
  - its own parameters and any `let` it declares itself (including
    inside its own nested blocks -- see below), and
  - every global (any `let` at the top level of the program, outside all
    functions and outside all blocks).
  It **cannot** see local variables of the function that called it. For a
  function that does see the variables around where it is written, use a
  closure, `fn (...) { ... }` as an expression -- see "Closures".
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
| 8 | `( )` grouping, function calls, `[ ]` list literals, `{ }` map literals, `fn (...) { }` closures, indexing and slicing | -- |

Semantics:

- `+ - * / %` operate on `int`s. Division and modulo by zero are runtime
  errors (not a crash, not a silently wrong answer).
- `+` on two `string`s concatenates them, and on two `list`s makes a new
  list of both lists' elements. `+` on any other mix of types (a `string`
  and an `int`, say) is a runtime error -- use `str(x)` to convert
  explicitly rather than relying on implicit coercion, so a typo'd
  variable doesn't silently concatenate the word "nil" into your output.
- `==` `!=` compare same-typed values for equality (`int`/`int`,
  `string`/`string`, `bool`/`bool`, `nil`/`nil`, and lists and maps by
  structure). Comparing different types is always `false` for `==` and
  `true` for `!=` (never an error).
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
NAME[INDEX] = EXPR;              # list element or map entry assignment (chains: a[i][j] = EXPR;)

if EXPR { ... }
if EXPR { ... } else { ... }
if EXPR { ... } elif EXPR { ... } else { ... }   # any number of elif

while EXPR { ... }
for NAME in EXPR { ... }         # EXPR: a list, a map or a string
break;                           # exit innermost while/for
continue;                        # skip to next iteration of innermost while/for

fn NAME(PARAM, PARAM, ...) {
    ...
    return EXPR;                 # or bare `return;` for nil, or fall off the end
}

fn (PARAM, PARAM, ...) { ... }   # an expression: a closure (see "Closures")

EXPR;                            # expression statement (e.g. a bare call: print("hi");)
```

`for x in EXPR { ... }` evaluates `EXPR` once. If it is a list, the
body runs once per element, in order, with `x` holding that element; the
loop visits the elements the list held when the loop *started* -- adding
to or removing from the list inside the body does not change, or
endlessly extend, the loop. If it is a map, the body runs once per key,
in key order, with `x` holding the key -- again the keys the map had when
the loop started. If it is a string, the body runs once per character
(not per byte), `x` a one-character string. Anything else is a runtime
error. Each pass runs in its own fresh scope holding `x`, so
`x` does not exist after the loop, and `let x` inside the body is an
"already declared" error. `break` and `continue` work as in `while`.
`for i in range(10) { ... }` is the counting loop.

Function declarations may appear anywhere at the top level of a program
(not nested inside other functions, not inside `if`/`while` bodies) and
are all registered before the program's top-level statements start
running, so a function can call another function declared later in the
file. Calling a function with the wrong number of arguments is a runtime
error. Recursion is supported, with a call-depth limit (256 nested calls)
and a combined nesting budget (1024 levels of nested calls, statements
and expressions together) that produce a clean "stack overflow" runtime
error instead of crashing the shell. A simple recursive function reaches
about 250 calls; one whose body nests deeper reaches the combined budget
sooner (a `for` statement counts twice, because the interpreter's own
work for one is larger). Independently, one block or expression may nest
at most 128 levels deep (parentheses, `not`/`-`, indexing, slicing,
blocks, and each operator in a chain like `1 + 2 + 3` all count) --
deeper is a parse error.

## Built-in functions

These are always available and cannot be shadowed by `let` or redefined
by `fn`:

- `print(a, b, ...)` -- converts every argument to text (as `str()`
  would) and writes them to the screen separated by a single space,
  followed by a newline. `print()` with no arguments prints a blank line.
- `input(prompt)` -- writes `prompt` to the screen (no trailing newline),
  then blocks (idling the CPU between keystrokes, same as the shell's own
  prompt) until the user presses Enter, and returns everything they typed
  as a `string`. The line can be edited while it is typed, as at the `>`
  prompt: Left/Right, Home/End, Backspace and Delete. (Up/Down history is
  not offered inside a program: a game's input should not recall shell
  commands.)
  Pressing Esc while `input()` is waiting stops the program instead of
  returning (see "Stopping a program").
- `len(v)` -- number of bytes in `v` if it's a `string`, number of
  elements if it's a `list`, or number of entries if it's a `map`.
  Runtime error for any other type.
- `str(v)` -- converts `v` (any type) to its `string` representation:
  ints print as decimal, `true`/`false` print as those words, `nil`
  prints as `nil`, a `string` is returned unchanged, a `list` renders
  as `[elem, elem, ...]` (see "Lists"), a `map` as `{key: value, ...}`
  (see "Maps"), a named function as `<fn name>` and a closure as
  `<fn (params)>`.
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
  `nil`/`bool`/`int`/`string` directly, or a `list` or `map` (nested
  ones are supported, to the same 64-level depth limit as everything else
  structural in this language; `load` gives back a new, independent copy
  each time). Saving `nil` deletes the key, freeing
  its slot. Returns `nil`. Each save is atomic: if the machine crashes
  or loses power while `save()` is running, the key afterwards holds
  either its old value or the new one, never neither -- and once
  `save()` returns, the new value is on the disk (see
  `docs/SAVE_FORMAT.md`). Runtime error if there's no save disk
  available this boot, `key` is empty or too long, `value` is too large
  to fit in one save slot (its encoding is over 4047 bytes), or `key` is
  new and 63 keys are already stored (the shell's `keys` command lists
  them, and `delete <key>` removes one).
- `load(key)` -- looks up the value previously saved under `key`. Returns
  `nil` if `key` was never saved (or was deleted by saving `nil`) -- WISP
  has no separate "undefined" value distinct from `nil`, so a script
  checks for "nothing saved yet" with `if load("x") == nil { ... }`.
  Runtime error if there's no save disk available this boot,
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

Strings (see "Strings" above; offsets are bytes; every result is a new
string; none longer than 256 KiB):

- `split(s)` -- the words of `s`: split on runs of whitespace, with
  leading/trailing whitespace ignored, so it never yields empty strings
  (`split("  go  north ")` is `["go", "north"]`). `split(s, sep)` splits
  on every occurrence of the non-empty string `sep`, keeping the empty
  pieces between adjacent separators, so `join(split(s, sep), sep)` is
  always `s` again. At most 32,768 pieces.
- `join(list, sep)` -- the list's elements joined with `sep` between
  them. Every element must be a string (use `str()` first; nothing is
  converted for you).
- `find(s, part)` -- the byte offset of the first occurrence of `part` in
  `s`, or `-1`. `find(list, value)` -- the index of the first element
  `== value`, or `-1`.
- `contains(s, part)` / `contains(list, value)` -- `true` if `find` would
  not return `-1`. `contains(map, key)` -- `true` if `map` has an entry
  for `key` (see Maps below).
- `replace(s, old, new)` -- `s` with every occurrence of the non-empty
  string `old` replaced by `new`.
- `upper(s)` / `lower(s)` -- ASCII letters converted; every other
  character unchanged.
- `trim(s)` -- `s` without leading and trailing whitespace.
- `starts_with(s, part)` / `ends_with(s, part)` -- `bool`.
- `chars(s)` -- a list of `s`'s characters, each a one-character string.
- `repeat(s, n)` -- `s` repeated `n` times (`n >= 0`).
- `chr(n)` -- the one-character string for Unicode code point `n`
  (`chr(65)` is `"A"`). `ord(s)` -- the code point of a one-character
  string (`ord("A")` is `65`). Runtime error for anything that is not
  exactly one character / one valid code point.

Lists (in place unless noted):

- `insert(list, i, value)` -- `value` becomes element `i`; the elements
  from `i` on move up one. `0 <= i <= len(list)` (`i == len(list)`
  appends). Returns `nil`.
- `remove(list, i)` -- takes element `i` out (`0 <= i < len(list)`) and
  returns it; the elements after it move down one.
- `sort(list)` -- ascending: ints numerically, strings in the order `<`
  uses. Every element must be an int, or every element a string. Returns
  `nil`.
- `reverse(list)` -- reverses the order. Returns `nil`.
- `range(n)` -- a new list `[0, 1, ..., n - 1]`; `range(a, b)` --
  `[a, a + 1, ..., b - 1]`. Empty if the end is not past the start. At
  most 32,768 elements.
- `find`/`contains` -- see Strings above; both also search lists.

Maps (see "Maps"; everything here is in key order):

- `keys(map)` -- a new list of the map's keys.
- `values(map)` -- a new list of the map's values (the values themselves,
  shared as with any copy of a value: a list in it is the map's list).
- `get(map, key)` -- the value under `key`, or `nil` if there is none;
  `get(map, key, default)` -- the same, but `default` instead of `nil`.
  The way to read a key that may be missing (`map[key]` would be an
  error).
- `contains(map, key)` -- `true` if `key` has an entry.
- `remove(map, key)` -- takes `key`'s entry out and returns its value.
  Runtime error if `key` has no entry.
- `len(map)` -- the number of entries.

Numbers and types:

- `abs(n)` -- the absolute value of an int.
- `min(a, b)` / `max(a, b)` -- the smaller/larger of two ints, or of two
  strings (in the order `<` uses).
- `type(v)` -- `"nil"`, `"bool"`, `"int"`, `"string"`, `"list"`, `"map"`
  or `"func"`.

Functions as arguments (see "Functions as values"): `map(f, list)`,
`filter(f, list)`, `reduce(f, list, start)` and `sort_by(list, f)`.

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
  interpreter checks before every `while` and `for` iteration and every
  function call;
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

## The interactive prompt

Typing `wisp` at the shell's `>` prompt starts an interactive WISP
session with its own `wisp>` prompt. Each line you type runs at once,
against everything earlier lines defined:

```
wisp> let names = split("ada grace alan")
wisp> sort(names)
wisp> names
["ada", "alan", "grace"]
wisp> fn shout(s) { return upper(s) + "!"; }
wisp> shout(names[0])
"ADA!"
```

- A line that is just an expression shows its value (strings in quotes,
  so `"1"` and `1` look different); `nil` shows nothing.
- The final `;` may be left off.
- A line that opens more `{`, `(` or `[` than it closes continues on the
  next line, at a `...` prompt, until they balance -- so a `fn`, `if`,
  `while` or `for` can be typed over several lines.
- Unlike in a program file, `let` may re-declare a global and `fn` may
  redefine a function an earlier line defined -- that is how you fix a
  typo.
- A closure made on one line (`let add = fn (a, b) { return a + b; }`)
  keeps its variables for the lines after it, and sees a global that a
  later line re-declares.
- An error is reported in the usual form and the session carries on with
  everything it had.
- Esc stops a running line, as it stops a program. At the `wisp>`
  prompt it clears a half-typed line, as at the shell prompt; at a `...`
  continuation prompt it discards the whole unfinished input.
- Up/Down recall earlier lines (kept from one `wisp` session to the
  next until the machine restarts), and Left/Right/Home/End/Delete edit
  the line -- the quick way to fix a typo in a line that failed.
- `vars` lists the session's globals and functions; `exit` (or `quit`)
  ends the session, freeing everything it built.

## What's still not in WISP

Intentionally out of scope so far, to keep the interpreter small and
solid while it grows:

- No sets (a map whose values are all `true` stands in for one) and no
  user-defined types (a map with fixed keys groups named fields).
- No nested `fn NAME` declarations (use a closure, `let helper = fn (x)
  { ... };`, for a function local to another).
- No anonymous-function shorthand: a closure's body is always a `{ }`
  block with a `return`.
- No floating point.
- No string formatting beyond `+`, `str()`, `repeat()` and the string
  builtins.
- No file I/O from within a script (scripts can't read/write other files
  on the volume; only the shell's `run` command reads the script file
  itself).
- No `import`/multi-file programs -- one `.wisp` file is one whole program.

None of these are hard blockers for text adventures, quiz games, dice
games, calculators, or similar small text-mode programs.
