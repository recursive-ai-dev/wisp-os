# canvas.wisp -- an interactive drawing tool that exercises every WISP v2
# graphics primitive: screen_width(), screen_height(), rgb(), plot(),
# rect(), line(), and fill_screen(). See docs/WISP_GRAPHICS.md for what
# each one actually does; this program is meant to be read alongside that
# doc as a worked example of using all of them together.
#
# WISP has no string-splitting -- there's no string indexing/slicing at
# all in v1, see docs/WISP_LANGUAGE.md -- so a command needing several
# numbers prompts for each one separately with its own input() rather
# than trying to parse "10 20 30" out of one line.

fn color_by_name(name) {
    if name == "red" {
        return rgb(220, 40, 40);
    } elif name == "green" {
        return rgb(40, 200, 90);
    } elif name == "blue" {
        return rgb(60, 120, 240);
    } elif name == "yellow" {
        return rgb(230, 200, 40);
    } elif name == "white" {
        return rgb(255, 255, 255);
    } elif name == "black" {
        return rgb(0, 0, 0);
    } else {
        print("(unrecognized color '" + name + "', using white)");
        return rgb(255, 255, 255);
    }
}

fn prompt_int(message) {
    return int(input(message));
}

fn prompt_color() {
    return color_by_name(input("  color (red/green/blue/yellow/white/black): "));
}

# The background fill happens *before* any of these print() calls below,
# not after: fill_screen() and print() draw into the exact same one
# framebuffer (see docs/WISP_GRAPHICS.md), so filling the screen after
# printing the banner would immediately paint over every line of it --
# print() text is only safe from a later fill_screen() call, never from
# an earlier one.
let bg = rgb(15, 15, 25);
fill_screen(bg);

print("canvas.wisp -- screen is " + str(screen_width()) + "x" + str(screen_height()) + " pixels.");
print("(0,0) is the top-left corner; x increases right, y increases down.");
print("Commands: plot, rect, line, fillscreen, clearscreen, quit.");

let running = true;
while running {
    let cmd = input("canvas> ");

    if cmd == "quit" {
        running = false;
    } elif cmd == "plot" {
        let x = prompt_int("  x: ");
        let y = prompt_int("  y: ");
        let c = prompt_color();
        plot(x, y, c);
    } elif cmd == "rect" {
        let x = prompt_int("  x: ");
        let y = prompt_int("  y: ");
        let w = prompt_int("  w: ");
        let h = prompt_int("  h: ");
        let c = prompt_color();
        rect(x, y, w, h, c);
    } elif cmd == "line" {
        let x1 = prompt_int("  x1: ");
        let y1 = prompt_int("  y1: ");
        let x2 = prompt_int("  x2: ");
        let y2 = prompt_int("  y2: ");
        let c = prompt_color();
        line(x1, y1, x2, y2, c);
    } elif cmd == "fillscreen" {
        let c = prompt_color();
        fill_screen(c);
    } elif cmd == "clearscreen" {
        fill_screen(bg);
    } else {
        print("unknown command '" + cmd + "' -- try plot, rect, line, fillscreen, clearscreen, or quit.");
    }
}

clear();
print("canvas.wisp finished.");
