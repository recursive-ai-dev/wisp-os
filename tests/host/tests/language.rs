//! The language added after v1.0.0: `for` loops, slicing, string
//! indexing, list `+`, the string/list/number builtins, and interactive
//! sessions (`Session::run_interactive` / `wisp::parse_interactive`, which
//! back the shell's `wisp` prompt). Each feature's documented behaviour
//! and every error it can raise are checked here, against the real
//! interpreter sources (see `tests/host/Cargo.toml`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wisp_host_tests::wisp::interp::{Host, Session, OUT_OF_MEMORY_MESSAGE};
use wisp_host_tests::wisp::value::{Value, MAX_LIST_LEN, MAX_STR_LEN};
use wisp_host_tests::wisp::{self, WispError};

// ---- per-thread live-allocation accounting (as in regressions.rs) --------

struct Counting;

thread_local! {
    static LIVE_BYTES: Cell<isize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE_BYTES.with(|c| c.set(c.get() + layout.size() as isize));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.with(|c| c.set(c.get() - layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn live_bytes() -> isize {
    LIVE_BYTES.with(|c| c.get())
}

// ---- a host that records output ------------------------------------------

#[derive(Default)]
struct TestHost {
    out: String,
    /// `stop_requested` starts answering true after this many calls.
    stop_after: Option<usize>,
    stop_calls: usize,
    /// `memory_low` always answers this.
    low_memory: bool,
    /// Lines `input()` returns, in order; then "quit".
    inputs: std::collections::VecDeque<String>,
}

impl Host for TestHost {
    fn print(&mut self, s: &str) {
        self.out.push_str(s);
    }
    fn input(&mut self, prompt: &str) -> String {
        let line = self.inputs.pop_front().unwrap_or_else(|| "quit".into());
        self.out.push_str(prompt);
        self.out.push_str(&line);
        self.out.push('\n');
        line
    }
    fn clear(&mut self) {}
    fn sleep_ms(&mut self, _ms: u64) {}
    fn rand_u64(&mut self) -> u64 {
        4
    }
    fn save(&mut self, _key: &str, _value: &Value) -> Result<(), &'static str> {
        Ok(())
    }
    fn load(&mut self, _key: &str) -> Result<Option<Value>, &'static str> {
        Ok(None)
    }
    fn stop_requested(&mut self) -> bool {
        self.stop_calls += 1;
        self.stop_after.is_some_and(|n| self.stop_calls > n)
    }
    fn memory_low(&mut self) -> bool {
        self.low_memory
    }
    fn time_ms(&mut self) -> u64 {
        0
    }
    fn screen_width(&self) -> usize {
        640
    }
    fn screen_height(&self) -> usize {
        480
    }
    fn plot(&mut self, _x: i64, _y: i64, _color: u32) {}
    fn draw_rect(&mut self, _x: i64, _y: i64, _w: usize, _h: usize, _color: u32) {}
    fn draw_line(&mut self, _x1: i64, _y1: i64, _x2: i64, _y2: i64, _color: u32) {}
    fn fill_screen(&mut self, _color: u32) {}
}

fn run_with(src: &str, mut host: TestHost) -> (Result<(), WispError>, TestHost) {
    let r = wisp::run_source(src, &mut host);
    (r, host)
}

/// Run `src` and return what it printed, one `print` per line.
fn out(src: &str) -> String {
    let (r, host) = run_with(src, TestHost::default());
    if let Err(e) = r {
        panic!("script failed: {e}\n--- script ---\n{src}\n--- output ---\n{}", host.out);
    }
    host.out
}

/// The value of one expression, as `print` shows it.
fn eval(expr: &str) -> String {
    out(&format!("print({expr});")).trim_end_matches('\n').to_string()
}

/// Run `src`, which must fail, and return the error message.
fn err(src: &str) -> String {
    match run_with(src, TestHost::default()).0 {
        Ok(()) => panic!("script unexpectedly succeeded:\n{src}"),
        Err(e) => e.message,
    }
}

fn assert_err(src: &str, expected_fragment: &str) {
    let msg = err(src);
    assert!(msg.contains(expected_fragment), "error for\n  {src}\nwas\n  {msg}\nexpected it to contain\n  {expected_fragment}");
}

// ---- for loops ------------------------------------------------------------

#[test]
fn for_loops_over_list_elements() {
    assert_eq!(out("let t = 0; for x in [1, 2, 3, 4] { t = t + x; } print(t);"), "10\n");
    assert_eq!(out(r#"for w in ["a", "b"] { print(w); }"#), "a\nb\n");
    assert_eq!(out("for x in [] { print(x); } print(\"done\");"), "done\n");
}

#[test]
fn for_loops_support_break_continue_and_return() {
    let src = r#"
        for x in range(10) {
            if x == 2 { continue; }
            if x == 5 { break; }
            print(x);
        }
        fn first_even(xs) {
            for x in xs { if x % 2 == 0 { return x; } }
            return nil;
        }
        print(first_even([3, 7, 8, 10]));
        print(first_even([1]));
    "#;
    assert_eq!(out(src), "0\n1\n3\n4\n8\nnil\n");
}

#[test]
fn for_loops_iterate_a_snapshot_of_the_list() {
    // Growing the list inside the loop does not extend the loop...
    assert_eq!(out("let a = [1, 2]; for x in a { push(a, x); } print(a);"), "[1, 2, 1, 2]\n");
    // ...and shrinking it does not cut the loop short or skip elements.
    assert_eq!(out("let a = [1, 2, 3]; for x in a { pop(a); print(x); } print(a);"), "1\n2\n3\n[]\n");
}

#[test]
fn for_loop_variables_are_scoped_to_one_pass() {
    assert_err("for x in [1] { } print(x);", "undefined variable 'x'");
    assert_err("for x in [1] { let x = 2; }", "'x' is already declared");
    // Shadowing an outer variable leaves it alone.
    assert_eq!(out("let x = 9; for x in [1, 2] { } print(x);"), "9\n");
    // Each pass gets a fresh scope, so `let` inside the body works every time.
    assert_eq!(out("for x in [1, 2] { let y = x * 10; print(y); }"), "10\n20\n");
}

#[test]
fn for_loops_over_strings_visit_characters() {
    assert_eq!(out(r#"for c in "héy" { print(c); }"#), "h\né\ny\n");
    assert_eq!(out(r#"let n = 0; for c in "" { n = n + 1; } print(n);"#), "0\n");
}

#[test]
fn for_loops_reject_other_types() {
    assert_err("for x in 5 { }", "'for' loops over a list or a string, found int");
    assert_err("for x in nil { }", "found nil");
}

#[test]
fn for_loops_stop_on_esc() {
    let host = TestHost { stop_after: Some(3), ..Default::default() };
    let (r, host) = run_with("for x in range(1000) { print(x); }", host);
    let e = r.unwrap_err();
    assert!(e.message.contains("stopped"), "{}", e.message);
    assert_eq!(host.out, "0\n1\n2\n");
}

#[test]
fn for_and_in_are_keywords() {
    assert_err("let for = 1;", "expected identifier");
    assert_err("let in = 1;", "expected identifier");
    assert_err("for x [1] { }", "expected 'in'");
}

// ---- indexing and slicing -------------------------------------------------

#[test]
fn strings_index_by_byte_offset() {
    assert_eq!(eval(r#""hello"[1]"#), "e");
    assert_eq!(eval(r#""héllo"[1]"#), "é");
    assert_eq!(eval(r#""héllo"[3]"#), "l");
    assert_err(r#"print("héllo"[2]);"#, "falls inside a multi-byte character");
    assert_err(r#"print("abc"[3]);"#, "string index 3 out of bounds (length 3)");
    assert_err(r#"print("abc"[-1]);"#, "out of bounds");
    assert_err(r#"print("abc"["0"]);"#, "string index must be an int");
}

#[test]
fn strings_are_immutable() {
    assert_err(r#"let s = "abc"; s[0] = "x";"#, "strings are immutable");
}

#[test]
fn lists_and_strings_slice() {
    assert_eq!(eval("[1, 2, 3, 4][1:3]"), "[2, 3]");
    assert_eq!(eval("[1, 2, 3, 4][2:]"), "[3, 4]");
    assert_eq!(eval("[1, 2, 3, 4][:2]"), "[1, 2]");
    assert_eq!(eval("[1, 2, 3, 4][:]"), "[1, 2, 3, 4]");
    assert_eq!(eval("[1, 2][2:2]"), "[]");
    assert_eq!(eval(r#""hello"[1:4]"#), "ell");
    assert_eq!(eval(r#""hello"[3:]"#), "lo");
    assert_eq!(eval(r#""héllo"[1:3]"#), "é");
    assert_eq!(eval(r#"len("hello"[:0])"#), "0");
}

#[test]
fn slices_are_copies() {
    assert_eq!(out("let a = [1, 2]; let b = a[:]; push(b, 3); print(a); print(b);"), "[1, 2]\n[1, 2, 3]\n");
    // Elements are shared like any copy of an element: a nested list is
    // the same list in both.
    assert_eq!(out("let a = [[1]]; let b = a[:]; push(b[0], 2); print(a);"), "[[1, 2]]\n");
}

#[test]
fn slice_errors_are_precise() {
    assert_err("print([1, 2][1:5]);", "slice end 5 out of bounds (length 2)");
    assert_err("print([1, 2][2:1]);", "slice start 2 is after its end 1");
    assert_err("print([1, 2][-1:]);", "slice start must not be negative");
    assert_err("print([1, 2][:\"x\"]);", "slice end must be an int");
    assert_err(r#"print("héllo"[2:]);"#, "string start 2 falls inside a multi-byte character");
    assert_err(r#"print("héllo"[:2]);"#, "string end 2 falls inside a multi-byte character");
    assert_err("print(5[0:1]);", "cannot slice int");
    assert_err("let a = [1, 2]; a[0:1] = [3];", "cannot assign to a slice");
}

#[test]
fn slicing_nests_with_indexing_and_calls() {
    assert_eq!(eval(r#"split("a b c")[1:][0]"#), "b");
    assert_eq!(eval("[[1, 2, 3]][0][1:]"), "[2, 3]");
    assert_eq!(eval(r#""abcdef"[1:5][1:3]"#), "cd");
}

// ---- list + ---------------------------------------------------------------

#[test]
fn lists_concatenate() {
    assert_eq!(eval("[1, 2] + [3]"), "[1, 2, 3]");
    assert_eq!(eval("[] + []"), "[]");
    assert_eq!(out("let a = [1]; let b = a + a; push(a, 2); print(b);"), "[1, 1]\n");
    assert_err("print([1] + 1);", "'+' expects two ints, two strings or two lists, found list and int");
    let src = format!("let a = range({}); let b = a + [1];", MAX_LIST_LEN);
    assert_err(&src, "list is too long");
}

// ---- string builtins ------------------------------------------------------

#[test]
fn split_and_join() {
    assert_eq!(eval(r#"split("  take   the key ")"#), r#"["take", "the", "key"]"#);
    assert_eq!(eval(r#"split("a,b,,c", ",")"#), r#"["a", "b", "", "c"]"#);
    assert_eq!(eval(r#"split("", ",")"#), r#"[""]"#);
    assert_eq!(eval(r#"split("")"#), "[]");
    assert_eq!(eval(r#"join(["a", "b", "c"], ", ")"#), "a, b, c");
    assert_eq!(eval(r#"join([], "-")"#), "");
    assert_eq!(eval(r#"join(split("x::y::z", "::"), "::")"#), "x::y::z");
    assert_err(r#"print(split("a", ""));"#, "separator must not be empty");
    assert_err(r#"print(split(5));"#, "split() expects a string for the text, found int");
    assert_err(r#"print(split("a", "b", "c"));"#, "split() expects 1 or 2 arguments, got 3");
    assert_err(r#"print(join([1, "a"], ""));"#, "join() expects a list of strings, but element 0 is int");
    assert_err(r#"print(join("ab", ""));"#, "join() expects a list as its first argument");
}

#[test]
fn split_is_bounded() {
    // More pieces than a list may hold.
    let src = format!(r#"let s = repeat("a,", {}); let parts = split(s, ",");"#, MAX_LIST_LEN + 1);
    assert_err(&src, "more than 32768 elements");
}

#[test]
fn find_and_contains() {
    assert_eq!(eval(r#"find("hello", "l")"#), "2");
    assert_eq!(eval(r#"find("hello", "z")"#), "-1");
    assert_eq!(eval(r#"find("héllo", "l")"#), "3");
    assert_eq!(eval(r#"find("abc", "")"#), "0");
    assert_eq!(eval(r#"find([3, "x", [1]], [1])"#), "2");
    assert_eq!(eval(r#"find([1, 2], 3)"#), "-1");
    assert_eq!(eval(r#"contains("hello", "ell")"#), "true");
    assert_eq!(eval(r#"contains(["key", "lamp"], "lamp")"#), "true");
    assert_eq!(eval(r#"contains([], nil)"#), "false");
    assert_err(r#"print(find("abc", 1));"#, "find() expects a string for the text to look for, found int");
    assert_err(r#"print(contains(5, 5));"#, "contains() expects a string or a list to search, found int");
}

#[test]
fn replace_upper_lower_trim() {
    assert_eq!(eval(r#"replace("a-b-c", "-", "+")"#), "a+b+c");
    assert_eq!(eval(r#"replace("aaa", "a", "bb")"#), "bbbbbb");
    assert_eq!(eval(r#"replace("abc", "x", "y")"#), "abc");
    assert_err(r#"print(replace("abc", "", "y"));"#, "must not be empty");
    let src = format!(r#"let s = repeat("ab", {}); let t = replace(s, "a", "aaa");"#, MAX_STR_LEN / 2);
    assert_err(&src, "the limit is 262144");

    assert_eq!(eval(r#"upper("Hello, wörld")"#), "HELLO, WöRLD");
    assert_eq!(eval(r#"lower("Hello, WÖRLD")"#), "hello, wÖrld");
    assert_eq!(eval(r#"trim("  \t hi there \n ")"#), "hi there");
    assert_eq!(eval(r#"len(trim("   "))"#), "0");
}

#[test]
fn starts_and_ends_with() {
    assert_eq!(eval(r#"starts_with("go north", "go ")"#), "true");
    assert_eq!(eval(r#"starts_with("go", "go ")"#), "false");
    assert_eq!(eval(r#"ends_with("notes.wisp", ".wisp")"#), "true");
    assert_err(r#"print(ends_with("a", nil));"#, "ends_with() expects a string for the text to look for, found nil");
}

#[test]
fn chr_ord_and_chars() {
    assert_eq!(eval("chr(65)"), "A");
    assert_eq!(eval("chr(233)"), "é");
    assert_eq!(eval(r#"ord("A")"#), "65");
    assert_eq!(eval(r#"ord("🙂")"#), "128578");
    assert_eq!(eval(r#"ord(chr(1114111))"#), "1114111");
    assert_err("print(chr(55296));", "Unicode code point");
    assert_err("print(chr(-1));", "Unicode code point");
    assert_err("print(chr(1114112));", "Unicode code point");
    assert_err(r#"print(ord("ab"));"#, "exactly one character, found 2 bytes");
    assert_err(r#"print(ord(""));"#, "exactly one character, found 0 bytes");
    assert_eq!(eval(r#"chars("aé🙂")"#), r#"["a", "é", "🙂"]"#);
    assert_eq!(eval(r#"chars("")"#), "[]");
}

#[test]
fn repeat_is_bounded() {
    assert_eq!(eval(r#"repeat("ab", 3)"#), "ababab");
    assert_eq!(eval(r#"len(repeat("ab", 0))"#), "0");
    assert_err(r#"print(repeat("ab", -1));"#, "a count of 0 or more");
    assert_err(r#"print(repeat("ab", 9223372036854775807));"#, "the limit is 262144");
    // An empty string must not be "repeated" n times one by one.
    let started = std::time::Instant::now();
    assert_eq!(eval(r#"len(repeat("", 9223372036854775807))"#), "0");
    assert!(started.elapsed().as_secs() < 1, "repeat(\"\", n) looped n times");
}

#[test]
fn searching_a_list_honours_esc() {
    // Comparing two large lists can take MAX_EQ_WORK steps, so a search
    // over many of them checks for Esc between elements.
    let host = TestHost { stop_after: Some(0), ..Default::default() };
    let (r, _) = run_with("print(find(range(100), -1));", host);
    assert!(r.unwrap_err().message.contains("stopped"));
    let host = TestHost { stop_after: Some(5), ..Default::default() };
    let (r, _) = run_with("print(contains([1, 2, 3, 4, 5, 6, 7, 8], 9));", host);
    assert!(r.unwrap_err().message.contains("stopped"));
    // Searching a string involves no comparison of values; nothing to stop.
    let host = TestHost { stop_after: Some(0), ..Default::default() };
    let (r, h) = run_with(r#"print(find("abc", "c"));"#, host);
    r.unwrap();
    assert_eq!(h.out, "2\n");
}

#[test]
fn builtins_that_build_lists_respect_the_heap_reserve() {
    for src in [r#"let p = split("a b c");"#, r#"let p = chars("abc");"#] {
        let host = TestHost { low_memory: true, ..Default::default() };
        let (r, _) = run_with(src, host);
        assert_eq!(r.unwrap_err().message, OUT_OF_MEMORY_MESSAGE, "{src}");
    }
}

// ---- list builtins --------------------------------------------------------

#[test]
fn insert_and_remove() {
    assert_eq!(out("let a = [1, 3]; insert(a, 1, 2); insert(a, 3, 4); insert(a, 0, 0); print(a);"), "[0, 1, 2, 3, 4]\n");
    assert_eq!(out("let a = [1, 2, 3]; print(remove(a, 1)); print(a);"), "2\n[1, 3]\n");
    assert_err("let a = [1]; insert(a, 2, 0);", "insert(): index 2 out of bounds (length 1)");
    assert_err("let a = [1]; insert(a, -1, 0);", "out of bounds");
    assert_err("let a = [1]; remove(a, 1);", "remove(): index 1 out of bounds (length 1)");
    assert_err("let a = []; remove(a, 0);", "remove() called on an empty list");
    assert_err("insert(\"ab\", 0, 1);", "insert() expects a list as its first argument, found string");
    let src = format!("let a = range({}); insert(a, 0, 1);", MAX_LIST_LEN);
    assert_err(&src, "list is full");
}

#[test]
fn sort_and_reverse() {
    assert_eq!(out("let a = [3, -1, 2, 2]; sort(a); print(a);"), "[-1, 2, 2, 3]\n");
    assert_eq!(out(r#"let a = ["pear", "Apple", "fig"]; sort(a); print(a);"#), "[\"Apple\", \"fig\", \"pear\"]\n");
    assert_eq!(out("let a = []; sort(a); print(a);"), "[]\n");
    assert_eq!(out("let a = [1, 2, 3]; reverse(a); print(a);"), "[3, 2, 1]\n");
    assert_err(r#"sort([1, "a"]);"#, "element 0 is int and element 1 is string");
    assert_err("sort([nil]);", "but element 0 is nil");
    assert_err("sort([[1], 2]);", "but element 0 is list");
    assert_err(r#"sort(["a", "b", 3]);"#, "element 0 is string and element 2 is int");
}

#[test]
fn range_counts() {
    assert_eq!(eval("range(4)"), "[0, 1, 2, 3]");
    assert_eq!(eval("range(2, 5)"), "[2, 3, 4]");
    assert_eq!(eval("range(-2, 1)"), "[-2, -1, 0]");
    assert_eq!(eval("range(0)"), "[]");
    assert_eq!(eval("range(5, 2)"), "[]");
    assert_eq!(eval(&format!("len(range({MAX_LIST_LEN}))")), MAX_LIST_LEN.to_string());
    assert_err(&format!("print(range({}));", MAX_LIST_LEN + 1), "range() would make 32769 elements");
    assert_err("print(range(-9223372036854775807 - 1, 9223372036854775807));", "the limit is 32768");
    assert_err("print(range());", "range() expects 1 or 2 arguments, got 0");
}

// ---- numbers and types ----------------------------------------------------

#[test]
fn abs_min_max_type() {
    assert_eq!(eval("abs(-5)"), "5");
    assert_eq!(eval("abs(5)"), "5");
    assert_err("print(abs(-9223372036854775807 - 1));", "integer overflow");
    assert_eq!(eval("min(3, -2)"), "-2");
    assert_eq!(eval("max(3, -2)"), "3");
    assert_eq!(eval(r#"min("b", "a")"#), "a");
    assert_eq!(eval(r#"max("b", "a")"#), "b");
    assert_err("print(min(1, \"a\"));", "min() expects two ints or two strings, found int and string");
    for (v, t) in [("nil", "nil"), ("true", "bool"), ("1", "int"), ("\"s\"", "string"), ("[]", "list")] {
        assert_eq!(eval(&format!("type({v})")), t);
    }
}

#[test]
fn new_builtin_names_are_reserved_for_functions_only() {
    assert_err("fn split(x) { return x; }", "'split' is a built-in function and cannot be redefined");
    assert_err("fn type(x) { return x; }", "'type' is a built-in function");
    // Variables live in their own namespace, as before.
    assert_eq!(out("let type = 1; let range = 2; print(type + range);"), "3\n");
}

// ---- memory ---------------------------------------------------------------

#[test]
fn cycles_built_from_builtin_lists_are_reclaimed() {
    // Lists made by split(), range(), slices and `+` must be registered
    // with the session like literals are, or a cycle made from one leaks.
    let src = r#"
        let i = 0;
        while i < 300 {
            let a = split("x y z");
            push(a, a);
            let b = range(3);
            push(b, b);
            let c = b[:];
            push(c, c);
            let d = a + b;
            push(d, d);
            i = i + 1;
        }
    "#;
    // One warm-up run first, so one-time allocations are not counted.
    run_with(src, TestHost::default()).0.unwrap();
    let before = live_bytes();
    for _ in 0..3 {
        run_with(src, TestHost::default()).0.unwrap();
    }
    assert_eq!(live_bytes(), before, "cyclic lists from builtins leaked across runs");
}

// ---- interactive sessions -------------------------------------------------

#[test]
fn open_brackets_ignores_strings_and_comments() {
    assert_eq!(wisp::open_brackets("fn f(x) {"), 1);
    assert_eq!(wisp::open_brackets("fn f(x) {\n  if x {\n"), 2);
    assert_eq!(wisp::open_brackets("fn f(x) { return [x]; }"), 0);
    assert_eq!(wisp::open_brackets(r#"print("{[(");"#), 0);
    assert_eq!(wisp::open_brackets(r#"let s = "a \" {"; {"#), 1);
    assert_eq!(wisp::open_brackets("let a = 1; # { not code\n"), 0);
    assert_eq!(wisp::open_brackets(")) {"), 1);
    assert_eq!(wisp::open_brackets("}"), 0);
}

#[test]
fn echoed_values_quote_strings() {
    assert_eq!(Value::Str(std::rc::Rc::new("1".into())).to_echo_string(), "\"1\"");
    assert_eq!(Value::Int(1).to_echo_string(), "1");
    let mut s = Session::new();
    let mut h = TestHost::default();
    let v = piece(&mut s, &mut h, r#"split("a b")"#).unwrap().unwrap();
    assert_eq!(v.to_echo_string(), r#"["a", "b"]"#);
}

#[test]
fn the_documented_examples_run() {
    // docs/WISP_LANGUAGE.md, "Strings".
    let printed = out(r#"
        let s = "hello, world";
        print(s[0]);
        print(s[7:]);
        print(upper(s[:5]));
        for word in split("go north now") {
            print(word);
        }
        print(s[find(s, ","):]);
    "#);
    assert_eq!(printed, "h\nworld\nHELLO\ngo\nnorth\nnow\n, world\n");
    // docs/WISP_LANGUAGE.md, "The interactive prompt".
    let mut s = Session::new();
    let mut h = TestHost::default();
    let mut echo = |src: &str| piece(&mut s, &mut h, src).unwrap().map(|v| v.to_echo_string());
    assert_eq!(echo(r#"let names = split("ada grace alan")"#), None);
    assert_eq!(echo("sort(names)").as_deref(), Some("nil"));
    assert_eq!(echo("names").as_deref(), Some(r#"["ada", "alan", "grace"]"#));
    assert_eq!(echo(r#"fn shout(s) { return upper(s) + "!"; }"#), None);
    assert_eq!(echo("shout(names[0])").as_deref(), Some(r#""ADA!""#));
}

fn piece(session: &mut Session, host: &mut TestHost, src: &str) -> Result<Option<Value>, WispError> {
    let program = wisp::parse_interactive(src)?;
    session.run_interactive(host, &program)
}

fn show(v: Option<Value>) -> String {
    v.map_or("<none>".into(), |v| v.to_display_string())
}

#[test]
fn an_interactive_session_keeps_state_between_pieces() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    assert_eq!(show(piece(&mut s, &mut h, "let x = 20;").unwrap()), "<none>");
    assert_eq!(show(piece(&mut s, &mut h, "x + 1").unwrap()), "21");
    assert_eq!(show(piece(&mut s, &mut h, "fn double(n) { return n * 2; }").unwrap()), "<none>");
    assert_eq!(show(piece(&mut s, &mut h, "double(x)").unwrap()), "40");
    assert!(s.function_names().any(|n| n == "double"));
    assert_eq!(s.global_names().collect::<Vec<_>>(), ["x"]);
    assert_eq!(s.function_names().collect::<Vec<_>>(), ["double"]);
}

#[test]
fn an_interactive_session_allows_redefinition_across_pieces() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    piece(&mut s, &mut h, "let x = 1;").unwrap();
    piece(&mut s, &mut h, "let x = 2;").unwrap();
    assert_eq!(show(piece(&mut s, &mut h, "x").unwrap()), "2");
    piece(&mut s, &mut h, "fn f() { return 1; }").unwrap();
    piece(&mut s, &mut h, "fn f() { return 2; }").unwrap();
    assert_eq!(show(piece(&mut s, &mut h, "f()").unwrap()), "2");
    // Within one piece the usual rules still hold.
    let e = piece(&mut s, &mut h, "fn g() { return 1; } fn g() { return 2; }").unwrap_err();
    assert!(e.message.contains("already defined"), "{}", e.message);
    assert!(!s.function_names().any(|n| n == "g"), "a piece that failed to register must define nothing");
    let e = piece(&mut s, &mut h, "if true { let y = 1; let y = 2; }").unwrap_err();
    assert!(e.message.contains("already declared"), "{}", e.message);
}

#[test]
fn an_interactive_error_leaves_the_session_usable() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    piece(&mut s, &mut h, "let y = 1;").unwrap();
    let e = piece(&mut s, &mut h, "y = nope + 1;").unwrap_err();
    assert!(e.message.contains("undefined variable 'nope'"), "{}", e.message);
    assert_eq!(show(piece(&mut s, &mut h, "y").unwrap()), "1");
}

#[test]
fn interactive_pieces_may_omit_the_final_semicolon() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    assert_eq!(show(piece(&mut s, &mut h, "1 + 2").unwrap()), "3");
    piece(&mut s, &mut h, "let z = 4").unwrap();
    assert_eq!(show(piece(&mut s, &mut h, "z").unwrap()), "4");
    piece(&mut s, &mut h, r#"print("hi")"#).unwrap();
    assert_eq!(h.out, "hi\n");
    // Only a missing final `;` is forgiven: other errors are reported as
    // they are, at their own line.
    let e = wisp::parse_interactive("let = 3").unwrap_err();
    assert!(e.message.contains("expected identifier"), "{}", e.message);
    // Only the last statement's value is echoed, and only a bare expression's.
    assert_eq!(show(piece(&mut s, &mut h, "let q = 1; q + 1").unwrap()), "2");
    assert_eq!(show(piece(&mut s, &mut h, "q + 1; let r = 0;").unwrap()), "<none>");
}

#[test]
fn an_interactive_session_reclaims_its_lists_when_it_ends() {
    let before = live_bytes();
    {
        let mut s = Session::new();
        let mut h = TestHost::default();
        for _ in 0..50 {
            piece(&mut s, &mut h, "let a = [1, 2]; push(a, a);").unwrap();
        }
    }
    assert_eq!(live_bytes(), before, "an ended session left lists behind");
}

// ---- the bundled calc.wisp ------------------------------------------------

/// Drive `esp/APPS/calc.wisp` through `lines`, on a stack smaller than the
/// kernel's, and return its transcript. The program must end normally:
/// every bad expression is its own error message, never a WISP error.
fn calc_session(lines: &[&str]) -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../esp/APPS/calc.wisp")).unwrap();
    let inputs = lines.iter().map(|l| l.to_string()).collect();
    std::thread::Builder::new()
        .stack_size(640 * 1024)
        .spawn(move || {
            let (r, host) = run_with(&src, TestHost { inputs, ..Default::default() });
            if let Err(e) = r {
                panic!("calc.wisp ended with a WISP error: {e}\n--- transcript ---\n{}", host.out);
            }
            host.out
        })
        .unwrap()
        .join()
        .expect("calc.wisp overflowed the stack or panicked")
}

/// The answer calc printed for each input line, in order (the line after
/// each echoed `calc> ` prompt).
fn calc_answers(lines: &[&str]) -> Vec<String> {
    let transcript = calc_session(lines);
    let mut answers = Vec::new();
    let mut rows = transcript.lines().peekable();
    while let Some(row) = rows.next() {
        if row.starts_with("calc> ") && row != "calc> quit" {
            let typed = &row["calc> ".len()..];
            if typed.trim().is_empty() {
                answers.push(String::new());
            } else {
                answers.push(rows.next().unwrap_or("").to_string());
            }
        }
    }
    answers
}

#[test]
fn calc_evaluates_with_precedence() {
    let answers = calc_answers(&["2 + 3 * (4 - 1)", "-2 * 3", "2 - -3", "17 % 5", "100 / 7", "  42  ", "(((7)))", ""]);
    assert_eq!(answers, ["= 11", "= -6", "= 5", "= 2", "= 14", "= 42", "= 7", ""]);
}

#[test]
fn calc_reports_every_mistake_and_keeps_going() {
    let answers = calc_answers(&[
        "10 / (5 - 5)",
        "7 % 0",
        "(1 + 2",
        "1 +",
        "1 2",
        ")",
        "2 ^ 3",
        "9999999999999999999",
        "999999999999999999 * 999999999999999999",
        "999999999999999999 + 999999999999999999",
        "1 + 1",
    ]);
    assert_eq!(
        answers,
        [
            "error: division by zero",
            "error: division by zero",
            "error: a '(' is missing its ')'",
            "error: the expression ends too early",
            "error: unexpected '2'",
            "error: unexpected ')'",
            "I don't know what '^' means here.",
            "error: numbers may have at most 18 digits",
            "error: the result is too large (the limit is 1000000000000000000)",
            "error: the result is too large (the limit is 1000000000000000000)",
            "= 2",
        ]
    );
}

#[test]
fn calc_bounds_its_own_recursion() {
    let deepest = format!("{}1{}", "(".repeat(40), ")".repeat(40));
    let too_deep = format!("{}1{}", "(".repeat(41), ")".repeat(41));
    let minus_signs = format!("{}5", "-".repeat(300));
    let answers = calc_answers(&[&deepest, &too_deep, &minus_signs, "--5"]);
    assert_eq!(
        answers,
        ["= 1", "error: the expression nests too deeply", "error: the expression nests too deeply", "= 5"]
    );
}
