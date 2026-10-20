# words.wisp -- a word counter: type some text, and it keeps a tally of
# every word you use.
#
# The worked example of WISP maps. `counts` maps each word to how many
# times it has been typed: counts[w] reads a tally, counts[w] = n writes
# one (adding the word the first time), get() reads a word that may not
# be there yet, contains() asks whether it is, and remove() forgets one.
# A map keeps its keys in order, so `for w in counts` visits the words
# alphabetically -- which is all `list` has to do.
#
# A line that is just a command (`top`, `list`, `quit`, or `count`/`forget`
# and one word) runs it; any other line is counted, so "top of the hill"
# is four words, not a command.

let LETTERS = "abcdefghijklmnopqrstuvwxyz0123456789'";
let counts = {};
let total = 0;

# Whether `c` (one character) can be part of a word: a letter, a digit or
# an apostrophe -- or any character outside ASCII, so that "café" keeps
# its "é". Everything else in ASCII is punctuation.
fn in_word(c) {
    return ord(c) > 127 or contains(LETTERS, c);
}

# A word as it is counted: lower case, without punctuation at either end,
# so "The", "the" and "the," are one word. "" if nothing is left. Walks
# characters (not bytes), so any text is safe to type.
fn clean(word) {
    let cs = chars(lower(word));
    let start = 0;
    let end = len(cs);
    while start < end and not in_word(cs[start]) {
        start = start + 1;
    }
    while end > start and not in_word(cs[end - 1]) {
        end = end - 1;
    }
    return join(cs[start:end], "");
}

fn times(n) {
    if n == 1 {
        return "once";
    } elif n == 2 {
        return "twice";
    }
    return str(n) + " times";
}

fn plural(n, noun) {
    if n == 1 {
        return "1 " + noun;
    }
    return str(n) + " " + noun + "s";
}

# Count every word on `line`; returns how many there were.
fn tally(line) {
    let added = 0;
    for piece in split(line) {
        let w = clean(piece);
        if w != "" {
            counts[w] = get(counts, w, 0) + 1;
            total = total + 1;
            added = added + 1;
        }
    }
    return added;
}

# The `n` most-used words, most-used first. Each pass picks the biggest
# tally not chosen yet; visiting the words in order and replacing the
# best only on a strictly bigger tally puts ties in alphabetical order.
fn top(n) {
    let chosen = {};
    let order = [];
    while len(order) < n and len(order) < len(counts) {
        let best = nil;
        for w in counts {
            if not contains(chosen, w) and (best == nil or counts[w] > counts[best]) {
                best = w;
            }
        }
        chosen[best] = true;
        push(order, best);
    }
    return order;
}

fn show_top() {
    if len(counts) == 0 {
        print("Nothing counted yet.");
        return;
    }
    print(plural(total, "word") + ", " + str(len(counts)) + " different. Most used:");
    for w in top(5) {
        print("  " + w + ": " + times(counts[w]));
    }
}

fn show_all() {
    if len(counts) == 0 {
        print("Nothing counted yet.");
        return;
    }
    for w in counts {
        print("  " + w + ": " + times(counts[w]));
    }
}

print("WORDS -- type some text and I'll count the words in it.");
print("Commands: top, list, count WORD, forget WORD, quit.");

while true {
    let line = input("words> ");
    let parts = split(line);
    if len(parts) == 0 {
        continue;
    }
    let command = lower(parts[0]);
    if len(parts) == 1 and command == "quit" {
        break;
    } elif len(parts) == 1 and command == "top" {
        show_top();
    } elif len(parts) == 1 and command == "list" {
        show_all();
    } elif len(parts) == 2 and command == "count" {
        let w = clean(parts[1]);
        if contains(counts, w) {
            print(w + ": " + times(counts[w]));
        } else {
            print("Not seen yet: " + parts[1]);
        }
    } elif len(parts) == 2 and command == "forget" {
        let w = clean(parts[1]);
        if contains(counts, w) {
            total = total - remove(counts, w);
            print("Forgot " + w + ".");
        } else {
            print("Not seen yet: " + parts[1]);
        }
    } else {
        let added = tally(line);
        print("Counted " + plural(added, "word") + ", " + plural(total, "word") + " in all.");
    }
}

print("words done");
