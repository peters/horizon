//! Answers that the coordination worker could not write. The request stays
//! claimed on disk, so the host keeps the result and writes it again on a later
//! frame, at most every [`RETRY_INTERVAL`], until the deadline of its request.
//! A retry cannot answer twice: the write of a result removes the request file,
//! and a write that finds no request file writes nothing.

use std::time::{Duration, Instant};

/// How often the host writes a refused answer again. The retry is bounded by
/// the request's own deadline, so a disk that stays busy costs one write of the
/// coordination worker per interval and never stalls a frame.
pub(super) const RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// An answer that the coordination worker could not write, with the deadline of
/// the request it answers.
pub(super) struct RefusedAnswer<T> {
    pub(super) result: T,
    /// The request this answer belongs to, for the log of a drop.
    pub(super) request_id: String,
    pub(super) deadline_at_millis: i64,
}

impl<T> RefusedAnswer<T> {
    pub(super) fn new(result: T, request_id: String, deadline_at_millis: i64) -> Self {
        Self {
            result,
            request_id,
            deadline_at_millis,
        }
    }
}

/// The answers of one queue that the coordination worker could not write.
pub(super) struct RefusedAnswers<T> {
    answers: Vec<RefusedAnswer<T>>,
    /// When the kept answers are written again.
    retry_at: Option<Instant>,
}

impl<T> Default for RefusedAnswers<T> {
    fn default() -> Self {
        Self {
            answers: Vec::new(),
            retry_at: None,
        }
    }
}

impl<T> RefusedAnswers<T> {
    /// Keeps an answer whose write failed, for a later frame.
    pub(super) fn keep(&mut self, answer: RefusedAnswer<T>) {
        self.answers.push(answer);
    }

    /// Takes the answers to write again now, or none before [`RETRY_INTERVAL`]
    /// passed. The answers are out of the list until a failed retry puts each
    /// one back, so a slow disk never queues one retry per frame. An answer
    /// whose request deadline passed is dropped: the agent has its own timeout
    /// by then, and the reason is logged.
    pub(super) fn due(&mut self, now_millis: i64) -> Vec<RefusedAnswer<T>> {
        if self.answers.is_empty() {
            self.retry_at = None;
            return Vec::new();
        }
        let now = Instant::now();
        if self.retry_at.is_some_and(|retry_at| now < retry_at) {
            return Vec::new();
        }
        let mut due = Vec::new();
        for answer in std::mem::take(&mut self.answers) {
            if now_millis >= answer.deadline_at_millis {
                tracing::warn!(
                    request_id = %answer.request_id,
                    "the coordination worker could not answer a request before its deadline"
                );
                continue;
            }
            due.push(answer);
        }
        self.retry_at = (!due.is_empty()).then(|| now + RETRY_INTERVAL);
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(request_id: &str, deadline_at_millis: i64) -> RefusedAnswer<&'static str> {
        RefusedAnswer::new("an answer", request_id.to_string(), deadline_at_millis)
    }

    #[test]
    fn a_kept_answer_is_written_again_once() {
        let mut refused: RefusedAnswers<&str> = RefusedAnswers::default();
        assert!(refused.due(0).is_empty(), "nothing is refused yet");
        refused.keep(answer("request", 10));
        assert_eq!(refused.due(0).len(), 1, "the kept answer is written again");
        assert!(
            refused.due(0).is_empty(),
            "an answer that a retry has does not wait in the list"
        );
    }

    #[test]
    fn a_second_retry_waits_for_the_interval() {
        let mut refused: RefusedAnswers<&str> = RefusedAnswers::default();
        refused.keep(answer("request", 10));
        for answer in refused.due(0) {
            // The retry failed again, as a disk that stays busy does.
            refused.keep(answer);
        }
        assert!(
            refused.due(0).is_empty(),
            "a refused answer is written again at most every RETRY_INTERVAL"
        );
    }

    #[test]
    fn an_answer_whose_deadline_passed_is_dropped() {
        let mut refused: RefusedAnswers<&str> = RefusedAnswers::default();
        refused.keep(answer("expired", 5));
        assert!(
            refused.due(5).is_empty(),
            "a retry at the request deadline writes nothing"
        );
        assert!(refused.due(0).is_empty(), "the answer is gone");
    }
}
