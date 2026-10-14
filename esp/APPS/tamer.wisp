# tamer.wisp -- classify a creature's temperament from two 0-10 stats
# (aggression, size) using a real trained neural network: 2 inputs -> 4
# ReLU hidden units -> 1 linear output, thresholded. The training rule
# (never seen by the network directly, only via labeled examples) is
# XOR(aggression > 5, size > 5): a MATCHED pair (both calm/small or both
# intense/big) is Predictable; a MISMATCHED pair is Erratic. That's not
# linearly separable, which is why this needs a hidden layer at all --
# see tools/train_tiny_nn.py, which trains these exact weights with
# full-batch Adam and re-verifies 100% accuracy using WISP's own
# fixed-point integer arithmetic before they're transcribed below. See
# docs/WISP_AI.md for the general pattern this follows.

let SCALE = 1000;

# W1[j] = [weight from aggression, weight from size] feeding hidden unit
# j; B1[j] is that unit's bias. Both multiply plain exact-integer inputs
# (aggression/size are raw 0-10 stats, not fixed-point), so layer 1
# needs no fixed_mul -- see docs/WISP_AI.md.
let W1 = [[145, 198], [229, 189], [-47, 325], [47, 317]];
let B1 = [-872, -1068, -1514, -1991];

# W2[j] is the fixed-point-scaled weight from hidden unit j to the
# output; B2 is the output bias (0 after quantization here). Unlike
# layer 1, this multiplies a scaled weight against a scaled hidden
# activation, so it DOES need fixed_mul's divide-back-down step.
let W2 = [-1822, 1887, 3252, -3334];
let B2 = 0;

fn fixed_mul(a, b) {
    let prod = a * b;
    if prod >= 0 {
        return (prod + SCALE / 2) / SCALE;
    }
    return (prod - SCALE / 2) / SCALE;
}

fn relu(x) {
    if x > 0 {
        return x;
    }
    return 0;
}

fn classify(aggression, size) {
    let hidden = [];
    let j = 0;
    while j < len(B1) {
        let w = W1[j];
        let raw = w[0] * aggression + w[1] * size + B1[j];
        push(hidden, relu(raw));
        j = j + 1;
    }

    let y = B2;
    j = 0;
    while j < len(hidden) {
        y = y + fixed_mul(W2[j], hidden[j]);
        j = j + 1;
    }

    if y > (SCALE / 2) {
        return "Erratic";
    }
    return "Predictable";
}

fn read_stat(prompt) {
    while true {
        let text = input(prompt);
        if text == "quit" {
            return -1;
        }
        let v = int(text);
        if v >= 0 and v <= 10 {
            return v;
        }
        print("Please enter a stat from 0 to 10 (or 'quit').");
    }
}

print("tamer: classify a creature from two 0-10 stats.");
print("A matched pair (both low or both high) is Predictable;");
print("a mismatched pair is Erratic. Type 'quit' for either stat to exit.");

while true {
    let aggression = read_stat("Aggression (0-10): ");
    if aggression == -1 {
        break;
    }
    let size = read_stat("Size (0-10): ");
    if size == -1 {
        break;
    }
    print(classify(aggression, size));
}

print("tamer done.");
