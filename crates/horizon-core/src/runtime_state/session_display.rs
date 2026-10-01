use time::OffsetDateTime;

use super::AgentSessionBinding;

impl AgentSessionBinding {
    /// A stable UTC date for identifying a session by its last activity.
    #[must_use]
    pub fn last_used_display(&self) -> String {
        let Some(date) = self
            .updated_at
            .filter(|timestamp| *timestamp > 0)
            .and_then(|timestamp| OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp) * 1_000_000).ok())
        else {
            return "Last used: unknown".to_string();
        };
        format!(
            "Last used: {:02} {} {} · {:02}:{:02} UTC",
            date.day(),
            date.month(),
            date.year(),
            date.hour(),
            date.minute()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PanelKind;

    #[test]
    fn activity_dates_use_milliseconds_and_a_named_timezone() {
        let binding = AgentSessionBinding::new(PanelKind::Codex, "session".into(), None, None, Some(1_790_883_985_973));
        assert_eq!(binding.last_used_display(), "Last used: 01 October 2026 · 19:46 UTC");
    }

    #[test]
    fn missing_or_invalid_activity_does_not_look_recent() {
        for timestamp in [None, Some(0), Some(-1), Some(i64::MAX)] {
            let binding = AgentSessionBinding::new(PanelKind::Codex, "session".into(), None, None, timestamp);
            assert_eq!(binding.last_used_display(), "Last used: unknown");
        }
    }
}
