# primes.wisp -- prints prime numbers, forever, as fast as it can.
#
# A pure compute loop with no input() and no sleep_ms(): exactly the kind
# of program that, before v1.0, could only be ended by resetting the
# machine. Now Esc stops it, and the shell reports the line it was on.
#
# Trial division by 2 and then odd divisors up to the square root -- plain
# integer arithmetic, since WISP has no floating point.

fn is_prime(n) {
    if n < 2 {
        return false;
    }
    if n % 2 == 0 {
        return n == 2;
    }
    let d = 3;
    while d * d <= n {
        if n % d == 0 {
            return false;
        }
        d = d + 2;
    }
    return true;
}

print("Counting primes. Press Esc to stop.");

let n = 2;
let found = 0;
while true {
    if is_prime(n) {
        found = found + 1;
        print("prime #" + str(found) + ": " + str(n));
    }
    n = n + 1;
}
