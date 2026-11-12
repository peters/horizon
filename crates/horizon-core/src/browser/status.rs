//! Coarse liveness of the browser session of one panel, as the chrome reads it.

/// Coarse liveness of the panel's browser session.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BrowserStatus {
    #[default]
    Starting,
    Ready,
    Error {
        message: String,
    },
    Stopped {
        code: Option<i32>,
    },
    /// The link to the browser of another machine was lost and the panel waits
    /// to connect again by itself, as attempt `attempt`. The browser of that
    /// machine keeps running, and the panel opens the last address again in a
    /// new page.
    Reconnecting {
        attempt: u32,
        retry_at: std::time::Instant,
    },
}

impl BrowserStatus {
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        matches!(self, Self::Starting | Self::Ready)
    }

    /// Whether this panel waits for an automatic attempt of its own.
    #[must_use]
    pub const fn is_reconnecting(&self) -> bool {
        matches!(self, Self::Reconnecting { .. })
    }
}
