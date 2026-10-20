//! Runtime value representation. Per `docs/WISP_LANGUAGE.md`, WISP has
//! five value types: `nil`, `bool`, `int`, `string`, and `list`.
//!
//! # Resource bounds
//!
//! Lists are shared by reference, so a handful of lists can describe a
//! graph whose *expansion* is astronomically large (`v = [v, v]` doubled
//! 22 times is 23 lists but 29 MB of display text). Every structural
//! operation here is therefore bounded by work or output, not just by
//! nesting depth:
//!
//! - display stops after [`MAX_DISPLAY_LEN`] bytes and marks the cut;
//! - equality visits each pair of lists at most once and gives up with an
//!   error after [`MAX_EQ_WORK`] element comparisons;
//! - dropping a list is iterative ([`ListCell`]'s `Drop`), so a deeply
//!   nested chain is freed without recursing on the kernel stack.
//!
//! Cycles (`let a = []; push(a, a);`) are allowed, and are reclaimed when
//! the script ends: every list a run creates is recorded in its
//! [`ListRegistry`], and [`ListRegistry::reclaim`] empties them all.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::Deref;

/// Recursion guard for structural operations over nested lists (equality,
/// display, serialization): bounding how deep they descend keeps a
/// pathologically nested value from overflowing the kernel stack. 64
/// levels is far beyond any real script's data.
pub(crate) const MAX_VALUE_DEPTH: u32 = 64;

/// Most bytes [`Value::to_display_string`] produces before cutting the
/// text off with [`TRUNCATION_MARKER`]. Far more than a screen holds.
pub const MAX_DISPLAY_LEN: usize = 16 * 1024;

/// Appended to display text that hit [`MAX_DISPLAY_LEN`].
pub const TRUNCATION_MARKER: &str = "...(truncated)";

/// Most element comparisons one `==`/`!=` may perform.
pub const MAX_EQ_WORK: usize = 100_000;

/// Longest string a script may build (by `+`). Strings are immutable and
/// shared, but each concatenation allocates a fresh copy, so this bounds
/// any single allocation a script can request through them.
pub const MAX_STR_LEN: usize = 256 * 1024;

/// Most elements one list may hold. Each element is 16 bytes, so this
/// bounds a single list's backing allocation to 512 KiB.
pub const MAX_LIST_LEN: usize = 32 * 1024;

/// The shared, mutable storage behind every `Value::List` handle.
///
/// A newtype rather than a bare `RefCell<Vec<Value>>` only so it can have
/// its own `Drop`: the default drop of a list whose last element is
/// another list (and so on) recurses once per level, and a script can
/// build such a chain in a loop far deeper than the kernel stack.
#[derive(Debug, Default)]
pub struct ListCell(RefCell<Vec<Value>>);

impl ListCell {
    pub fn new(items: Vec<Value>) -> Self {
        Self(RefCell::new(items))
    }
}

impl Deref for ListCell {
    type Target = RefCell<Vec<Value>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for ListCell {
    fn drop(&mut self) {
        // Move the elements onto an explicit work stack. A child list this
        // was the last owner of is unwrapped and its elements moved onto
        // the same stack, so it is then dropped empty -- its own `drop`
        // has nothing left to recurse into.
        let mut pending = core::mem::take(self.0.get_mut());
        while let Some(value) = pending.pop() {
            if let Value::List(rc) = value {
                if let Ok(cell) = Rc::try_unwrap(rc) {
                    let mut cell = cell;
                    pending.append(cell.0.get_mut());
                }
            }
        }
    }
}

/// A list handle, as stored in `Value::List`.
pub type List = Rc<ListCell>;

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
    /// A growable, shared, *mutable* list. Every `Value::List` clone (from
    /// copying it into a new variable, passing it as a function argument,
    /// etc.) is a handle to the *same* underlying vector, so
    /// `push`/`pop`/`list[i] = ...` through any one handle is visible
    /// through every other handle to it -- the same "shared, mutable
    /// collection" behavior most scripting languages give you for
    /// reference types.
    List(List),
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
    ///
    /// Output past [`MAX_DISPLAY_LEN`] bytes is cut off and ends with
    /// [`TRUNCATION_MARKER`]; see [`Value::try_display_string`] for a
    /// variant that reports that instead.
    pub fn to_display_string(&self) -> String {
        let (mut out, complete) = self.display_bounded();
        if !complete {
            out.push_str(TRUNCATION_MARKER);
        }
        out
    }

    /// How the interactive prompt shows a value: as
    /// [`Value::to_display_string`] does, except that a string is quoted
    /// even at the top level (as it already is inside a list), so the
    /// string `"1"` and the int `1` look different. Bounded the same way.
    pub fn to_echo_string(&self) -> String {
        let mut out = DisplayBuf { text: String::new() };
        let complete = self.write_nested(&mut out, 0);
        let mut text = out.text;
        if !complete {
            text.push_str(TRUNCATION_MARKER);
        }
        text
    }

    /// Like [`Value::to_display_string`], but `None` if the text would
    /// exceed [`MAX_DISPLAY_LEN`] -- for callers (`str()`) that must not
    /// silently hand back a shortened value as if it were complete.
    pub fn try_display_string(&self) -> Option<String> {
        match self.display_bounded() {
            (out, true) => Some(out),
            (_, false) => None,
        }
    }

    /// Render up to [`MAX_DISPLAY_LEN`] bytes; the flag is `false` if the
    /// text was cut short. Work is bounded by the output budget: every
    /// value visited emits at least one byte before descending further.
    fn display_bounded(&self) -> (String, bool) {
        let mut out = DisplayBuf { text: String::new() };
        let complete = match self {
            Value::Str(s) => out.push(s),
            other => other.write_nested(&mut out, 0),
        };
        (out.text, complete)
    }

    /// Returns `false` once the output budget is exhausted.
    fn write_nested(&self, out: &mut DisplayBuf, depth: u32) -> bool {
        match self {
            Value::Nil => out.push("nil"),
            Value::Bool(true) => out.push("true"),
            Value::Bool(false) => out.push("false"),
            Value::Int(i) => out.push(&format!("{i}")),
            Value::Str(s) => out.push("\"") && out.push(s) && out.push("\""),
            Value::List(items) => {
                if depth >= MAX_VALUE_DEPTH {
                    return out.push("[...]");
                }
                if !out.push("[") {
                    return false;
                }
                for (i, v) in items.borrow().iter().enumerate() {
                    if i > 0 && !out.push(", ") {
                        return false;
                    }
                    if !v.write_nested(out, depth + 1) {
                        return false;
                    }
                }
                out.push("]")
            }
        }
    }
}

/// Display output with a hard byte budget.
struct DisplayBuf {
    text: String,
}

impl DisplayBuf {
    /// Append as much of `s` as fits; `false` if any of it did not.
    fn push(&mut self, s: &str) -> bool {
        let room = MAX_DISPLAY_LEN - self.text.len();
        if s.len() <= room {
            self.text.push_str(s);
            return true;
        }
        let mut cut = room;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        self.text.push_str(&s[..cut]);
        false
    }
}

/// Structural equality, as used by `==`/`!=`.
///
/// Each pair of list identities is compared at most once: a pair met again
/// (through sharing, or around a cycle) is assumed equal for the rest of
/// the comparison, which is exactly bisimilarity -- two cyclic lists are
/// equal iff no finite path through them finds a difference. That keeps
/// shared graphs linear instead of exponential, and makes cyclic lists
/// compare meaningfully instead of failing at a depth limit.
///
/// `Err` if the comparison would exceed [`MAX_EQ_WORK`] element
/// comparisons or nest deeper than [`MAX_VALUE_DEPTH`].
pub fn try_equal(a: &Value, b: &Value) -> Result<bool, &'static str> {
    let mut state = EqState { seen: BTreeSet::new(), work: 0 };
    values_eq(a, b, 0, &mut state)
}

struct EqState {
    seen: BTreeSet<(usize, usize)>,
    work: usize,
}

fn values_eq(a: &Value, b: &Value, depth: u32, state: &mut EqState) -> Result<bool, &'static str> {
    state.work += 1;
    if state.work > MAX_EQ_WORK {
        return Err("comparison is too large (more than 100000 elements visited)");
    }
    match (a, b) {
        (Value::Nil, Value::Nil) => Ok(true),
        (Value::Bool(x), Value::Bool(y)) => Ok(x == y),
        (Value::Int(x), Value::Int(y)) => Ok(x == y),
        (Value::Str(x), Value::Str(y)) => Ok(x == y),
        (Value::List(x), Value::List(y)) => {
            if Rc::ptr_eq(x, y) {
                return Ok(true);
            }
            if !state.seen.insert((Rc::as_ptr(x) as usize, Rc::as_ptr(y) as usize)) {
                return Ok(true);
            }
            if depth >= MAX_VALUE_DEPTH {
                return Err("lists are nested too deeply to compare (over 64 levels)");
            }
            let xb = x.borrow();
            let yb = y.borrow();
            if xb.len() != yb.len() {
                return Ok(false);
            }
            for (ex, ey) in xb.iter().zip(yb.iter()) {
                if !values_eq(ex, ey, depth + 1, state)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

impl PartialEq for Value {
    /// Infallible convenience over [`try_equal`]: a comparison too large
    /// to finish counts as "not equal". The interpreter itself uses
    /// `try_equal` so a script sees the error instead.
    fn eq(&self, other: &Self) -> bool {
        try_equal(self, other).unwrap_or(false)
    }
}

/// Every list one script run creates, so the run's cyclic garbage can be
/// reclaimed when it ends.
///
/// Reference counting alone never frees a cycle: `push(a, a)` keeps `a`
/// alive after its last variable is gone. Rather than forbid cycles (the
/// language allows them) or add a tracing collector, the interpreter
/// records a weak handle to each list as it is created and, at the end of
/// the run, empties every list still alive. Nothing outside the run can
/// hold a list (values cross the boundary only by being serialized to the
/// save disk), so emptying them is unobservable -- and once emptied, no
/// list owns another, so every cycle falls apart and is freed.
pub struct ListRegistry {
    lists: Vec<Weak<ListCell>>,
    /// Size at which dead entries are next pruned; doubles with the live
    /// count so registration stays amortized O(1).
    prune_at: usize,
}

impl ListRegistry {
    const MIN_PRUNE_AT: usize = 1024;

    pub fn new() -> Self {
        Self { lists: Vec::new(), prune_at: Self::MIN_PRUNE_AT }
    }

    /// Create a list owned by this run.
    pub fn new_list(&mut self, items: Vec<Value>) -> Value {
        let list = Rc::new(ListCell::new(items));
        self.track(&list);
        Value::List(list)
    }

    /// Record `list`, and every list reachable from it, as owned by this
    /// run -- for values that arrive from outside the interpreter (`load`).
    /// Iterative, and each list identity is visited once.
    pub fn adopt(&mut self, value: &Value) {
        let mut seen = BTreeSet::new();
        let mut pending: Vec<List> = Vec::new();
        if let Value::List(l) = value {
            pending.push(l.clone());
        }
        while let Some(list) = pending.pop() {
            if !seen.insert(Rc::as_ptr(&list) as usize) {
                continue;
            }
            self.track(&list);
            for item in list.borrow().iter() {
                if let Value::List(child) = item {
                    pending.push(child.clone());
                }
            }
        }
    }

    fn track(&mut self, list: &List) {
        if self.lists.len() >= self.prune_at {
            self.lists.retain(|w| w.strong_count() > 0);
            self.prune_at = (self.lists.len() * 2).max(Self::MIN_PRUNE_AT);
        }
        self.lists.push(Rc::downgrade(list));
    }

    /// Empty every list this run created that is still alive, freeing all
    /// of the run's cyclic garbage. Dropping the moved-out elements may
    /// free other lists outright; [`ListCell`]'s iterative `Drop` keeps
    /// that from recursing, and their registry entries simply fail to
    /// upgrade when reached.
    pub fn reclaim(&mut self) {
        for weak in core::mem::take(&mut self.lists) {
            if let Some(list) = weak.upgrade() {
                let items = core::mem::take(&mut *list.borrow_mut());
                drop(list);
                drop(items);
            }
        }
        self.prune_at = Self::MIN_PRUNE_AT;
    }
}

impl Drop for ListRegistry {
    fn drop(&mut self) {
        self.reclaim();
    }
}
