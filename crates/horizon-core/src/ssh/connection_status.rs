#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
}

impl SshConnectionStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting...",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
        }
    }
}
