# adventure.wisp -- a small self-contained text adventure.
#
# Five rooms, two items, one locked door -- built entirely from WISP v1
# features: no maps (see docs/WISP_LANGUAGE.md's "Lists" section), so
# room state lives in plain globals (which room you're in, which items
# you're carrying) rather than a data structure keyed by room name. No
# string indexing/splitting either, so commands are whole-line literals
# compared with `==` (e.g. "take lantern" is one opaque string, never
# parsed apart) -- the same pattern oracle.wisp uses for "ask"/"quit".
#
# Map:
#   0 Entrance Hall --north--> 1 Reading Room --east--> 2 Storage Closet (dark)
#                                    |
#                                  north
#                                    v
#                          3 Vault Antechamber --north--> 4 Vault (locked)
#
# The lantern (room 1) lets you see the key (room 2); the key (room 2)
# unlocks the vault door (room 3 -> 4). Reaching room 4 wins the game.

let room = 0;
let has_lantern = false;
let has_key = false;
let won = false;

fn describe_room() {
    if room == 0 {
        print("ENTRANCE HALL");
        print("A cold draft comes down a stone stairway behind you. The");
        print("only way onward is a passage to the north.");
    } elif room == 1 {
        print("READING ROOM");
        print("Rotted shelves line the walls. A passage leads south back");
        print("to the entrance, another leads east, and a heavy doorway");
        print("opens to the north.");
        if not has_lantern {
            print("A brass lantern sits on the nearest shelf, still full of oil.");
        }
    } elif room == 2 {
        print("STORAGE CLOSET");
        if has_lantern {
            print("Lantern light finds a small iron key hanging on a nail,");
            print("half-buried under a century of dust.");
        } else {
            print("It's pitch black in here. You can make out a doorway back");
            print("west, but nothing else -- you'll want a light source first.");
        }
    } elif room == 3 {
        print("VAULT ANTECHAMBER");
        print("A passage leads south. To the north, a vault door of dull");
        print("iron is set into the rock, sealed with a heavy lock.");
        if not has_key {
            print("The lock has no visible keyhole trick -- you'll need an");
            print("actual key.");
        }
    } elif room == 4 {
        print("THE VAULT");
        print("Firelight from your lantern catches gold on every shelf.");
        print("You made it.");
    }
}

fn print_help() {
    print("Commands: look, north, south, east, west, take lantern,");
    print("          take key, inventory, help, quit");
}

fn print_inventory() {
    if not has_lantern and not has_key {
        print("You aren't carrying anything.");
        return;
    }
    print("You are carrying:");
    if has_lantern {
        print("  a brass lantern");
    }
    if has_key {
        print("  a small iron key");
    }
}

fn try_move(dest) {
    if dest == 4 and not has_key {
        print("The vault door is locked tight. You need a key.");
        return;
    }
    room = dest;
    if dest == 4 {
        won = true;
    }
    describe_room();
}

print("=== THE SEALED ARCHIVE ===");
print_help();
print();
describe_room();

while not won {
    let cmd = input("> ");

    if cmd == "quit" {
        print("You leave the archive. The vault stays sealed forever.");
        break;
    } elif cmd == "help" {
        print_help();
    } elif cmd == "look" {
        describe_room();
    } elif cmd == "inventory" {
        print_inventory();
    } elif cmd == "north" {
        if room == 0 {
            try_move(1);
        } elif room == 1 {
            try_move(3);
        } elif room == 3 {
            try_move(4);
        } else {
            print("You can't go that way.");
        }
    } elif cmd == "south" {
        if room == 1 {
            try_move(0);
        } elif room == 3 {
            try_move(1);
        } else {
            print("You can't go that way.");
        }
    } elif cmd == "east" {
        if room == 1 {
            try_move(2);
        } else {
            print("You can't go that way.");
        }
    } elif cmd == "west" {
        if room == 2 {
            try_move(1);
        } else {
            print("You can't go that way.");
        }
    } elif cmd == "take lantern" {
        if room == 1 and not has_lantern {
            has_lantern = true;
            print("You take the brass lantern. Its light steadies your hand.");
        } elif has_lantern {
            print("You already have the lantern.");
        } else {
            print("There's no lantern here.");
        }
    } elif cmd == "take key" {
        if room == 2 and has_lantern and not has_key {
            has_key = true;
            print("You pry the small iron key loose and pocket it.");
        } elif room == 2 and not has_lantern {
            print("It's too dark to find anything in here.");
        } elif has_key {
            print("You already have the key.");
        } else {
            print("There's no key here.");
        }
    } else {
        print("I don't understand that. Type 'help' for a list of commands.");
    }
}

if won {
    print("*** YOU WIN ***");
}
