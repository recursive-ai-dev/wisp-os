//! Runtime value representation. Per `docs/WISP_LANGUAGE.md`, WISP has
//! seven value types: `nil`, `bool`, `int`, `string`, `list`, `map` and
//! `func`.
//!
//! # Resource bounds
//!
//! Lists and maps are shared by reference, so a handful of them can
//! describe a graph whose *expansion* is astronomically large
//! (`v = [v, v]` doubled 22 times is 23 lists but 29 MB of display text).
//! Every structural operation here is therefore bounded by work or
//! output, not just by nesting depth:
//!
//! - display stops after [`MAX_DISPLAY_LEN`] bytes and marks the cut;
//! - equality visits each pair of lists (or maps) at most once and gives
//!   up with an error after [`MAX_EQ_WORK`] element comparisons;
//! - dropping a list or map is iterative ([`release`]), so a deeply
//!   nested chain is freed without recursing on the kernel stack.
//!
//! Cycles (`let a = []; push(a, a);`, `m["me"] = m;`) are allowed, and are
//! reclaimed when the script ends: every list and map a run creates is
//! recorded in its [`ObjectRegistry`], and [`ObjectRegistry::reclaim`]
//! empties them all.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::Deref;

/// Recursion guard for structural operations over nested lists and maps (equality,
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

/// Most entries one map may hold -- the same bound as a list's elements.
/// An entry costs a key and a value (32 bytes) plus its share of the
/// tree's nodes, so a full map is around 1.5 MiB: large, but within what
/// the shell's heap reserve check stops a script at.
pub const MAX_MAP_LEN: usize = 32 * 1024;

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
        release(core::mem::take(self.0.get_mut()));
    }
}

/// A list handle, as stored in `Value::List`.
pub type List = Rc<ListCell>;

/// A map key. Only ints and strings can be keys: they are the two types
/// with an order (`<`), and a key must never change once it is in a map,
/// which rules out the shared, mutable list and map.
///
/// The derived order -- every int before every string, ints numerically,
/// strings by byte -- is the order a map's entries are kept, displayed and
/// visited in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapKey {
    Int(i64),
    Str(Rc<String>),
}

impl MapKey {
    /// The key for `value`, or `None` if that type cannot be a key.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Int(i) => Some(MapKey::Int(*i)),
            Value::Str(s) => Some(MapKey::Str(s.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Int(i) => Value::Int(*i),
            MapKey::Str(s) => Value::Str(s.clone()),
        }
    }

    /// The key as a script would write it: `3`, `"gold"`. For error
    /// messages; a string key is cut short past 64 bytes.
    pub fn describe(&self) -> String {
        match self {
            MapKey::Int(i) => format!("{i}"),
            MapKey::Str(s) => {
                let mut cut = s.len().min(64);
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                let more = if cut < s.len() { "..." } else { "" };
                format!("\"{}{more}\"", &s[..cut])
            }
        }
    }
}

/// The shared, mutable storage behind every `Value::Map` handle; a newtype
/// for the same iterative-`Drop` reason as [`ListCell`].
#[derive(Debug, Default)]
pub struct MapCell(RefCell<BTreeMap<MapKey, Value>>);

impl MapCell {
    pub fn new(entries: BTreeMap<MapKey, Value>) -> Self {
        Self(RefCell::new(entries))
    }
}

impl Deref for MapCell {
    type Target = RefCell<BTreeMap<MapKey, Value>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for MapCell {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        pending.extend(core::mem::take(self.0.get_mut()).into_values());
        release(pending);
    }
}

/// A map handle, as stored in `Value::Map`.
pub type Map = Rc<MapCell>;

/// Drop `pending` without recursing. A list or map in it that this was the
/// last owner of is unwrapped and its contents moved onto the same work
/// stack, so the cell itself is then dropped empty -- its own `drop` has
/// nothing left to recurse into.
fn release(mut pending: Vec<Value>) {
    while let Some(value) = pending.pop() {
        match value {
            Value::List(rc) => {
                if let Ok(mut cell) = Rc::try_unwrap(rc) {
                    pending.append(cell.0.get_mut());
                }
            }
            Value::Map(rc) => {
                if let Ok(mut cell) = Rc::try_unwrap(rc) {
                    pending.extend(core::mem::take(cell.0.get_mut()).into_values());
                }
            }
            _ => {}
        }
    }
}

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
    /// A shared, mutable map from int or string keys to values, kept in
    /// key order (see [`MapKey`]). Shared on copy exactly as a list is.
    Map(Map),
    /// A function used as a value: the *name* of a user function or
    /// built-in. Looked up when it is called, not when it is made, so it
    /// holds no reference into the session (it cannot take part in a
    /// cycle, needs no registry entry, and a function redefined at the
    /// `wisp>` prompt is the new one wherever it was stored). There are
    /// no closures: it carries nothing but the name.
    Func(Rc<String>),
}

impl Value {
    pub const fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Func(_) => "func",
        }
    }

    /// The `str()` builtin's conversion, also used to build `print()`
    /// output and error messages that quote a value back to the user.
    /// A bare top-level `string` displays without quotes (so
    /// `print("hi")` prints `hi`, not `"hi"`); a `string` *nested* inside
    /// a list or map displays quoted, so `print([1, "a"])` reads
    /// unambiguously as `[1, "a"]` rather than `[1, a]`.
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
            Value::Map(entries) => write_map(entries, out, depth),
            Value::Func(name) => out.push("<fn ") && out.push(name) && out.push(">"),
        }
    }
}

/// [`Value::write_nested`]'s map arm, outlined so that its locals are not
/// paid at every level of list nesting too.
#[inline(never)]
fn write_map(entries: &Map, out: &mut DisplayBuf, depth: u32) -> bool {
    if depth >= MAX_VALUE_DEPTH {
        return out.push("{...}");
    }
    if !out.push("{") {
        return false;
    }
    for (i, (key, v)) in entries.borrow().iter().enumerate() {
        if i > 0 && !out.push(", ") {
            return false;
        }
        let key_written = match key {
            MapKey::Int(k) => out.push(&format!("{k}")),
            MapKey::Str(k) => out.push("\"") && out.push(k) && out.push("\""),
        };
        if !key_written || !out.push(": ") || !v.write_nested(out, depth + 1) {
            return false;
        }
    }
    out.push("}")
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
/// Two maps are equal when they hold the same keys with equal values.
/// Each pair of list (or map) identities is compared at most once: a pair met again
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
        (Value::Map(x), Value::Map(y)) => maps_eq(x, y, depth, state),
        (Value::Func(x), Value::Func(y)) => Ok(x == y),
        _ => Ok(false),
    }
}

/// [`values_eq`]'s map arm, outlined for the same reason as [`write_map`].
#[inline(never)]
fn maps_eq(x: &Map, y: &Map, depth: u32, state: &mut EqState) -> Result<bool, &'static str> {
    if Rc::ptr_eq(x, y) {
        return Ok(true);
    }
    if !state.seen.insert((Rc::as_ptr(x) as usize, Rc::as_ptr(y) as usize)) {
        return Ok(true);
    }
    if depth >= MAX_VALUE_DEPTH {
        return Err("maps are nested too deeply to compare (over 64 levels)");
    }
    let xb = x.borrow();
    let yb = y.borrow();
    if xb.len() != yb.len() {
        return Ok(false);
    }
    // Both iterate in key order, so equal maps pair up entry by entry.
    for ((kx, vx), (ky, vy)) in xb.iter().zip(yb.iter()) {
        if kx != ky || !values_eq(vx, vy, depth + 1, state)? {
            return Ok(false);
        }
    }
    Ok(true)
}

impl PartialEq for Value {
    /// Infallible convenience over [`try_equal`]: a comparison too large
    /// to finish counts as "not equal". The interpreter itself uses
    /// `try_equal` so a script sees the error instead.
    fn eq(&self, other: &Self) -> bool {
        try_equal(self, other).unwrap_or(false)
    }
}

/// Every list and map one script run creates, so the run's cyclic garbage
/// can be reclaimed when it ends.
///
/// Reference counting alone never frees a cycle: `push(a, a)` keeps `a`
/// alive after its last variable is gone. Rather than forbid cycles (the
/// language allows them) or add a tracing collector, the interpreter
/// records a weak handle to each list and map as it is created and, at the
/// end of the run, empties every one still alive. Nothing outside the run
/// can hold one (values cross the boundary only by being serialized to the
/// save disk), so emptying them is unobservable -- and once emptied, none
/// owns another, so every cycle falls apart and is freed.
pub struct ObjectRegistry {
    objects: Vec<Tracked>,
    /// Size at which dead entries are next pruned; doubles with the live
    /// count so registration stays amortized O(1).
    prune_at: usize,
}

enum Tracked {
    List(Weak<ListCell>),
    Map(Weak<MapCell>),
}

impl Tracked {
    fn is_alive(&self) -> bool {
        match self {
            Tracked::List(w) => w.strong_count() > 0,
            Tracked::Map(w) => w.strong_count() > 0,
        }
    }
}

impl ObjectRegistry {
    const MIN_PRUNE_AT: usize = 1024;

    pub fn new() -> Self {
        Self { objects: Vec::new(), prune_at: Self::MIN_PRUNE_AT }
    }

    /// Create a list owned by this run.
    pub fn new_list(&mut self, items: Vec<Value>) -> Value {
        let list = Rc::new(ListCell::new(items));
        self.track(Tracked::List(Rc::downgrade(&list)));
        Value::List(list)
    }

    /// Create a map owned by this run.
    pub fn new_map(&mut self, entries: BTreeMap<MapKey, Value>) -> Value {
        let map = Rc::new(MapCell::new(entries));
        self.track(Tracked::Map(Rc::downgrade(&map)));
        Value::Map(map)
    }

    /// Record `value`, and every list and map reachable from it, as owned
    /// by this run -- for values that arrive from outside the interpreter
    /// (`load`). Iterative, and each identity is visited once. Outlined: it
    /// runs once per `load`, and inlined it would enlarge the frame of
    /// every WISP function call.
    #[inline(never)]
    pub fn adopt(&mut self, value: &Value) {
        let mut seen = BTreeSet::new();
        let mut pending: Vec<Value> = Vec::new();
        pending.push(value.clone());
        while let Some(value) = pending.pop() {
            match value {
                Value::List(list) => {
                    if seen.insert(Rc::as_ptr(&list) as usize) {
                        self.track(Tracked::List(Rc::downgrade(&list)));
                        pending.extend(list.borrow().iter().filter(|v| is_container(v)).cloned());
                    }
                }
                Value::Map(map) => {
                    if seen.insert(Rc::as_ptr(&map) as usize) {
                        self.track(Tracked::Map(Rc::downgrade(&map)));
                        pending.extend(map.borrow().values().filter(|v| is_container(v)).cloned());
                    }
                }
                _ => {}
            }
        }
    }

    fn track(&mut self, object: Tracked) {
        if self.objects.len() >= self.prune_at {
            self.objects.retain(Tracked::is_alive);
            self.prune_at = (self.objects.len() * 2).max(Self::MIN_PRUNE_AT);
        }
        self.objects.push(object);
    }

    /// Empty every list and map this run created that is still alive,
    /// freeing all of the run's cyclic garbage. Dropping the moved-out
    /// contents may free other lists and maps outright; [`release`] keeps
    /// that from recursing, and their registry entries simply fail to
    /// upgrade when reached.
    pub fn reclaim(&mut self) {
        for object in core::mem::take(&mut self.objects) {
            match object {
                Tracked::List(weak) => {
                    if let Some(list) = weak.upgrade() {
                        let items = core::mem::take(&mut *list.borrow_mut());
                        drop(list);
                        release(items);
                    }
                }
                Tracked::Map(weak) => {
                    if let Some(map) = weak.upgrade() {
                        let entries = core::mem::take(&mut *map.borrow_mut());
                        drop(map);
                        let mut values = Vec::new();
                        values.extend(entries.into_values());
                        release(values);
                    }
                }
            }
        }
        self.prune_at = Self::MIN_PRUNE_AT;
    }
}

impl Drop for ObjectRegistry {
    fn drop(&mut self) {
        self.reclaim();
    }
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::List(_) | Value::Map(_))
}
