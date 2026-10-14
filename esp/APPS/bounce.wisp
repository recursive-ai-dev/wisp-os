# bounce.wisp -- a short, fully automatic demo: one square bouncing
# around the screen, showing off WISP v2's graphics primitives
# (fill_screen, rect, screen_width, screen_height) alongside the ordinary
# control flow and arithmetic that was already here in v1.
#
# No input needed -- just run it and watch. It ends on its own after a
# fixed number of frames rather than running forever, so it's safe to
# 'run bounce' without needing to know how to interrupt it.

let w = screen_width();
let h = screen_height();

let size = 30;
let x = 40;
let y = 40;
let dx = 4;
let dy = 3;

let bg = rgb(10, 10, 20);
let ball = rgb(255, 140, 0);

let frame = 0;
let frames = 240;

while frame < frames {
    fill_screen(bg);
    rect(x, y, size, size, ball);

    x = x + dx;
    y = y + dy;

    # Bounce off whichever wall was crossed this frame -- clamp back
    # onto the screen and reverse that axis's direction. x and y are
    # checked independently (not elif'd together), so a true corner hit
    # correctly reverses both axes on the same frame.
    if x <= 0 {
        x = 0;
        dx = -dx;
    } elif x + size >= w {
        x = w - size;
        dx = -dx;
    }

    if y <= 0 {
        y = 0;
        dy = -dy;
    } elif y + size >= h {
        y = h - size;
        dy = -dy;
    }

    sleep_ms(16);
    frame = frame + 1;
}

# clear() (not fill_screen()) on the way out: it resets the text cursor
# to the top-left along with the pixels, so the closing message always
# lands somewhere predictable instead of wherever this shell's cursor
# happened to be sitting before 'run bounce' was typed. That's the
# difference between the two builtins in one line: fill_screen() is a
# pure graphics primitive (leaves the cursor alone, used every frame
# above), clear() is a console operation (also used by the plain 'clear'
# shell command and WISP's own clear() builtin before this feature ever
# existed) -- see docs/WISP_GRAPHICS.md.
clear();
print("bounce.wisp finished -- " + str(frames) + " frames.");
