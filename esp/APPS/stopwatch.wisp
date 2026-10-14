# stopwatch.wisp -- a stopwatch that runs until you stop it.
#
# Shows off two v1.0 additions together:
#   - time_ms(), the milliseconds-since-boot clock, used here to measure
#     real elapsed time instead of trusting that each sleep_ms() took
#     exactly as long as asked (it rounds up to whole 10 ms ticks);
#   - the Esc key, which stops any running program. This one deliberately
#     never ends on its own -- `while true` -- so Esc is the only way out,
#     and a long sleep_ms() wakes up for it straight away.

let start = time_ms();
let lap = 0;

print("Stopwatch started. Press Esc to stop.");

while true {
    sleep_ms(1000);
    lap = lap + 1;
    let elapsed = time_ms() - start;
    let seconds = elapsed / 1000;
    let tenths = (elapsed % 1000) / 100;
    print("lap " + str(lap) + ": " + str(seconds) + "." + str(tenths) + " s elapsed");
}
