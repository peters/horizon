//! Bounded recognition of a rejected user-completed Cloudflare challenge.

use std::time::{Duration, Instant};

use serde_json::Value;

const HANDOFF_WINDOW_SECONDS: u64 = 5 * 60;
const HANDOFF_WINDOW: Duration = Duration::from_secs(HANDOFF_WINDOW_SECONDS);
const REJECTION_REPORT_DELAY: Duration = Duration::from_secs(1);

pub(crate) const REJECTION_MESSAGE: &str =
    "Cloudflare rejected the completed verification and presented the same challenge again";

#[derive(Clone, Debug)]
struct Observation {
    url: String,
    observed_at: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct ChallengeLoopDetector {
    last_challenge: Option<Observation>,
    armed: Option<Observation>,
    rejection_due: Option<Instant>,
    reported_url: Option<String>,
    handoff_active: bool,
}

impl ChallengeLoopDetector {
    pub(crate) fn observe_document_response(&mut self, url: &str, status: Option<u16>, headers: &Value) {
        self.observe_document_response_at(Instant::now(), url, status, is_cloudflare_challenge(headers));
    }

    pub(crate) fn handoff_completed(&mut self) {
        self.handoff_completed_at(Instant::now());
    }

    pub(crate) fn observe_handoff_change(&mut self, previous: Option<&str>, current: Option<&str>) {
        self.observe_handoff_change_at(Instant::now(), previous, current);
    }

    pub(crate) fn take_rejection(&mut self) -> Option<&'static str> {
        self.take_rejection_at(Instant::now())
    }

    fn observe_document_response_at(&mut self, now: Instant, url: &str, status: Option<u16>, is_challenge: bool) {
        if is_challenge {
            let repeated_after_handoff = self.armed.as_ref().is_some_and(|armed| {
                armed.url == url && now.saturating_duration_since(armed.observed_at) <= HANDOFF_WINDOW
            });
            if repeated_after_handoff && self.reported_url.as_deref() != Some(url) {
                self.rejection_due = Some(now + REJECTION_REPORT_DELAY);
                self.reported_url = Some(url.to_string());
            }
            if self.armed.as_ref().is_some_and(|armed| armed.url != url) {
                self.armed = None;
            }
            self.last_challenge = Some(Observation {
                url: url.to_string(),
                observed_at: now,
            });
            if self.handoff_active && self.armed.is_none() {
                self.arm_latest_challenge(now);
            }
            return;
        }

        if status.is_some_and(|status| (200..300).contains(&status)) {
            self.last_challenge = None;
            self.armed = None;
            self.rejection_due = None;
            self.reported_url = None;
        }
    }

    fn handoff_started_at(&mut self, now: Instant) {
        self.handoff_active = true;
        self.rejection_due = None;
        self.reported_url = None;
        self.arm_latest_challenge(now);
    }

    fn observe_handoff_change_at(&mut self, now: Instant, previous: Option<&str>, current: Option<&str>) {
        match (previous, current) {
            (None, Some(_)) => self.handoff_started_at(now),
            (Some(previous), Some(current)) if previous != current => {
                self.handoff_cancelled_at();
                self.handoff_started_at(now);
            }
            (Some(_), None) => self.handoff_cancelled_at(),
            _ => {}
        }
    }

    fn handoff_completed_at(&mut self, now: Instant) {
        self.handoff_active = false;
        if let Some(armed) = &mut self.armed {
            armed.observed_at = now;
        } else {
            self.arm_latest_challenge(now);
        }
    }

    fn handoff_cancelled_at(&mut self) {
        self.handoff_active = false;
        self.armed = None;
        self.rejection_due = None;
        self.reported_url = None;
    }

    fn arm_latest_challenge(&mut self, now: Instant) {
        self.armed = self.last_challenge.as_ref().and_then(|challenge| {
            (now.saturating_duration_since(challenge.observed_at) <= HANDOFF_WINDOW).then(|| Observation {
                url: challenge.url.clone(),
                observed_at: now,
            })
        });
    }

    fn take_rejection_at(&mut self, now: Instant) -> Option<&'static str> {
        if self.handoff_active {
            return None;
        }
        let due = self.rejection_due.filter(|due| now >= *due)?;
        debug_assert!(now >= due);
        self.rejection_due = None;
        Some(REJECTION_MESSAGE)
    }
}

fn is_cloudflare_challenge(headers: &Value) -> bool {
    match headers {
        Value::Object(headers) => headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("cf-mitigated")
                && value
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("challenge"))
        }),
        Value::Array(headers) => headers.iter().any(|header| {
            header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case("cf-mitigated"))
                && header_value(header).is_some_and(|value| value.eq_ignore_ascii_case("challenge"))
        }),
        _ => false,
    }
}

fn header_value(header: &Value) -> Option<&str> {
    header
        .get("value")
        .and_then(Value::as_str)
        .or_else(|| header.pointer("/value/value").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_then_same_challenge_reports_one_rejection() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_completed_at(started + Duration::from_secs(1));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        assert_eq!(
            detector.take_rejection_at(started + Duration::from_secs(3)),
            Some(REJECTION_MESSAGE)
        );
        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(4)), None);
    }

    #[test]
    fn repeated_challenge_during_handoff_reports_after_hand_back() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_started_at(started + Duration::from_secs(1));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(4)), None);
        detector.handoff_completed_at(started + Duration::from_secs(5));
        assert_eq!(
            detector.take_rejection_at(started + Duration::from_secs(5)),
            Some(REJECTION_MESSAGE)
        );
    }

    #[test]
    fn long_handoff_starts_a_fresh_rejection_window_at_hand_back() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_started_at(started + Duration::from_secs(1));

        let hand_back = started + HANDOFF_WINDOW + Duration::from_secs(2);
        detector.handoff_completed_at(hand_back);
        detector.observe_document_response_at(
            hand_back + Duration::from_secs(1),
            "https://example.test/protected",
            Some(403),
            true,
        );

        assert_eq!(
            detector.take_rejection_at(hand_back + Duration::from_secs(2)),
            Some(REJECTION_MESSAGE)
        );
    }

    #[test]
    fn first_challenge_during_handoff_arms_the_next_repetition() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.handoff_started_at(started);
        detector.observe_document_response_at(
            started + Duration::from_secs(1),
            "https://example.test/protected",
            Some(403),
            true,
        );
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(4)), None);
        detector.handoff_completed_at(started + Duration::from_secs(5));
        assert_eq!(
            detector.take_rejection_at(started + Duration::from_secs(5)),
            Some(REJECTION_MESSAGE)
        );
    }

    #[test]
    fn replacement_handoff_discards_the_previous_requests_rejection() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.observe_handoff_change_at(started + Duration::from_secs(1), None, Some("first"));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        detector.observe_handoff_change_at(started + Duration::from_secs(3), Some("first"), Some("replacement"));
        detector.handoff_completed_at(started + Duration::from_secs(4));

        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(5)), None);
    }

    #[test]
    fn new_handoff_discards_a_pending_prior_rejection() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_completed_at(started + Duration::from_secs(1));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        detector.observe_handoff_change_at(started + Duration::from_secs(3), None, Some("next"));
        detector.handoff_completed_at(started + Duration::from_secs(4));

        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(5)), None);
    }

    #[test]
    fn cancelled_handoff_discards_an_unreported_rejection() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_started_at(started + Duration::from_secs(1));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/protected",
            Some(403),
            true,
        );

        detector.handoff_cancelled_at();

        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(4)), None);
    }

    #[test]
    fn successful_document_or_different_url_does_not_report_a_rejection() {
        let started = Instant::now();
        let mut detector = ChallengeLoopDetector::default();
        detector.observe_document_response_at(started, "https://example.test/protected", Some(403), true);
        detector.handoff_completed_at(started + Duration::from_secs(1));
        detector.observe_document_response_at(
            started + Duration::from_secs(2),
            "https://example.test/other",
            Some(403),
            true,
        );
        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(4)), None);

        detector.observe_document_response_at(
            started + Duration::from_secs(5),
            "https://example.test/other",
            Some(200),
            false,
        );
        detector.handoff_completed_at(started + Duration::from_secs(6));
        detector.observe_document_response_at(
            started + Duration::from_secs(7),
            "https://example.test/other",
            Some(403),
            true,
        );
        assert_eq!(detector.take_rejection_at(started + Duration::from_secs(9)), None);
    }

    #[test]
    fn cloudflare_marker_accepts_cdp_and_bidi_header_shapes() {
        assert!(is_cloudflare_challenge(&serde_json::json!({
            "Cf-Mitigated": "challenge"
        })));
        assert!(is_cloudflare_challenge(&serde_json::json!([{
            "name": "cf-mitigated",
            "value": { "type": "string", "value": "challenge" }
        }])));
        assert!(!is_cloudflare_challenge(&serde_json::json!({
            "cf-mitigated": "block"
        })));
    }
}
