//! A screen of another computer that does not fit the room of the link.

use super::{OFFSCREEN, Window, text};

/// A screen that does not fit the room of the link goes in parts, in the order of
/// its bytes, and the output of the program follows the whole screen.
#[test]
fn a_screen_that_does_not_fit_the_room_of_the_link_goes_in_parts_before_its_output() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // The program writes enough rows that its screen is much larger than the room
    // of the link below.
    let lines: String = (1..=40).fold(String::new(), |text, line| format!("{text}line {line}\n"));
    assert!(
        window
            .app
            .board
            .panel_mut(owner)
            .expect("the owner")
            .write_input(lines.as_bytes()),
        "the panel takes the lines"
    );
    window.shows(owner, "GOT:line 40");

    // A new follower joins with a small room: the parts of its screen are refused
    // until the pump offers a part that fits.
    window.link.relay().room = 512;
    let follow = window.follow(owner);
    let link = window.link.clone();
    window.until("the screen of the join in parts", move |_| link.relay().refused > 0);

    // The program writes after the screen was taken, so its echo is the live
    // output that may not pass the parts of that screen.
    assert!(
        window
            .app
            .board
            .panel_mut(owner)
            .expect("the owner")
            .write_input(b"after\n"),
        "the panel takes the line"
    );
    window.shows(follow, "GOT:after");
    let shows = text(&window.app, follow);
    assert!(shows.contains("GOT:line 40"), "the screen of the join arrived in parts");

    // The second screen of the join goes in parts too, and the output after the
    // first one stays on the screen of the follower.
    let link = window.link.clone();
    window.until("the second screen of the join", move |_| link.relay().screens >= 2);
    let shows = text(&window.app, follow);
    assert!(
        shows.contains("GOT:after"),
        "the output stays after the screen: {shows}"
    );
}
