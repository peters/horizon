//! Why a viewer of a frame source program ends, in the words that a panel shows.
use crate::picture::Fault;

/// One failure of one viewer. The text is one sentence for a person; a caller
/// that branches uses [`Error::code`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The program of the settings is not on this computer.
    NotInstalled(String),
    /// The program is there and this process could not start it.
    NotStarted(String),
    /// A stream of the program ended or failed.
    Io(String),
    /// The picture socket did not carry the contract: one typed fault.
    Picture(Fault),
    /// The control stream did not carry the contract.
    Control(String),
    /// The program speaks another version of the contract, or another format.
    Unsupported(String),
    /// The pixels of one picture are not one frame of this UI.
    Frame(String),
}

impl Error {
    /// The code that a caller of `device_panel` and a panel branch on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotInstalled(_) => "frame_source_program_missing",
            Self::NotStarted(_) => "frame_source_not_started",
            Self::Picture(_) | Self::Control(_) => "frame_source_protocol",
            Self::Unsupported(_) => "frame_source_unsupported",
            Self::Io(_) => "frame_source_io",
            Self::Frame(_) => "frame_source_frame",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled(program) => write!(formatter, "{program} is not installed on this computer"),
            Self::NotStarted(program) => write!(formatter, "{program} could not be started"),
            Self::Io(message) => write!(formatter, "the frame source program: {message}"),
            Self::Picture(fault) => write!(formatter, "{fault}"),
            Self::Control(message) => write!(formatter, "the control messages of the frame source program: {message}"),
            Self::Unsupported(message) => write!(formatter, "the frame source program {message}"),
            Self::Frame(message) => write!(formatter, "a picture of the frame source program: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<horizon_vnc::Error> for Error {
    fn from(error: horizon_vnc::Error) -> Self {
        Self::Frame(error.to_string())
    }
}

impl From<Fault> for Error {
    fn from(fault: Fault) -> Self {
        Self::Picture(fault)
    }
}
