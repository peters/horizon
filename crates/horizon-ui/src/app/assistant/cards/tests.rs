use super::*;

fn approval(text: &str) -> CardKind {
    CardKind::Approval {
        target: PanelId(1),
        title: "codex".into(),
        text: text.into(),
        submit: true,
    }
}

fn note(title: &str) -> CardKind {
    CardKind::Note {
        title: title.into(),
        markdown: "- done".into(),
    }
}

#[test]
fn cards_get_distinct_ids_and_can_be_dismissed() {
    let mut cards = Cards::default();
    let first = cards.push(note("one"));
    let second = cards.push(note("two"));
    assert_ne!(first, second);
    cards.dismiss(first);
    assert!(cards.get_mut(first).is_none());
    assert!(cards.get_mut(second).is_some());
}

#[test]
fn trimming_never_drops_a_card_that_waits_on_the_person() {
    let mut cards = Cards::default();
    let waiting = cards.push(approval("keep me"));
    for index in 0..(MAX_CARDS * 2) {
        cards.push(note(&format!("note {index}")));
    }
    assert!(cards.items.len() <= MAX_CARDS);
    assert!(
        cards.get_mut(waiting).is_some(),
        "the oldest card is an approval and must survive"
    );
}

#[test]
fn only_unanswered_approvals_expire() {
    let mut cards = Cards::default();
    let waiting = cards.push(approval("old"));
    let kept = cards.push(note("kept"));
    cards.expire(Instant::now() + APPROVAL_LIFETIME + Duration::from_secs(1));
    assert!(cards.get_mut(waiting).is_none());
    assert!(cards.get_mut(kept).is_some());
}

#[test]
fn reply_tail_keeps_the_last_rows_and_bounds_the_length() {
    let text = "a\n\n  \nb\nc\nd\ne\nf\ng\nh\n";
    assert_eq!(reply_tail(text), "c\nd\ne\nf\ng\nh");
    let long = "x".repeat(SHOWN_CHARS * 2);
    let shown = reply_tail(&long);
    assert!(shown.ends_with("..."));
    assert_eq!(shown.chars().count(), SHOWN_CHARS + 3);
}
