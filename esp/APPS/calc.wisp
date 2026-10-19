# calc.wisp -- an integer calculator: + - * / % and parentheses, with the
# usual precedence, so "2 + 3 * (4 - 1)" is 11.
#
# It is a small recursive-descent parser written in WISP itself, built on
# for-loops over strings, string indexing, contains() and trim(). WISP has
# no exceptions, so every way an expression can go wrong is caught here
# and reported, and the calculator keeps running: numbers are limited to
# 18 digits and every result to LIMIT, which keeps every operation far
# from integer overflow, and parentheses and minus signs may nest at most
# MAX_DEPTH deep, well inside WISP's own recursion limit.

let DIGITS = "0123456789";
let OPERATORS = "+-*/%()";
let LIMIT = 1000000000000000000;
let MAX_DEPTH = 40;

# The parser's state, shared by its functions through globals (WISP
# functions see globals, never each other's locals).
let tokens = [];
let pos = 0;
let depth = 0;
let problem = "";

# Split a line into tokens: each number a string of digits, each operator a
# one-character string. Returns nil after explaining a character it does
# not understand.
fn tokenize(text) {
    let found = [];
    let number = "";
    for c in text {
        if contains(DIGITS, c) {
            number = number + c;
            continue;
        }
        if number != "" {
            push(found, number);
            number = "";
        }
        if contains(OPERATORS, c) {
            push(found, c);
        } elif c != " " {
            print("I don't know what '" + c + "' means here.");
            return nil;
        }
    }
    if number != "" {
        push(found, number);
    }
    return found;
}

fn peek() {
    if pos < len(tokens) {
        return tokens[pos];
    }
    return "";
}

# Record the first problem found; the result no longer matters after one.
fn fail(message) {
    if problem == "" {
        problem = message;
    }
    return 0;
}

fn checked(value) {
    if abs(value) > LIMIT {
        return fail("the result is too large (the limit is " + str(LIMIT) + ")");
    }
    return value;
}

# expression := term (("+" | "-") term)*
fn expression() {
    let value = term();
    while problem == "" and (peek() == "+" or peek() == "-") {
        let op = peek();
        pos = pos + 1;
        let rhs = term();
        if op == "+" {
            value = checked(value + rhs);
        } else {
            value = checked(value - rhs);
        }
    }
    return value;
}

# term := factor (("*" | "/" | "%") factor)*
fn term() {
    let value = factor();
    while problem == "" and (peek() == "*" or peek() == "/" or peek() == "%") {
        let op = peek();
        pos = pos + 1;
        let rhs = factor();
        if problem != "" {
            return 0;
        }
        if op == "*" {
            if rhs != 0 and abs(value) > LIMIT / abs(rhs) {
                return fail("the result is too large (the limit is " + str(LIMIT) + ")");
            }
            value = value * rhs;
        } elif rhs == 0 {
            return fail("division by zero");
        } elif op == "/" {
            value = value / rhs;
        } else {
            value = value % rhs;
        }
    }
    return value;
}

# factor := number | "-" factor | "(" expression ")"
fn factor() {
    let t = peek();
    if t == "" {
        return fail("the expression ends too early");
    }
    pos = pos + 1;
    if t == "-" or t == "(" {
        # Both recurse, so both count towards the nesting limit.
        depth = depth + 1;
        if depth > MAX_DEPTH {
            return fail("the expression nests too deeply");
        }
    }
    if t == "-" {
        let negated = -factor();
        depth = depth - 1;
        return negated;
    }
    if t == "(" {
        let value = expression();
        if problem == "" and peek() != ")" {
            return fail("a '(' is missing its ')'");
        }
        pos = pos + 1;
        depth = depth - 1;
        return value;
    }
    if contains(DIGITS, t[0]) {
        if len(t) > 18 {
            return fail("numbers may have at most 18 digits");
        }
        return int(t);
    }
    return fail("unexpected '" + t + "'");
}

print("calc: integer arithmetic with + - * / % and parentheses.");
print("Type an expression, or 'quit' to leave.");
while true {
    let line = trim(input("calc> "));
    if line == "quit" {
        break;
    }
    if line == "" {
        continue;
    }
    let found = tokenize(line);
    if found == nil {
        continue;
    }
    tokens = found;
    pos = 0;
    depth = 0;
    problem = "";
    let value = expression();
    if problem == "" and pos < len(tokens) {
        problem = "unexpected '" + tokens[pos] + "'";
    }
    if problem == "" {
        print("= " + str(value));
    } else {
        print("error: " + problem);
    }
}
