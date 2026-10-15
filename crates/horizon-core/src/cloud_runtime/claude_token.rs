//! Claude's token from `claude setup-token`, saved machine-wide like the agent API keys.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A token from `claude setup-token`: a year of Claude on the person's subscription. The
/// worker gets it as `CLAUDE_CODE_OAUTH_TOKEN` and needs no sign-in of its own.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClaudeToken {
    /// The private file that holds the token.
    pub file: PathBuf,
    /// When it was saved here, in seconds since 1970. A token runs out a year after it was
    /// made, which is taken to be just before it was saved.
    pub saved_at: i64,
}

impl ClaudeToken {
    /// How long a setup token lasts.
    pub const LIFETIME_SECONDS: i64 = 365 * 24 * 60 * 60;

    /// When the token runs out, in seconds since 1970, counted from when it was saved.
    #[must_use]
    pub fn expires_at(&self) -> i64 {
        self.saved_at.saturating_add(Self::LIFETIME_SECONDS)
    }

    /// The day the token was saved, in UTC.
    #[must_use]
    pub fn saved_on(&self) -> Option<time::Date> {
        day(self.saved_at)
    }

    /// The day the token runs out, in UTC.
    #[must_use]
    pub fn expires_on(&self) -> Option<time::Date> {
        day(self.expires_at())
    }
}

fn day(seconds: i64) -> Option<time::Date> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .map(time::OffsetDateTime::date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_runs_out_a_year_after_it_was_saved() {
        // 2026-10-15 12:00 UTC.
        let token = ClaudeToken {
            file: "/private/claude-token".into(),
            saved_at: 1_792_065_600,
        };
        assert_eq!(token.saved_on().unwrap().to_string(), "2026-10-15");
        assert_eq!(token.expires_on().unwrap().to_string(), "2027-10-15");
        let far = ClaudeToken {
            saved_at: i64::MAX,
            ..token
        };
        assert_eq!(far.expires_at(), i64::MAX);
        assert_eq!(far.expires_on(), None);
    }
}
