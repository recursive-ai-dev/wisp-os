#!/usr/bin/env python3
"""
train_tiny_nn.py -- derives/trains the weights baked into two WISP demo
programs (esp/APPS/xor_brain.wisp and esp/APPS/tamer.wisp), and prints
them as WISP-ready fixed-point integers.

This script is NOT part of the shipped firmware -- WISP has no
training/backprop machinery at all, by design (a tree-walking interpreter
in a bare-metal shell is the wrong place for that). It's an offline dev
tool, kept in the repo purely for reproducibility: anyone can re-run this
(`python3 tools/train_tiny_nn.py`, needs only numpy) and regenerate the
exact constants embedded in the .wisp source files, rather than those
numbers being unexplained magic. Every set of weights below is verified,
at the end of this script, using the *exact* integer/fixed-point
arithmetic the shipped WISP code performs -- not the float math used to
train them -- since that's the number that actually matters once it
ships.

--------------------------------------------------------------------------
Part 1: xor_brain.wisp's weights -- derived analytically, not trained.

XOR is not linearly separable, which is *the* textbook reason a neural
net needs a hidden layer + nonlinearity at all. A 2-2-1 network with a
ReLU hidden layer solves it exactly with small integer weights (a
well-known closed-form construction, not something discovered by search
here). It's included first because it's the cleanest possible proof that
"matrix multiply + a real nonlinearity, running as WISP forward-pass
code" actually works, with zero training noise to explain away.

--------------------------------------------------------------------------
Part 2: tamer.wisp's weights -- genuinely trained (numpy, full-batch
Adam, analytic backprop -- no autodiff library, no torch) on a synthetic
dataset that is DELIBERATELY not linearly separable, for the same reason
as part 1: a plain single-layer perceptron provably cannot solve it, so
this network also earns its hidden layer rather than having one for
show (demonstrated explicitly below: perceptron first, then the 2-layer
net). Trained weights come out fractional, which is the representative
case for "run a real trained model in WISP": fixed-point (scaled-integer)
arithmetic, since WISP has no floats -- and, unlike part 1, genuinely
needs `fixed_mul`'s divide-back-down step in its output layer, because a
fixed-point weight there is multiplied against a fixed-point *hidden
activation* (not a plain exact-integer input the way layer 1's inputs
are), so the two SCALE factors actually compound and must be divided
back out.
"""

import numpy as np

SCALE = 1000  # fixed-point scale used by both demos' WISP source


def relu(x):
    return np.maximum(x, 0)


def _trunc_div(n: int, d: int) -> int:
    """Python's `//` is floor division (rounds toward -infinity). WISP's
    `/` compiles to Rust's `i64::checked_div`, which TRUNCATES toward
    zero -- a different result for negative operands. This reference
    implementation must match WISP's actual behavior, not Python's
    default, or "verified" fixed-point weights below could silently be
    wrong the moment a negative product is involved."""
    q = abs(n) // abs(d)
    return -q if (n < 0) != (d < 0) else q


def fixed_mul(a: int, b: int, scale: int = SCALE) -> int:
    """The exact integer operation `fixed_mul()` in tamer.wisp performs:
    multiply two SCALE-scaled fixed-point integers and divide back down
    by SCALE, rounding to nearest (not truncating) so error doesn't
    compound layer over layer. Uses `_trunc_div`, not `//`, to match
    WISP's truncating `/` -- see its docstring."""
    prod = a * b
    if prod >= 0:
        return _trunc_div(prod + scale // 2, scale)
    return _trunc_div(prod - scale // 2, scale)


# ---- Part 1: xor_brain.wisp ----------------------------------------------

def xor_forward(x1: int, x2: int) -> float:
    h1 = max(0, x1 + x2)
    h2 = max(0, x1 + x2 - 1)
    return h1 - 2 * h2


def part1_xor():
    print("=== xor_brain.wisp: analytic ReLU solution ===")
    print("h1 = ReLU(x1 + x2)")
    print("h2 = ReLU(x1 + x2 - 1)")
    print("y  = h1 - 2*h2")
    print()
    print("Truth table check:")
    ok = True
    for x1 in (0, 1):
        for x2 in (0, 1):
            y = xor_forward(x1, x2)
            expected = x1 ^ x2
            match = "OK" if y == expected else "MISMATCH"
            ok = ok and y == expected
            print(f"  xor({x1}, {x2}) = {y}  (expected {expected})  [{match}]")
    assert ok, "analytic XOR construction failed its own truth table -- do not ship"
    print("All weights are plain integers (1, -1, -2, 0) -- no fixed-point")
    print("scaling needed for this one.")
    print()


# ---- Part 2: tamer.wisp ---------------------------------------------------

# Ground-truth rule used only to LABEL the synthetic training set (the
# network never sees this rule directly, only the labeled points): a
# creature is "erratic" (dangerous) if exactly one of its two stats is
# above the midpoint -- a matched pair (both calm or both intense) is
# predictable and safe; a mismatched pair is not. This is XOR over two
# thresholds, so -- like part 1 -- it is NOT linearly separable, which is
# demonstrated explicitly below by first showing a plain single-layer
# perceptron fail on it before the 2-layer network succeeds.
def build_dataset():
    a, s = np.meshgrid(np.arange(11), np.arange(11), indexing="ij")
    a, s = a.flatten(), s.flatten()
    labels = ((a > 5) != (s > 5)).astype(np.float64)
    return a, s, labels


def train_perceptron(a, s, labels, epochs=300, lr=0.05, seed=1234):
    """A plain single-layer perceptron -- included only to demonstrate it
    CAN'T solve this dataset, motivating the hidden layer below."""
    rng = np.random.default_rng(seed)
    w1, w2, b = rng.uniform(-0.1, 0.1, 3)
    for _ in range(epochs):
        for ai, si, lab in zip(a, s, labels):
            z = w1 * ai + w2 * si + b
            pred = 1.0 if z > 0 else 0.0
            err = lab - pred
            if err != 0:
                w1 += lr * err * ai
                w2 += lr * err * si
                b += lr * err
    z = w1 * a + w2 * s + b
    pred = (z > 0).astype(np.float64)
    return (pred == labels).mean()


def train_mlp(a, s, labels, hidden=4, seed=6, epochs=3000, lr=0.02):
    """2 inputs -> `hidden` ReLU units -> 1 linear output. Trained with
    full-batch Adam (analytic backprop, hand-rolled -- see the update
    rule below) against a 0.0/1.0 regression target, thresholded at 0.5.
    Inputs are normalized to [0, 1] for training stability (raw 0..10
    inputs times randomly-initialized weights produce large, unstable
    early gradients otherwise); the /10 normalization is folded directly
    into W1 afterward so the deployed WISP code can use the raw integer
    stats with no normalization step of its own.
    """
    x = np.stack([a / 10.0, s / 10.0], axis=1)  # (121, 2), normalized

    rng = np.random.default_rng(seed)
    W1 = rng.normal(0, 1.0, size=(2, hidden))
    b1 = np.zeros(hidden)
    W2 = rng.normal(0, 1.0, size=(hidden,))
    b2 = 0.0

    mW1, vW1 = np.zeros_like(W1), np.zeros_like(W1)
    mb1, vb1 = np.zeros_like(b1), np.zeros_like(b1)
    mW2, vW2 = np.zeros_like(W2), np.zeros_like(W2)
    mb2 = vb2 = 0.0
    beta1, beta2, eps = 0.9, 0.999, 1e-8

    for t in range(1, epochs + 1):
        h_raw = x @ W1 + b1
        h = relu(h_raw)
        y = h @ W2 + b2
        dy = (y - labels) / len(labels)

        gW2 = h.T @ dy
        gb2 = dy.sum()
        d_h = np.outer(dy, W2) * (h_raw > 0)
        gW1 = x.T @ d_h
        gb1 = d_h.sum(axis=0)

        for param, grad, m, v in ((W1, gW1, mW1, vW1), (b1, gb1, mb1, vb1), (W2, gW2, mW2, vW2)):
            m[:] = beta1 * m + (1 - beta1) * grad
            v[:] = beta2 * v + (1 - beta2) * grad * grad
            mhat, vhat = m / (1 - beta1**t), v / (1 - beta2**t)
            param -= lr * mhat / (np.sqrt(vhat) + eps)
        mb2 = beta1 * mb2 + (1 - beta1) * gb2
        vb2 = beta2 * vb2 + (1 - beta2) * gb2 * gb2
        mhat, vhat = mb2 / (1 - beta1**t), vb2 / (1 - beta2**t)
        b2 -= lr * mhat / (np.sqrt(vhat) + eps)

    h = relu(x @ W1 + b1)
    y = h @ W2 + b2
    acc = ((y > 0.5).astype(np.float64) == labels).mean()

    # Fold the /10 input normalization into W1 so the deployed network
    # can be evaluated directly on raw 0..10 integer inputs.
    W1_folded = W1 / 10.0
    return acc, W1_folded, b1, W2, b2


def part2_tamer():
    print("=== tamer.wisp: a 2-hidden-unit ReLU network, actually trained ===")
    a, s, labels = build_dataset()
    print(f"dataset: {len(labels)} points, rule = XOR(aggression > 5, size > 5)")
    print()

    print("First, a plain single-layer perceptron (no hidden layer) on the same data:")
    perceptron_acc = train_perceptron(a, s, labels)
    print(f"  accuracy: {perceptron_acc * 100:.2f}% "
          f"(capped well below 100% -- this dataset genuinely isn't linearly separable)")
    print()

    print("Now a 4-hidden-unit ReLU network (full-batch Adam) on the same data:")
    acc, W1, b1, W2, b2 = train_mlp(a, s, labels)
    print(f"  final training accuracy: {acc * 100:.2f}%")
    assert acc == 1.0, "MLP did not reach 100% -- do not ship unverified weights"
    print(f"  W1 (input->hidden, folded /10):\n{W1}")
    print(f"  b1 (hidden bias): {b1}")
    print(f"  W2 (hidden->output): {W2}")
    print(f"  b2 (output bias): {b2}")
    print()

    print(f"Quantizing to fixed-point (SCALE={SCALE}) for WISP:")
    fW1 = np.round(W1 * SCALE).astype(int)
    fb1 = np.round(b1 * SCALE).astype(int)
    fW2 = np.round(W2 * SCALE).astype(int)
    fb2 = int(round(b2 * SCALE))
    hidden = len(b1)
    for j in range(hidden):
        print(f"  hidden[{j}]: w=({fW1[0][j]}, {fW1[1][j]})  b={fb1[j]}")
    print(f"  output:    w={list(fW2)}  b={fb2}")

    # Re-verify using the SAME integer math tamer.wisp will actually run:
    # layer 1 is (scaled weight) * (plain exact-integer input), which
    # needs no fixed_mul (the SCALE factor just carries straight
    # through additively); layer 2 is (scaled weight) * (SCALE-scaled
    # hidden activation), which DOES need fixed_mul's divide-back-down.
    correct = 0
    for ai, si, lab in zip(a, s, labels):
        ai, si = int(ai), int(si)
        h_raw = [fW1[0][j] * ai + fW1[1][j] * si + fb1[j] for j in range(hidden)]
        h = [max(0, v) for v in h_raw]  # ReLU on a scaled quantity is still scaled
        y = sum(fixed_mul(int(fW2[j]), h[j]) for j in range(hidden)) + fb2
        pred = 1 if y > (SCALE // 2) else 0  # threshold at fixed-point 0.5
        correct += pred == lab
    fixed_acc = correct / len(labels)
    print(f"  fixed-point-quantized accuracy (WISP's exact integer math): {fixed_acc * 100:.2f}%")
    assert fixed_acc == 1.0, "quantized weights lost accuracy -- widen SCALE and retry"
    print()


if __name__ == "__main__":
    part1_xor()
    part2_tamer()
