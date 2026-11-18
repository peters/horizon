//! The size of a shared terminal follows the computer that typed last, in one
//! window: the follower of this window asks the relay for the grid that fits the
//! body of its panel, the owner side of this window gives it to the program, and
//! the person of the owner takes it back.

use std::time::Duration;

use horizon_core::Panel;
use horizon_sync::LiveEvent;

use super::{OFFSCREEN, Window, chip_of, grid};
use crate::app::HorizonApp;

/// The grid that the terminal of `panel` shows, which a held grid shows too.
pub(super) fn shown(app: &HorizonApp, panel: horizon_core::PanelId) -> (u16, u16) {
    let terminal = app.board.panel(panel).and_then(Panel::terminal).expect("a terminal");
    terminal.shown_grid()
}

/// The grid that fits the body of the panel of `app` now.
pub(super) fn fit(app: &HorizonApp, panel: horizon_core::PanelId) -> (u16, u16) {
    let feed = app.board.panel(panel).and_then(Panel::follow_feed).expect("a feed");
    feed.fit()
}

/// The size that the program of the owner printed last: rows and columns. The
/// whole buffer is read, because a held grid is larger than the rows of the body
/// of its panel, which `last_lines_text` counts.
pub(super) fn size(app: &HorizonApp, panel: horizon_core::PanelId) -> Option<(u16, u16)> {
    let terminal = app.board.panel(panel).and_then(Panel::terminal)?;
    let (lines, _) = terminal.full_text_lines(4000);
    let line = lines.iter().rev().find(|line| line.contains("SIZE:"))?;
    let mut words = line.rsplit_once("SIZE:")?.1.split_whitespace();
    let rows = words.next()?.parse().ok()?;
    let cols = words.next()?.parse().ok()?;
    Some((rows, cols))
}

#[test]
fn the_grid_of_the_body_of_the_follower_reaches_the_program_until_the_person_of_the_owner_acts() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    assert_eq!(
        grid(&window.app, follow),
        (80, 24),
        "the follower starts on the grid of the owner"
    );

    // The body of the follow panel is larger than the grid of the stream. A
    // frame writes that grid into its feed, and a key of the person asks for it.
    let body = [1100.0, 700.0];
    window
        .app
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .resize_layout(body);
    window.frames(2);
    let asked = fit(&window.app, follow);
    assert_ne!(asked, (80, 24), "the body of the follow panel fits another grid");
    window.write(follow, b"\n");
    let link = window.link.clone();
    window.until("the request for the grid of the body", move |_| {
        link.relay().resizes.first() == Some(&asked)
    });

    // The owner side of this window gives the program that grid, its terminal
    // takes it, and the follower shows the stream in the same grid.
    window.until("the owner holds the grid", |app| {
        shown(app, owner) == (asked.1, asked.0)
    });
    window.until("the follower shows that grid", |app| grid(app, follow) == asked);
    window.write(follow, b"size\n");
    window.until("the program reads the grid of the follower", |app| {
        size(app, owner) == Some((asked.1, asked.0))
    });
    assert_eq!(
        chip_of(&window.app, owner).as_deref(),
        Some("sized by Here \u{00b7} typed by Here"),
        "the chip of the owner says who holds the size, with what it said before"
    );

    // The person of the owner types in the panel: the size goes back to the body
    // of that panel, and the follower asks again only if the person types there.
    window.write(owner, b"size\n");
    window.until("the size of the body of the owner", |app| {
        size(app, owner) == Some((24, 80))
    });
    window.until("the follower shows the grid of the owner again", |app| {
        grid(app, follow) == (80, 24)
    });
    assert_eq!(
        shown(&window.app, owner),
        (24, 80),
        "the owner pushes the grid of its body again"
    );
    assert_eq!(
        chip_of(&window.app, owner).as_deref(),
        Some("typed by Here"),
        "the chip of the owner says who typed, and not who held the size"
    );
}

#[test]
fn the_request_of_a_grid_waits_for_the_half_second_of_its_panel_and_for_the_switch() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    window
        .app
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .resize_layout([1100.0, 700.0]);
    window.frames(2);
    let first = fit(&window.app, follow);

    // The person types: one request, and the grid reaches the program. A second
    // press then asks nothing, because the stream has that grid.
    window.write(follow, b"a");
    let link = window.link.clone();
    window.until("the request for the grid of the body", move |_| {
        link.relay().resizes.first() == Some(&first)
    });
    window.write(follow, b"b");
    window.frames(2);
    assert_eq!(
        window.link.relay().resizes,
        vec![first],
        "the grid of the stream is not asked again"
    );

    // A larger body comes inside the half second of the panel, so its grid waits
    // for the second to pass.
    window
        .app
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .resize_layout([1300.0, 800.0]);
    window.frames(2);
    let second = fit(&window.app, follow);
    window.write(follow, b"c");
    window.frames(2);
    assert_eq!(
        window.link.relay().resizes,
        vec![first],
        "two requests a second at most"
    );
    std::thread::sleep(Duration::from_millis(550));
    window.write(follow, b"d");
    window.frames(2);
    assert_eq!(
        window.link.relay().resizes,
        vec![first, second],
        "the next grid after the wait"
    );
    window.until("the owner holds the grid of the second body", |app| {
        shown(app, owner) == (second.1, second.0)
    });

    // A body that changes while this computer holds the grid asks again for its
    // own grid, with no key press: the person works from this window.
    std::thread::sleep(Duration::from_millis(550));
    let asks = window.link.relay().resizes.len();
    let relay = window.link.clone();
    window
        .app
        .board
        .panel_mut(follow)
        .expect("the follow panel")
        .resize_layout([1500.0, 900.0]);
    window.until("the request for the grid of the larger body", move |_| {
        relay.relay().resizes.len() > asks
    });
    window.until("the owner holds the grid of the larger body", |app| {
        let grid = shown(app, owner);
        grid != (second.1, second.0) && super::grid(app, follow) == (grid.1, grid.0)
    });

    // The owner refuses a grid that comes to it while it lets nobody type, even
    // through a relay that checks nothing.
    window.app.template_config.server.remote_typing = false;
    window.frames(1);
    let listed = window.id(owner);
    window.link.relay().events.push(LiveEvent::Resized {
        panel: listed,
        from: "desk-unknown".to_owned(),
        label: "Unknown".to_owned(),
        cols: 120,
        rows: 40,
    });
    window.frames(2);
    let held = shown(&window.app, owner);
    assert_ne!(held, (40, 120), "the owner refuses the grid");
}

#[test]
fn a_grid_for_a_panel_that_this_window_does_not_share_changes_nothing() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    let follow = window.follow(owner);
    window.shows(follow, "READY");
    let follow_id = window.id(follow);
    for panel in [follow_id, "no-such-panel".to_owned()] {
        window.link.relay().events.push(LiveEvent::Resized {
            panel,
            from: "desk-unknown".to_owned(),
            label: "Unknown".to_owned(),
            cols: 120,
            rows: 40,
        });
    }
    window.frames(3);
    assert_eq!(
        shown(&window.app, owner),
        (24, 80),
        "a follow panel takes no grid of a program"
    );
    assert!(
        !window.link.relay().panels.is_empty() && window.link.relay().panels.iter().all(|panel| panel.cols == 80),
        "the list of the owner keeps the grid of its body"
    );
}

/// The body of a follow panel fits the grid of its stream in the cell size of the
/// terminal font, for the least and the greatest panel: the first key that the
/// person presses in it asks the program of the owner for no other grid.
#[test]
fn a_follow_panel_opens_at_a_body_that_fits_the_grid_of_its_stream() {
    // 65 by 21 is the grid of the live run whose panel opened one column wider,
    // and the others are the bounds of a follow panel.
    for (cols, rows) in [(65, 21), (20, 4), (240, 80)] {
        let mut window = Window::new();
        let owner = window.owner(OFFSCREEN);
        window
            .app
            .board
            .panel_mut(owner)
            .expect("the owner panel")
            .resize_immediately(rows, cols, 8, 17);
        // The list of the window names the grid of the owner, and the follow panel
        // opens at it.
        let listed = window.id(owner);
        window.until("the list names the grid of the owner", |app| {
            app.shared_terminals
                .found
                .computers
                .iter()
                .flat_map(|computer| computer.panels.iter())
                .any(|panel| panel.id == listed && (panel.cols, panel.rows) == (cols, rows))
        });
        let follow = window.follow(owner);
        window.shows(follow, "READY");
        window.settle();

        assert_eq!(
            grid(&window.app, follow),
            (cols, rows),
            "the follow panel shows the grid of the stream"
        );
        assert_eq!(
            fit(&window.app, follow),
            (cols, rows),
            "the body of the follow panel fits the grid of the stream"
        );

        // The person types in the follow panel, and the stream has the grid that
        // fits its body, so no request for another grid goes out on the link.
        window.write(follow, b"x");
        window.frames(3);
        window.settle();
        assert!(
            window.link.relay().resizes.is_empty(),
            "a key press asks the owner for no other grid: {:?}",
            window.link.relay().resizes
        );
    }
}
