# strings.wisp -- string padding, centering, wrapping, and formatted joining

fn pad_left_with(s, width, fill) {
    return pad_left(s, width, fill);
}

fn pad_right_with(s, width, fill) {
    return pad_right(s, width, fill);
}

fn center(s, width, fill) {
    let text = str(s);
    let n = len(text);
    if width <= n {
        return text;
    }
    let total = width - n;
    let left = total / 2;
    let right = total - left;
    return repeat(fill, left) + text + repeat(fill, right);
}

fn wrap(text, width) {
    if width <= 0 {
        error("wrap() width must be positive");
    }
    let words = split(str(text), " ");
    let lines = [];
    let current = "";
    for w in words {
        if w == "" {
            continue;
        }
        if current == "" {
            current = w;
        } elif len(current) + 1 + len(w) <= width {
            current = current + " " + w;
        } else {
            push(lines, current);
            current = w;
        }
    }
    if current != "" {
        push(lines, current);
    }
    return join(lines, "\n");
}

fn join_fmt(items, sep, template) {
    let parts = [];
    for item in items {
        push(parts, fmt(template, item));
    }
    return join(parts, sep);
}

return {
    "pad_left": pad_left_with,
    "pad_right": pad_right_with,
    "center": center,
    "wrap": wrap,
    "join_fmt": join_fmt
};
