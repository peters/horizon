#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshConnectionStatus {
    Connecting,
    /// The link was lost, and Horizon waits to start the panel's command again
    /// at the same tmux session. The body keeps the last screen.
    Reconnecting,
    Connected,
    Disconnected,
}

/// The attempt from which a reconnect mark carries its number. A person reads
/// the first two marks as one wait; from the third attempt on, the number says
/// that Horizon keeps trying.
const NUMBERED_FROM_ATTEMPT: u32 = 3;

impl SshConnectionStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting...",
            Self::Reconnecting => "Reconnecting",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
        }
    }

    /// The mark of a reconnect that reached `attempt`, or `None` while the mark
    /// is the plain word. The text is built once per attempt, not per frame.
    #[must_use]
    pub fn reconnecting_label(attempt: u32) -> Option<String> {
        (attempt >= NUMBERED_FROM_ATTEMPT).then(|| format!("{} ({attempt})", Self::Reconnecting.label()))
    }
}

#[cfg(test)]
mod tests {
    use super::SshConnectionStatus;

    #[test]
    fn a_reconnect_names_its_attempt_from_the_third_one() {
        assert_eq!(SshConnectionStatus::reconnecting_label(0), None);
        assert_eq!(SshConnectionStatus::reconnecting_label(1), None);
        assert_eq!(SshConnectionStatus::reconnecting_label(2), None);
        assert_eq!(
            SshConnectionStatus::reconnecting_label(3).as_deref(),
            Some("Reconnecting (3)")
        );
        assert_eq!(
            SshConnectionStatus::reconnecting_label(12).as_deref(),
            Some("Reconnecting (12)")
        );
    }

    #[test]
    fn the_plain_marks_keep_their_words() {
        assert_eq!(SshConnectionStatus::Connecting.label(), "Connecting...");
        assert_eq!(SshConnectionStatus::Reconnecting.label(), "Reconnecting");
        assert_eq!(SshConnectionStatus::Connected.label(), "Connected");
        assert_eq!(SshConnectionStatus::Disconnected.label(), "Disconnected");
    }
}
