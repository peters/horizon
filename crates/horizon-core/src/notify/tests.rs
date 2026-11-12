//! The text of one notification: one line of clean names, and the counts of
//! several changes.

use super::{Change, ChangeKind, MAX_NAME_CHARS, NotificationSetting, TITLE, body, clean_name};

fn change(panel: &str, workspace: Option<&str>, kind: ChangeKind) -> Change {
    Change {
        panel: panel.to_owned(),
        workspace: workspace.map(str::to_owned),
        kind,
    }
}

#[test]
fn one_change_names_the_panel_and_its_workspace_when_there_is_more_than_one() {
    let needs_you = change("Fix the sync", Some("Billing"), ChangeKind::NeedsYou);
    assert_eq!(body(std::slice::from_ref(&needs_you), false), "Fix the sync needs you");
    assert_eq!(
        body(&[needs_you], true),
        "Fix the sync needs you \u{00B7} Billing",
        "the workspace follows the name when the board holds more than one"
    );
    let done = change("Write the docs", Some("Docs"), ChangeKind::Done);
    assert_eq!(body(std::slice::from_ref(&done), false), "Write the docs is done");
    assert_eq!(body(&[done], true), "Write the docs is done \u{00B7} Docs");
    assert_eq!(
        body(&[change("Notes", None, ChangeKind::Done)], true),
        "Notes is done",
        "a local panel has no workspace name to add"
    );
    assert_eq!(body(&[], true), "");
}

#[test]
fn several_changes_become_one_count() {
    let needs = |name: &str| change(name, Some("Lane"), ChangeKind::NeedsYou);
    let done = |name: &str| change(name, Some("Lane"), ChangeKind::Done);
    assert_eq!(body(&[needs("a"), needs("b"), needs("c")], true), "3 agents need you");
    assert_eq!(body(&[done("a"), done("b")], false), "2 are done");
    assert_eq!(
        body(&[needs("a"), needs("b"), done("c")], false),
        "2 agents need you, 1 is done",
        "the count of each kind keeps its own words"
    );
    assert_eq!(
        body(&[needs("a"), done("b")], false),
        "1 agent needs you, 1 is done",
        "one agent of each kind takes the singular verb"
    );
}

#[test]
fn a_name_is_one_line_of_printable_text() {
    assert_eq!(clean_name("Fix the sync"), "Fix the sync");
    // C0, DEL and C1 go, and a line break becomes the space of a run.
    assert_eq!(clean_name("Fix\u{0}\u{7}\u{1b}\u{7f}the sync"), "Fixthe sync");
    assert_eq!(clean_name("Fix\u{85}the sync"), "Fix the sync", "NEL is a line break");
    assert_eq!(clean_name("Fix\nthe  sync"), "Fix the sync");
    assert_eq!(clean_name("Fix\r\nthe sync"), "Fix the sync");
    assert_eq!(clean_name("\t  Fix \u{2028} the sync  "), "Fix the sync");
    assert_eq!(clean_name("   "), "", "a name of spaces says nothing");
    assert_eq!(clean_name(""), "");
}

#[test]
fn a_name_is_cut_at_sixty_characters() {
    let long = "\u{0444}".repeat(200);
    let cut = clean_name(&long);
    assert_eq!(cut.chars().count(), MAX_NAME_CHARS);
    assert!(cut.ends_with('\u{2026}'), "the cut name says that it was cut: {cut:?}");
    let spaced = "word ".repeat(60);
    assert!(clean_name(&spaced).chars().count() <= MAX_NAME_CHARS);
}

#[test]
fn markup_in_a_name_stays_text() {
    assert_eq!(clean_name("<b>Fix</b> & more"), "<b>Fix</b> & more");
    assert_eq!(clean_name("Fix</body>"), "Fix</body>");
}

#[test]
fn a_name_that_looks_like_an_option_stays_a_name() {
    let notice = super::body(&[change("--urgency=critical", None, ChangeKind::NeedsYou)], false);
    assert_eq!(notice, "--urgency=critical needs you");
}

#[test]
fn the_notification_takes_the_title_of_horizon() {
    let notice = super::Notice::new("Fix the sync needs you");
    assert_eq!(notice.title, TITLE);
    assert_eq!(notice.body, "Fix the sync needs you");
}

#[test]
fn the_setting_lists_its_three_values_in_the_order_of_its_row() {
    assert_eq!(
        NotificationSetting::VALUES,
        [
            NotificationSetting::Off,
            NotificationSetting::NeedsYou,
            NotificationSetting::NeedsYouAndDone
        ]
    );
    assert_eq!(NotificationSetting::default(), NotificationSetting::NeedsYouAndDone);
    assert!(!NotificationSetting::Off.speaks());
    assert!(NotificationSetting::NeedsYou.speaks());
    assert!(!NotificationSetting::NeedsYou.tells_done());
    assert!(NotificationSetting::NeedsYouAndDone.tells_done());
    for setting in NotificationSetting::VALUES {
        assert!(!setting.label().is_empty(), "{setting:?} has a label");
    }
}
