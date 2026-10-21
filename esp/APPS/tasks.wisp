# tasks.wisp -- a to-do list driven by a table of functions.
#
# The worked example of WISP functions as values. A function's name, used
# where a value is expected, *is* that function: `commands` below is a map
# from what you type to the function that handles it, and
# `commands[word](rest)` looks one up and calls it -- so adding a command
# is writing a function and adding a line to the table, with no if/elif
# chain to extend. The same idea runs through the rest: `filter` keeps the
# tasks a function accepts, `map` turns every task into a line of text,
# `reduce` folds the tasks into one number, and `sort_by` orders them by
# the key a function computes.
#
# A task is a map: {"text": ..., "pri": 1-9, "done": true/false}. Priority
# 1 is the most urgent.

let DIGITS = "0123456789";
let tasks = [];

# ---- what the higher-order built-ins call ----------------------------

fn is_open(t) {
    return not t["done"];
}

fn priority(t) {
    return t["pri"];
}

fn describe(t) {
    let mark = "[ ]";
    if t["done"] {
        mark = "[x]";
    }
    return mark + " p" + str(t["pri"]) + " " + t["text"];
}

fn add_priority(total, t) {
    return total + t["pri"];
}

# ---- input checking --------------------------------------------------

# Whether `word` is a whole number of 1 to 4 digits.
fn is_number(word) {
    if len(word) < 1 or len(word) > 4 {
        return false;
    }
    for c in word {
        if not contains(DIGITS, c) {
            return false;
        }
    }
    return true;
}

# ---- the commands: each takes the words after its name ---------------

fn cmd_add(rest) {
    if len(rest) < 2 or not is_number(rest[0]) or int(rest[0]) < 1 or int(rest[0]) > 9 {
        print("Usage: add PRIORITY TEXT   (priority 1 to 9, 1 most urgent)");
        return nil;
    }
    push(tasks, {"text": join(rest[1:], " "), "pri": int(rest[0]), "done": false});
    print("Added task " + str(len(tasks)) + ".");
}

fn cmd_done(rest) {
    if len(rest) != 1 or not is_number(rest[0]) or int(rest[0]) < 1 or int(rest[0]) > len(tasks) {
        print("Usage: done N   (N is a task number from 'list', 1 to " + str(len(tasks)) + ")");
        return nil;
    }
    let t = tasks[int(rest[0]) - 1];
    t["done"] = true;
    print("Done: " + t["text"]);
}

fn cmd_list(rest) {
    if len(tasks) == 0 {
        print("No tasks.");
        return nil;
    }
    let lines = map(describe, tasks);
    for i in range(len(lines)) {
        print(str(i + 1) + ". " + lines[i]);
    }
}

fn cmd_open(rest) {
    let pending = filter(is_open, tasks);
    if len(pending) == 0 {
        print("Nothing left to do.");
        return nil;
    }
    for line in map(describe, pending) {
        print(line);
    }
}

fn cmd_sort(rest) {
    sort_by(tasks, priority);
    print("Sorted by priority; equal priorities keep their order.");
}

fn cmd_clear(rest) {
    let before = len(tasks);
    tasks = filter(is_open, tasks);
    print("Cleared " + str(before - len(tasks)) + " finished task(s).");
}

fn cmd_stats(rest) {
    let pending = filter(is_open, tasks);
    print(str(len(tasks)) + " task(s), " + str(len(pending)) + " open, open priority load "
        + str(reduce(add_priority, pending, 0)) + ".");
}

fn cmd_help(rest) {
    print("add PRIORITY TEXT   add a task (priority 1 to 9, 1 most urgent)");
    print("done N              finish task N");
    print("list                show every task");
    print("open                show the tasks still to do");
    print("sort                order by priority");
    print("clear               forget finished tasks");
    print("stats               counts and priority load");
    print("quit                leave");
}

# What you type, and the function that handles it.
let commands = {
    "add": cmd_add,
    "done": cmd_done,
    "list": cmd_list,
    "open": cmd_open,
    "sort": cmd_sort,
    "clear": cmd_clear,
    "stats": cmd_stats,
    "help": cmd_help
};

print("TASKS -- a to-do list. Type help for the commands.");

while true {
    let parts = split(input("tasks> "));
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

print("tasks done");
