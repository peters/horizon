//! The grid that the computer which typed last gives the program of a shared
//! panel: the relay of the hub, which holds it to a follower of a panel that lets
//! another computer type in it, and to two requests a second.

use std::time::Duration;

use super::super::hub::RESIZE_EVERY;
use super::super::protocol::{LiveEvent, LiveRequest};
use super::{Computer, OWNER, computer, follow, panel, text};
use crate::server::LinkRegistry;

/// The grid that a follower gives the program of a panel that it follows.
fn resize(cols: u16, rows: u16) -> LiveRequest {
    LiveRequest::Resize {
        owner: text(OWNER),
        panel: text("p1"),
        cols,
        rows,
    }
}

/// The grid that a follower gives the program of a panel reaches the owner only
/// from a computer that follows the panel now, only while the owner lets another
/// computer type in it, and no sooner than twice a second. The grid is cut at the
/// one pair of bounds of a screen.
/// Mutation: drop the `follows` check (the stranger's grid arrives), drop the
/// `typing` check (the grid of a panel that only looks arrives), drop the hold of
/// `resized` (both requests arrive), or send `cols`/`rows` as they came.
#[test]
fn a_grid_reaches_the_owner_from_a_follower_of_a_panel_that_lets_it_type_twice_a_second() {
    let registry = LinkRegistry::default();
    let owner = computer(&registry, OWNER);
    let follower = computer(&registry, "laptop-b");
    let stranger = computer(&registry, "laptop-c");
    owner.ask(
        &registry,
        LiveRequest::Panels {
            panels: vec![panel("p1", true), panel("p2", false)],
        },
    );
    follower.ask(&registry, follow("p1"));
    owner.take(&registry);
    follower.take(&registry);
    let grid = |cols, rows| {
        vec![LiveEvent::Resized {
            panel: text("p1"),
            from: text("laptop-b"),
            label: text("laptop-b"),
            cols,
            rows,
        }]
    };

    // A computer that does not follow the panel sends nothing to its owner.
    stranger.ask(&registry, resize(100, 30));
    assert_eq!(owner.take(&registry), Vec::new(), "a stranger sends no grid");

    // A follower sends one grid, and the second request inside the half second
    // is dropped, also when it names another grid.
    follower.ask(&registry, resize(100, 30));
    follower.ask(&registry, resize(101, 31));
    assert_eq!(owner.take(&registry), grid(100, 30), "one grid a half second");
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(120, 40));
    assert_eq!(owner.take(&registry), grid(120, 40), "the next grid after the wait");

    // The bounds of a screen hold for a grid from another computer.
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(0, 5000));
    assert_eq!(owner.take(&registry), grid(2, 500), "a grid over its bounds is cut");

    // The panel of the grid is shared, and the owner lets nobody type in it: the
    // relay holds the grid back, because a grid changes what the program does.
    let follows = |link: &Computer| link.ask(&registry, follow("p2"));
    follows(&follower);
    owner.take(&registry);
    follower.take(&registry);
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(
        &registry,
        LiveRequest::Resize {
            owner: text(OWNER),
            panel: text("p2"),
            cols: 100,
            rows: 30,
        },
    );
    assert_eq!(owner.take(&registry), Vec::new(), "typing is off in that panel");

    // A computer that left asks for no grid any more.
    follower.ask(
        &registry,
        LiveRequest::Unfollow {
            owner: text(OWNER),
            panel: text("p1"),
        },
    );
    owner.take(&registry);
    std::thread::sleep(RESIZE_EVERY + Duration::from_millis(50));
    follower.ask(&registry, resize(140, 50));
    assert_eq!(owner.take(&registry), Vec::new(), "a computer that left sends no grid");
}
