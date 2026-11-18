//! The typing of another computer into a panel of this window: the value of the
//! newest list, in both directions, and the chip that says who types.

use std::time::Instant;

use super::typed_event;
use super::{OFFSCREEN, PATIENCE, Window, chip_of, text};

#[test]
fn a_follow_panel_takes_the_typing_of_the_newest_list_in_both_directions() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // The owner lets nobody type when the person opens the panel.
    window.app.template_config.server.remote_typing = false;
    let link = window.link.clone();
    window.until("the list says that nobody types", move |_| {
        link.relay().panels.first().is_some_and(|panel| !panel.typing)
    });
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let view_only = "on There \u{00b7} view only";
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some(view_only)
    });

    // The owner turns typing on: the open panel takes it, with its chip.
    window.app.template_config.server.remote_typing = true;
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some("on There")
    });
    window.write(follow, b"typed now\n");
    window.shows(owner, "GOT:typed now");

    // And it loses it again when the owner turns typing off.
    window.app.template_config.server.remote_typing = false;
    window.until("the chip of the follow panel", |app| {
        chip_of(app, follow).as_deref() == Some(view_only)
    });
    window.write(follow, b"no longer\n");
    window.frames(4);
    assert!(
        !text(&window.app, owner).contains("GOT:no longer"),
        "the panel drops the bytes after the owner turned typing off"
    );
}

#[test]
fn the_chip_of_a_watched_panel_says_who_types_and_goes_back_after_two_seconds() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let id = window.id(owner);
    window.until("the chip of the owner panel", |app| {
        chip_of(app, owner).as_deref() == Some("seen by Here")
    });
    assert!(
        window.app.shared_terminals.found.chips.contains_key(&id),
        "the owner panel is watched"
    );

    // The bytes of another computer reach the program, and the chip says who.
    // The claim of typing lasts two seconds, and a frame of a loaded computer can
    // take longer than that: the wait for the chip reads what the pump published,
    // and the wait for the echo runs after it.
    typed_event(&window.link, &id, b"who\n");
    window.shows(owner, "GOT:who");
    // The chip is the copy that the frame took of what the pump published, and its
    // claim lasts two seconds: a frame of a loaded computer can take longer than
    // that, so the person types again in each round of the wait.
    let deadline = Instant::now() + PATIENCE;
    while chip_of(&window.app, owner).as_deref() != Some("typed by Unknown") {
        assert!(
            Instant::now() < deadline,
            "the chip of the typed bytes: {:?}",
            chip_of(&window.app, owner)
        );
        typed_event(&window.link, &id, b"who\n");
        window.frames(1);
    }

    // Two seconds after the last bytes the chip names the followers again.
    window.until("the chip of the followers", |app| {
        chip_of(app, owner).as_deref() == Some("seen by Here")
    });
}
