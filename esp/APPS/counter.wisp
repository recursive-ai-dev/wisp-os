# counter.wisp -- the simplest possible proof that save()/load() actually
# survive a reboot.
#
# Run it once, quit the shell (or just reboot the machine), run it again:
# the number goes up, because it was really written to the AHCI save disk
# (see CHANGES.md's Phase 2, Part B entry) and really read back on the
# next boot -- not just held in RAM, which resets to nothing every time
# firmware hands off to this kernel from scratch.
#
# load() returns nil for a key that was never saved -- not an error -- so
# "first ever run, nothing saved yet" and "saved before, load it back" are
# just the two branches of one ordinary if/else, exactly like checking any
# other value against nil elsewhere in a WISP script.

let count = load("boot_count");

if count == nil {
    print("This is the first time counter.wisp has ever been run.");
    count = 0;
} else {
    print("Welcome back.");
}

count = count + 1;
save("boot_count", count);

if count == 1 {
    print("You have run this 1 time.");
} else {
    print("You have run this " + str(count) + " times.");
}

print("Run it again -- even after a full reboot -- and the count keeps climbing.");
