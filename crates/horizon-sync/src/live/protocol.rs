//! The frames of a live link. Every text, list and byte in them comes from
//! another computer: the server cuts each one at its bound before it keeps or
//! relays it, and neither side writes one to a file or to a log line.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most raw bytes of terminal output in one frame.
pub const MAX_LIVE_OUTPUT_BYTES: usize = 48 * 1024;
/// The most raw bytes of typed input in one frame.
pub const MAX_LIVE_TYPED_BYTES: usize = 16 * 1024;
/// The most panels that one computer shares.
pub const MAX_LIVE_PANELS: usize = 64;
/// The most panels of other computers that one computer follows.
pub const MAX_LIVE_FOLLOWS: usize = 8;
/// The most computers that follow one panel.
pub const MAX_LIVE_WATCHERS: usize = 4;
/// The most characters of the label of a computer, of a device name and of the
/// identifier of a panel, and the largest side of the grid of a panel.
pub(crate) const LABEL_CHARS: usize = 60;
pub(crate) const DEVICE_CHARS: usize = 40;
pub(crate) const ID_CHARS: usize = 64;
pub(crate) const MAX_GRID: u16 = 1000;

/// Terminal bytes, which travel as base64 in the JSON of a frame.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveBytes(pub Arc<[u8]>);

/// The bytes of a terminal never reach a log line: a frame prints their count.
impl std::fmt::Debug for LiveBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} bytes", self.0.len())
    }
}

impl Serialize for LiveBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for LiveBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let decoded = STANDARD.decode(String::deserialize(deserializer)?);
        decoded
            .map(|bytes| Self(bytes.into()))
            .map_err(serde::de::Error::custom)
    }
}

/// One terminal panel that a computer shares. The texts hold at most 64, 120, 24,
/// 24 and 80 characters.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LivePanel {
    pub id: String,
    pub title: String,
    /// What runs in the panel, such as `claude` or `shell`.
    pub kind: String,
    /// What the panel does now: `working`, `blocked`, `done`, `idle` or empty.
    pub state: String,
    pub workspace: String,
    pub cols: u16,
    pub rows: u16,
    /// Whether the owner lets other computers type in the panel.
    pub typing: bool,
}

/// One computer that has a live link, and the panels that it shares.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveComputer {
    pub device: String,
    pub label: String,
    pub panels: Vec<LivePanel>,
    /// Whether this is the computer that reads the list. The handle of a
    /// computer sets it, and it does not travel.
    #[serde(skip)]
    pub own: bool,
}

/// One computer that follows a panel.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveWatcher {
    pub device: String,
    pub label: String,
}

/// Why the follow of one panel ended or did not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEndReason {
    /// The owner does not share the panel any more.
    Closed,
    /// The live link of the owner ended.
    Offline,
    /// The owner has no live link or does not share that panel.
    NotFound,
    /// The panel has the most followers, or this computer follows the most panels.
    Limit,
}

/// The state of the live link of a computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LiveState {
    #[default]
    Connecting,
    Open,
    /// The server is of a version without live links. Its sync link is unaffected.
    Unsupported,
    /// The link is away, with the reason of the last attempt.
    Down(String),
}

/// One request of a live link, from a computer to the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRequest {
    /// Turns this link into a live link. `label` is the readable name of the computer.
    Open {
        label: String,
    },
    /// The full list of the panels that this computer shares. It replaces the last.
    Panels {
        panels: Vec<LivePanel>,
    },
    /// Starts the follow of a panel of `owner`, or asks for a new screen of one.
    Follow {
        owner: String,
        panel: String,
    },
    Unfollow {
        owner: String,
        panel: String,
    },
    /// A new screen of a panel of this computer. The output that follows draws it.
    Screen {
        panel: String,
        cols: u16,
        rows: u16,
    },
    Output {
        panel: String,
        data: LiveBytes,
    },
    /// A panel of this computer is gone.
    Ended {
        panel: String,
    },
    /// What the person typed in a panel that this computer follows.
    Typed {
        owner: String,
        panel: String,
        data: LiveBytes,
    },
}

/// One event of a live link: a frame of the server, or the state of the link.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveEvent {
    /// The answer to `Open`: the device name of this link, and the list now. The
    /// handle of a computer gives it to the application as `Computers`.
    Opened {
        device: String,
        computers: Vec<LiveComputer>,
    },
    /// The computers that have a live link, this computer too.
    Computers { computers: Vec<LiveComputer> },
    /// To the owner: who follows this panel now. `refresh` asks for a new screen
    /// and the bytes that draw it, and an empty set ends the output of the panel.
    Watchers {
        panel: String,
        watchers: Vec<LiveWatcher>,
        refresh: bool,
    },
    /// To a follower: a new screen of the panel starts with this grid.
    Screen {
        owner: String,
        panel: String,
        cols: u16,
        rows: u16,
    },
    Output {
        owner: String,
        panel: String,
        data: LiveBytes,
    },
    Ended {
        owner: String,
        panel: String,
        reason: LiveEndReason,
    },
    /// To the owner: what the person of the computer `from` typed in the panel.
    Typed {
        panel: String,
        from: String,
        label: String,
        data: LiveBytes,
    },
    /// The state of the link changed. The handle of a computer makes this event,
    /// and it does not travel.
    #[serde(skip)]
    State(LiveState),
}

impl LiveEvent {
    /// About the bytes of this event in a frame, for the ceiling of one link.
    pub(crate) fn cost(&self) -> usize {
        match self {
            Self::Output { data, .. } | Self::Typed { data, .. } => data.0.len().div_ceil(3) * 4 + 512,
            other => serde_json::to_vec(other).map_or(512, |json| json.len() + 8),
        }
    }
}

/// `text` without its control characters, cut at `max` characters.
pub(crate) fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|char| !char.is_control()).take(max).collect()
}

/// The list of one computer: at most [`MAX_LIVE_PANELS`] panels with different
/// identifiers, each one cut at its bounds.
pub(crate) fn clean_panels(panels: Vec<LivePanel>) -> Vec<LivePanel> {
    let mut cleaned: Vec<LivePanel> = Vec::new();
    for panel in panels.into_iter().take(MAX_LIVE_PANELS) {
        let id = clean(&panel.id, ID_CHARS);
        if !id.is_empty() && cleaned.iter().all(|kept| kept.id != id) {
            cleaned.push(LivePanel {
                id,
                title: clean(&panel.title, 120),
                kind: clean(&panel.kind, 24),
                state: clean(&panel.state, 24),
                workspace: clean(&panel.workspace, 80),
                cols: panel.cols.clamp(1, MAX_GRID),
                rows: panel.rows.clamp(1, MAX_GRID),
                typing: panel.typing,
            });
        }
    }
    cleaned
}
