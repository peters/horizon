//! The group of two seconds and the two bounds, on a clock that the test moves.

use std::time::{Duration, Instant};

use super::{Bounds, CAPACITY, GROUP, SPACING, WINDOW};
use crate::notify::{Change, ChangeKind, Notice, SILENCE};

fn change(panel: &str) -> Change {
    Change {
        panel: panel.to_owned(),
        machine: None,
        workspace: None,
        kind: ChangeKind::NeedsYou,
    }
}

fn body(notice: Option<&Notice>) -> String {
    notice.map(|notice| notice.body.clone()).unwrap_or_default()
}

#[test]
fn a_change_leaves_at_the_end_of_its_group() {
    let start = Instant::now();
    let mut bounds = Bounds::default();
    bounds.note(change("Fix the sync"), start);
    assert!(bounds.due_at().is_some());
    assert_eq!(bounds.due_at(), Some(start + GROUP));
    assert!(
        bounds.due(false, start + GROUP / 2).is_none(),
        "the change still collects others"
    );
    let notice = bounds.due(false, start + GROUP);
    assert_eq!(body(notice.as_ref()), "Fix the sync needs you");
    assert!(bounds.due_at().is_none(), "the change left once");
    assert!(bounds.due(false, start + GROUP + Duration::from_secs(60)).is_none());
}

#[test]
fn changes_of_one_group_become_one_notification() {
    let start = Instant::now();
    let mut bounds = Bounds::default();
    for (index, name) in ["a", "b", "c"].into_iter().enumerate() {
        bounds.note(change(name), start + Duration::from_millis(index as u64 * 500));
    }
    let notice = bounds.due(false, start + GROUP);
    assert_eq!(
        body(notice.as_ref()),
        "3 agents need you",
        "one notification for the three"
    );
}

#[test]
fn a_second_group_waits_for_the_spacing_of_the_notification_before_it() {
    let start = Instant::now();
    let mut bounds = Bounds::default();
    bounds.note(change("a"), start);
    let first = bounds.due(false, start + GROUP);
    assert_eq!(body(first.as_ref()), "a needs you");

    // A change right after the first notification collects its own group, and
    // leaves no sooner than two seconds after the notification before it.
    let after = start + GROUP;
    bounds.note(change("b"), after);
    assert!(
        bounds.due(false, after + Duration::from_millis(500)).is_none(),
        "the group of the second change ends two seconds after the first notification"
    );
    let second = bounds.due(false, after + SPACING);
    assert_eq!(body(second.as_ref()), "b needs you");
}

#[test]
fn a_person_who_is_in_front_again_clears_the_changes_that_wait() {
    let start = Instant::now();
    let mut bounds = Bounds::default();
    bounds.note(change("Fix the sync"), start);
    // The person came back inside the two seconds.
    bounds.note_front();
    assert!(bounds.due_at().is_none());
    assert!(
        bounds.due(false, start + GROUP).is_none(),
        "a cleared change sends nothing"
    );
}

#[test]
fn the_share_of_one_window_is_twenty_and_ends_with_one_line() {
    let mut now = Instant::now();
    let mut bounds = Bounds::default();
    for index in 0..CAPACITY {
        bounds.note(change(&format!("panel {index}")), now);
        let notice = bounds.due(false, now + GROUP).expect("a notification of this change");
        assert_eq!(notice.body, format!("panel {index} needs you"));
        now += WINDOW / u32::try_from(CAPACITY + 1).expect("a share");
    }
    assert!(!bounds.silenced());
    assert_eq!(CAPACITY, 20);

    // The share is spent: one line says that more agents wait, and then nothing.
    bounds.note(change("one more"), now);
    let last = bounds.due(false, now + GROUP).expect("the line that ends the share");
    assert_eq!(last.body, SILENCE);
    assert!(bounds.silenced());
    bounds.note(change("another"), now + GROUP);
    assert!(bounds.due(false, now + GROUP + GROUP).is_none(), "the silence holds");

    // A window of Horizon gets the focus, and the next change speaks again,
    // also inside the window of ten minutes that the person spent away.
    bounds.note_front();
    assert!(!bounds.silenced());
    let later = now + GROUP;
    bounds.note(change("after the silence"), later);
    let notice = bounds.due(false, later + GROUP).expect("the change after the pause");
    assert_eq!(notice.body, "after the silence needs you");
    assert!(!bounds.silenced());
}

#[test]
fn the_window_of_ten_minutes_starts_again() {
    let mut now = Instant::now();
    let mut bounds = Bounds::default();
    for _ in 0..CAPACITY {
        bounds.note(change("panel"), now);
        assert!(bounds.due(false, now + GROUP).is_some());
        now += SPACING;
    }
    // The next change is in a window of its own, so the share of the first one
    // does not count against it.
    now += WINDOW;
    bounds.note(change("later"), now);
    let notice = bounds
        .due(false, now + GROUP)
        .expect("a notification of the next window");
    assert_eq!(notice.body, "later needs you");
    assert!(!bounds.silenced());
}
