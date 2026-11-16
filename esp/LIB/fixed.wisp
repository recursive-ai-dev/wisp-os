# fixed.wisp -- 128-bit overflow-safe fixed-point arithmetic and formatting

let SCALE = 1000;

fn fixed_mul(a, b, scale) {
    return muldiv(a, b, scale);
}

fn fixed_div(a, b, scale) {
    return muldiv(a, scale, b);
}

fn to_text(n, scale) {
    return fixed(n, scale);
}

return {
    "SCALE": SCALE,
    "fixed_mul": fixed_mul,
    "fixed_div": fixed_div,
    "to_text": to_text
};
