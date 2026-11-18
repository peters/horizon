//! The history that a follower takes at its join, in one window: the lines that
//! the program wrote before the follower came, and the join of a second computer.

use super::*;

/// The lines of `panel` above its viewport, oldest first.
pub(super) fn above_the_viewport(app: &HorizonApp, panel: PanelId) -> Vec<String> {
    let terminal = app.board.panel(panel).and_then(Panel::terminal).expect("a terminal");
    let (lines, _) = terminal.full_text_lines(2000);
    let rows = usize::from(terminal.rows());
    lines[..lines.len().saturating_sub(rows)].to_vec()
}

#[test]
fn a_follower_reads_the_lines_that_the_program_wrote_before_it_came() {
    let mut window = Window::new();
    let owner = window.owner(OFFSCREEN);
    // Three hundred lines are written before anybody follows the panel.
    for line in 1..=300 {
        window.write(owner, format!("line {line}\n").as_bytes());
    }
    window.shows(owner, "GOT:line 300");
    let follow = window.follow(owner);
    window.shows(follow, "GOT:line 300");

    // The person scrolls back in a panel that follows: the lines of the program
    // that ran before the join are there.
    let above = above_the_viewport(&window.app, follow);
    assert!(
        above.iter().any(|line| line == "GOT:line 1"),
        "the oldest line is above the viewport of the follower: {} lines",
        above.len()
    );

    // A second computer follows the same panel. The owner makes the screen of
    // that computer, and it reaches that computer alone: the follower of this
    // window takes no screen that starts a new one, so its history and its
    // screen stay. The screens are not counted: a join owes its screen and a
    // second one 250 ms later, which the frames of a busy computer reach, and
    // that second screen draws in place.
    let sent = window.link.relay().screens;
    let taken = window.link.relay().taken;
    let lines = above_the_viewport(&window.app, follow);
    let screen = text(&window.app, follow);
    let other = LiveWatcher {
        device: "desk-other".to_owned(),
        label: "Other".to_owned(),
    };
    let here = LiveWatcher {
        device: HERE.to_owned(),
        label: "Here".to_owned(),
    };
    let id = window.id(owner);
    window.link.relay().events.push(LiveEvent::Watchers {
        panel: id,
        watchers: vec![here, other],
        refresh: true,
    });
    window.frames(3);
    let relay = window.link.relay();
    let kept = above_the_viewport(&window.app, follow);
    let shown = text(&window.app, follow);
    assert!(relay.screens > sent, "a screen for the joined computer");
    assert_eq!(relay.taken, taken, "a follower takes no new screen of another");
    assert_eq!(kept, lines, "a relay keeps its history");
    assert_eq!(shown, screen, "a relay keeps its screen");
}
