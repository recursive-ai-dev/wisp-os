//! WISP closures: `fn (params) { body }` as an expression, the variables a
//! closure captures (by reference, shared with the code that made it and
//! with every other closure made in the same scope), per-pass loop
//! scopes, interaction with named functions and the higher-order
//! built-ins, every error message, and the resource bounds the rest of
//! the language has: Esc, native stack depth, parse depth, reclamation of
//! cycles through captured scopes, and non-recursive teardown of very
//! long chains of closures.

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

fn piece(session: &mut Session, host: &mut TestHost, src: &str) -> Result<Option<Value>, WispError> {
    let program = wisp::parse_interactive(src)?;
    session.run_interactive(host, &program)
}

// ---- the examples in docs/WISP_LANGUAGE.md, as written ---------------------------

#[test]
fn the_documented_examples_run_as_written() {
    let printed = out(
        r#"
        let double = fn (x) { return x * 2; };
        print(double(21));                  # 42
        print(double, type(double));        # <fn (x)> func
        print(fn (a, b) { return a + b; }(1, 2));   # 3
        "#,
    );
    assert_eq!(printed, "42\n<fn (x)> func\n3\n");

    let printed = out(
        r#"
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
        print(b());             # 1 -- its own n
        "#,
    );
    assert_eq!(printed, "1 2 3\n1\n");

    let printed = out(
        r#"
        fn adder(n) { return fn (x) { return x + n; }; }
        print(map(adder(10), [1, 2, 3]));                  # [11, 12, 13]
        print(filter(fn (w) { return len(w) > 2; }, ["a", "abc", "abcd"]));
        print(reduce(fn (acc, x) { return acc + x; }, [1, 2, 3, 4], 0));
        let by_length = ["bb", "a", "ccc"];
        sort_by(by_length, fn (s) { return len(s); });
        print(by_length);
        "#,
    );
    assert_eq!(printed, "[11, 12, 13]\n[\"abc\", \"abcd\"]\n10\n[\"a\", \"bb\", \"ccc\"]\n");
}

// ---- the literal ---------------------------------------------------------------------

#[test]
fn a_closure_is_a_function_value() {
    let printed = out(
        r#"
        let f = fn (x) { return x * 2; };
        let g = fn () { return "hi"; };
        let h = fn (a, b, c) { return a; };
        print(f(21), g(), type(f));
        print(f, g, h);
        print([f, g]);
        print({"f": f});
        "#,
    );
    assert_eq!(
        printed,
        "42 hi func\n<fn (x)> <fn ()> <fn (a, b, c)>\n[<fn (x)>, <fn ()>]\n{\"f\": <fn (x)>}\n"
    );
}

#[test]
fn a_closure_can_be_called_where_it_is_written() {
    assert_eq!(eval("fn (x) { return x + 1; }(41)"), "42");
    assert_eq!(eval("(fn (x) { return x + 1; })(41)"), "42");
    assert_eq!(eval("fn () { return fn (y) { return y * 3; }; }()(5)"), "15");
    // As a statement: `fn (` starts an expression, `fn name` a declaration.
    assert_eq!(out("fn (x) { print(\"got\", x); }(7);"), "got 7\n");
    // A declaration is still hoisted, and may sit after the code that calls it.
    assert_eq!(out("print(later(2)); fn later(x) { return x * x; }"), "4\n");
}

#[test]
fn a_closure_can_sit_in_any_expression_position() {
    let printed = out(
        r#"
        let fs = [fn (x) { return x + 1; }, fn (x) { return x * 10; }];
        print(fs[0](1), fs[1](2));
        let table = {"inc": fn (x) { return x + 1; }, "neg": fn (x) { return 0 - x; }};
        print(table["inc"](1), table["neg"](5));
        fn apply(f, x) { return f(x); }
        print(apply(fn (x) { return x - 1; }, 10));
        fn pick() { return fn () { return "picked"; }; }
        print(pick()());
        if fn () { return true; }() { print("yes"); }
        let i = 0;
        while fn (n) { return n < 3; }(i) { i = i + 1; }
        print(i);
        "#,
    );
    assert_eq!(printed, "2 20\n2 -5\n9\npicked\nyes\n3\n");
}

// ---- capturing ---------------------------------------------------------------------------

#[test]
fn a_closure_keeps_the_variables_of_the_call_that_made_it() {
    let printed = out(
        r#"
        fn make_counter(start) {
            let n = start;
            return fn () { n = n + 1; return n; };
        }
        let a = make_counter(0);
        let b = make_counter(100);
        print(a(), a(), b(), a(), b());
        "#,
    );
    assert_eq!(printed, "1 2 101 3 102\n");
}

#[test]
fn closures_made_in_one_scope_share_its_variables() {
    let printed = out(
        r#"
        fn make_cell(v) {
            let value = v;
            return [fn () { return value; }, fn (x) { value = x; }];
        }
        let cell = make_cell(1);
        let get = cell[0];
        let set = cell[1];
        print(get());
        set(42);
        print(get());
        "#,
    );
    assert_eq!(printed, "1\n42\n");
}

#[test]
fn the_enclosing_code_sees_what_the_closure_assigns_and_the_reverse() {
    let printed = out(
        r#"
        fn run() {
            let x = 1;
            let bump = fn () { x = x + 10; };
            let read = fn () { return x; };
            print(read());
            bump();
            print(x, read());
            x = 5;
            print(read());
            bump();
            return x;
        }
        print(run());
        "#,
    );
    assert_eq!(printed, "1\n11 11\n5\n15\n");
}

#[test]
fn a_variable_declared_after_the_closure_is_seen_when_it_is_called() {
    // Lookup is by name in the captured scope, at the time of the call.
    let printed = out(
        r#"
        fn run() {
            let f = fn () { return later; };
            let later = "declared afterwards";
            return f();
        }
        print(run());
        "#,
    );
    assert_eq!(printed, "declared afterwards\n");
    assert_err(
        "fn run() { let f = fn () { return nope; }; return f(); } run();",
        "undefined variable 'nope'",
    );
}

#[test]
fn each_pass_of_a_loop_gives_its_closures_their_own_variables() {
    let printed = out(
        r#"
        let fs = [];
        for i in range(3) {
            let sq = i * i;
            push(fs, fn () { return [i, sq]; });
        }
        print(fs[0](), fs[1](), fs[2]());

        let gs = [];
        let n = 0;
        while n < 3 {
            let mine = n * 100;
            push(gs, fn () { mine = mine + 1; return mine; });
            n = n + 1;
        }
        print(gs[0](), gs[0](), gs[1](), gs[2]());

        let hs = [];
        for ch in "ab" { push(hs, fn () { return ch; }); }
        print(hs[0](), hs[1]());
        "#,
    );
    assert_eq!(printed, "[0, 0] [1, 1] [2, 4]\n1 2 101 201\na b\n");
}

#[test]
fn a_closure_sees_the_blocks_around_it() {
    let printed = out(
        r#"
        fn f(a) {
            let r = [];
            if a > 0 {
                let b = a * 2;
                while len(r) < 2 {
                    let c = b + len(r);
                    push(r, fn (d) { return [a, b, c, d]; });
                }
            }
            return r;
        }
        let r = f(1);
        print(r[0](9), r[1](9));
        "#,
    );
    assert_eq!(printed, "[1, 2, 2, 9] [1, 2, 3, 9]\n");
}

#[test]
fn closures_nest_to_any_depth() {
    let printed = out(
        r#"
        let add3 = fn (a) { return fn (b) { return fn (c) { return a + b + c; }; }; };
        print(add3(1)(2)(3));
        fn compose(f, g) { return fn (x) { return f(g(x)); }; }
        let inc = fn (x) { return x + 1; };
        let dbl = fn (x) { return x * 2; };
        print(compose(inc, dbl)(5), compose(dbl, inc)(5));
        print(compose(compose(inc, inc), compose(dbl, dbl))(1));
        "#,
    );
    assert_eq!(printed, "6\n11 12\n6\n");
}

#[test]
fn parameters_and_locals_shadow_what_is_captured() {
    let printed = out(
        r#"
        fn run() {
            let x = "outer";
            let f = fn (x) { return x; };
            let g = fn () { let x = "inner"; return x; };
            let h = fn () { return x; };
            print(f("param"), g(), h(), x);
        }
        run();
        "#,
    );
    assert_eq!(printed, "param inner outer outer\n");
    // A name may not be declared twice in one scope: the parameters and the
    // body's own `let`s share one.
    assert_err("let f = fn (x) { let x = 1; return x; }; f(0);", "'x' is already declared in this scope");
}

#[test]
fn the_nearest_enclosing_variable_wins_and_is_the_one_assigned() {
    let printed = out(
        r#"
        fn run() {
            let x = "call";
            let seen = [];
            if true {
                let x = "if";
                while len(seen) < 1 {
                    let x = "while";
                    push(seen, fn () { x = x + "!"; return x; });
                }
                push(seen, fn () { return x; });
            }
            push(seen, fn () { return x; });
            print(seen[0](), seen[0](), seen[1](), seen[2]());
        }
        run();
        "#,
    );
    assert_eq!(printed, "while! while!! if call\n");
    // A global of the same name is the last resort, and a captured
    // variable hides it.
    let printed = out(
        r#"
        let x = "global";
        fn run() { let x = "local"; return fn () { return x; }; }
        print(run()(), (fn () { return x; })());
        "#,
    );
    assert_eq!(printed, "local global\n");
}

#[test]
fn a_closure_reaches_globals_even_declared_after_it() {
    let printed = out(
        r#"
        let f = fn () { total = total + 1; return total; };
        let total = 10;
        print(f(), f(), total);
        "#,
    );
    assert_eq!(printed, "11 12 12\n");
}

#[test]
fn a_named_function_still_sees_only_its_parameters_locals_and_globals() {
    // The caller's locals, and a closure's captured variables, stay invisible.
    assert_err("fn peek() { return secret; } fn run() { let secret = 1; return peek(); } run();", "undefined variable 'secret'");
    assert_err(
        r#"
        fn peek() { return secret; }
        fn run() { let secret = 1; let f = fn () { return peek(); }; return f(); }
        run();
        "#,
        "undefined variable 'secret'",
    );
    assert_err("fn g(f) { return f(); } fn run() { let v = 1; return g(fn () { return v; }) + w; } run();", "undefined variable 'w'");
    // ... but a closure passed in carries its own scope with it.
    assert_eq!(out("fn g(f) { return f(); } fn run() { let v = 7; return g(fn () { return v; }); } print(run());"), "7\n");
}

// ---- recursion ----------------------------------------------------------------------------

#[test]
fn a_closure_can_call_itself_through_the_variable_it_is_stored_in() {
    let printed = out(
        r#"
        let fact = fn (n) { if n <= 1 { return 1; } return n * fact(n - 1); };
        print(fact(10));

        fn run() {
            let fib = fn (n) { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); };
            return fib(15);
        }
        print(run());

        # Through a parameter instead of a name: no variable needed.
        let y = fn (f, n) { if n == 0 { return 0; } return n + f(f, n - 1); };
        print(y(y, 4));
        "#,
    );
    assert_eq!(printed, "3628800\n610\n10\n");
}

#[test]
fn runaway_recursion_through_a_closure_is_a_clean_error() {
    for src in [
        "let f = fn (n) { return f(n + 1); }; f(0);",
        "let f = fn (g) { return g(g); }; f(f);",
        "fn run() { let f = fn () { return f(); }; return f(); } run();",
        "let f = fn (l) { return map(f, [l]); }; f(0);",
    ] {
        let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
        assert!(e.message.contains("stack overflow") || e.message.contains("too much recursion"), "{src}: {}", e.message);
    }
}

#[test]
fn deep_but_legal_recursion_through_closures_works() {
    let printed = on_small_stack(|| {
        out("let down = fn (n) { if n == 0 { return 0; } return 1 + down(n - 1); }; print(down(100));")
    });
    assert_eq!(printed, "100\n");
}

// ---- calling ---------------------------------------------------------------------------------

#[test]
fn a_variable_holding_a_closure_is_called_like_a_function() {
    let printed = out(
        r#"
        fn name_clash() { return "named"; }
        let name_clash = fn () { return "variable"; };
        print(name_clash());
        fn run() {
            let f = fn () { return 1; };
            return f() + f();
        }
        print(run());
        "#,
    );
    assert_eq!(printed, "variable\n2\n");
    assert_err("let x = 3; x(1);", "cannot call 'x': it holds int, not a function");
    assert_err("let f = fn () { return 1; }; f()(1);", "cannot call int");
}

#[test]
fn a_wrong_argument_count_names_what_was_called() {
    assert_err("let f = fn (a, b) { return a; }; f(1);", "the anonymous function expects 2 argument(s), got 1");
    assert_err("let f = fn () { return 1; }; f(1, 2, 3);", "the anonymous function expects 0 argument(s), got 3");
    assert_err("fn named(a) { return a; } named();", "'named' expects 1 argument(s), got 0");
}

#[test]
fn break_and_continue_cannot_cross_a_closure_boundary() {
    assert_err("let f = fn () { break; }; while true { f(); }", "'break' outside of a loop");
    assert_err("let f = fn () { continue; }; for i in [1] { f(); }", "'continue' outside of a loop");
    // A loop inside the closure is fine.
    assert_eq!(
        out("let f = fn () { let t = 0; for i in range(5) { if i == 3 { break; } t = t + i; } return t; }; print(f());"),
        "3\n"
    );
}

#[test]
fn a_closure_returns_early_and_returns_nothing() {
    let printed = out(
        r#"
        let first_neg = fn (l) { for x in l { if x < 0 { return x; } } return nil; };
        print(first_neg([3, -2, -5]), first_neg([1]));
        let nothing = fn () { };
        print(nothing());
        "#,
    );
    assert_eq!(printed, "-2 nil\nnil\n");
}

// ---- values ------------------------------------------------------------------------------------

#[test]
fn closures_compare_by_identity() {
    let printed = out(
        r#"
        let f = fn (x) { return x; };
        let g = f;
        let h = fn (x) { return x; };
        fn named(x) { return x; }
        print(f == f, f == g, f != g);
        print(f == h, f != h);
        print(f == named, named == f, f == len);
        print([f] == [g], [f] == [h]);
        print(contains([h, f], f), find([h, f], f), find([h], f));
        "#,
    );
    assert_eq!(printed, "true true false\nfalse true\nfalse false false\ntrue false\ntrue 1 -1\n");
}

#[test]
fn a_closure_cannot_be_a_map_key_or_be_saved() {
    assert_err("let m = {}; m[fn () { }] = 1;", "map keys must be ints or strings, found func");
    assert_err("save(\"k\", fn () { return 1; });", "a function cannot be saved");
    assert_err("save(\"k\", [1, [fn () { return 1; }]]);", "a function cannot be saved");
    assert_err("save(\"k\", {\"a\": fn () { return 1; }});", "a function cannot be saved");
    // A failed save leaves what was stored under the key as it was.
    let (r, host) = run_with("save(\"k\", 1); save(\"k\", [fn () { }]);", TestHost::default());
    assert!(r.is_err());
    let kept = unsafe { save_format::load(&host.disk, "k") }.unwrap();
    assert_eq!(kept.map(|v| v.to_display_string()).as_deref(), Some("1"));
}

#[test]
fn a_closure_has_no_length_or_other_operations() {
    assert_err("let f = fn () { }; print(f + 1);", "'+' expects");
    assert_err("let f = fn () { }; print(len(f));", "len");
    assert_err("let f = fn () { }; if f { }", "expected bool");
    assert_err("let f = fn () { }; print(f[0]);", "cannot index into func");
    assert_err("for x in fn () { } { }", "'for' loops over a list, a map or a string, found func");
}

// ---- with the higher-order built-ins -------------------------------------------------------------

#[test]
fn closures_work_with_map_filter_reduce_and_sort_by() {
    let printed = out(
        r#"
        let k = 3;
        print(map(fn (x) { return x * k; }, [1, 2, 3]));
        print(filter(fn (x) { return x % k == 0; }, range(10)));
        print(reduce(fn (acc, x) { return acc + x * x; }, [1, 2, 3], 0));
        let words = ["pear", "fig", "banana", "kiwi"];
        sort_by(words, fn (w) { return len(w); });
        print(words);
        let rank = {"low": 3, "mid": 2, "top": 1};
        let tags = ["low", "top", "mid", "top", "low"];
        sort_by(tags, fn (t) { return rank[t]; });
        print(tags);
        let seen = 0;
        map(fn (x) { seen = seen + x; return x; }, [1, 2, 3, 4]);
        print(seen);
        "#,
    );
    assert_eq!(
        printed,
        "[3, 6, 9]\n[0, 3, 6, 9]\n14\n[\"fig\", \"pear\", \"kiwi\", \"banana\"]\n[\"top\", \"top\", \"mid\", \"low\", \"low\"]\n10\n"
    );
}

#[test]
fn the_higher_order_built_ins_name_a_closure_in_their_errors() {
    assert_err("filter(fn (x) { return 1; }, [1]);", "filter() needs the anonymous function to return a bool, but it returned int");
    assert_err("sort_by([1], fn (x) { return nil; });", "sort_by() needs the anonymous function to return an int or a string");
    assert_err(
        "sort_by([1, 2], fn (x) { if x == 1 { return 1; } return \"a\"; });",
        "sort_by() needs keys that are all ints or all strings, but the anonymous function returned int and string",
    );
    assert_err("let l = [1, 2]; sort_by(l, fn (x) { push(l, 0); return x; });", "the anonymous function changed the size of the list being sorted");
    assert_err("reduce(fn (a) { return a; }, [1], 0);", "the anonymous function expects 1 argument(s), got 2");
    assert_err("map(fn (a, b) { return a; }, [1]);", "the anonymous function expects 2 argument(s), got 1");
    // A named function still reads as before.
    assert_err("fn f(x) { return 1; } filter(f, [1]);", "filter() needs 'f' to return a bool, but it returned int");
    assert_err("map(1, [1]);", "map() expects a function as its first argument, found int");
}

#[test]
fn a_closure_that_fails_stops_the_whole_call_and_names_its_own_line() {
    let e = error("let l = [1, 2, 3];\nmap(fn (x) {\n  return 10 / (x - 2);\n}, l);");
    assert_eq!(e.message, "division by zero");
    assert_eq!(e.line, 3);
}

// ---- the prompt ---------------------------------------------------------------------------------------

#[test]
fn closures_live_across_pieces_of_an_interactive_session() {
    let mut session = Session::new();
    let mut host = TestHost::default();
    let echo = |session: &mut Session, host: &mut TestHost, src: &str| {
        piece(session, host, src).unwrap().map(|v| v.to_echo_string())
    };
    assert_eq!(echo(&mut session, &mut host, "let add = fn (a, b) { return a + b; }"), None);
    assert_eq!(echo(&mut session, &mut host, "add(1, 2)").as_deref(), Some("3"));
    assert_eq!(echo(&mut session, &mut host, "add").as_deref(), Some("<fn (a, b)>"));
    assert_eq!(echo(&mut session, &mut host, "fn counter() { let n = 0; return fn () { n = n + 1; return n; }; }"), None);
    assert_eq!(echo(&mut session, &mut host, "let c = counter()"), None);
    assert_eq!(echo(&mut session, &mut host, "c()").as_deref(), Some("1"));
    assert_eq!(echo(&mut session, &mut host, "c()").as_deref(), Some("2"));
    // Redefining a global is seen by a closure that reads it.
    assert_eq!(echo(&mut session, &mut host, "let base = 10"), None);
    assert_eq!(echo(&mut session, &mut host, "let plus = fn (x) { return x + base; }"), None);
    assert_eq!(echo(&mut session, &mut host, "plus(1)").as_deref(), Some("11"));
    assert_eq!(echo(&mut session, &mut host, "let base = 20"), None);
    assert_eq!(echo(&mut session, &mut host, "plus(1)").as_deref(), Some("21"));
    // So is a redefined function a closure calls by name.
    assert_eq!(echo(&mut session, &mut host, "fn helper() { return 1; }"), None);
    assert_eq!(echo(&mut session, &mut host, "let via = fn () { return helper(); }"), None);
    assert_eq!(echo(&mut session, &mut host, "fn helper() { return 2; }"), None);
    assert_eq!(echo(&mut session, &mut host, "via()").as_deref(), Some("2"));
    // An error in a call leaves the session and the closure usable.
    assert!(piece(&mut session, &mut host, "add(1)").is_err());
    assert_eq!(echo(&mut session, &mut host, "c()").as_deref(), Some("3"));
}

// ---- syntax ------------------------------------------------------------------------------------------------

fn parse_error(src: &str) -> String {
    match wisp::parse_interactive(src) {
        Ok(_) => panic!("unexpectedly parsed: {src}"),
        Err(e) => e.message,
    }
}

#[test]
fn syntax_errors_in_a_closure_literal() {
    assert!(parse_error("let f = fn (1) { };").contains("expected identifier"));
    assert!(parse_error("let f = fn (a b) { };").contains("expected ')'"));
    assert!(parse_error("let f = fn (a) return a;").contains("expected '{'"));
    assert!(parse_error("let f = fn (a, ) { };").contains("expected identifier"));
    assert!(parse_error("let f = fn;").contains("expected '('"));
    assert!(parse_error("let f = fn () { ").contains("unterminated block"));
    // A named declaration is a top-level thing; in a block or an
    // expression the message says what to write instead.
    let msg = parse_error("fn run() { fn inner() { return 1; } }");
    assert!(msg.contains("'fn inner' is a declaration") && msg.contains("fn (parameters)"), "{msg}");
    let msg = parse_error("if true { fn inner() { } }");
    assert!(msg.contains("'fn inner' is a declaration"), "{msg}");
    let msg = parse_error("let f = fn inner() { };");
    assert!(msg.contains("'fn inner' is a declaration"), "{msg}");
}

#[test]
fn closures_nested_too_deeply_are_a_parse_error() {
    let depth = 300;
    let src = format!("let f = {}0{};", "fn () { return ".repeat(depth), "; }".repeat(depth));
    let msg = on_small_stack(move || parse_error(&src));
    assert!(msg.contains("nested too deeply"), "{msg}");
    // Reasonable nesting works.
    let depth = 20;
    let src = format!("let f = {}7{};\nlet g = f;", "fn () { return ".repeat(depth), "; }".repeat(depth));
    let src = format!("{src} let r = g; let i = 0; while i < {depth} {{ r = r(); i = i + 1; }} print(r);");
    assert_eq!(on_small_stack(move || out(&src)), "7\n");
}

// ---- resources ------------------------------------------------------------------------------------------------

#[test]
fn esc_stops_a_loop_inside_a_closure() {
    let src = "let spin = fn () { while true { } }; spin();";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);
    // And a run of closure calls, each of which makes none of its own loops.
    let src = "let f = fn (x) { return x; }; let i = 0; while true { f(i); i = i + 1; }";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);
    let src = "let l = range(20000); map(fn (x) { return x; }, l);";
    let host = TestHost { stop_after: Some(100), ..Default::default() };
    assert_eq!(run_with(src, host).0.unwrap_err().message, STOPPED_MESSAGE);
}

#[test]
fn closures_that_capture_themselves_do_not_leak() {
    // Each of these builds a cycle: scope -> closure -> scope.
    let src = r#"
        fn make() {
            let f = fn () { return f; };
            let g = fn () { return [f, g]; };
            return g;
        }
        let i = 0;
        while i < 200 {
            let keep = make();
            let l = [];
            push(l, l);
            push(l, fn () { return l; });
            let m = {};
            m["me"] = m;
            m["f"] = fn () { return m; };
            let h = fn () { return h; };
            i = i + 1;
        }
        let top = fn () { return top; };
    "#;
    run_with(src, TestHost::default()).0.unwrap();
    let before = live_bytes();
    for _ in 0..3 {
        run_with(src, TestHost::default()).0.unwrap();
    }
    assert_eq!(live_bytes(), before, "closures, or the scopes they captured, leaked");
}

#[test]
fn closures_made_by_the_prompt_are_freed_with_the_session() {
    // Cycles through a captured scope, built one piece at a time.
    let run = || {
        let mut session = Session::new();
        let mut host = TestHost::default();
        for src in [
            "fn make() { let f = fn () { return f; }; return f; }",
            "let kept = make()",
            "let me = fn () { return me; }",
            "let l = []",
            "push(l, fn () { return l; })",
        ] {
            piece(&mut session, &mut host, src).unwrap();
        }
    };
    run();
    let before = live_bytes();
    run();
    run();
    assert_eq!(live_bytes(), before, "a session leaked closures or their scopes");
}

#[test]
fn a_long_chain_of_closures_is_freed_without_recursion() {
    // Each closure holds a scope holding the one before it: 50,000 deep.
    // Dropping the chain by recursion would overflow this thread's stack.
    let src = r#"
        let f = fn () { return 0; };
        let i = 0;
        while i < 50000 {
            let previous = f;
            f = fn () { return previous; };
            i = i + 1;
        }
        print(type(f));
    "#;
    let printed = on_small_stack(move || out(src));
    assert_eq!(printed, "func\n");
    // The same chain, freed when the session ends rather than by the
    // run's own scopes, and calling down it ends in a clean error.
    let src = r#"
        let f = fn () { return 0; };
        let i = 0;
        while i < 5000 {
            let previous = f;
            f = fn () { return previous(); };
            i = i + 1;
        }
        f();
    "#;
    let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("too much recursion") || e.message.contains("stack overflow"), "{}", e.message);
}

#[test]
fn a_long_chain_of_scopes_holding_lists_of_closures_is_freed_without_recursion() {
    let src = r#"
        let keep = [];
        let i = 0;
        while i < 30000 {
            let wrap = [keep, fn () { return i; }];
            keep = wrap;
            i = i + 1;
        }
        print(len(keep));
    "#;
    assert_eq!(on_small_stack(move || out(src)), "2\n");
}

#[test]
fn creating_and_dropping_closures_in_a_loop_gives_everything_back() {
    let src = r#"
        let total = 0;
        let i = 0;
        while i < 3000 {
            let n = i;
            let f = fn (x) { return x + n; };
            total = total + f(1) + map(fn (y) { return y * 2; }, [1])[0];
            i = i + 1;
        }
        print(total);
    "#;
    let expected = (0..3000i64).map(|n| 1 + n + 2).sum::<i64>();
    assert_eq!(out(src), format!("{expected}\n"));
    let before = live_bytes();
    out(src);
    assert_eq!(live_bytes(), before);
    // The nesting budget is handed back after every call, too.
    let printed = out(
        r#"
        let g = fn (x) { return x; };
        let t = 0;
        let j = 0;
        while j < 5000 { t = t + g(1); j = j + 1; }
        print(t);
        "#,
    );
    assert_eq!(printed, "5000\n");
}

// ---- native stack, in the shapes that cost the most per unit of the budget ----------------------------

#[test]
fn deep_blocks_inside_deep_closure_recursion_are_a_clean_error() {
    // The same shapes `regressions.rs` uses for named functions, through a
    // closure: a `while` body, a `for` body and a deep expression per level.
    let whiles = format!("{}return f(n + 1);{}", "while true { ".repeat(50), "}".repeat(50));
    let fors = format!("{}return f(n + 1);{}", "for x in [1] { ".repeat(60), "}".repeat(60));
    let ifs = format!("{}return f(n + 1);{}", "if true { ".repeat(60), "}".repeat(60));
    let expr = format!("return {}f(n + 1){};", "(1 + ".repeat(60), ")".repeat(60));
    for body in [whiles, fors, ifs, expr] {
        let src = format!("let f = fn (n) {{ {body} return 0; }}; f(0);");
        let e = on_small_stack(move || run_with(&src, TestHost::default()).0.unwrap_err());
        assert!(e.message.contains("stack overflow"), "{}", e.message);
    }
}

#[test]
fn recursion_through_closures_made_inside_closures_is_a_clean_error() {
    // Every call makes a closure and calls it: the captured scope grows
    // with the nesting of the *source*, not with the calls, so this stops
    // at the call-depth limit like any other recursion.
    let src = r#"
        let f = fn (n) {
            let g = fn () { return f(n + 1); };
            return g();
        };
        f(0);
    "#;
    let e = on_small_stack(move || run_with(src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("stack overflow") || e.message.contains("too much recursion"), "{}", e.message);
}

#[test]
fn a_chain_of_calls_on_results_is_bounded() {
    let chain = "()".repeat(5000);
    let src = format!("let f = fn () {{ return f; }};\nf(){chain};");
    let e = on_small_stack(move || run_with(&src, TestHost::default()).0.unwrap_err());
    assert!(e.message.contains("too deeply") || e.message.contains("stack overflow"), "{}", e.message);
}

// ---- the bundled undo.wisp ---------------------------------------------------------------------------------

fn undo_session(lines: &[&str]) -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../esp/APPS/undo.wisp")).unwrap();
    let inputs = lines.iter().map(|l| l.to_string()).collect();
    on_small_stack(move || {
        let (r, host) = run_with(&src, TestHost { inputs, ..Default::default() });
        if let Err(e) = r {
            panic!("undo.wisp ended with a WISP error: {e}\n--- transcript ---\n{}", host.out);
        }
        assert!(host.out.ends_with("undo done\n"), "{}", host.out);
        host.out
    })
}

#[test]
fn undo_edits_undoes_and_redoes() {
    let t = undo_session(&[
        "set apples 5",
        "set pears 3",
        "add apples 10",
        "show",
        "undo",
        "show",
        "undo",
        "undo",
        "show",
        "redo",
        "redo",
        "show",
        "history",
        "quit",
    ]);
    assert!(t.contains("undo> set apples 5\nset apples = 5\n"), "{t}");
    assert!(t.contains("undo> add apples 10\nadd 10 to apples (now 15)\n"), "{t}");
    assert!(t.contains("undo> show\napples = 15\npears = 3\n"), "{t}");
    assert!(t.contains("undo> undo\nUndid #3: add 10 to apples (now 15)\nundo> show\napples = 5\npears = 3\n"), "{t}");
    assert!(t.contains("undo> undo\nUndid #2: set pears = 3\nundo> undo\nUndid #1: set apples = 5\nundo> show\nThe notebook is empty.\n"), "{t}");
    assert!(t.contains("undo> redo\nRedid #1: set apples = 5\nundo> redo\nRedid #2: set pears = 3\nundo> show\napples = 5\npears = 3\n"), "{t}");
    assert!(t.contains("undo> history\n#1 set apples = 5\n#2 set pears = 3\n(1 undone, ready to redo)\n"), "{t}");
}

#[test]
fn undo_puts_back_what_a_delete_and_an_overwrite_replaced() {
    let t = undo_session(&[
        "set a 1",
        "set a 2",
        "del a",
        "show",
        "undo",
        "show",
        "undo",
        "show",
        "undo",
        "show",
        "undo",
        "redo",
        "redo",
        "redo",
        "show",
        "quit",
    ]);
    assert!(t.contains("undo> del a\ndel a\nundo> show\nThe notebook is empty.\n"), "{t}");
    assert!(t.contains("undo> undo\nUndid #3: del a\nundo> show\na = 2\n"), "{t}");
    assert!(t.contains("undo> undo\nUndid #2: set a = 2\nundo> show\na = 1\n"), "{t}");
    assert!(t.contains("undo> undo\nUndid #1: set a = 1\nundo> show\nThe notebook is empty.\n"), "{t}");
    assert!(
        t.contains("undo> undo\nNothing to undo.\nundo> redo\nRedid #1: set a = 1\nundo> redo\nRedid #2: set a = 2\nundo> redo\nRedid #3: del a\nundo> show\nThe notebook is empty.\n"),
        "{t}"
    );
}

#[test]
fn undo_a_new_edit_ends_the_redo_chain() {
    let t = undo_session(&["set a 1", "set b 2", "undo", "set c 3", "redo", "history", "show", "quit"]);
    assert!(t.contains("undo> redo\nNothing to redo.\n"), "{t}");
    assert!(t.contains("undo> history\n#1 set a = 1\n#3 set c = 3\n"), "{t}");
    assert!(t.contains("undo> show\na = 1\nc = 3\n"), "{t}");
}

#[test]
fn undo_find_top_and_sum_use_closures_over_their_arguments() {
    let t = undo_session(&[
        "set apple 4",
        "set avocado 9",
        "set banana 4",
        "set cherry -2",
        "find a",
        "find AV",
        "find z",
        "top",
        "sum",
        "quit",
    ]);
    assert!(t.contains("undo> find a\napple = 4\navocado = 9\nundo> find AV\navocado = 9\nundo> find z\nNo names start with z.\n"), "{t}");
    assert!(t.contains("undo> top\navocado = 9\napple = 4\nbanana = 4\ncherry = -2\n"), "{t}");
    assert!(t.contains("undo> sum\n15\n"), "{t}");
}

#[test]
fn undo_keeps_a_bounded_history_and_survives_bad_input() {
    let mut lines: Vec<String> = (0..60).map(|i| format!("add n {}", i + 1)).collect();
    lines.extend(["history", "undo", "undo"].map(String::from));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let t = undo_session(&refs);
    // Only the last 50 edits are kept: #11 is the oldest.
    assert!(t.contains("undo> history\n#11 add 11 to n (now 66)\n"), "{t}");
    assert!(!t.contains("\n#10 add"), "{t}");
    assert!(t.contains("#60 add 60 to n (now 1830)\n"), "{t}");
    assert!(t.contains("Undid #60: add 60 to n (now 1830)\n"), "{t}");

    let t = undo_session(&[
        "",
        "   ",
        "frobnicate",
        "set",
        "set x",
        "set x y",
        "set x 1 2",
        "set 9x 1",
        "set x 1234567890",
        "set toolonganame_toolonganame 1",
        "add x -",
        "del nothing",
        "del",
        "find",
        "find a b",
        "undo",
        "redo",
        "show",
        "sum",
        "top",
        "history",
        "SET Big -5",
        "show",
        "HELP",
        "quit",
    ]);
    assert!(t.contains("undo> frobnicate\nUnknown command: frobnicate. Type help.\n"), "{t}");
    assert_eq!(t.matches("Usage: set NAME NUMBER").count(), 7, "{t}");
    assert!(t.contains("Usage: add NAME NUMBER"), "{t}");
    assert_eq!(t.matches("Usage: del NAME").count(), 2, "{t}");
    assert_eq!(t.matches("Usage: find PREFIX").count(), 2, "{t}");
    assert!(t.contains("undo> undo\nNothing to undo.\nundo> redo\nNothing to redo.\n"), "{t}");
    assert!(t.contains("undo> show\nThe notebook is empty.\nundo> sum\n0\nundo> top\nundo> history\nNo edits yet.\n"), "{t}");
    assert!(t.contains("undo> SET Big -5\nset big = -5\nundo> show\nbig = -5\n"), "{t}");
    assert!(t.contains("quit                leave\n"), "{t}");
}
