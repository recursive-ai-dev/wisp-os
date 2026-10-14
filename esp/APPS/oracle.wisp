# oracle.wisp -- a fortune-telling oracle backed by a shuffle bag.
#
# Draws are without replacement until the bag empties, then the bag is
# refilled and reshuffled -- the standard "shuffle bag" pattern, so you
# see every fortune once before any repeat, rather than plain rand()
# picking with replacement (which can repeat immediately, or take a
# long time to surface a rare line).
#
# draw(bag) removes and returns one random element from bag, mutating it
# in place. WISP has no remove-at-index builtin, so this uses the
# standard workaround described in docs/WISP_LANGUAGE.md's "Lists"
# section: swap the chosen element with the last element, then pop().

fn draw(bag) {
    let i = rand(0, len(bag) - 1);
    let last = len(bag) - 1;
    let chosen = bag[i];
    bag[i] = bag[last];
    bag[last] = chosen;
    return pop(bag);
}

fn refill(bag, source) {
    let i = 0;
    while i < len(source) {
        push(bag, source[i]);
        i = i + 1;
    }
}

let fortunes = [
    "A stranger's advice will save you a week of trouble.",
    "The thing you're avoiding is smaller than you think.",
    "Patience now, a windfall before the season turns.",
    "Something you wrote will be read by more eyes than you expect.",
    "The next door you knock on opens easier than the last.",
    "An old plan deserves one more look before you bury it.",
    "You already know the answer; you're just waiting for permission.",
    "A short trip pays back more than a long one would.",
    "Guard the small promise -- it matters more than the big one.",
    "The quiet option is the correct one this time."
];

let bag = [];
refill(bag, fortunes);

print("The oracle is listening. Ask, and it will answer.");
print("Commands: ask, quit");

while true {
    let cmd = input("> ");

    if cmd == "quit" {
        print("The oracle falls silent.");
        break;
    } elif cmd == "ask" {
        if len(bag) == 0 {
            refill(bag, fortunes);
        }
        print(draw(bag));
    } else {
        print("The oracle only understands 'ask' and 'quit'.");
    }
}
