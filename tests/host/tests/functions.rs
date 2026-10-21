//! WISP functions as values: a function's name used as a value, calls
//! through variables and through any expression (`table[k](x)`), and the
//! higher-order built-ins `map`, `filter`, `reduce` and `sort_by`. Every
//! documented behaviour and every error is checked here against the real
//! interpreter (see `tests/host/Cargo.toml`), along with the resource
//! bounds the rest of the language has: Esc, native stack depth, and
//! reclamation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wisp_host_tests::ahci::Hba;
use wisp_host_tests::save_format;
use wisp_host_tests::wisp::interp::{Host, Session, STOPPED_MESSAGE};
use wisp_host_tests::wisp::value::Value;
use wisp_host_tests::wisp::{self, WispError};

// ---- per-thread live-allocation accounting (as in regressions.rs) --------

struct Counting;

thread_local! {
    static LIVE_BYTES: Cell<isize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LIVE_BYTES.try_with(|c| c.set(c.get() + layout.size() as isize));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE_BYTES.try_with(|c| c.set(c.get() - layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn live_bytes() -> isize {
    LIVE_BYTES.with(|c| c.get())
}

// ---- a host with a real (in-memory) save disk -----------------------------

const DISK_SECTORS: usize = 2048;

struct TestHost {
    out: String,
    /// Lines `input()` returns, in order; then "quit".
    inputs: std::collections::VecDeque<String>,
    /// `stop_requested` starts answering true after this many calls.
    stop_after: Option<usize>,
    stop_calls: usize,
    /// A provisioned save store: `save`/`load` go through the real
    /// `save_format` code, so a map is encoded to sectors and decoded back.
    disk: Hba,
}

impl Default for TestHost {
    fn default() -> Self {
        let disk = Hba::new(DISK_SECTORS);
        unsafe { save_format::provision(&disk, disk.total_sectors()) }.unwrap();
        Self { out: String::new(), inputs: Default::default(), stop_after: None, stop_calls: 0, disk }
    }
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
    fn save(&mut self, key: &str, value: &Value) -> Result<(), &'static str> {
        unsafe { save_format::save(&self.disk, key, value) }
    }
    fn load(&mut self, key: &str) -> Result<Option<Value>, &'static str> {
        unsafe { save_format::load(&self.disk, key) }
    }
    fn stop_requested(&mut self) -> bool {
        self.stop_calls += 1;
        self.stop_after.is_some_and(|n| self.stop_calls > n)
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

/// Run `src` and return what it printed.
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

/// Run `src`, which must fail, and return the error.
fn error(src: &str) -> WispError {
    match run_with(src, TestHost::default()).0 {
        Ok(()) => panic!("script unexpectedly succeeded:\n{src}"),
        Err(e) => e,
    }
}

fn assert_err(src: &str, expected_fragment: &str) {
    let e = error(src);
    assert!(
        e.message.contains(expected_fragment),
        "error for\n  {src}\nwas\n  {}\nexpected it to contain\n  {expected_fragment}",
        e.message
    );
}

/// Run `f` on a thread with a stack smaller than the kernel's 1 MiB
/// mainline stack (the same margin `regressions.rs` uses).
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(640 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .expect("thread overflowed its stack or panicked")
}

// ---- the examples in docs/WISP_LANGUAGE.md, as written ---------------------------

#[test]
fn the_documented_examples_run_as_written() {
    let printed = out(
        r#"
        fn double(x) { return x * 2; }
        fn twice(f, x) { return f(f(x)); }

        let f = double;
        print(f(21));            # 42
        print(twice(double, 5)); # 20
        print(f, type(f));       # <fn double> func

        let table = {"d": double, "len": len};
        print(table["d"](4));    # 8
        "#,
    );
    assert_eq!(printed, "42\n20\n<fn double> func\n8\n");
    let printed = out(
        r#"
        fn square(x) { return x * x; }
        fn is_even(x) { return x % 2 == 0; }
        fn add(a, b) { return a + b; }
        print(reduce(add, filter(is_even, map(square, range(6))), 0));   # 0+4+16 = 20
        "#,
    );
    assert_eq!(printed, "20\n");
}

// ---- a function's name is a value ---------------------------------------------

#[test]
fn a_function_name_is_a_value() {
    let printed = out(
        r#"
        fn double(x) { return x * 2; }
        let f = double;
        print(f(21), type(f), f);
        print(type(double), double);
        print(f == double, f != double, f == len, double == double);
        "#,
    );
    assert_eq!(printed, "42 func <fn double>\nfunc <fn double>\ntrue false false true\n");
}

#[test]
fn built_ins_are_values_too() {
    let printed = out(
        r#"
        let n = len;
        let up = upper;
        print(n("four"), up("abc"), type(n), n);
        let table = {"len": len, "str": str};
        print(table["str"](12) + "!", table["len"]([1, 2, 3]));
        "#,
    );
    assert_eq!(printed, "4 ABC func <fn len>\n12! 3\n");
}

#[test]
fn a_name_that_is_neither_variable_nor_function_is_still_undefined() {
    assert_err("print(nothing);", "undefined variable 'nothing'");
    assert_err("let f = nothing;", "undefined variable 'nothing'");
    assert_err("nothing(1);", "undefined function 'nothing'");
}

#[test]
fn a_variable_shadows_a_function_of_the_same_name_as_a_value() {
    // As a value, the nearest variable wins: this is how every program
    // written before functions were values already read `x`.
    let printed = out(
        r#"
        fn f() { return 1; }
        let g = f;
        let f = 5;
        print(f, g());
        "#,
    );
    assert_eq!(printed, "5 1\n");
}

#[test]
fn functions_are_passed_and_returned() {
    let printed = out(
        r#"
        fn double(x) { return x * 2; }
        fn inc(x) { return x + 1; }
        fn apply(f, x) { return f(x); }
        fn twice(f, x) { return f(f(x)); }
        fn pick(name) { if name == "d" { return double; } return inc; }
        print(apply(double, 4), apply(inc, 4), twice(double, 3), twice(inc, 3));
        print(pick("d")(10), pick("i")(10));
        "#,
    );
    assert_eq!(printed, "8 5 12 5\n20 11\n");
}

#[test]
fn a_parameter_holding_a_function_wins_over_a_function_of_that_name() {
    let printed = out(
        r#"
        fn f(x) { return "global f"; }
        fn g(x) { return "g"; }
        fn call_f(f, x) { return f(x); }
        print(call_f(g, 1), f(1));
        "#,
    );
    assert_eq!(printed, "g global f\n");
}

#[test]
fn a_variable_holding_a_non_function_does_not_hide_a_function() {
    // Written before functions were values, and still working.
    let printed = out(
        r#"
        fn count(x) { return x + 1; }
        let count = 10;
        fn bump() { let count = 0; return count(count); }
        print(count(1), bump());
        "#,
    );
    assert_eq!(printed, "2 1\n");
}

#[test]
fn calling_something_that_is_not_a_function() {
    assert_err("let x = 5; x(1);", "cannot call 'x': it holds int, not a function");
    assert_err("fn f(s) { return s(1); } f(\"text\");", "cannot call 's': it holds string, not a function");
    assert_err("[1, 2](0);", "cannot call list: only a function can be called");
    assert_err("let t = {\"a\": 1}; t[\"a\"](0);", "cannot call int: only a function can be called");
    assert_err("(3)(4);", "cannot call int: only a function can be called");
    // The error is reported where the call is.
    assert_eq!(error("\n\nlet x = 1;\nx();").line, 4);
}

#[test]
fn calls_on_any_expression() {
    let printed = out(
        r#"
        fn double(x) { return x * 2; }
        fn inc(x) { return x + 1; }
        fn compose() { return [double, inc]; }
        fn id(f) { return f; }
        let fs = [double, inc];
        print((double)(4), fs[0](5), fs[1](5), compose()[0](6), id(double)(7), id(id)(double)(8));
        print(fs[0](fs[1](1)) + fs[1](fs[0](1)));
        let m = {"go": {"deep": inc}};
        print(m["go"]["deep"](1));
        "#,
    );
    assert_eq!(printed, "8 10 6 12 14 16\n7\n2\n");
}

#[test]
fn calls_through_values_check_arity_and_name_the_function() {
    assert_err("fn f(a) { return a; } let g = f; g();", "'f' expects 1 argument(s), got 0");
    assert_err("fn f(a) { return a; } [f][0](1, 2);", "'f' expects 1 argument(s), got 2");
    assert_err("let g = len; g();", "len() expects 1 argument, got 0");
}

#[test]
fn a_function_redefined_at_the_prompt_is_the_one_a_stored_value_calls() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    let mut echo = |src: &str| piece(&mut s, &mut h, src).unwrap().map(|v| v.to_echo_string());
    assert_eq!(echo("fn f() { return 1; }"), None);
    assert_eq!(echo("let g = f"), None);
    assert_eq!(echo("let table = [f]"), None);
    assert_eq!(echo("g()").as_deref(), Some("1"));
    assert_eq!(echo("fn f() { return 2; }"), None);
    assert_eq!(echo("g()").as_deref(), Some("2"));
    assert_eq!(echo("table[0]()").as_deref(), Some("2"));
    // A function value echoes as itself.
    assert_eq!(echo("g").as_deref(), Some("<fn f>"));
    assert_eq!(echo("[g, len]").as_deref(), Some("[<fn f>, <fn len>]"));
}

// ---- function values in data ----------------------------------------------------

#[test]
fn function_values_in_lists_and_maps() {
    let printed = out(
        r#"
        fn a() { return "a"; }
        fn b() { return "b"; }
        let l = [a, b, a];
        let m = {"x": a, "y": b};
        print(l, m, l[0] == l[2], l[0] == l[1], l == [a, b, a]);
        for k in m { print(k + ":" + m[k]()); }
        print(contains(l, b), find(l, b), contains(l, len));
        "#,
    );
    assert_eq!(
        printed,
        "[<fn a>, <fn b>, <fn a>] {\"x\": <fn a>, \"y\": <fn b>} true false true\nx:a\ny:b\ntrue 1 false\n"
    );
}

#[test]
fn a_function_cannot_be_a_map_key_or_be_saved() {
    assert_err("let m = {len: 1};", "map keys must be ints or strings, found func");
    assert_err("let m = {}; m[len] = 1;", "map keys must be ints or strings, found func");
    assert_err("save(\"k\", len);", "a function cannot be saved");
    assert_err("fn f() { return 1; } save(\"k\", [1, [f]]);", "a function cannot be saved");
    assert_err("fn f() { return 1; } save(\"k\", {\"a\": f});", "a function cannot be saved");
    // A failed save leaves what was stored under the key as it was.
    let (r, host) = run_with("save(\"k\", 1); save(\"k\", [len]);", TestHost::default());
    assert!(r.is_err());
    let kept = unsafe { save_format::load(&host.disk, "k") }.unwrap();
    assert_eq!(kept.map(|v| v.to_display_string()).as_deref(), Some("1"));
}

#[test]
fn str_and_type_of_a_function() {
    assert_eq!(eval("str(len)"), "<fn len>");
    assert_eq!(eval("type(len)"), "func");
    assert_eq!(eval("\"it is \" + str(len)"), "it is <fn len>");
    assert_err("print(len + 1);", "func");
}

// ---- map ------------------------------------------------------------------------

#[test]
fn map_applies_a_function_to_every_element() {
    let printed = out(
        r#"
        fn sq(x) { return x * x; }
        let src = [1, 2, 3, 4];
        let out = map(sq, src);
        print(out, src);
        print(map(sq, []), map(str, [1, "a", nil, [2]]), map(len, ["", "ab", "abc"]));
        print(map(upper, split("a bc d")));
        "#,
    );
    assert_eq!(printed, "[1, 4, 9, 16] [1, 2, 3, 4]\n[] [\"1\", \"a\", \"nil\", \"[2]\"] [0, 2, 3]\n[\"A\", \"BC\", \"D\"]\n");
}

#[test]
fn map_makes_a_new_list_that_is_independent_of_the_source() {
    let printed = out(
        r#"
        fn same(x) { return x; }
        let a = [[1], [2]];
        let b = map(same, a);
        push(b, [3]);
        push(b[0], 99);
        print(a, b);
        "#,
    );
    // The new outer list is separate; the inner lists are shared, as with any copy of a handle.
    assert_eq!(printed, "[[1, 99], [2]] [[1, 99], [2], [3]]\n");
}

#[test]
fn map_runs_its_callback_in_order() {
    let printed = out(
        r#"
        fn show(x) { print("saw " + str(x)); return x; }
        map(show, [3, 1, 2]);
        "#,
    );
    assert_eq!(printed, "saw 3\nsaw 1\nsaw 2\n");
}

#[test]
fn map_filter_reduce_work_from_a_snapshot() {
    // A callback that grows the list cannot make the loop run on.
    let printed = out(
        r#"
        let l = [1, 2, 3];
        fn grow(x) { push(l, x); return x; }
        map(grow, l);
        print(l);
        let k = [1, 2];
        fn grow_keep(x) { push(k, x); return true; }
        print(filter(grow_keep, k), k);
        let r = [1, 2];
        fn grow_sum(acc, x) { push(r, x); return acc + x; }
        print(reduce(grow_sum, r, 0), r);
        "#,
    );
    assert_eq!(printed, "[1, 2, 3, 1, 2, 3]\n[1, 2] [1, 2, 1, 2]\n3 [1, 2, 1, 2]\n");
}

#[test]
fn map_argument_errors() {
    assert_err("map(1, [1]);", "map() expects a function as its first argument, found int");
    assert_err("map(len, \"abc\");", "map() expects a list as its second argument, found string");
    assert_err("map(len);", "map() expects 2 arguments, got 1");
    assert_err("map(len, [1], [2]);", "map() expects 2 arguments, got 3");
    // The callback's own arity errors surface as usual.
    assert_err("fn two(a, b) { return a; } map(two, [1]);", "'two' expects 2 argument(s), got 1");
    // And so do errors inside it, at their own line.
    let e = error("fn bad(x) {\n  return 1 / x;\n}\nmap(bad, [1, 0]);");
    assert!(e.message.contains("division by zero"), "{}", e.message);
    assert_eq!(e.line, 2);
}

// ---- filter ---------------------------------------------------------------------

#[test]
fn filter_keeps_what_the_function_accepts() {
    let printed = out(
        r#"
        fn even(x) { return x % 2 == 0; }
        fn nonempty(s) { return len(s) > 0; }
        let src = [1, 2, 3, 4, 5, 6];
        print(filter(even, src), src);
        print(filter(even, []), filter(even, [1, 3]));
        print(filter(nonempty, split("a,,b", ",")));
        "#,
    );
    assert_eq!(printed, "[2, 4, 6] [1, 2, 3, 4, 5, 6]\n[] []\n[\"a\", \"b\"]\n");
}

#[test]
fn filter_needs_a_bool() {
    assert_err("fn one(x) { return 1; } filter(one, [1]);", "filter() needs 'one' to return a bool, but it returned int");
    assert_err("fn none(x) { } filter(none, [1]);", "returned nil");
    assert_err("filter(len, [\"a\"]);", "filter() needs 'len' to return a bool");
    assert_err("filter(true, []);", "filter() expects a function as its first argument, found bool");
    assert_err("filter(len, 3);", "filter() expects a list as its second argument, found int");
}

// ---- reduce ---------------------------------------------------------------------

#[test]
fn reduce_folds_left_to_right() {
    let printed = out(
        r#"
        fn add(a, b) { return a + b; }
        fn cat(a, b) { return a + b; }
        fn sub(a, b) { return a - b; }
        print(reduce(add, [1, 2, 3, 4], 0), reduce(add, [], 7), reduce(cat, ["a", "b", "c"], ""));
        print(reduce(sub, [1, 2, 3], 10));
        fn longest(best, w) { if len(w) > len(best) { return w; } return best; }
        print(reduce(longest, split("a bbb cc"), ""));
        fn count(m, w) { m[w] = get(m, w, 0) + 1; return m; }
        print(reduce(count, split("x y x"), {}));
        "#,
    );
    assert_eq!(printed, "10 7 abc\n4\nbbb\n{\"x\": 2, \"y\": 1}\n");
}

#[test]
fn reduce_argument_errors() {
    assert_err("reduce(len, [1]);", "reduce() expects 3 arguments, got 2");
    assert_err("reduce(1, [1], 0);", "reduce() expects a function as its first argument, found int");
    assert_err("reduce(len, 1, 0);", "reduce() expects a list as its second argument, found int");
}

// ---- sort_by --------------------------------------------------------------------

#[test]
fn sort_by_orders_in_place_by_a_key_and_is_stable() {
    let printed = out(
        r#"
        fn first_char(w) { return w[0:1]; }
        fn size(w) { return len(w); }
        let words = ["pear", "fig", "apple", "kiwi", "plum", "date"];
        let r = sort_by(words, size);
        print(r, words);
        sort_by(words, first_char);
        print(words);
        fn neg(x) { return -x; }
        let n = [3, 1, 2];
        sort_by(n, neg);
        print(n);
        let empty = [];
        sort_by(empty, neg);
        print(empty);
        "#,
    );
    assert_eq!(
        printed,
        "nil [\"fig\", \"pear\", \"kiwi\", \"plum\", \"date\", \"apple\"]\n\
         [\"apple\", \"date\", \"fig\", \"kiwi\", \"pear\", \"plum\"]\n\
         [3, 2, 1]\n\
         []\n"
    );
}

#[test]
fn sort_by_runs_the_key_once_per_element() {
    let printed = out(
        r#"
        let calls = 0;
        fn key(x) { calls = calls + 1; return x; }
        let l = [5, 4, 3, 2, 1, 9, 8, 7];
        sort_by(l, key);
        print(calls, l);
        "#,
    );
    assert_eq!(printed, "8 [1, 2, 3, 4, 5, 7, 8, 9]\n");
}

#[test]
fn sort_by_sorts_maps_by_a_field() {
    let printed = out(
        r#"
        fn age(p) { return p["age"]; }
        fn name(p) { return p["name"]; }
        let people = [{"name": "Zed", "age": 30}, {"name": "Amy", "age": 25}, {"name": "Bo", "age": 30}];
        sort_by(people, age);
        print(map(name, people));
        sort_by(people, name);
        print(map(name, people));
        "#,
    );
    assert_eq!(printed, "[\"Amy\", \"Zed\", \"Bo\"]\n[\"Amy\", \"Bo\", \"Zed\"]\n");
}

#[test]
fn sort_by_errors() {
    assert_err("fn bad(x) { return [x]; } sort_by([1], bad);", "sort_by() needs 'bad' to return an int or a string, but it returned list");
    assert_err("fn nothing(x) { } sort_by([1], nothing);", "returned nil");
    assert_err(
        "fn mixed(x) { if x == 2 { return \"two\"; } return x; } sort_by([1, 2], mixed);",
        "sort_by() needs keys that are all ints or all strings, but 'mixed' returned int and string",
    );
    assert_err("sort_by([1]);", "sort_by() expects 2 arguments, got 1");
    assert_err("sort_by(3, len);", "sort_by() expects a list as its first argument, found int");
    assert_err("sort_by(len, [1]);", "sort_by() expects a function as its second argument, found list");
    assert_err("sort_by([1], 5);", "sort_by() expects a function as its second argument, found int");
}

#[test]
fn a_failed_sort_by_leaves_the_list_as_it_was() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    piece(&mut s, &mut h, "let l = [3, 2, 1];").unwrap();
    piece(&mut s, &mut h, "fn mixed(x) { if x == 1 { return \"one\"; } return x; }").unwrap();
    let e = piece(&mut s, &mut h, "sort_by(l, mixed);").unwrap_err();
    assert!(e.message.contains("all ints or all strings"), "{}", e.message);
    assert_eq!(piece(&mut s, &mut h, "l").unwrap().unwrap().to_echo_string(), "[3, 2, 1]");
    piece(&mut s, &mut h, "fn grow(x) { push(l, 0); return x; }").unwrap();
    assert!(piece(&mut s, &mut h, "sort_by(l, grow);").is_err());
    assert_eq!(piece(&mut s, &mut h, "l").unwrap().unwrap().to_echo_string(), "[3, 2, 1, 0, 0, 0]");
}

#[test]
fn sort_by_rejects_a_key_function_that_resizes_the_list() {
    assert_err(
        "let l = [3, 1, 2]; fn key(x) { push(l, 0); return x; } sort_by(l, key);",
        "'key' changed the size of the list being sorted",
    );
    assert_err(
        "let l = [3, 1, 2]; fn key(x) { pop(l); return x; } sort_by(l, key);",
        "changed the size of the list being sorted",
    );
}

#[test]
fn sort_by_is_stable_on_large_lists() {
    // Small slices are sorted by insertion sort, which is stable even when
    // the algorithm is not promised to be; this size is past that.
    let printed = out(
        r#"
        fn bucket(x) { return x % 3; }
        let l = range(300);
        sort_by(l, bucket);
        let broken = 0;
        for i in range(1, len(l)) {
            if bucket(l[i - 1]) == bucket(l[i]) and l[i - 1] > l[i] { broken = broken + 1; }
            if bucket(l[i - 1]) > bucket(l[i]) { broken = broken + 1000; }
        }
        print(broken, l[0], l[1], l[100], l[299]);
        "#,
    );
    assert_eq!(printed, "0 0 3 1 299\n");
}

#[test]
fn higher_order_calls_give_back_their_nesting_budget() {
    // MAX_NESTING is a budget for *simultaneous* depth: calls that have
    // returned must not count, however many a loop makes.
    let printed = out(
        r#"
        fn inc(x) { return x + 1; }
        fn yes(x) { return true; }
        fn add(a, b) { return a + b; }
        let i = 0;
        let total = 0;
        while i < 3000 {
            total = total + len(map(inc, [1])) + len(filter(yes, [1])) + reduce(add, [1], 0);
            sort_by([2, 1], inc);
            let f = inc;
            total = total + f(0) + [inc][0](0);
            i = i + 1;
        }
        print(total);
        "#,
    );
    assert_eq!(printed, "15000\n");
}

// ---- names ------------------------------------------------------------------------

#[test]
fn the_higher_order_names_are_reserved_for_functions_only() {
    assert_err("fn map(f, l) { return l; }", "'map' is a built-in function and cannot be redefined");
    assert_err("fn filter(f, l) { return l; }", "'filter' is a built-in function");
    assert_err("fn reduce(f, l, s) { return s; }", "'reduce' is a built-in function");
    assert_err("fn sort_by(l, f) { return l; }", "'sort_by' is a built-in function");
    // Variables live in their own namespace.
    assert_eq!(out("let map = 1; let filter = 2; let reduce = 3; let sort_by = 4; print(map + filter + reduce + sort_by);"), "10\n");
    // And a higher-order built-in is a value like any other.
    assert_eq!(out("let m = map; print(m(str, [1, 2]), type(m), m);"), "[\"1\", \"2\"] func <fn map>\n");
}

// ---- nesting ------------------------------------------------------------------------

#[test]
fn higher_order_built_ins_nest() {
    let printed = out(
        r#"
        fn inc(x) { return x + 1; }
        fn incs(l) { return map(inc, l); }
        fn big(x) { return x > 3; }
        fn add(a, b) { return a + b; }
        fn sum(l) { return reduce(add, l, 0); }
        print(map(incs, [[1, 2], [3, 4]]));
        print(map(sum, map(incs, [[1, 2], [3, 4]])));
        print(sum(filter(big, map(inc, range(6)))));
        "#,
    );
    assert_eq!(printed, "[[2, 3], [4, 5]]\n[5, 9]\n15\n");
}

#[test]
fn a_callback_can_be_a_higher_order_built_in() {
    // map(filter, ...) needs two arguments per call, so it is an arity error that names the built-in.
    assert_err("map(filter, [1]);", "filter() expects 2 arguments, got 1");
}

#[test]
fn recursion_through_higher_order_built_ins_ends_in_a_clean_error() {
    let src = r#"
        fn f(n) { return map(f, [n + 1]); }
        f(0);
    "#;
    let e = on_small_stack(move || {
        let (r, _) = run_with(src, TestHost::default());
        r.unwrap_err()
    });
    assert!(e.message.contains("stack overflow"), "{}", e.message);

    let src = r#"
        fn add(a, b) { return reduce(add, [a, b], 0); }
        add(1, 2);
    "#;
    let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("stack overflow"), "{}", e.message);

    let src = r#"
        fn k(x) { return sort_by([x, x], k); }
        k(1);
    "#;
    let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("stack overflow"), "{}", e.message);
}

#[test]
fn recursion_through_a_function_value_ends_in_a_clean_error() {
    let src = r#"
        fn f(g) { return g(g); }
        f(f);
    "#;
    let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("stack overflow"), "{}", e.message);
}

#[test]
fn a_very_long_chain_of_calls_is_a_clean_error() {
    // f(1)(1)(1)... -- each call in the chain nests one level deeper.
    let chain = "(1)".repeat(5000);
    let src = format!("fn f(x) {{ return f; }}\nf(1){chain};");
    let e = on_small_stack(move || run_with(&src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("too deeply") || e.message.contains("stack overflow"), "{}", e.message);
    // A chain well inside the limits works.
    let chain = "(1)".repeat(40);
    let printed = on_small_stack(move || {
        let src = format!("fn f(x) {{ return f; }}\nprint(f(1){chain} == f);");
        let (r, host) = run_with(&src, TestHost::default());
        r.unwrap();
        host.out
    });
    assert_eq!(printed, "true\n");
}

// ---- Esc and memory -------------------------------------------------------------------

#[test]
fn esc_stops_a_higher_order_call_even_with_a_built_in_callback() {
    // `upper` is not a WISP function, so no per-call check runs for it:
    // the loop has to make one itself.
    let src = "let big = repeat(\"a \", 20000); let words = split(big); map(upper, words);";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    let (r, _) = run_with(src, host);
    assert_eq!(r.unwrap_err().message, STOPPED_MESSAGE);

    let src = "let l = range(20000); fn t(x) { return true; } filter(t, l);";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);

    let src = "let l = range(20000); fn k(x) { return x; } sort_by(l, k);";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);

    let src = "let l = range(20000); fn a(x, y) { return x; } reduce(a, l, 0);";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);
}

#[test]
fn function_values_do_not_leak_and_do_not_pin_lists() {
    let src = r#"
        fn id(x) { return x; }
        let i = 0;
        while i < 200 {
            let cell = [id];
            cell[0] = [cell, id];
            let m = map(id, [cell, {"f": id}]);
            m[0][0] = m;
            sort_by(m, len);
            i = i + 1;
        }
    "#;
    run_with(src, TestHost::default()).0.unwrap();
    let before = live_bytes();
    for _ in 0..3 {
        run_with(src, TestHost::default()).0.unwrap();
    }
    assert_eq!(live_bytes(), before, "function values or the lists they ride in leaked");
}

// ---- piece helper for the prompt --------------------------------------------------------

fn piece(session: &mut Session, host: &mut TestHost, src: &str) -> Result<Option<Value>, WispError> {
    let program = wisp::parse_interactive(src)?;
    session.run_interactive(host, &program)
}

// ---- the bundled tasks.wisp ----------------------------------------------------------------

fn tasks_session(lines: &[&str]) -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../esp/APPS/tasks.wisp")).unwrap();
    let inputs = lines.iter().map(|l| l.to_string()).collect();
    on_small_stack(move || {
        let (r, host) = run_with(&src, TestHost { inputs, ..Default::default() });
        if let Err(e) = r {
            panic!("tasks.wisp ended with a WISP error: {e}\n--- transcript ---\n{}", host.out);
        }
        assert!(host.out.ends_with("tasks done\n"), "{}", host.out);
        host.out
    })
}

#[test]
fn tasks_adds_lists_and_finishes() {
    let t = tasks_session(&[
        "add 3 write the report",
        "add 1 fix the build",
        "add 2 call mum",
        "list",
        "done 2",
        "open",
        "stats",
        "quit",
    ]);
    assert!(t.contains("tasks> add 3 write the report\nAdded task 1.\n"), "{t}");
    assert!(t.contains("tasks> add 2 call mum\nAdded task 3.\n"), "{t}");
    assert!(
        t.contains(
            "tasks> list\n1. [ ] p3 write the report\n2. [ ] p1 fix the build\n3. [ ] p2 call mum\n"
        ),
        "{t}"
    );
    assert!(t.contains("tasks> done 2\nDone: fix the build\n"), "{t}");
    assert!(t.contains("tasks> open\n[ ] p3 write the report\n[ ] p2 call mum\ntasks> stats\n"), "{t}");
    assert!(t.contains("3 task(s), 2 open, open priority load 5.\n"), "{t}");
}

#[test]
fn tasks_sorts_stably_and_clears_finished() {
    let t = tasks_session(&[
        "add 2 b-two",
        "add 1 a-one",
        "add 2 c-two",
        "add 1 d-one",
        "sort",
        "list",
        "done 1",
        "done 3",
        "clear",
        "list",
        "stats",
        "quit",
    ]);
    assert!(t.contains("Sorted by priority; equal priorities keep their order.\n"), "{t}");
    assert!(
        t.contains("tasks> list\n1. [ ] p1 a-one\n2. [ ] p1 d-one\n3. [ ] p2 b-two\n4. [ ] p2 c-two\n"),
        "{t}"
    );
    assert!(t.contains("tasks> clear\nCleared 2 finished task(s).\n"), "{t}");
    assert!(t.contains("tasks> list\n1. [ ] p1 d-one\n2. [ ] p2 c-two\n"), "{t}");
    assert!(t.contains("2 task(s), 2 open, open priority load 3.\n"), "{t}");
}

#[test]
fn tasks_survives_bad_input() {
    let t = tasks_session(&[
        "",
        "   ",
        "frobnicate now",
        "add",
        "add x something",
        "add 0 something",
        "add 10 something",
        "add 5",
        "done 1",
        "list",
        "open",
        "stats",
        "add 4 real",
        "done 0",
        "done 2",
        "done x",
        "done 1 2",
        "done 99999",
        "HELP",
        "ADD 9 shouting",
        "quit",
    ]);
    assert!(t.contains("tasks> frobnicate now\nUnknown command: frobnicate. Type help.\n"), "{t}");
    assert!(t.matches("Usage: add PRIORITY TEXT").count() == 5, "{t}");
    assert!(t.contains("tasks> done 1\nUsage: done N   (N is a task number from 'list', 1 to 0)\n"), "{t}");
    assert!(t.contains("tasks> list\nNo tasks.\n"), "{t}");
    assert!(t.contains("tasks> open\nNothing left to do.\n"), "{t}");
    assert!(t.contains("0 task(s), 0 open, open priority load 0.\n"), "{t}");
    assert!(t.matches("Usage: done N").count() == 6, "{t}");
    assert!(t.contains("quit                leave\n"), "{t}");
    assert!(t.contains("tasks> ADD 9 shouting\nAdded task 2.\n"), "{t}");
}
