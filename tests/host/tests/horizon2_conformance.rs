//! Horizon 2 (v1.4.0) language conformance suite: soft built-ins, bitwise and
//! integer helpers, formatting, `pcall`/`error` (with uncatchable Esc and OOM),
//! richer diagnostics (columns, carets, call stacks, module attribution),
//! `import()` modules, standard library (`LIB/*.wisp`), and compound assignments.

use std::collections::BTreeMap;
use wisp_host_tests::wisp::interp::{Host, OUT_OF_MEMORY_MESSAGE, STOPPED_MESSAGE};
use wisp_host_tests::wisp::value::Value;
use wisp_host_tests::wisp::{self, WispError};

#[derive(Default)]
struct TestHost {
    output: Vec<String>,
    stop_after_calls: Option<usize>,
    low_mem: bool,
    custom_modules: BTreeMap<String, String>,
    beeps: Vec<(u32, u64)>,
    store: BTreeMap<String, Value>,
}

impl Host for TestHost {
    fn print(&mut self, s: &str) {
        if let Some(last) = self.output.last_mut() {
            if !last.ends_with('\n') {
                last.push_str(s);
                return;
            }
        }
        self.output.push(String::from(s));
    }
    fn input(&mut self, _prompt: &str) -> String {
        String::new()
    }
    fn clear(&mut self) {}
    fn sleep_ms(&mut self, _ms: u64) {}
    fn rand_u64(&mut self) -> u64 {
        42
    }
    fn save(&mut self, key: &str, value: &Value) -> Result<(), &'static str> {
        if matches!(value, Value::Nil) {
            self.store.remove(key);
        } else {
            self.store.insert(String::from(key), value.clone());
        }
        Ok(())
    }
    fn load(&mut self, key: &str) -> Result<Option<Value>, &'static str> {
        Ok(self.store.get(key).cloned())
    }
    fn file_list(&mut self) -> Result<Vec<String>, &'static str> {
        Ok(self
            .store
            .keys()
            .filter_map(|k| k.strip_prefix("file.").map(String::from))
            .collect())
    }
    fn stop_requested(&mut self) -> bool {
        if let Some(rem) = self.stop_after_calls.as_mut() {
            if *rem == 0 {
                return true;
            }
            *rem -= 1;
        }
        false
    }
    fn memory_low(&mut self) -> bool {
        self.low_mem
    }
    fn time_ms(&mut self) -> u64 {
        1234
    }
    fn screen_width(&self) -> usize {
        800
    }
    fn screen_height(&self) -> usize {
        600
    }
    fn plot(&mut self, _x: i64, _y: i64, _color: u32) {}
    fn draw_rect(&mut self, _x: i64, _y: i64, _w: usize, _h: usize, _color: u32) {}
    fn draw_line(&mut self, _x1: i64, _y1: i64, _x2: i64, _y2: i64, _color: u32) {}
    fn fill_screen(&mut self, _color: u32) {}
    fn beep(&mut self, hz: u32, ms: u64) {
        self.beeps.push((hz, ms));
    }
    fn load_module(&mut self, name: &str) -> Result<Option<String>, String> {
        Ok(self.custom_modules.get(name).cloned())
    }
}

fn run_ok(src: &str) -> Vec<String> {
    let mut host = TestHost::default();
    wisp::run_source(src, &mut host).unwrap_or_else(|e| panic!("unexpected error: {e}"));
    host.output
        .into_iter()
        .map(|s| s.trim_end_matches('\n').to_string())
        .collect()
}

fn run_err(src: &str) -> WispError {
    let mut host = TestHost::default();
    wisp::run_source(src, &mut host).expect_err("expected program to fail")
}

#[test]
fn soft_builtins_work_by_default_and_yield_to_user_functions_and_variables() {
    let out = run_ok(
        r#"
        let d = date();
        print(d["year"], d["month"], d["day"]);
        "#,
    );
    assert_eq!(out, vec!["2026 11 15"]);

    // A user `fn` with the name of any soft builtin shadows it cleanly.
    let out = run_ok(
        r#"
        fn date() { return "custom-date"; }
        fn band(a, b) { return a + b; }
        fn pcall(x) { return x * 10; }
        fn import(name) { return "mock:" + name; }
        print(date(), band(3, 4), pcall(7), import("foo"));
        "#,
    );
    assert_eq!(out, vec!["custom-date 7 70 mock:foo"]);

    // A user `let` variable also shadows a soft builtin.
    let out = run_ok(
        r#"
        let hex = fn(n) { return "H:" + str(n); };
        print(hex(16));
        "#,
    );
    assert_eq!(out, vec!["H:16"]);

    // A non-function `let` variable with a soft builtin's name is not ignored
    // when called.
    let err = run_err("let date = 42; date();");
    assert!(err.message.contains("cannot call 'date': it holds int"));

    // Reserved 1.1 hard builtins still cannot be redefined.
    let err = run_err("fn len(x) { return 0; }");
    assert!(err.message.contains("'len' is a built-in function"));
}

#[test]
fn bitwise_builtins_and_rgb_parts_match_twos_complement_semantics() {
    let out = run_ok(
        r#"
        print(band(12, 10), bor(12, 10), bxor(12, 10));
        print(bnot(0), bnot(-1));
        print(shl(1, 0), shl(1, 10), shr(-16, 2), lshr(-16, 60));
        let c = rgb(18, 52, 86);
        print(rgb_parts(c));
        "#,
    );
    assert_eq!(
        out,
        vec![
            format!("{} {} {}", 0b1100 & 0b1010, 0b1100 | 0b1010, 0b1100 ^ 0b1010),
            "-1 0".to_string(),
            "1 1024 -4 15".to_string(),
            "[18, 52, 86]".to_string(),
        ]
    );

    for bad in ["shl(1, -1);", "shl(1, 64);", "shr(1, 64);", "lshr(1, -5);"] {
        let err = run_err(bad);
        assert!(err.message.contains("shift count must be between 0 and 63"));
    }
}

#[test]
fn formatting_builtins_handle_padding_radices_fixed_point_and_fmt() {
    let out = run_ok(
        r#"
        print(pad_left(7, 3, "0"), pad_left("abcd", 3, " "), pad_right("hi", 5, "."));
        print(hex(255), hex(4096), hex(0), hex(-255));
        print(bin(5), bin(0), bin(-5));
        print(int("ff", 16), int("-0xff", 16), int("1011", 2), int("0b1011", 2), int("z", 36));
        print(fixed(1205, 100), fixed(5, 100), fixed(-250, 100), fixed(-5, 100), fixed(42, 1));
        print(fmt("{} scored {} in {}!", "Ada", 99, "WISP"));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "007 abcd hi...",
            "ff 1000 0 -ff",
            "101 0 -101",
            "255 -255 11 11 35",
            "12.05 0.05 -2.50 -0.05 42",
            "Ada scored 99 in WISP!",
        ]
    );

    assert!(run_err(r#"fmt("{} {}", 1);"#).message.contains("placeholder"));
    assert!(run_err("fixed(10, 3);").message.contains("power of 10"));
    assert!(run_err(r#"int("10", 37);"#).message.contains("base must be between 2 and 36"));
}

#[test]
fn integer_and_collection_helpers_preserve_exact_128bit_and_checked_bounds() {
    let out = run_ok(
        r#"
        print(divmod(17, 5), divmod(-17, 5));
        print(pow(2, 10), pow(-3, 3), pow(7, 0), isqrt(0), isqrt(99), isqrt(1000000));
        # 5000000000000 * 4000000000000 overflows i64, but / 1000000000000 fits!
        print(muldiv(5000000000000, 4000000000000, 1000000000000));
        print(min(9, 2, 7, -4, 5), max(9, 2, 7, -4, 5));
        print(min([9, 2, 7]), max(["beta", "alpha", "gamma"]));
        print(sum([]), sum([10, 20, 30, -5]));
        print(enumerate(["a", "b"]), last([10, 20, 30]));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "[3, 2] [-3, -2]",
            "1024 -27 1 0 9 1000",
            "20000000000000",
            "-4 9",
            "2 gamma",
            "0 55",
            "[[0, \"a\"], [1, \"b\"]] 30",
        ]
    );

    assert!(run_err("isqrt(-1);").message.contains("non-negative"));
    assert!(run_err("pow(2, -1);").message.contains("negative"));
    assert!(run_err("pow(2, 63);").message.contains("overflow"));
    assert!(run_err("last([]);").message.contains("empty list"));
}

#[test]
fn pcall_catches_runtime_errors_restores_budget_and_never_catches_esc_or_oom() {
    let out = run_ok(
        r#"
        fn boom(x) {
            if x < 0 {
                error("negative input: " + str(x));
            }
            return 100 / x;
        }
        let r1 = pcall(boom, 4);
        let r2 = pcall(boom, 0);
        let r3 = pcall(boom, -7);
        print(r1["ok"], r1["value"]);
        print(r2["ok"], r2["error"], r2["line"] > 0);
        print(r3["ok"], r3["error"]);

        # 4,000 iterations of failing pcall must never leak nesting or call depth.
        let caught = 0;
        for i in range(4000) {
            let res = pcall(boom, 0);
            if not res["ok"] {
                caught += 1;
            }
        }
        print(caught);
        "#,
    );
    assert_eq!(
        out,
        vec![
            "true 25",
            "false division by zero true",
            "false negative input: -7",
            "4000",
        ]
    );

    // Esc inside `pcall` MUST NOT be caught!
    let mut host = TestHost {
        stop_after_calls: Some(5),
        ..Default::default()
    };
    let err = wisp::run_source(
        r#"
        fn spin() {
            while true {}
        }
        let r = pcall(spin);
        print("should never reach here", r);
        "#,
        &mut host,
    )
    .expect_err("Esc inside pcall must stop the program");
    assert_eq!(err.message, STOPPED_MESSAGE);
    assert!(host.output.is_empty());

    // Out-of-memory inside `pcall` MUST NOT be caught!
    let mut oom_host = TestHost {
        low_mem: false,
        ..Default::default()
    };
    oom_host.low_mem = true;
    let err = wisp::run_source(
        r#"
        fn work() { return 1 + 1; }
        let r = pcall(work);
        "#,
        &mut oom_host,
    )
    .expect_err("OOM inside pcall must stop the program");
    assert_eq!(err.message, OUT_OF_MEMORY_MESSAGE);
}

#[test]
fn diagnostics_report_columns_source_caret_call_stack_and_module_name() {
    let mut host = TestHost::default();
    let src = "fn inner(x) {\n    return 10 / x;\n}\nfn outer() {\n    return inner(0);\n}\nouter();\n";
    let (err, details) = wisp::run_source_detailed(src, &mut host).unwrap_err();
    assert_eq!(err.to_display_string(), "wisp: line 2: division by zero");
    let detailed = err.to_detailed_string(&details);
    assert!(detailed.contains("return 10 / x;"), "{detailed}");
    assert!(detailed.contains("in inner (called from line 5)"), "{detailed}");
    assert!(detailed.contains("in outer (called from line 7)"), "{detailed}");

    // Parse error includes column and caret.
    let (parse_err, parse_details) =
        wisp::run_source_detailed("let x = 1 + ;\n", &mut host).unwrap_err();
    assert_eq!(parse_err.line, 1);
    assert_eq!(parse_err.col, 13);
    let rendered = parse_err.to_detailed_string(&parse_details);
    assert!(rendered.contains("let x = 1 + ;\n              ^"), "{rendered}");
}

#[test]
fn modules_and_standard_library_run_isolated_once_and_detect_cycles() {
    let out = run_ok(
        r##"
        let strings = import("strings");
        let math = import("math");
        let col = import("collections");
        let fixed_lib = import("fixed");
        let text = import("text");

        print(strings["center"]("hi", 6, "-"), strings["join_fmt"]([1, 2, 3], ", ", "#{}"));
        print(math["gcd"](84, 36), math["lcm"](12, 18), math["clamp"](15, 0, 10), math["abs_diff"](3, 10));
        print(col["zip"]([1, 2], ["a", "b"]), col["unique"]([3, 1, 3, 2, 1]), col["flatten"]([[1, 2], [3], 4]));
        let a = 1500;
        let b = 2500;
        let prod = fixed_lib["fixed_mul"](a, b, fixed_lib["SCALE"]);
        print(prod, fixed_lib["to_text"](prod, fixed_lib["SCALE"]));
        print(text["wrap_lines"]("one two three four", 9));
        "##,
    );
    assert_eq!(
        out,
        vec![
            "--hi-- #1, #2, #3",
            "12 36 10 7",
            "[[1, \"a\"], [2, \"b\"]] [3, 1, 2] [1, 2, 3, 4]",
            "3750 3.750",
            "[\"one two\", \"three\", \"four\"]",
        ]
    );

    // Custom modules: single evaluation per session and cycle detection.
    let mut host = TestHost::default();
    host.custom_modules.insert(
        String::from("counter"),
        String::from(
            r#"
            print("initializing counter module");
            let count = 0;
            fn next() {
                count += 1;
                return count;
            }
            return {"next": next};
            "#,
        ),
    );
    host.custom_modules.insert(
        String::from("cyc_a"),
        String::from(r#"let b = import("cyc_b"); return b;"#),
    );
    host.custom_modules.insert(
        String::from("cyc_b"),
        String::from(r#"let a = import("cyc_a"); return a;"#),
    );

    wisp::run_source(
        r#"
        let c1 = import("counter");
        let c2 = import("counter");
        print(c1["next"](), c2["next"]());
        let cyc = pcall(import, "cyc_a");
        print(cyc["ok"], contains(cyc["error"], "import cycle detected: cyc_a -> cyc_b -> cyc_a"));
        "#,
        &mut host,
    )
    .unwrap();
    let lines: Vec<String> = host
        .output
        .into_iter()
        .map(|s| s.trim_end_matches('\n').to_string())
        .collect();
    assert_eq!(
        lines,
        vec!["initializing counter module", "1 2", "false true"]
    );
}

#[test]
fn compound_assignments_evaluate_targets_once_and_update_vars_lists_and_maps() {
    let out = run_ok(
        r#"
        let x = 10;
        x += 5;
        x -= 3;
        x *= 4;
        x /= 6;
        x %= 5;
        let s = "wi";
        s += "sp";

        let calls = 0;
        let xs = [10, 20, 30];
        fn get_list() {
            calls += 1;
            return xs;
        }
        get_list()[1] += 7;
        let m = {"score": 100};
        m["score"] -= 15;
        print(x, s, calls, xs, m["score"]);
        "#,
    );
    assert_eq!(out, vec!["3 wisp 1 [10, 27, 30] 85"]);
}

#[test]
fn sandboxed_file_builtins_isolate_file_namespace_and_reject_traversal() {
    let out = run_ok(
        r#"
        save("prog.secret", "program-code");
        save("sys.keymap", "dvorak");
        print(read_file("missing.txt"));
        write_file("notes.txt", "line1\n");
        append_file("notes.txt", "line2");
        write_file("alpha.log", "ok");
        print(list_files());
        print(replace(read_file("notes.txt"), "\n", "|"));
        print(delete_file("notes.txt"), delete_file("notes.txt"));
        print(list_files());
        print(load("prog.secret"), load("sys.keymap"));
        "#,
    );
    assert_eq!(
        out,
        vec![
            "nil",
            "[\"alpha.log\", \"notes.txt\"]",
            "line1|line2",
            "true false",
            "[\"alpha.log\"]",
            "program-code dvorak",
        ]
    );

    for bad in [
        r#"read_file("");"#,
        r#"write_file("../prog.secret", "x");"#,
        r#"write_file(".hidden", "x");"#,
        r#"write_file("trailing.", "x");"#,
        r#"write_file("dir/file", "x");"#,
    ] {
        assert!(run_err(bad).message.contains("file name"));
    }
}
