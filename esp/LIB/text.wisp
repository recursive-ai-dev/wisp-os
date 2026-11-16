# text.wisp -- console word-wrapping and bordered text formatting

fn wrap_lines(text, width) {
    if width <= 0 {
        error("wrap_lines() width must be positive");
    }
    let out = [];
    for raw_line in split(str(text), "\n") {
        let words = split(raw_line, " ");
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
                push(out, current);
                current = w;
            }
        }
        push(out, current);
    }
    return out;
}

fn word_wrap(text, width) {
    return join(wrap_lines(text, width), "\n");
}

return {
    "wrap_lines": wrap_lines,
    "word_wrap": word_wrap
};
