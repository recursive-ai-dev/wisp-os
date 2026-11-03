//! Fault injection for the lexer allocation boundary. The failing request
//! is a token-vector growth, and the failure is one-shot so constructing
//! the diagnostic remains possible, as it is with a fragmented heap.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use wisp_host_tests::wisp;

struct FaultAllocator;
thread_local! {
    static FAIL_ABOVE: Cell<usize> = const { Cell::new(0) };
    static FAILED: Cell<bool> = const { Cell::new(false) };
}

fn reject(size: usize) -> bool {
    FAIL_ABOVE.try_with(|limit| {
        if limit.get() != 0 && size > limit.get() {
            limit.set(0);
            let _ = FAILED.try_with(|failed| failed.set(true));
            true
        } else {
            false
        }
    }).unwrap_or(false)
}

unsafe impl GlobalAlloc for FaultAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject(layout.size()) { std::ptr::null_mut() } else { unsafe { System.alloc(layout) } }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if reject(size) { std::ptr::null_mut() } else { unsafe { System.realloc(ptr, layout, size) } }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: FaultAllocator = FaultAllocator;

#[test]
fn token_vector_allocation_failure_is_a_recoverable_error() {
    let source = ";".repeat(2000);
    for _ in 0..8 {
        FAILED.with(|failed| failed.set(false));
        FAIL_ABOVE.with(|limit| limit.set(4096));
        let problem = wisp::lexer::lex(&source).unwrap_err();
        FAIL_ABOVE.with(|limit| limit.set(0));
        assert!(FAILED.with(Cell::get), "the allocator fault must have fired");
        assert_eq!(problem.message, wisp::interp::OUT_OF_MEMORY_MESSAGE);
        assert_eq!(problem.line, 1);
        let tokens = wisp::lexer::lex("print(3);").unwrap();
        assert!(wisp::parser::parse(&tokens).is_ok());
    }
}

#[test]
fn interactive_append_allocation_failure_preserves_the_pending_piece() {
    let mut pending = String::from("if true {");
    let line = " ".repeat(8000);
    FAILED.with(|failed| failed.set(false));
    FAIL_ABOVE.with(|limit| limit.set(4096));
    let problem = wisp::append_interactive_line(&mut pending, &line).unwrap_err();
    FAIL_ABOVE.with(|limit| limit.set(0));
    assert!(FAILED.with(Cell::get));
    assert_eq!(problem, wisp::interp::OUT_OF_MEMORY_MESSAGE);
    assert_eq!(pending, "if true {");
    wisp::append_interactive_line(&mut pending, "}").unwrap();
    assert!(wisp::parse_interactive(&pending).is_ok());
}
