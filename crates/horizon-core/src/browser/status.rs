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
}

impl BrowserStatus {
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        matches!(self, Self::Starting | Self::Ready)
    }
}
