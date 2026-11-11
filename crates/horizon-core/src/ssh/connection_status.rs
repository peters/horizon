#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshConnectionStatus {
    Connecting,
    /// The link was lost, and Horizon waits to start the panel's command again
    /// at the same tmux session. The body keeps the last screen.
    Reconnecting,
    Connected,
    Disconnected,
}

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
}
