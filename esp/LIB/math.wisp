# math.wisp -- integer number theory, clamping, roots, and powers

fn gcd(a, b) {
    a = abs(a);
    b = abs(b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    return a;
}

fn lcm(a, b) {
    if a == 0 or b == 0 {
        return 0;
    }
    let g = gcd(a, b);
    return abs(muldiv(a, b, g));
}

fn clamp(x, lo, hi) {
    if lo > hi {
        error("clamp() lower bound exceeds upper bound");
    }
    if x < lo {
        return lo;
    }
    if x > hi {
        return hi;
    }
    return x;
}

fn int_sqrt(n) {
    return isqrt(n);
}

fn power(base, exp) {
    return pow(base, exp);
}

fn abs_diff(a, b) {
    if a >= b {
        return a - b;
    }
    return b - a;
}

return {
    "gcd": gcd,
    "lcm": lcm,
    "clamp": clamp,
    "isqrt": int_sqrt,
    "power": power,
    "abs_diff": abs_diff
};
