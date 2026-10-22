# undo.wisp -- a variable notebook with undo and redo, built from closures.
#
# The worked example of WISP closures. `fn (...) { ... }` written as an
# expression makes a function that remembers the variables around it.
# Here every change to the notebook is an *edit*: a little record holding
# two closures, "do" and "undo", made by make_edit(). They remember what
# that one edit needs -- the name, the new value, the value it replaced --
# in the scope make_edit() was running in, so undoing an edit twenty
# commands later still knows exactly what to put back, with no table of
# old values kept anywhere. The same idea runs through the rest:
# make_counter() hands out edit numbers from a variable only its closure
# can reach, and `find` and `top` give filter() and sort_by() a closure
# that remembers the prefix or the notebook they are working with.

let DIGITS = "0123456789";
let NAME_CHARS = "abcdefghijklmnopqrstuvwxyz_";
let MAX_HISTORY = 50;

let vars = {};      # the notebook: name -> int
let done = [];      # edits that have been applied, oldest first
let undone = [];    # edits that were undone, most recent last

# ---- a counter only its own closure can change -----------------------

fn make_counter() {
    let n = 0;
    return fn () {
        n = n + 1;
        return n;
    };
}

let next_id = make_counter();

# ---- input checking --------------------------------------------------

# Whether `word` is a whole number: an optional "-", then 1 to 9 digits.
fn is_number(word) {
    let digits = word;
    if starts_with(word, "-") {
        digits = word[1:];
    }
    if len(digits) < 1 or len(digits) > 9 {
        return false;
    }
    for c in digits {
        if not contains(DIGITS, c) {
            return false;
        }
    }
    return true;
}

# Whether `word` is a usable variable name: 1 to 16 letters or "_".
fn is_name(word) {
    if len(word) < 1 or len(word) > 16 {
        return false;
    }
    for c in word {
        if not contains(NAME_CHARS, c) {
            return false;
        }
    }
    return true;
}

# ---- edits -----------------------------------------------------------

# An edit that leaves `name` holding `value` (or, if `present` is false,
# not in the notebook at all). What `name` holds right now is remembered by
# the two closures below, which is what makes the edit undoable.
fn make_edit(label, name, present, value) {
    let was_present = contains(vars, name);
    let was = get(vars, name);

    # Both "do" and "undo" call this: a closure that another pair of
    # closures use.
    let write = fn (is_present, v) {
        if is_present {
            vars[name] = v;
        } elif contains(vars, name) {
            remove(vars, name);
        }
    };

    return {
        "id": next_id(),
        "label": label,
        "do": fn () { write(present, value); },
        "undo": fn () { write(was_present, was); }
    };
}

# Run an edit and keep it for undo. A new edit ends any chain of redos.
fn apply_edit(edit) {
    edit["do"]();
    push(done, edit);
    if len(done) > MAX_HISTORY {
        remove(done, 0);
    }
    undone = [];
    print(edit["label"]);
}

# ---- the commands: each takes the words after its name ---------------

fn cmd_set(rest) {
    if len(rest) != 2 or not is_name(lower(rest[0])) or not is_number(rest[1]) {
        print("Usage: set NAME NUMBER   (NAME: letters and _, NUMBER: up to 9 digits)");
        return nil;
    }
    let name = lower(rest[0]);
    apply_edit(make_edit("set " + name + " = " + rest[1], name, true, int(rest[1])));
}

fn cmd_add(rest) {
    if len(rest) != 2 or not is_name(lower(rest[0])) or not is_number(rest[1]) {
        print("Usage: add NAME NUMBER   (adds to NAME; a new NAME starts at 0)");
        return nil;
    }
    let name = lower(rest[0]);
    let total = get(vars, name, 0) + int(rest[1]);
    apply_edit(make_edit("add " + rest[1] + " to " + name + " (now " + str(total) + ")", name, true, total));
}

fn cmd_del(rest) {
    if len(rest) != 1 or not contains(vars, lower(rest[0])) {
        print("Usage: del NAME   (NAME must be in the notebook; 'show' lists them)");
        return nil;
    }
    let name = lower(rest[0]);
    apply_edit(make_edit("del " + name, name, false, nil));
}

fn cmd_undo(rest) {
    if len(done) == 0 {
        print("Nothing to undo.");
        return nil;
    }
    let edit = pop(done);
    edit["undo"]();
    push(undone, edit);
    print("Undid #" + str(edit["id"]) + ": " + edit["label"]);
}

fn cmd_redo(rest) {
    if len(undone) == 0 {
        print("Nothing to redo.");
        return nil;
    }
    let edit = pop(undone);
    edit["do"]();
    push(done, edit);
    print("Redid #" + str(edit["id"]) + ": " + edit["label"]);
}

fn cmd_show(rest) {
    if len(vars) == 0 {
        print("The notebook is empty.");
        return nil;
    }
    for name in keys(vars) {
        print(name + " = " + str(vars[name]));
    }
}

fn cmd_history(rest) {
    if len(done) == 0 {
        print("No edits yet.");
        return nil;
    }
    for edit in done {
        print("#" + str(edit["id"]) + " " + edit["label"]);
    }
    if len(undone) > 0 {
        print("(" + str(len(undone)) + " undone, ready to redo)");
    }
}

fn cmd_sum(rest) {
    print(reduce(fn (total, name) { return total + vars[name]; }, keys(vars), 0));
}

# The names starting with a prefix: the closure remembers the prefix.
fn cmd_find(rest) {
    if len(rest) != 1 {
        print("Usage: find PREFIX");
        return nil;
    }
    let prefix = lower(rest[0]);
    let hits = filter(fn (name) { return starts_with(name, prefix); }, keys(vars));
    if len(hits) == 0 {
        print("No names start with " + prefix + ".");
        return nil;
    }
    for name in hits {
        print(name + " = " + str(vars[name]));
    }
}

# Largest value first; names with equal values stay in alphabetical order.
fn cmd_top(rest) {
    let names = keys(vars);
    sort_by(names, fn (name) { return 0 - vars[name]; });
    for name in names {
        print(name + " = " + str(vars[name]));
    }
}

fn cmd_help(rest) {
    print("set NAME NUMBER     put NUMBER in NAME");
    print("add NAME NUMBER     add NUMBER to NAME (a new NAME starts at 0)");
    print("del NAME            remove NAME");
    print("undo / redo         step back / forward through your edits");
    print("show                every name and value");
    print("top                 the same, largest first");
    print("find PREFIX         the names starting with PREFIX");
    print("sum                 the total of all values");
    print("history             the edits so far");
    print("quit                leave");
}

# What you type, and the function that handles it.
let commands = {
    "set": cmd_set,
    "add": cmd_add,
    "del": cmd_del,
    "undo": cmd_undo,
    "redo": cmd_redo,
    "show": cmd_show,
    "top": cmd_top,
    "find": cmd_find,
    "sum": cmd_sum,
    "history": cmd_history,
    "help": cmd_help
};

print("UNDO -- a notebook of numbers with undo and redo. Type help for the commands.");

while true {
    let parts = split(input("undo> "));
    if len(parts) == 0 {
        continue;
    }
    let word = lower(parts[0]);
    if word == "quit" {
        break;
    }
    if contains(commands, word) {
        commands[word](parts[1:]);
    } else {
        print("Unknown command: " + parts[0] + ". Type help.");
    }
}

print("undo done");
