//! Runtime value representation. Per `docs/WISP_LANGUAGE.md`, WISP has
//! five value types: `nil`, `bool`, `int`, `string`, and `list`.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// Recursion guard for structural operations over nested lists (equality,
/// display): a list that -- deliberately or accidentally, e.g.
/// `let a = []; push(a, a);` -- contains itself would otherwise recurse
/// forever and take down the whole shell with a real Rust stack overflow,
/// which is not something WISP's own error reporting can catch (unlike a
/// WISP-level infinite `while` loop, which just runs forever rather than
/// crashing anything). Bounding structural recursion at a depth no real,
/// non-self-referential script would ever approach turns that failure mode
/// into "stops looking any deeper past 64 levels" instead of "crashes the
/// firmware."
pub(crate) const MAX_VALUE_DEPTH: u32 = 64;

#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    /// `Rc` rather than an owned `String` so that passing a string as a
    /// function argument, or copying it into a new variable, is an O(1)
    /// refcount bump instead of an O(n) byte copy -- strings in WISP are
    /// immutable, so sharing the backing buffer is always safe.
    Str(Rc<String>),
    /// A growable, shared, *mutable* list. `Rc<RefCell<..>>` for the same
    /// sharing reason as `Str` above, plus interior mutability: every
    /// `Value::List` clone (from copying it into a new variable, passing
    /// it as a function argument, etc.) is a handle to the *same*
    /// underlying vector, so `push`/`pop`/`list[i] = ...` through any one
    /// handle is visible through every other handle to it -- the same
    /// "shared, mutable collection" behavior most scripting languages give
    /// you for reference types.
    List(Rc<RefCell<Vec<Value>>>),
}

impl Value {
    pub const fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Str(_) => "string",
            Value::List(_) => "list",
        }
    }

    /// The `str()` builtin's conversion, also used to build `print()`
    /// output and error messages that quote a value back to the user.
    /// A bare top-level `string` displays without quotes (so
    /// `print("hi")` prints `hi`, not `"hi"`); a `string` *nested* inside
    /// a list displays quoted, so `print([1, "a"])` reads unambiguously
    /// as `[1, "a"]` rather than `[1, a]`.
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        match self {
            Value::Str(s) => out.push_str(s),
            other => other.write_nested(&mut out, 0),
        }
        out
    }

    fn write_nested(&self, out: &mut String, depth: u32) {
        match self {
            Value::Nil => out.push_str("nil"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Int(i) => out.push_str(&format!("{i}")),
            Value::Str(s) => {
                out.push('"');
                out.push_str(s);
                out.push('"');
            }
            Value::List(items) => {
                if depth >= MAX_VALUE_DEPTH {
                    out.push_str("[...]");
                    return;
                }
                out.push('[');
                for (i, v) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_nested(out, depth + 1);
                }
                out.push(']');
            }
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        values_eq(self, other, 0)
    }
}

fn values_eq(a: &Value, b: &Value, depth: u32) -> bool {
    if depth >= MAX_VALUE_DEPTH {
        return false;
    }
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            // Same underlying list (including the direct-self-reference
            // case, `push(a, a)`): equal without walking it at all, which
            // also means a direct cycle can never even reach the depth
            // guard above -- only an *indirect* cycle (a contains b
            // contains a) would, and that's exactly what the guard is for.
            if Rc::ptr_eq(x, y) {
                return true;
            }
            let xb = x.borrow();
            let yb = y.borrow();
            xb.len() == yb.len()
                && xb
                    .iter()
                    .zip(yb.iter())
                    .all(|(ex, ey)| values_eq(ex, ey, depth + 1))
        }
        _ => false,
    }
}
