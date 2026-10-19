//! Regression tests for CODEBASE_AUDIT.md findings that can be exercised
//! without hardware: AUD-01/04/12 (save-disk ownership, durability,
//! interrupted provisioning, via the volatile-cache disk model), AUD-08
//! (shared-graph blowup), AUD-09 (cycle reclamation), AUD-10 (stack depth)
//! and AUD-13 (UTF-8 literals).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use wisp_host_tests::ahci::{Hba, SECTOR_SIZE};
use wisp_host_tests::save_format::{self, Probe, REGION_SECTORS, VALUE_MAX_LEN};
use wisp_host_tests::wisp::interp::Host;
use wisp_host_tests::wisp::value::{Value, MAX_DISPLAY_LEN, TRUNCATION_MARKER};
use wisp_host_tests::wisp::{self, WispError};

// ---- per-thread live-allocation accounting -------------------------------

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

// ---- a host that records output -------------------------------------------

#[derive(Default)]
struct TestHost {
    out: String,
    saved: std::collections::BTreeMap<String, Vec<u8>>,
}

impl Host for TestHost {
    fn print(&mut self, s: &str) {
        self.out.push_str(s);
    }
    fn input(&mut self, _prompt: &str) -> String {
        String::new()
    }
    fn clear(&mut self) {}
    fn sleep_ms(&mut self, _ms: u64) {}
    fn rand_u64(&mut self) -> u64 {
        4
    }
    fn save(&mut self, key: &str, value: &Value) -> Result<(), &'static str> {
        let bytes = save_format::encode(value)?;
        self.saved.insert(key.to_string(), bytes);
        Ok(())
    }
    fn load(&mut self, _key: &str) -> Result<Option<Value>, &'static str> {
        Ok(None)
    }
    fn stop_requested(&mut self) -> bool {
        false
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

fn run(src: &str) -> (Result<(), WispError>, TestHost) {
    let mut host = TestHost::default();
    let r = wisp::run_source(src, &mut host);
    (r, host)
}

fn run_ok(src: &str) -> String {
    let (r, host) = run(src);
    if let Err(e) = r {
        panic!("script failed: {e}\n--- output ---\n{}", host.out);
    }
    host.out
}

fn run_err(src: &str) -> String {
    match run(src).0 {
        Ok(()) => panic!("script unexpectedly succeeded"),
        Err(e) => e.message,
    }
}

/// Run `f` on a thread with a stack smaller than the kernel's 1 MiB
/// mainline stack: the interpreter's limits must keep every script within
/// this, leaving the rest as margin for the UEFI target's somewhat larger
/// frames and the shell's own frames underneath the interpreter.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(640 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .expect("thread overflowed its stack or panicked")
}

// ---- AUD-13: UTF-8 string literals ---------------------------------------

#[test]
fn utf8_literals_keep_their_bytes() {
    let out = run_ok(
        r#"
        print("é", len("é"));
        print("日本", len("日本"));
        print("🙂", len("🙂"));
        print("a\tb\"é\\", len("a\tb\"é\\"));
        "#,
    );
    assert_eq!(out, "é 2\n日本 6\n🙂 4\na\tb\"é\\ 7\n");
}

#[test]
fn utf8_literals_round_trip_through_save_encoding() {
    let (r, host) = run(r#"save("k", ["é", "🙂"]);"#);
    r.unwrap();
    let bytes = &host.saved["k"];
    assert!(bytes.windows(2).any(|w| w == "é".as_bytes()));
    assert!(bytes.windows(4).any(|w| w == "🙂".as_bytes()));
}

// ---- AUD-08: shared list graphs ------------------------------------------

const DOUBLING: &str = r#"
    let v = [0];
    let i = 0;
    while i < 22 {
        v = [v, v];
        i = i + 1;
    }
"#;

#[test]
fn print_of_exponential_graph_is_bounded() {
    let out = run_ok(&format!("{DOUBLING} print(v);"));
    assert!(out.len() <= MAX_DISPLAY_LEN + TRUNCATION_MARKER.len() + 1);
    assert!(out.trim_end().ends_with(TRUNCATION_MARKER));
}

#[test]
fn str_of_exponential_graph_is_an_error() {
    let msg = run_err(&format!("{DOUBLING} let s = str(v);"));
    assert!(msg.contains("str()"), "{msg}");
}

#[test]
fn save_of_exponential_graph_is_rejected_within_budget() {
    let src = format!("{DOUBLING} save(\"wide\", v);");
    let before = live_bytes();
    let ok = run_err(&src).contains("too large");
    assert_eq!(live_bytes(), before, "rejected save leaked memory");
    assert!(ok);
}

#[test]
fn equality_of_distinct_shared_graphs_is_fast() {
    let out = run_ok(
        r#"
        let a = [0];
        let b = [0];
        let i = 0;
        while i < 40 {
            a = [a, a];
            b = [b, b];
            i = i + 1;
        }
        print(a == b);
        push(b, 1);
        print(a == b);
        "#,
    );
    assert_eq!(out, "true\nfalse\n");
}

#[test]
fn cyclic_lists_compare_by_structure() {
    let out = run_ok(
        r#"
        let a = [1];
        push(a, a);
        let b = [1];
        push(b, b);
        let c = [2];
        push(c, c);
        print(a == b, a == c, a != c);
        "#,
    );
    assert_eq!(out, "true false true\n");
}

#[test]
fn encode_respects_value_max_len() {
    let s = "x".repeat(VALUE_MAX_LEN);
    let v = Value::Str(std::rc::Rc::new(s));
    assert!(save_format::encode(&v).is_err());
    let s = "x".repeat(VALUE_MAX_LEN - 5);
    let v = Value::Str(std::rc::Rc::new(s));
    assert_eq!(save_format::encode(&v).unwrap().len(), VALUE_MAX_LEN);
}

// ---- AUD-09: cycles are reclaimed ----------------------------------------

#[test]
fn cyclic_garbage_is_freed_when_the_run_ends() {
    let script = r#"
        let i = 0;
        while i < 5000 {
            let a = [];
            push(a, a);
            let b = [a];
            push(a, b);
            i = i + 1;
        }
        let keep = [1, 2, 3];
        push(keep, keep);
    "#;
    // Warm up any one-time allocations, then measure.
    run_ok(script);
    let before = live_bytes();
    for _ in 0..3 {
        run_ok(script);
    }
    assert_eq!(live_bytes(), before, "cyclic lists leaked across runs");
}

#[test]
fn deep_value_chains_drop_without_recursing() {
    on_small_stack(|| {
        run_ok(
            r#"
            let v = [];
            let i = 0;
            while i < 200000 {
                v = [v];
                i = i + 1;
            }
            v = nil;
            "#,
        );
    });
}

// ---- AUD-10: native stack depth ------------------------------------------

#[test]
fn deep_parentheses_are_a_parse_error() {
    let src = format!("let x = {}1{};", "(".repeat(20000), ")".repeat(20000));
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("nested too deeply"), "{msg}");
}

#[test]
fn long_flat_chain_is_a_parse_error() {
    let src = format!("let x = 1{};", " + 1".repeat(50000));
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("nested too deeply"), "{msg}");
}

#[test]
fn deep_unary_and_blocks_are_parse_errors() {
    let src = format!("let x = {}true;", "not ".repeat(20000));
    assert!(on_small_stack(move || run_err(&src)).contains("nested too deeply"));
    let src = format!("{}{}", "if true { ".repeat(5000), "}".repeat(5000));
    assert!(on_small_stack(move || run_err(&src)).contains("nested too deeply"));
}

#[test]
fn moderate_chains_still_parse() {
    let src = format!("print(1{});", " + 1".repeat(100));
    assert_eq!(run_ok(&src), "101\n");
}

#[test]
fn recursion_to_the_documented_limit_still_works() {
    let out = on_small_stack(|| {
        run_ok(
            r#"
            fn down(n) {
                if n == 0 {
                    return 0;
                }
                return 1 + down(n - 1);
            }
            print(down(250));
            "#,
        )
    });
    assert_eq!(out, "250\n");
}

#[test]
fn nested_loops_inside_deep_recursion_are_a_clean_error() {
    // The costliest unit of the nesting budget: a `while` body per level.
    let body = format!("{}return f(n + 1);{}", "while true { ".repeat(50), "}".repeat(50));
    let src = format!("fn f(n) {{ {body} return 0; }} f(0);");
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("stack overflow"), "{msg}");
}

#[test]
fn nested_for_loops_inside_deep_recursion_are_a_clean_error() {
    // A `for` body's native frames are larger than a `while` body's; the
    // interpreter charges it two nesting units so this still ends in a
    // WISP error, not a stack overflow.
    let body = format!("{}return f(n + 1);{}", "for x in [1] { ".repeat(60), "}".repeat(60));
    let src = format!("fn f(n) {{ {body} return 0; }} f(0);");
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("stack overflow"), "{msg}");
    // And mixed with slices and string indexing in the recursion.
    let body = format!("{}return f(n + 1)[0:1][0];{}", "for c in \"ab\"[0:1] { ".repeat(60), "}".repeat(60));
    let src = format!("fn f(n) {{ {body} return \"z\"; }} f(0);");
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("stack overflow"), "{msg}");
}

#[test]
fn runaway_recursion_is_a_clean_error() {
    let msg = on_small_stack(|| run_err("fn f(n) { return f(n + 1); } f(0);"));
    assert!(msg.contains("stack overflow"), "{msg}");
}

#[test]
fn deep_expressions_inside_deep_recursion_are_a_clean_error() {
    // Each call sits under a ~50-level expression: under the parser's
    // limit, and under the call limit, but not under both combined.
    let deep = format!("{}f(n + 1){}", "(1 + ".repeat(50), ")".repeat(50));
    let src = format!("fn f(n) {{ if n > 250 {{ return 0; }} return {deep}; }} f(0);");
    let msg = on_small_stack(move || run_err(&src));
    assert!(msg.contains("stack overflow"), "{msg}");
}

// ---- growth caps ---------------------------------------------------------

#[test]
fn string_doubling_is_capped() {
    let msg = run_err(r#"let s = "x"; while true { s = s + s; }"#);
    assert!(msg.contains("string is too long"), "{msg}");
}

#[test]
fn list_growth_is_capped() {
    let msg = run_err("let l = []; while true { push(l, 0); }");
    assert!(msg.contains("list is full"), "{msg}");
}

// ---- bundled programs still parse ----------------------------------------

#[test]
fn bundled_apps_lex_and_parse() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../esp/APPS");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("wisp") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let tokens = wisp::lexer::lex(&src).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        wisp::parser::parse(&tokens).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        n += 1;
    }
    assert!(n >= 10);
}

// ---- AUD-01/04/12: save-disk ownership and durability --------------------

const DISK_SECTORS: usize = 2048;

fn sector(fill: u8) -> [u8; SECTOR_SIZE as usize] {
    [fill; SECTOR_SIZE as usize]
}

#[test]
fn probe_never_writes_and_rejects_foreign_disks() {
    let hba = Hba::new(DISK_SECTORS);
    assert!(matches!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Blank)));

    let mut mbr = sector(0);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    hba.seed(0, &mbr);
    let before = hba.media();
    assert!(matches!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Foreign)));
    assert_eq!(hba.writes.get(), 0);
    assert_eq!(hba.media(), before);

    // Damaged magic is foreign, not "reformat me".
    let mut damaged = sector(0);
    damaged[..9].copy_from_slice(b"WISPSAVE2");
    hba.seed(0, &damaged);
    assert!(matches!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Foreign)));
    assert_eq!(hba.writes.get(), 0);
}

#[test]
fn probe_rejects_unsupported_versions_and_geometry() {
    let hba = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    let mut sb = [0u8; SECTOR_SIZE as usize];
    unsafe { hba.read_sector(0, &mut sb) }.unwrap();
    // Version 0 and any version past the current one are unsupported; so
    // is any other geometry. (Versions 1 and 2 are both read: see the
    // save-format v2 tests below.)
    for (offset, value) in [(9usize, 0u8), (9, 3), (9, 0xFF), (10, 63), (12, 0xFF)] {
        let mut bad = sb;
        bad[offset] = value;
        hba.seed(0, &bad);
        assert!(matches!(
            unsafe { save_format::probe(&hba, hba.total_sectors()) },
            Ok(Probe::Unsupported)
        ));
    }
}

#[test]
fn provisioning_writes_only_its_region() {
    let hba = Hba::new(DISK_SECTORS);
    for lba in 0..DISK_SECTORS as u64 {
        hba.seed(lba, &sector(0xEE));
    }
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    let media = hba.media();
    for (lba, s) in media.iter().enumerate() {
        if lba as u64 >= REGION_SECTORS {
            assert_eq!(*s, sector(0xEE), "sector {lba} outside the region was modified");
        }
    }
    assert!(matches!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Formatted)));
}

#[test]
fn interrupted_provisioning_is_never_accepted() {
    // Fail at every write of a provisioning run, then lose power. Two
    // starting points: a disk full of foreign data (first-time format,
    // the AUD-12 scenario) and an existing store holding a value.
    let total_writes = {
        let hba = Hba::new(DISK_SECTORS);
        unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
        hba.writes.get()
    };
    for existing_store in [false, true] {
        for fail_at in 0..total_writes {
            let hba = Hba::new(DISK_SECTORS);
            for lba in 0..DISK_SECTORS as u64 {
                hba.seed(lba, &sector(0xA5));
            }
            if existing_store {
                unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
                unsafe { save_format::save(&hba, "stale", &Value::Int(7)) }.unwrap();
            }
            let before = hba.media();
            hba.writes.set(0);
            hba.fail_write_at.set(Some(fail_at));
            assert!(unsafe { save_format::provision(&hba, hba.total_sectors()) }.is_err());
            hba.power_loss();
            hba.fail_write_at.set(None);

            let probe = unsafe { save_format::probe(&hba, hba.total_sectors()) }.unwrap();
            // Formatted is acceptable only if the failed run changed
            // nothing durable (the old store is then still intact).
            if probe == Probe::Formatted {
                assert!(
                    existing_store && hba.media() == before,
                    "partially provisioned disk accepted (failure at write {fail_at}, existing={existing_store})"
                );
            }
        }
    }
}

#[test]
fn successful_save_survives_power_loss() {
    let hba = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    hba.power_loss();
    assert!(matches!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Formatted)));

    unsafe { save_format::save(&hba, "greeting", &Value::Str(std::rc::Rc::new("hi".into()))) }.unwrap();
    hba.power_loss();
    let v = unsafe { save_format::load(&hba, "greeting") }.unwrap().expect("value lost");
    assert_eq!(v.to_display_string(), "hi");
}

#[test]
fn failed_flush_is_reported_not_swallowed() {
    let hba = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    hba.fail_flush.set(true);
    assert!(unsafe { save_format::save(&hba, "k", &Value::Int(1)) }.is_err());
}

#[test]
fn save_disk_refuses_io_until_formatted() {
    let disk = save_format::SaveDisk::new(Hba::new(DISK_SECTORS), DISK_SECTORS as u64, false);
    let err = disk.save("k", &Value::Int(1)).unwrap_err();
    assert!(err.contains("not formatted"), "{err}");
    assert!(disk.load("k").is_err());
    assert_eq!(disk.hba().writes.get(), 0);
    assert_eq!(disk.hba().reads.get(), 0);

    let mut disk = disk;
    disk.format().unwrap();
    disk.save("k", &Value::Int(1)).unwrap();
    assert_eq!(disk.load("k").unwrap().unwrap().to_display_string(), "1");
}
