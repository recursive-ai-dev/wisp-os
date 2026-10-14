# guess.wisp -- a number-guessing game
let secret = rand(1, 20);
let tries = 0;

print("I'm thinking of a number from 1 to 20.");

while true {
    let guess_text = input("Your guess: ");
    let guess = int(guess_text);
    tries = tries + 1;

    if guess == secret {
        print("Got it in " + str(tries) + " tries!");
        break;
    } elif guess < secret {
        print("Higher.");
    } else {
        print("Lower.");
    }
}
