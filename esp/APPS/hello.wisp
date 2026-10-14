# hello.wisp -- the simplest possible WISP program.
#
# If you're trying WISP for the first time, start here: one input(),
# one string concatenation, one print(). Everything else in APPS/ builds
# up from these same three moves. See docs/WISP_LANGUAGE.md for the
# full language reference.

let name = input("What's your name? ");
print("Hello, " + name + "! Welcome to WISP, running straight from firmware.");
