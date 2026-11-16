//! The decision after a lost link, and the delays that it names.

use std::time::Duration;

use super::{
    AfterConnectionLoss, ConnectionLoss, RECONNECT_RESET_AFTER, SSH_CONNECTION_FAILURE, after_connection_loss,
    attaches_a_tmux_session, reconnect_delay,
};

/// The loss of a machine terminal that the link ended.
fn lost(attempts: u32) -> ConnectionLoss {
    ConnectionLoss {
        exit_code: Some(SSH_CONNECTION_FAILURE),
        connected_for: None,
        machine_tied: true,
        closed_by_the_person: false,
        attempts,
    }
}

fn reconnect(attempt: u32, seconds: u64) -> AfterConnectionLoss {
    AfterConnectionLoss::Reconnect {
        attempt,
        after: Duration::from_secs(seconds),
    }
}

#[test]
fn a_lost_link_tries_again_after_three_seconds() {
    assert_eq!(after_connection_loss(lost(0)), reconnect(1, 3));
}

#[test]
fn the_delays_grow_and_repeat_after_the_last_one() {
    assert_eq!(
        [1, 2, 3, 4, 5, 6].map(|attempt| reconnect_delay(attempt).as_secs()),
        [3, 6, 12, 24, 30, 30],
        "3 s, 6 s, 12 s, 24 s, then every 30 s"
    );
    assert_eq!(
        after_connection_loss(lost(3)),
        reconnect(4, 24),
        "the fourth attempt of a run waits 24 s"
    );
    assert_eq!(
        after_connection_loss(lost(4)),
        reconnect(5, 30),
        "every attempt after the fourth waits 30 s, with no count of its own"
    );
    assert_eq!(
        after_connection_loss(lost(500)),
        reconnect(501, 30),
        "a long outage keeps trying every 30 s"
    );
}

#[test]
fn a_connection_that_lasted_starts_the_delays_again() {
    let lasted = ConnectionLoss {
        connected_for: Some(RECONNECT_RESET_AFTER),
        ..lost(3)
    };
    assert_eq!(
        after_connection_loss(lasted),
        reconnect(1, 3),
        "ten seconds of connection reset the run"
    );

    let short = ConnectionLoss {
        connected_for: Some(Duration::from_millis(9_999)),
        ..lost(3)
    };
    assert_eq!(
        after_connection_loss(short),
        reconnect(4, 24),
        "a connection that came up and dropped again continues the run"
    );
}

#[test]
fn a_command_that_failed_does_not_reconnect() {
    for exit_code in [Some(0), Some(1), Some(130), None] {
        let loss = ConnectionLoss { exit_code, ..lost(0) };
        assert_eq!(
            after_connection_loss(loss),
            AfterConnectionLoss::Stay,
            "only the failure of the connection itself starts an attempt: {exit_code:?}"
        );
    }
}

#[test]
fn a_panel_of_another_kind_and_a_closed_panel_do_not_reconnect() {
    assert_eq!(
        after_connection_loss(ConnectionLoss {
            machine_tied: false,
            ..lost(0)
        }),
        AfterConnectionLoss::Stay,
        "a plain SSH panel of the remote hosts chooser has no session to reach again"
    );
    assert_eq!(
        after_connection_loss(ConnectionLoss {
            closed_by_the_person: true,
            ..lost(0)
        }),
        AfterConnectionLoss::Stay,
        "a person who closed the panel starts nothing"
    );
}

#[test]
fn a_place_command_attaches_its_tmux_session() {
    assert!(attaches_a_tmux_session(Some(&crate::place::session_command(
        "hz-c8dfe3af-1",
        None,
    ))));
    assert!(
        attaches_a_tmux_session(Some("tmux new -A -s build-1")),
        "a terminal of a machine record attaches its session too"
    );
    assert!(attaches_a_tmux_session(Some("tmux attach -t work")));
    assert!(attaches_a_tmux_session(Some("tmux attach-session -d -t work")));
}

#[test]
fn a_remote_shell_and_a_new_session_are_not_an_attachment() {
    assert!(!attaches_a_tmux_session(None));
    assert!(!attaches_a_tmux_session(Some("bash -l")));
    assert!(
        !attaches_a_tmux_session(Some("tmux new-session -s work")),
        "a new-session without -A would make a second session"
    );
    assert!(
        !attaches_a_tmux_session(Some("tmux new -s work")),
        "a new session without -A would make a second session"
    );
    assert!(!attaches_a_tmux_session(Some("tmux ls")));
    assert!(!attaches_a_tmux_session(Some("tmux attach")));
}
