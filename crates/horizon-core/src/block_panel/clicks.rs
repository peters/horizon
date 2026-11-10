//! The clicks a person made on a card, kept for the agent that owns it.
//!
//! The queue lives in the Horizon process only: a click is a moment in the
//! person's screen, not saved state. It keeps at most [`MAX_PENDING_CLICKS`]
//! clicks in insertion order, drops the ones older than
//! [`CLICK_RETENTION_MILLIS`], and never removes an older click to admit a
//! newer one.

use std::collections::VecDeque;

/// Most clicks one card holds for its agent.
pub const MAX_PENDING_CLICKS: usize = 32;
/// How long a queued click stays readable, in milliseconds.
pub const CLICK_RETENTION_MILLIS: i64 = 5 * 60 * 1000;

/// One click as the card holds it until its agent reads it. The host adds the
/// card's public id and the freshness of its revision when it answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingClick {
    /// Unique in one card, counted up from 1.
    pub event_id: u64,
    /// The id of the button or row the person hit.
    pub component_id: String,
    /// The document revision the person saw.
    pub revision: u64,
    /// Ids of the rows that were selected at that moment, in document order.
    pub selected: Vec<String>,
    /// Wall-clock time of the click, in milliseconds.
    pub at_millis: i64,
}

/// Clicks in insertion order, bounded in count and age.
#[derive(Clone, Debug, Default)]
pub struct ClickQueue {
    clicks: VecDeque<PendingClick>,
    next_event_id: u64,
    /// Set when a click was refused because the queue was full. Cleared when
    /// the queue drains.
    refused: bool,
}

impl ClickQueue {
    /// Records one click and returns whether the queue took it. A full queue
    /// keeps the older clicks and refuses the new one.
    pub fn push(&mut self, component_id: &str, revision: u64, selected: Vec<String>, at_millis: i64) -> bool {
        self.prune(at_millis);
        if self.clicks.len() >= MAX_PENDING_CLICKS {
            self.refused = true;
            return false;
        }
        self.next_event_id = self.next_event_id.saturating_add(1);
        self.clicks.push_back(PendingClick {
            event_id: self.next_event_id,
            component_id: component_id.to_owned(),
            revision,
            selected,
            at_millis,
        });
        true
    }

    /// The oldest click that is still young enough, removed: a click reaches
    /// an agent at most once.
    pub fn take(&mut self, now_millis: i64) -> Option<PendingClick> {
        self.prune(now_millis);
        let click = self.clicks.pop_front();
        if self.clicks.is_empty() {
            self.refused = false;
        }
        click
    }

    /// The time of the oldest click that still waits, after dropping the ones
    /// that expired.
    pub fn oldest_at(&mut self, now_millis: i64) -> Option<i64> {
        self.prune(now_millis);
        self.clicks.front().map(|click| click.at_millis)
    }

    /// How many clicks wait, after dropping the ones that expired.
    pub fn pending(&mut self, now_millis: i64) -> usize {
        self.prune(now_millis);
        self.clicks.len()
    }

    /// Whether a click was refused while the queue stayed full.
    pub fn refused_notice(&mut self, now_millis: i64) -> bool {
        self.prune(now_millis);
        if self.clicks.is_empty() {
            self.refused = false;
        }
        self.refused
    }

    /// How many clicks wait, as last pruned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.clicks.len()
    }

    /// Whether the card holds nothing that a later board tick could deliver.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clicks.is_empty()
    }

    /// Drops the clicks older than [`CLICK_RETENTION_MILLIS`].
    fn prune(&mut self, now_millis: i64) {
        while self
            .clicks
            .front()
            .is_some_and(|click| now_millis.saturating_sub(click.at_millis) >= CLICK_RETENTION_MILLIS)
        {
            self.clicks.pop_front();
        }
        if self.clicks.is_empty() {
            self.refused = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(queue: &mut ClickQueue, id: &str, at: i64) -> bool {
        queue.push(id, 3, Vec::new(), at)
    }

    #[test]
    fn clicks_come_out_in_insertion_order_with_rising_event_ids() {
        let mut queue = ClickQueue::default();
        assert!(push(&mut queue, "stop", 1_000));
        assert!(push(&mut queue, "start", 1_001));

        assert_eq!(queue.pending(1_002), 2);
        let first = queue.take(1_002).expect("first click");
        assert_eq!((first.event_id, first.component_id.as_str()), (1, "stop"));
        let second = queue.take(1_002).expect("second click");
        assert_eq!((second.event_id, second.component_id.as_str()), (2, "start"));
        assert!(queue.take(1_002).is_none(), "a click is delivered at most once");
        assert!(queue.is_empty());
    }

    #[test]
    fn the_queue_keeps_at_most_thirty_two_clicks_and_refuses_the_newest() {
        let mut queue = ClickQueue::default();
        for index in 0..MAX_PENDING_CLICKS {
            assert!(push(&mut queue, &format!("row.{index}"), 1_000));
        }
        assert!(!queue.refused_notice(1_000), "a full queue is not a refusal");
        assert!(
            !push(&mut queue, "row.overflow", 1_000),
            "a full queue drops the new click"
        );
        assert_eq!(queue.pending(1_000), MAX_PENDING_CLICKS);
        assert!(queue.refused_notice(1_000));

        let oldest = queue.take(1_000).expect("oldest click");
        assert_eq!(oldest.component_id, "row.0", "the oldest click is never lost");
        assert!(queue.refused_notice(1_000), "the notice stays while clicks wait");
        while queue.take(1_000).is_some() {}
        assert!(!queue.refused_notice(1_000), "the notice clears when the queue drains");
    }

    #[test]
    fn the_oldest_click_reports_its_time() {
        let mut queue = ClickQueue::default();
        assert_eq!(queue.oldest_at(1_000), None);
        assert!(push(&mut queue, "stop", 1_000));
        assert!(push(&mut queue, "start", 1_500));
        assert_eq!(queue.oldest_at(1_600), Some(1_000));
        let _ = queue.take(1_600);
        assert_eq!(queue.oldest_at(1_600), Some(1_500));
    }

    #[test]
    fn a_click_older_than_five_minutes_is_dropped() {
        let mut queue = ClickQueue::default();
        assert!(push(&mut queue, "stale", 10_000));
        assert!(push(&mut queue, "young", 10_000 + CLICK_RETENTION_MILLIS - 1));
        assert_eq!(queue.pending(10_000 + CLICK_RETENTION_MILLIS), 1, "one click expired");

        let click = queue.take(10_000 + CLICK_RETENTION_MILLIS).expect("young click");
        assert_eq!(click.component_id, "young");
        assert!(queue.is_empty());
    }

    #[test]
    fn a_click_carries_the_revision_and_selection_the_person_saw() {
        let mut queue = ClickQueue::default();
        assert!(queue.push("continue", 7, vec!["size.cx32".to_owned()], 500));

        let click = queue.take(500).expect("click");
        assert_eq!(click.revision, 7);
        assert_eq!(click.selected, vec!["size.cx32".to_owned()]);
        assert_eq!(click.at_millis, 500);
    }
}
