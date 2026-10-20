//! WISP maps: `{key: value}` literals, `map[key]` reads and writes, `for`
//! over a map's keys, the map builtins (`keys`, `values`, `get`, and the
//! map forms of `len`, `contains` and `remove`), and maps on the save disk.
//! Every documented behaviour and every error a map can raise is checked
//! here against the real interpreter and save-format sources (see
//! `tests/host/Cargo.toml`), along with the resource bounds lists already
//! have: cycle reclamation, iterative drop, and bounded display, equality
//! and encoding.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wisp_host_tests::ahci::{Hba, SECTOR_SIZE};
use wisp_host_tests::save_format;
use wisp_host_tests::wisp::interp::{Host, Session, STOPPED_MESSAGE};
use wisp_host_tests::wisp::value::{Value, MAX_DISPLAY_LEN, MAX_MAP_LEN, TRUNCATION_MARKER};
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

// ---- literals, display and types ------------------------------------------

#[test]
fn literals_display_in_key_order() {
    assert_eq!(eval("{}"), "{}");
    assert_eq!(eval(r#"{"a": 1}"#), r#"{"a": 1}"#);
    // Ints before strings; ints numerically, strings by byte -- whatever
    // order the literal was written in.
    assert_eq!(
        eval(r#"{"b": 2, "a": 1, 10: "ten", -3: "minus three", 2: "two", "B": 0}"#),
        r#"{-3: "minus three", 2: "two", 10: "ten", "B": 0, "a": 1, "b": 2}"#
    );
    assert_eq!(eval(r#"{"l": [1, {"k": nil}], "t": true}"#), r#"{"l": [1, {"k": nil}], "t": true}"#);
    assert_eq!(eval(r#"[{"x": "y"}]"#), r#"[{"x": "y"}]"#);
    // A top-level string prints bare; inside a map, keys and values are quoted.
    assert_eq!(out(r#"print({"k": "v"}["k"]);"#), "v\n");
}

#[test]
fn literal_keys_and_values_are_evaluated_in_source_order() {
    let printed = out(
        r#"
        fn note(tag, v) { print(tag); return v; }
        let m = {note("k1", "a"): note("v1", 1), note("k2", "b"): note("v2", 2)};
        print(m);
        "#,
    );
    assert_eq!(printed, "k1\nv1\nk2\nv2\n{\"a\": 1, \"b\": 2}\n");
}

#[test]
fn every_literal_run_builds_a_new_map() {
    let printed = out(
        r#"
        let all = [];
        for i in range(3) {
            let m = {"i": i};
            push(all, m);
        }
        all[0]["i"] = 99;
        print(all);
        "#,
    );
    assert_eq!(printed, "[{\"i\": 99}, {\"i\": 1}, {\"i\": 2}]\n");
}

#[test]
fn type_len_and_str() {
    assert_eq!(eval("type({})"), "map");
    assert_eq!(eval(r#"len({})"#), "0");
    assert_eq!(eval(r#"len({"a": 1, "b": 2, 3: 3})"#), "3");
    assert_eq!(eval(r#"str({1: "x"}) == "{1: \"x\"}""#), "true");
    assert_err("print(len(5));", "len() expects a string, list or map, found int");
}

#[test]
fn literal_errors() {
    assert_err(r#"let m = {[1]: 2};"#, "map keys must be ints or strings, found list");
    assert_err(r#"let m = {nil: 2};"#, "map keys must be ints or strings, found nil");
    assert_err(r#"let m = {{}: 2};"#, "map keys must be ints or strings, found map");
    assert_err(r#"let m = {true: 2};"#, "map keys must be ints or strings, found bool");
    assert_err(r#"let m = {"a": 1, "b": 2, "a": 3};"#, r#"map literal has the key "a" twice"#);
    assert_err(r#"let m = {1: 1, 2 - 1: 2};"#, "map literal has the key 1 twice");
    // Parse errors.
    assert_err(r#"let m = {"a" 1};"#, "expected ':' after a map key, found integer '1'");
    assert_err(r#"let m = {"a": 1,};"#, "unexpected '}'");
    assert_err(r#"let m = {"a": 1"#, "expected '}', found end of input");
    // Distinct keys: the int 1 and the string "1" are different keys.
    assert_eq!(eval(r#"{1: "int", "1": "string"}"#), r#"{1: "int", "1": "string"}"#);
}

#[test]
fn long_string_keys_are_cut_short_in_messages() {
    let key = "k".repeat(100);
    let e = error(&format!(r#"let m = {{}}; print(m["{key}"]);"#));
    assert!(e.message.contains(&format!("key \"{}...\" is not in the map", "k".repeat(64))), "{}", e.message);
}

// ---- reading and writing --------------------------------------------------

#[test]
fn index_reads_writes_and_adds() {
    let printed = out(
        r#"
        let m = {"gold": 5};
        print(m["gold"]);
        m["gold"] = m["gold"] + 10;
        m["gems"] = 1;
        m[7] = "seven";
        print(m);
        print(len(m));
        "#,
    );
    assert_eq!(printed, "5\n{7: \"seven\", \"gems\": 1, \"gold\": 15}\n3\n");
}

#[test]
fn nested_reads_and_writes_chain() {
    let printed = out(
        r#"
        let world = {"rooms": [{"name": "hall", "exits": {"n": 1}}, {"name": "cave", "exits": {}}]};
        world["rooms"][1]["exits"]["s"] = 0;
        world["rooms"][0]["name"] = "great hall";
        print(world["rooms"][0]["name"], world["rooms"][1]["exits"]);
        let grid = [{"x": 0}];
        grid[0]["x"] = 4;
        print(grid);
        "#,
    );
    assert_eq!(printed, "great hall {\"s\": 0}\n[{\"x\": 4}]\n");
}

#[test]
fn index_errors_are_precise() {
    assert_err(r#"let m = {"a": 1}; print(m["b"]);"#, r#"key "b" is not in the map (check with contains(), or read it with get())"#);
    assert_err(r#"let m = {"a": 1}; print(m[1]);"#, "key 1 is not in the map");
    assert_err(r#"let m = {}; print(m[[1]]);"#, "map keys must be ints or strings, found list");
    assert_err(r#"let m = {}; m[nil] = 1;"#, "map keys must be ints or strings, found nil");
    assert_err(r#"let m = {}; print(m[0:1]);"#, "cannot slice map (only lists and strings can be sliced)");
    assert_err(r#"let m = {}; m[0:1] = 1;"#, "cannot assign to a slice");
    assert_err("let x = 5; print(x[0]);", "cannot index into int (only lists, maps and strings can be indexed)");
    // The error is reported at the line of the read.
    let e = error("let m = {};\n\nprint(m[\"x\"]);");
    assert_eq!(e.line, 3);
}

#[test]
fn maps_are_shared_not_copied() {
    let printed = out(
        r#"
        fn add_gold(inv, n) { inv["gold"] = get(inv, "gold", 0) + n; }
        let a = {};
        let b = a;
        add_gold(b, 3);
        add_gold(a, 4);
        print(a, b);
        "#,
    );
    assert_eq!(printed, "{\"gold\": 7} {\"gold\": 7}\n");
}

#[test]
fn operators_on_maps() {
    assert_err("print({} < {});", "comparison expects two ints or two strings, found map and map");
    assert_err("print({} + {});", "'+' expects two ints, two strings or two lists, found map and map");
    assert_err("if {} { }", "expected bool here, found map ({})");
    assert_err("print(not {});", "'not' expects bool, found map");
    assert_err("print(-{});", "unary '-' expects int, found map");
}

// ---- equality -------------------------------------------------------------

#[test]
fn equality_is_structural() {
    assert_eq!(eval(r#"{"a": 1, "b": [2]} == {"b": [2], "a": 1}"#), "true");
    assert_eq!(eval(r#"{"a": 1} == {"a": 2}"#), "false");
    assert_eq!(eval(r#"{"a": 1} == {"b": 1}"#), "false");
    assert_eq!(eval(r#"{"a": 1} == {"a": 1, "b": 1}"#), "false");
    assert_eq!(eval(r#"{1: "x"} == {"1": "x"}"#), "false");
    assert_eq!(eval("{} == []"), "false");
    assert_eq!(eval("{} != nil"), "true");
    assert_eq!(eval("{} == {}"), "true");
}

#[test]
fn cyclic_maps_compare_by_structure() {
    let printed = out(
        r#"
        let a = {}; a["me"] = a;
        let b = {}; b["me"] = b;
        let c = {}; c["me"] = c; c["x"] = 1;
        print(a == b, a == c);
        "#,
    );
    assert_eq!(printed, "true false\n");
}

#[test]
fn equality_of_exponential_map_graphs_is_bounded() {
    // Equal shared graphs compare in time linear in their size; different
    // ones give up with an error rather than walking 2^22 paths.
    let src = r#"
        let a = {"v": 0};
        let b = {"v": 0};
        let i = 0;
        while i < 22 {
            a = {"l": a, "r": a};
            b = {"l": b, "r": b};
            i = i + 1;
        }
        print(a == b);
    "#;
    assert_eq!(out(src), "true\n");
}

// ---- for loops --------------------------------------------------------------

#[test]
fn for_loops_visit_keys_in_order() {
    let printed = out(
        r#"
        let m = {"b": 2, "a": 1, 3: "c"};
        for k in m {
            print(k, m[k]);
        }
        for k in {} {
            print("never");
        }
        "#,
    );
    assert_eq!(printed, "3 c\na 1\nb 2\n");
}

#[test]
fn for_loops_over_a_map_iterate_a_snapshot_of_its_keys() {
    let printed = out(
        r#"
        let m = {"a": 1, "b": 2, "c": 3};
        for k in m {
            m[k + k] = 0;
            if k == "a" { remove(m, "c"); }
            print(k);
        }
        print(keys(m));
        "#,
    );
    // "c" is still visited (the keys were copied when the loop started),
    // and the keys added inside the loop are not.
    assert_eq!(printed, "a\nb\nc\n[\"a\", \"aa\", \"b\", \"bb\", \"cc\"]\n");
}

#[test]
fn for_loops_over_a_map_support_break_continue_and_return() {
    let printed = out(
        r#"
        fn first_over(m, n) {
            for k in m {
                if m[k] > n { return k; }
            }
            return nil;
        }
        let m = {"a": 1, "b": 5, "c": 9};
        print(first_over(m, 4), first_over(m, 10));
        for k in m {
            if k == "a" { continue; }
            if k == "c" { break; }
            print(k);
        }
        "#,
    );
    assert_eq!(printed, "b nil\nb\n");
}

#[test]
fn for_loops_over_a_map_stop_on_esc() {
    let host = TestHost { stop_after: Some(5), ..Default::default() };
    let (r, _) = run_with(r#"let m = {}; for i in range(100) { m[i] = i; } while true { for k in m { } }"#, host);
    assert_eq!(r.unwrap_err().message, STOPPED_MESSAGE);
}

// ---- builtins ----------------------------------------------------------------

#[test]
fn keys_and_values_are_in_key_order() {
    assert_eq!(eval(r#"keys({"b": 1, "a": 2, 5: 3})"#), r#"[5, "a", "b"]"#);
    assert_eq!(eval(r#"values({"b": 1, "a": 2, 5: 3})"#), "[3, 2, 1]");
    assert_eq!(eval("keys({})"), "[]");
    // They are new lists: changing one does not change the map.
    assert_eq!(out(r#"let m = {"a": 1}; let k = keys(m); push(k, "z"); print(m, k);"#), "{\"a\": 1} [\"a\", \"z\"]\n");
    // But values are the map's own values, shared like any copy.
    assert_eq!(out(r#"let m = {"l": []}; push(values(m)[0], 1); print(m);"#), "{\"l\": [1]}\n");
    assert_err("print(keys([]));", "keys() expects a map as its first argument, found list");
    assert_err("print(values(1));", "values() expects a map as its first argument, found int");
    assert_err("print(keys());", "keys() expects 1 argument, got 0");
}

#[test]
fn get_reads_a_key_that_may_be_missing() {
    assert_eq!(eval(r#"get({"a": 1}, "a")"#), "1");
    assert_eq!(eval(r#"get({"a": 1}, "b")"#), "nil");
    assert_eq!(eval(r#"get({"a": 1}, "b", 0)"#), "0");
    assert_eq!(eval(r#"get({"a": 1}, "a", 0)"#), "1");
    assert_eq!(eval(r#"get({"a": nil}, "a", 0)"#), "nil");
    assert_err(r#"print(get({}));"#, "get() expects 2 or 3 arguments, got 1");
    assert_err(r#"print(get({}, 1, 2, 3));"#, "get() expects 2 or 3 arguments, got 4");
    assert_err(r#"print(get([], 1));"#, "get() expects a map as its first argument, found list");
    assert_err(r#"print(get({}, [1]));"#, "get() expects an int or string key, found list");
}

#[test]
fn contains_tests_for_a_key() {
    assert_eq!(eval(r#"contains({"a": 1}, "a")"#), "true");
    assert_eq!(eval(r#"contains({"a": 1}, 1)"#), "false");
    // Keys, not values.
    assert_eq!(eval(r#"contains({"a": "b"}, "b")"#), "false");
    assert_err(r#"print(contains({}, nil));"#, "contains() expects an int or string key, found nil");
    // The list and string forms are unchanged.
    assert_eq!(eval(r#"contains([1, 2], 2)"#), "true");
    assert_eq!(eval(r#"contains("abc", "b")"#), "true");
}

#[test]
fn remove_takes_an_entry_out() {
    assert_eq!(out(r#"let m = {"a": 1, "b": 2}; print(remove(m, "a"), m, len(m));"#), "1 {\"b\": 2} 1\n");
    assert_err(r#"let m = {"a": 1}; remove(m, "z");"#, r#"remove(): key "z" is not in the map"#);
    assert_err(r#"let m = {}; remove(m, [1]);"#, "remove() expects an int or string key, found list");
    // The list form is unchanged.
    assert_eq!(out("let l = [5, 6]; print(remove(l, 0), l);"), "5 [6]\n");
}

#[test]
fn map_builtin_names_are_reserved_for_functions_only() {
    assert_err("fn keys(m) { return m; }", "'keys' is a built-in function and cannot be redefined");
    assert_err("fn values(m) { return m; }", "'values' is a built-in function");
    assert_err("fn get(m, k) { return m; }", "'get' is a built-in function");
    // Variables live in their own namespace.
    assert_eq!(out("let keys = 1; let values = 2; let get = 3; print(keys + values + get);"), "6\n");
}

// ---- blocks are not maps ----------------------------------------------------

#[test]
fn braces_after_a_condition_are_still_blocks() {
    let printed = out(
        r#"
        let m = {};
        if m == {} { print("empty"); }
        while len(m) < 3 { m[len(m)] = 0; }
        for k in {"x": 1} { print(k); }
        if contains(m, 2) { print(m); } elif true { print("no"); } else { print("no"); }
        fn f() { return {"r": 1}; }
        print(f()["r"]);
        "#,
    );
    assert_eq!(printed, "empty\nx\n{0: 0, 1: 0, 2: 0}\n1\n");
}

// ---- limits -------------------------------------------------------------------

#[test]
fn a_map_holds_at_most_max_map_len_entries() {
    let src = format!(
        r#"
        let m = {{}};
        for i in range({MAX_MAP_LEN}) {{ m[i] = 0; }}
        m[0] = "an existing key may still be replaced";
        print(len(m), m[0]);
        m[-1] = 0;
        "#
    );
    let (r, host) = run_with(&src, TestHost::default());
    assert_eq!(host.out, format!("{MAX_MAP_LEN} an existing key may still be replaced\n"));
    assert_eq!(r.unwrap_err().message, format!("map is full ({MAX_MAP_LEN} entries is the limit)"));
}

#[test]
fn display_of_a_cyclic_map_stops_at_the_depth_limit() {
    let printed = out(r#"let m = {}; m["me"] = m; print(m);"#);
    assert!(printed.starts_with(r#"{"me": {"me": "#), "{printed}");
    // 64 levels shown in full, then the cut-off marker, then 64 closers.
    assert!(printed.trim_end().ends_with(&format!("{{...}}{}", "}".repeat(64))), "{printed}");
    assert_eq!(printed.matches('{').count(), 65, "{printed}");
}

#[test]
fn display_of_an_exponential_map_graph_is_bounded() {
    let src = r#"
        let v = {"leaf": 0};
        let i = 0;
        while i < 22 { v = {"a": v, "b": v}; i = i + 1; }
    "#;
    let printed = out(&format!("{src} print(v);"));
    assert!(printed.len() <= MAX_DISPLAY_LEN + TRUNCATION_MARKER.len() + 1);
    assert!(printed.trim_end().ends_with(TRUNCATION_MARKER));
    assert_err(&format!("{src} let s = str(v);"), "str(): the result would be longer than");
}

// ---- memory -------------------------------------------------------------------

#[test]
fn cycles_through_maps_are_reclaimed() {
    let src = r#"
        let i = 0;
        while i < 300 {
            let m = {"n": i};
            m["me"] = m;
            let l = [m];
            m["list"] = l;
            let k = keys(m);
            push(k, k);
            let v = values(m);
            push(v, m);
            m["v"] = v;
            let lit = {"inner": {}};
            lit["inner"]["outer"] = lit;
            i = i + 1;
        }
    "#;
    // One warm-up run first, so one-time allocations are not counted.
    run_with(src, TestHost::default()).0.unwrap();
    let before = live_bytes();
    for _ in 0..3 {
        run_with(src, TestHost::default()).0.unwrap();
    }
    assert_eq!(live_bytes(), before, "cyclic maps leaked across runs");
}

#[test]
fn an_interactive_session_reclaims_its_maps_when_it_ends() {
    let before = live_bytes();
    {
        let mut s = Session::new();
        let mut h = TestHost::default();
        for _ in 0..50 {
            piece(&mut s, &mut h, r#"let m = {"a": [1]}; m["m"] = m; m["a"][0] = m;"#).unwrap();
        }
    }
    assert_eq!(live_bytes(), before, "an ended session left maps behind");
}

#[test]
fn deep_map_chains_drop_without_recursion() {
    // 100,000 levels would overflow any stack if dropping recursed once
    // per level. Two shapes, because each container's `Drop` unwinds the
    // other's: a chain of maps alone exercises `MapCell`'s own `Drop`, and
    // maps alternating with lists exercise the hand-off between the two.
    on_small_stack(|| {
        for step in [r#"m = {"next": m};"#, r#"m = {"next": [m]};"#] {
            let src = format!(
                r#"
                let m = {{}};
                let i = 0;
                while i < 100000 {{
                    {step}
                    i = i + 1;
                }}
                print("built");
                "#
            );
            assert_eq!(out(&src), "built\n", "{step}");
        }
    });
}

#[test]
fn deeply_nested_map_literals_stay_within_the_stack() {
    on_small_stack(|| {
        // 127 levels, plus one for the `let`'s own expression: exactly the
        // parser's 128-level limit, so it parses and evaluates. Display
        // and equality are bounded at 64 levels of their own, as for lists.
        let deep = format!("{}1{}", "{\"k\": ".repeat(127), "}".repeat(127));
        let printed = out(&format!("let a = {deep}; print(a);"));
        assert!(printed.starts_with("{\"k\": {\"k\": "), "{printed}");
        assert!(printed.contains("{...}"), "{printed}");
        let e = error(&format!("let a = {deep}; let b = {deep}; print(a == b);"));
        assert_eq!(e.message, "maps are nested too deeply to compare (over 64 levels)");
        // One more is a clean parse error, not a crash.
        let too_deep = format!("{}1{}", "{\"k\": ".repeat(200), "}".repeat(200));
        let e = error(&format!("let m = {too_deep};"));
        assert!(e.message.contains("nested too deeply"), "{}", e.message);
    });
}

// ---- the save disk ------------------------------------------------------------

#[test]
fn maps_round_trip_through_the_save_disk() {
    let printed = out(
        r#"
        let m = {"name": "ada", "hp": 12, -4: [1, {"deep": true}], "none": nil, "é": "ü"};
        save("player", m);
        let back = load("player");
        print(back);
        print(back == m, type(back), back["hp"]);
        save("empty", {});
        print(load("empty"), len(load("empty")));
        "#,
    );
    assert_eq!(
        printed,
        "{-4: [1, {\"deep\": true}], \"hp\": 12, \"name\": \"ada\", \"none\": nil, \"é\": \"ü\"}\n\
         true map 12\n\
         {} 0\n"
    );
}

#[test]
fn a_loaded_map_is_a_new_mutable_map() {
    let printed = out(
        r#"
        let m = {"n": 1};
        save("k", m);
        let a = load("k");
        a["n"] = 2;
        let b = load("k");
        print(m, a, b);
        "#,
    );
    assert_eq!(printed, "{\"n\": 1} {\"n\": 2} {\"n\": 1}\n");
}

#[test]
fn cycles_built_from_loaded_maps_are_reclaimed() {
    // A map that arrives from the disk must join the session's registry
    // (`ObjectRegistry::adopt`), or a cycle built out of it leaks.
    let src = r#"
        save("k", {"inner": {"x": [1]}});
        let i = 0;
        while i < 100 {
            let m = load("k");
            m["inner"]["self"] = m;
            m["inner"]["x"][0] = m["inner"];
            i = i + 1;
        }
    "#;
    run_with(src, TestHost::default()).0.unwrap();
    let before = live_bytes();
    for _ in 0..3 {
        run_with(src, TestHost::default()).0.unwrap();
    }
    assert_eq!(live_bytes(), before, "cyclic maps built from load() leaked");
}

#[test]
fn saving_a_cyclic_or_oversized_map_is_an_error() {
    assert_err(r#"let m = {}; m["me"] = m; save("k", m);"#, "save(): save_format: value nesting is too deep to serialize");
    assert_err(
        r#"let m = {}; for i in range(1000) { m[i] = i; } save("k", m);"#,
        "save(): save_format: serialized value is too large to fit in one slot",
    );
}

#[test]
fn the_map_encoding_is_as_documented() {
    // docs/SAVE_FORMAT.md, "Value encoding": tag 5, a u32 entry count, then
    // each key and its value, in key order.
    let v = load_value_from(&encode_map_by_hand(&[(int(-1), int(7)), (st("a"), Some(vec![0]))]));
    assert_eq!(v.unwrap().unwrap().to_display_string(), r#"{-1: 7, "a": nil}"#);
    // And the real encoder writes exactly those bytes.
    let (r, host) = run_with(r#"save("k", {"a": nil, -1: 7});"#, TestHost::default());
    r.unwrap();
    let slot = read_slot(&host.disk, "k").expect("the key was saved");
    assert_eq!(slot, encode_map_by_hand(&[(int(-1), int(7)), (st("a"), Some(vec![0]))]));
}

#[test]
fn corrupt_map_encodings_are_rejected() {
    let cases: Vec<(Vec<u8>, &str)> = vec![
        // Keys out of order.
        (encode_map_by_hand(&[(st("b"), int(1)), (st("a"), int(2))]), "out of order"),
        // The same key twice.
        (encode_map_by_hand(&[(int(1), int(1)), (int(1), int(2))]), "out of order"),
        // A key that is not an int or a string.
        (encode_map_by_hand(&[(Some(vec![0]), int(1))]), "not an int or a string"),
        (encode_map_by_hand(&[(Some(vec![4, 0, 0, 0, 0]), int(1))]), "not an int or a string"),
        // An entry count no slot could hold.
        (vec![5, 0xff, 0xff, 0xff, 0xff], "implausibly large"),
        // Fewer entries than claimed.
        ([&[5u8, 2, 0, 0, 0][..], &int(1).unwrap()[..], &int(1).unwrap()[..]].concat(), "truncated"),
        // A key with no value.
        ([&[5u8, 1, 0, 0, 0][..], &int(1).unwrap()[..]].concat(), "truncated"),
        // A missing count.
        (vec![5, 1, 0], "missing map length"),
    ];
    for (bytes, expected) in cases {
        let e = load_value_from(&bytes).expect_err("a corrupt map must not load");
        assert!(e.contains(expected), "{bytes:?}: {e}");
    }
}

// ---- interactive sessions -----------------------------------------------------

fn piece(session: &mut Session, host: &mut TestHost, src: &str) -> Result<Option<Value>, WispError> {
    let program = wisp::parse_interactive(src)?;
    session.run_interactive(host, &program)
}

#[test]
fn maps_at_the_interactive_prompt() {
    let mut s = Session::new();
    let mut h = TestHost::default();
    let mut echo = |src: &str| piece(&mut s, &mut h, src).unwrap().map(|v| v.to_echo_string());
    // A piece that ends in a map literal's `}` may still omit its `;`.
    assert_eq!(echo(r#"let m = {"a": 1}"#), None);
    assert_eq!(echo(r#"m["b"] = "two""#), None);
    assert_eq!(echo("m").as_deref(), Some(r#"{"a": 1, "b": "two"}"#));
    assert_eq!(echo(r#"{"x": [1]}"#).as_deref(), Some(r#"{"x": [1]}"#));
    assert_eq!(echo("{}").as_deref(), Some("{}"));
    assert_eq!(echo(r#"m["b"]"#).as_deref(), Some(r#""two""#));
    // A block-ending piece is unaffected.
    assert_eq!(echo("if true { m[3] = 3; }"), None);
    assert_eq!(echo("keys(m)").as_deref(), Some(r#"[3, "a", "b"]"#));
    // A literal can be typed over several lines.
    assert_eq!(wisp::open_brackets(r#"let big = {"a": 1,"#), 1);
    assert_eq!(echo("let big = {\"a\": 1,\n\"b\": 2}"), None);
    assert_eq!(echo("big").as_deref(), Some(r#"{"a": 1, "b": 2}"#));
    // Only a missing final `;` is forgiven.
    let e = wisp::parse_interactive(r#"let m = {"a" 1}"#).unwrap_err();
    assert!(e.message.contains("expected ':' after a map key"), "{}", e.message);
    let e = wisp::parse_interactive("if true { print(1) }").unwrap_err();
    assert!(e.message.contains("expected ';'"), "{}", e.message);
}

// ---- helpers for the on-disk encoding -------------------------------------------

fn int(i: i64) -> Option<Vec<u8>> {
    Some([&[2u8][..], &i.to_le_bytes()[..]].concat())
}

fn st(s: &str) -> Option<Vec<u8>> {
    Some([&[3u8][..], &(s.len() as u32).to_le_bytes()[..], s.as_bytes()].concat())
}

/// A map encoding written from `docs/SAVE_FORMAT.md`, not by the code
/// under test: entries exactly as given, in the order given.
fn encode_map_by_hand(entries: &[(Option<Vec<u8>>, Option<Vec<u8>>)]) -> Vec<u8> {
    let mut out = vec![5u8];
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (k, v) in entries {
        out.extend_from_slice(k.as_deref().unwrap());
        out.extend_from_slice(v.as_deref().unwrap());
    }
    out
}

const SECTOR: usize = SECTOR_SIZE as usize;
const SLOT_SECTORS: usize = 8;
const OFF_LEN: usize = 33;
const OFF_CHECKSUM: usize = 37;
const OFF_GENERATION: usize = 41;
const OFF_VALUE: usize = 49;

fn fnv1a32(parts: &[&[u8]]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for part in parts {
        for &b in *part {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// Write `value_bytes` as key `"k"`'s committed copy into slot 0 of a
/// freshly provisioned store, exactly as `docs/SAVE_FORMAT.md` lays a
/// version-2 slot out, and load it back through the code under test.
fn load_value_from(value_bytes: &[u8]) -> Result<Option<Value>, &'static str> {
    let disk = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&disk, disk.total_sectors()) }.unwrap();
    let mut slot = vec![0u8; SLOT_SECTORS * SECTOR];
    slot[0] = 2;
    slot[1] = b'k';
    slot[OFF_LEN..OFF_LEN + 4].copy_from_slice(&(value_bytes.len() as u32).to_le_bytes());
    slot[OFF_GENERATION..OFF_VALUE].copy_from_slice(&1u64.to_le_bytes());
    slot[OFF_VALUE..OFF_VALUE + value_bytes.len()].copy_from_slice(value_bytes);
    let sum = fnv1a32(&[&slot[..OFF_CHECKSUM], &slot[OFF_GENERATION..OFF_VALUE + value_bytes.len()]]);
    slot[OFF_CHECKSUM..OFF_GENERATION].copy_from_slice(&sum.to_le_bytes());
    let mut media = disk.media();
    for (i, chunk) in slot.chunks(SECTOR).enumerate() {
        media[1 + i].copy_from_slice(chunk);
    }
    let disk = Hba::from_media(media);
    unsafe { save_format::load(&disk, "k") }
}

/// The value bytes of `key`'s committed copy, read straight from the media.
fn read_slot(disk: &Hba, key: &str) -> Option<Vec<u8>> {
    let media = disk.media();
    (0..64).find_map(|slot| {
        let first = &media[1 + slot * SLOT_SECTORS];
        let name_end = first[1..OFF_LEN].iter().position(|&b| b == 0).unwrap_or(32);
        if first[0] != 2 || &first[1..1 + name_end] != key.as_bytes() {
            return None;
        }
        let len = u32::from_le_bytes(first[OFF_LEN..OFF_LEN + 4].try_into().unwrap()) as usize;
        let bytes: Vec<u8> = media[1 + slot * SLOT_SECTORS..1 + (slot + 1) * SLOT_SECTORS].concat();
        Some(bytes[OFF_VALUE..OFF_VALUE + len].to_vec())
    })
}

#[test]
fn the_test_encoder_matches_the_documented_worked_example() {
    // The slot helpers above follow docs/SAVE_FORMAT.md; check them against
    // its worked example (`save("greeting", "hello world")` => checksum
    // 45 13 c2 97) so a mistake in them cannot mask one in the code.
    let mut slot = vec![0u8; 65];
    slot[0] = 2;
    slot[1..9].copy_from_slice(b"greeting");
    slot[OFF_LEN..OFF_LEN + 4].copy_from_slice(&16u32.to_le_bytes());
    slot[OFF_GENERATION..OFF_VALUE].copy_from_slice(&1u64.to_le_bytes());
    slot[OFF_VALUE..65].copy_from_slice(&st("hello world").unwrap());
    let sum = fnv1a32(&[&slot[..OFF_CHECKSUM], &slot[OFF_GENERATION..65]]);
    assert_eq!(sum.to_le_bytes(), [0x45, 0x13, 0xc2, 0x97]);
}

// ---- the bundled words.wisp -----------------------------------------------------

/// Drive `esp/APPS/words.wisp` through `lines` on a stack smaller than the
/// kernel's, and return its transcript (prompts and typed lines included).
/// The program must end normally, with its own goodbye.
fn words_session(lines: &[&str]) -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../esp/APPS/words.wisp")).unwrap();
    let inputs = lines.iter().map(|l| l.to_string()).collect();
    on_small_stack(move || {
        let (r, host) = run_with(&src, TestHost { inputs, ..Default::default() });
        if let Err(e) = r {
            panic!("words.wisp ended with a WISP error: {e}\n--- transcript ---\n{}", host.out);
        }
        assert!(host.out.ends_with("words done\n"), "{}", host.out);
        host.out
    })
}

#[test]
fn words_counts_and_ranks() {
    let t = words_session(&[
        "The cat saw the dog.",
        "THE DOG, the cat -- and a bird!",
        "",
        "   ",
        "top",
        "count the",
        "count Dog!",
        "count zebra",
        "list",
    ]);
    assert!(t.contains("words> The cat saw the dog.\nCounted 5 words, 5 words in all.\n"), "{t}");
    // "--" is punctuation only, so it is not a word.
    assert!(t.contains("Counted 7 words, 12 words in all.\n"), "{t}");
    // Ties (twice, once) in alphabetical order.
    assert!(t.contains(
        "12 words, 7 different. Most used:\n\
         \x20 the: 4 times\n\
         \x20 cat: twice\n\
         \x20 dog: twice\n\
         \x20 a: once\n\
         \x20 and: once\n"
    ), "{t}");
    assert!(t.contains("words> count the\nthe: 4 times\n"), "{t}");
    assert!(t.contains("words> count Dog!\ndog: twice\n"), "{t}");
    assert!(t.contains("words> count zebra\nNot seen yet: zebra\n"), "{t}");
    assert!(t.contains(
        "words> list\n\
         \x20 a: once\n\
         \x20 and: once\n\
         \x20 bird: once\n\
         \x20 cat: twice\n\
         \x20 dog: twice\n\
         \x20 saw: once\n\
         \x20 the: 4 times\n"
    ), "{t}");
}

#[test]
fn words_forgets_and_tells_commands_from_text() {
    let t = words_session(&[
        "top",
        "list",
        "go go go",
        "forget GO",
        "forget go",
        "top of the hill",
        "count",
        "quit now",
        "top",
    ]);
    assert!(t.contains("words> top\nNothing counted yet.\nwords> list\nNothing counted yet.\n"), "{t}");
    assert!(t.contains("words> forget GO\nForgot go.\nwords> forget go\nNot seen yet: go\n"), "{t}");
    // A line that only starts with a command word is text to count.
    assert!(t.contains("words> top of the hill\nCounted 4 words, 4 words in all.\n"), "{t}");
    assert!(t.contains("words> count\nCounted 1 word, 5 words in all.\n"), "{t}");
    assert!(t.contains("words> quit now\nCounted 2 words, 7 words in all.\n"), "{t}");
    assert!(t.contains(
        "7 words, 7 different. Most used:\n\
         \x20 count: once\n\
         \x20 hill: once\n\
         \x20 now: once\n\
         \x20 of: once\n\
         \x20 quit: once\n"
    ), "{t}");
}

#[test]
fn words_survives_any_text() {
    // Non-ASCII text, punctuation-only words, and very long lines are all
    // just text: none of them may stop the program with a WISP error.
    let long = "word ".repeat(2000);
    let t = words_session(&["Café, naïve 日本 🙂!", "... !!! ,,,", &long, "count CAFÉ", "count ...", "forget ,,,"]);
    // Characters outside ASCII are kept as part of words (upper() and
    // lower() only change ASCII letters, so "CAFÉ" is "cafÉ", another word).
    assert!(t.contains("Counted 4 words, 4 words in all.\n"), "{t}");
    assert!(t.contains("Counted 0 words, 4 words in all.\n"), "{t}");
    assert!(t.contains("Counted 2000 words, 2004 words in all.\n"), "{t}");
    assert!(t.contains("words> count CAFÉ\nNot seen yet: CAFÉ\n"), "{t}");
    assert!(t.contains("words> count ...\nNot seen yet: ...\n"), "{t}");
    assert!(t.contains("words> forget ,,,\nNot seen yet: ,,,\n"), "{t}");
}

#[test]
fn the_documented_examples_run() {
    // docs/WISP_LANGUAGE.md, "Maps".
    let printed = out(
        r#"
        let hero = {"name": "ada", "hp": 12, "items": ["lamp"]};
        let empty = {};
        print(hero["name"]);
        hero["hp"] = hero["hp"] - 3;
        hero["gold"] = 5;
        for key in hero {
            print(key, hero[key]);
        }
        print({"a": 1, 2: [3]});
        print(empty);
        "#,
    );
    assert_eq!(printed, "ada\ngold 5\nhp 9\nitems [\"lamp\"]\nname ada\n{2: [3], \"a\": 1}\n{}\n");
    // "B" sorts before "a": byte order, the order `<` uses.
    assert_eq!(eval(r#"keys({"a": 1, "B": 2})"#), r#"["B", "a"]"#);
    // A map whose values are all `true` stands in for a set.
    assert_eq!(out(r#"let seen = {}; for w in split("a b a c b") { seen[w] = true; } print(keys(seen));"#), "[\"a\", \"b\", \"c\"]\n");
}
