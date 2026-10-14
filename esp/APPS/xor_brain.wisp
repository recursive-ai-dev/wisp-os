# xor_brain.wisp -- the simplest possible trained-model demo: a 2-2-1
# ReLU network that computes XOR exactly, with small integer weights
# derived analytically (see docs/WISP_AI.md and tools/train_tiny_nn.py's
# part1_xor()) rather than learned by gradient descent. XOR needs a
# hidden layer + a real nonlinearity at all -- a single-layer network
# provably cannot separate it -- so this is the cleanest possible proof
# that "matrix multiply + ReLU, running as WISP forward-pass code"
# actually works, with zero training noise or fixed-point rounding to
# explain away: every weight here is an exact integer, so (unlike
# tamer.wisp) no SCALE or fixed_mul is needed at all.

fn relu(x) {
    if x > 0 {
        return x;
    }
    return 0;
}

fn xor_forward(x1, x2) {
    let h1 = relu(x1 + x2);
    let h2 = relu(x1 + x2 - 1);
    return h1 - 2 * h2;
}

fn read_bit(prompt) {
    while true {
        let text = input(prompt);
        if text == "quit" {
            return -1;
        }
        let v = int(text);
        if v == 0 or v == 1 {
            return v;
        }
        print("Please enter 0 or 1 (or 'quit').");
    }
}

print("xor_brain: a tiny hand-built neural net computing XOR.");
print("Enter two bits (0 or 1); type 'quit' for either to exit.");

while true {
    let x1 = read_bit("x1: ");
    if x1 == -1 {
        break;
    }
    let x2 = read_bit("x2: ");
    if x2 == -1 {
        break;
    }
    let y = xor_forward(x1, x2);
    print(str(x1) + " XOR " + str(x2) + " = " + str(y));
}

print("xor_brain done.");
