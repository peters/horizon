pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid topology: {0}")]
    InvalidTopology(String),
    #[error("plan no longer matches the current topology")]
    StalePlan,
    #[error("access denied")]
    Denied,
    #[error("unknown service: {0}")]
    UnknownService(String),
    #[error("invalid agent configuration: {0}")]
    InvalidConfiguration(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("message exceeds the protocol limit")]
    MessageTooLarge,
    #[error("protocol deadline exceeded")]
    Timeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid JSON document")]
    Serialization(#[from] serde_json::Error),
}
