//! The wire between a browser panel of this computer and the Horizon driver
//! that runs beside the browser on a browser host.
//!
//! One line of JSON is one message in each direction. The driver of the host
//! holds the browser and the pages of its profile; this computer sends what the
//! person and the agent ask for and receives a pushed picture and the state of
//! the page of its link.
//!
//! One link drives one page. The greeting of a link names that page, so the
//! session of the host gives each panel its own page of the one browser, and a
//! link that comes back after a loss finds the page that it had.
//!
//! The version of this file travels in every handshake. A build that speaks
//! another version refuses the other end in one line of words, because a field
//! that one side reads differently is a wrong page rather than a slow one.
//!
//! Two rules keep a slow link honest:
//!
//! - The picture is pushed, not polled, and only when it changes. One frame may
//!   be in flight: the driver sends the next frame after
//!   [`HostDriverRequest::FrameAck`] of the last, so a link that cannot carry the
//!   frames carries fewer and newer ones instead of a queue of old ones.
//! - Every line has a bound ([`MAX_HOST_DRIVER_LINE_BYTES`]). A larger line is a
//!   refusal that names its size, never a buffer that grows with the link.

use serde::{Deserialize, Serialize};

use crate::{
    AgentAction, AgentActionResult, AutomationDisclosureStatus, BrowserAuditEntry, BrowserControlAction, BrowserInput,
    FileChooserStatus,
};

/// The version of this wire. One number for the whole file: a reader refuses
/// every other version instead of guessing which fields it shares.
pub const HOST_DRIVER_VERSION: u32 = 2;

/// Longest one line may be, in bytes, before the newline. A frame of a large
/// page is the largest message, and a JPEG of a 4096 pixel side fits with room
/// for its base64 text; a longer line means that the other end sent something
/// that this wire does not describe, so the reader refuses it and ends the
/// link with words.
pub const MAX_HOST_DRIVER_LINE_BYTES: usize = 12 * 1024 * 1024;

/// Default quality of a pushed frame. The study of a browser host measured 47
/// to 111 KB per frame at this quality against 110 to 484 KB as PNG.
pub const DEFAULT_HOST_FRAME_QUALITY: u32 = 60;

/// Lowest and highest frame quality that a person can name as a setting.
pub const MIN_HOST_FRAME_QUALITY: u32 = 20;
pub const MAX_HOST_FRAME_QUALITY: u32 = 95;

/// Longest side of a frame. A picture of a larger page is a refusal with words,
/// because a panel cannot show it and the link cannot carry it.
pub const MAX_HOST_FRAME_SIDE: u32 = 4096;

/// The exit code of a host driver that refused its caller: the words are on the
/// line that it wrote first.
pub const HOST_DRIVER_REFUSED_EXIT: i32 = 3;

/// What this computer sends to the driver of its browser host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostDriverRequest {
    /// The first line of every link. The driver answers
    /// [`HostDriverReply::Welcome`] or a [`HostDriverReply::Refusal`].
    Hello {
        version: u32,
        /// Version of the Horizon of this computer, for the words of a mismatch.
        horizon_version: String,
        /// System and CPU of this computer, which the driver compares with its
        /// own before it serves a binary that a person copied to the host.
        system: HostSystem,
        /// Quality of the pushed frames that this link wants.
        quality: u32,
        /// The page that this link drives: the identity of the panel, which
        /// stays the same across a lost link and a restart. A session of a
        /// browser host keeps one page for each identity that a link names, and
        /// a link that names a page that the session has gives that page back
        /// with its typed text and its scroll place.
        #[serde(default)]
        page: String,
        /// Whether this page keeps nothing of the profile: an isolated browser
        /// context of the same browser, whose cookies and storage are apart and
        /// which is removed when the page ends.
        #[serde(default)]
        private: bool,
    },
    /// One input of the person or of an agent. A run of typed text is one
    /// [`BrowserInput::InsertText`], so typing costs one message rather than one
    /// per character.
    Input {
        input: BrowserInput,
    },
    Navigate {
        url: String,
    },
    Reload,
    Back,
    Forward,
    Viewport {
        width: u32,
        height: u32,
    },
    /// The frame `seq` reached this computer, so the driver may send the next.
    FrameAck {
        seq: u64,
    },
    /// One operation of the agent tools, executed on the browser of the host.
    Semantic {
        action_id: String,
        action: Box<BrowserControlAction>,
    },
    /// The coordination state of the agent manifest of this computer, so the
    /// driver can tell an agent from a person and hand the page over.
    Signals {
        signals: HostDriverSignals,
    },
    /// End the page of this link, and with the last page of a session the
    /// session itself: the browser stops and the files of the session go away.
    /// A lost link never sends this, because the point of the driver on the host
    /// is that the page outlives the link.
    Close,
}

/// What the driver of a browser host sends to this computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostDriverReply {
    Welcome(HostDriverWelcome),
    /// One picture of the page, pushed because it changed. The driver sends no
    /// frame before the acknowledgement of the one that is in flight.
    Frame(HostDriverFrame),
    /// The page that the driver shows: its address, its title, and whether it
    /// is the page of the link before a lost one.
    Page(HostDriverPage),
    /// The driver wrote the result of an agent tool, for this computer to hand
    /// to the agent.
    Action(HostDriverActionResult),
    /// The coordination of the agent manifest of this computer changed in the
    /// driver, and this computer writes it into the manifest.
    Coordination(Box<HostDriverCoordination>),
    /// The driver acknowledged a handoff request, so this computer marks it
    /// done in its manifest.
    HandoffAck {
        request_id: String,
    },
    /// One record of what an agent did to the page, for the audit sink of this
    /// computer.
    Audit(Box<BrowserAuditEntry>),
    /// A refusal or a failure with words for the person. The link stays open
    /// unless `fatal` says that the driver ends.
    Refusal {
        code: String,
        message: String,
        fatal: bool,
    },
    /// The driver ended the session of the host, and the browser with it.
    Closed,
}

/// The system and CPU of an end of this wire, so the two ends can compare them
/// before a copied binary is expected to run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSystem {
    pub os: String,
    pub arch: String,
}

impl HostSystem {
    /// The system and CPU of the program that calls this function.
    #[must_use]
    pub fn here() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }

    /// The words of a person for a host whose system or CPU differs, when a
    /// copy of this computer's binary is the way the host got its Horizon.
    #[must_use]
    pub fn copy_refusal(&self, host: &Self) -> Option<String> {
        (self != host).then(|| {
            format!(
                "the browser host runs {} on {}, and this computer runs {} on {}: a copy of this computer's horizon does not run there, so install a horizon built for the host, and keep both builds of the same version",
                host.os, host.arch, self.os, self.arch
            )
        })
    }
}

/// The first answer of the driver: what it runs, and which page this link came
/// back to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDriverWelcome {
    pub version: u32,
    pub horizon_version: String,
    pub system: HostSystem,
    /// The engine that the host driver runs: `firefox` is the one of this round.
    pub engine: String,
    pub profile: String,
    /// Whether the browser keeps the page of a link that was lost, or opened a
    /// new one because the old page was gone.
    pub page: HostDriverPageState,
    pub width: u32,
    pub height: u32,
    /// Quality of the frames that this driver sends.
    pub quality: u32,
}

/// Whether a link came back to the page that it had.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDriverPageState {
    /// The same page as the link before: its text, its scroll place and its
    /// session are still there.
    Same,
    /// A new page of the last address, so what the person typed into the old
    /// one is gone.
    New,
}

impl HostDriverPageState {
    #[must_use]
    pub const fn resumed(self) -> bool {
        matches!(self, Self::Same)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::New => "new",
        }
    }
}

/// One pushed picture of the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDriverFrame {
    /// Sequence of the picture, so this computer hands the newest to the panel
    /// and acknowledges exactly the one it received.
    pub seq: u64,
    /// Encoding of `data`. This version pushes JPEG only, and a reader refuses
    /// any other name rather than guess.
    pub format: HostDriverFrameFormat,
    pub quality: u32,
    pub width: u32,
    pub height: u32,
    /// The first picture of a link. A key frame is a whole picture of the page,
    /// sent after a lost link or after the driver changed what it shows.
    pub key: bool,
    /// The picture as base64 text, because one line of JSON carries no bytes.
    pub data: String,
}

/// How one pushed frame is encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostDriverFrameFormat {
    Jpeg,
}

impl HostDriverFrameFormat {
    /// The name that this wire uses for `format`, for a refusal that names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
        }
    }
}

/// The page that the driver shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDriverPage {
    pub url: String,
    pub title: String,
    /// Whether this page came back across a lost link.
    pub resumed: bool,
    /// Whether the browser is still fetching the page.
    pub loading: bool,
    /// Whether the session of that page proved that it keeps nothing of a profile:
    /// a page of an isolated browser context of that machine. A driver of an older
    /// host omits it, and then no panel calls that page private.
    #[serde(default)]
    pub keeps_nothing: bool,
}

/// The result of one operation of the agent tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostDriverActionResult {
    pub action_id: String,
    pub result: AgentActionResult,
}

/// The coordination state of the agent manifest of this computer, as the driver
/// of the host changed it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostDriverCoordination {
    /// Whether the session runs for an agent, and which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The handoff request that the driver asks this computer for, when it
    /// needs the person to take the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HostDriverHandoff>,
    pub url: String,
    pub title: String,
    pub target_id: String,
    pub page_resumed: bool,
    pub automation_disclosure: AutomationDisclosureStatus,
    pub file_chooser: FileChooserStatus,
    pub passkey_pending: bool,
}

/// One handoff request of the driver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDriverHandoff {
    pub request_id: String,
    pub reason: String,
}

/// What this computer tells the driver about its agent manifest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostDriverSignals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HostDriverHandoff>,
    /// Validated actions that this computer claimed for the driver, so the
    /// driver runs them on the page of the host and returns one result each.
    pub actions: Vec<AgentAction>,
}

/// One frame in flight over the link of a browser host.
///
/// The panel of a slow link must not receive a queue of pictures of the past.
/// The driver therefore sends one frame and waits for its acknowledgement; a
/// page that keeps changing is sampled again when the acknowledgement arrives,
/// so the panel shows the newest frame that the link could carry. The first
/// frame of a link is a key frame, because a panel that just attached has no
/// picture to keep.
#[derive(Clone, Debug, Default)]
pub struct FrameGate {
    in_flight: Option<u64>,
    key_pending: bool,
    sent: u64,
}

impl FrameGate {
    /// A gate that waits for the acknowledgement of its next frame, which is a
    /// key frame.
    #[must_use]
    pub fn new() -> Self {
        Self {
            in_flight: None,
            key_pending: true,
            sent: 0,
        }
    }

    /// A new link of the same browser: the frame after it is a key frame, and
    /// no frame of the old link is in flight.
    pub fn reattach(&mut self) {
        self.in_flight = None;
        self.key_pending = true;
    }

    /// Whether the driver may send a frame now.
    #[must_use]
    pub const fn may_send(&self) -> bool {
        self.in_flight.is_none()
    }

    /// Whether the next frame is a key frame, and clears the mark.
    pub fn take_key(&mut self) -> bool {
        std::mem::take(&mut self.key_pending)
    }

    /// Record that `seq` is in flight, chosen while [`Self::may_send`] said so.
    pub fn sent(&mut self, seq: u64) {
        debug_assert!(self.may_send(), "one frame in flight");
        self.in_flight = Some(seq);
        self.sent = seq;
    }

    /// The acknowledgement of `seq`: `true` when it was the frame in flight, so
    /// the next frame may go. An older or unknown sequence changes nothing.
    pub fn acknowledge(&mut self, seq: u64) -> bool {
        if self.in_flight == Some(seq) {
            self.in_flight = None;
            return true;
        }
        false
    }

    /// The sequence of the frame in flight, when one waits for its
    /// acknowledgement.
    #[must_use]
    pub const fn in_flight(&self) -> Option<u64> {
        self.in_flight
    }

    /// Frames that this gate sent on the current link, for the latency line of
    /// a link that carries fewer frames than the page has.
    #[must_use]
    pub const fn sent_frames(&self) -> u64 {
        self.sent
    }
}

/// Why a line of this wire could not be used.
#[derive(Debug, thiserror::Error)]
pub enum HostDriverLineError {
    #[error("the other end wrote a line of {size} bytes, and this wire carries at most {limit} bytes")]
    TooLong { size: usize, limit: usize },
    #[error("the line of the other end is not one message of browser host protocol version {version}: {reason}")]
    Message { version: u32, reason: String },
    #[error("the link of the browser host ended")]
    Ended,
    #[error("this computer could not read the link of its browser host: {0}")]
    Read(std::io::Error),
    #[error("this computer could not write to the link of its browser host: {0}")]
    Write(std::io::Error),
}

/// Read one message of at most `limit` bytes from `reader`.
///
/// Returns `Ok(None)` at the end of the link with no partial line, so a caller
/// can tell a closed link from a refusal.
///
/// The read stops at the bound instead of growing the line: the caller refuses
/// the other end and says how large the line was.
///
/// # Errors
/// See [`HostDriverLineError`]: the line is larger than `limit`, the link ended
/// in the middle of a line, or the read failed.
pub fn read_line(reader: &mut impl std::io::BufRead, limit: usize) -> Result<Option<Vec<u8>>, HostDriverLineError> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(HostDriverLineError::Read(error)),
        };
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(HostDriverLineError::Ended)
            };
        }
        if let Some(position) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + position > limit {
                return Err(HostDriverLineError::TooLong {
                    size: line.len() + position,
                    limit,
                });
            }
            line.extend_from_slice(&available[..position]);
            reader.consume(position + 1);
            return Ok(Some(line));
        }
        if line.len() + available.len() > limit {
            return Err(HostDriverLineError::TooLong {
                size: line.len() + available.len(),
                limit,
            });
        }
        let taken = available.len();
        line.extend_from_slice(available);
        reader.consume(taken);
    }
}

/// Decode one line of this computer's side of the wire.
///
/// # Errors
/// The line is not one [`HostDriverRequest`] of this version of the wire.
pub fn decode_request(line: &[u8]) -> Result<HostDriverRequest, HostDriverLineError> {
    serde_json::from_slice(line).map_err(|error| HostDriverLineError::Message {
        version: HOST_DRIVER_VERSION,
        reason: error.to_string(),
    })
}

/// Decode one line of the driver's side of the wire.
///
/// # Errors
/// The line is not one [`HostDriverReply`] of this version of the wire.
pub fn decode_reply(line: &[u8]) -> Result<HostDriverReply, HostDriverLineError> {
    serde_json::from_slice(line).map_err(|error| HostDriverLineError::Message {
        version: HOST_DRIVER_VERSION,
        reason: error.to_string(),
    })
}

/// Encode one message as one line, with its newline, and refuse a message that
/// is larger than the wire allows instead of writing a line that the other end
/// must drop.
///
/// # Errors
/// The message is larger than [`MAX_HOST_DRIVER_LINE_BYTES`].
pub fn encode_line<T: Serialize>(message: &T) -> Result<Vec<u8>, HostDriverLineError> {
    let mut line = serde_json::to_vec(message)
        .map_err(|error| HostDriverLineError::Write(std::io::Error::other(error.to_string())))?;
    if line.len() > MAX_HOST_DRIVER_LINE_BYTES {
        return Err(HostDriverLineError::TooLong {
            size: line.len(),
            limit: MAX_HOST_DRIVER_LINE_BYTES,
        });
    }
    line.push(b'\n');
    Ok(line)
}

/// The refusal that a reader writes when the version of the other end is not
/// [`HOST_DRIVER_VERSION`].
#[must_use]
pub fn version_refusal(theirs: u32) -> HostDriverReply {
    let message = format!(
        "this Horizon speaks browser host protocol version {HOST_DRIVER_VERSION}, and the other end speaks {theirs}: copy the same horizon binary to the browser host, and start the panel again"
    );
    HostDriverReply::Refusal {
        code: "protocol_version".to_string(),
        message,
        fatal: true,
    }
}

/// The refusal that a reader writes when a line is larger than the wire allows.
#[must_use]
pub fn line_refusal(size: usize, limit: usize) -> HostDriverReply {
    HostDriverReply::Refusal {
        code: "line_too_long".to_string(),
        message: format!(
            "the other end wrote a message of {size} bytes, and a message of this link carries at most {limit} bytes"
        ),
        fatal: true,
    }
}

/// Keep a quality inside the range that a frame can use.
#[must_use]
pub fn clamp_quality(quality: u32) -> u32 {
    quality.clamp(MIN_HOST_FRAME_QUALITY, MAX_HOST_FRAME_QUALITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserInput, BrowserKey, BrowserModifiers};

    fn round_trip_request(request: &HostDriverRequest) -> HostDriverRequest {
        let line = encode_line(request).expect("one line");
        assert!(line.ends_with(b"\n"), "a message ends with its newline");
        let mut reader = std::io::BufReader::new(line.as_slice());
        let read = read_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES)
            .expect("a readable line")
            .expect("a line");
        decode_request(&read).expect("one message")
    }

    fn round_trip_reply(reply: &HostDriverReply) -> HostDriverReply {
        let line = encode_line(reply).expect("one line");
        let mut reader = std::io::BufReader::new(line.as_slice());
        let read = read_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES)
            .expect("a readable line")
            .expect("a line");
        decode_reply(&read).expect("one message")
    }

    #[test]
    fn every_request_of_this_wire_round_trips() {
        let hello = HostDriverRequest::Hello {
            version: HOST_DRIVER_VERSION,
            horizon_version: "1.2.3".to_string(),
            system: HostSystem::here(),
            quality: DEFAULT_HOST_FRAME_QUALITY,
            page: "panel-1".to_string(),
            private: false,
        };
        for request in [
            hello,
            HostDriverRequest::Input {
                input: BrowserInput::InsertText {
                    text: "one run of typed text".to_string(),
                },
            },
            HostDriverRequest::Input {
                input: BrowserInput::KeyDown {
                    physical_key: Some(BrowserKey::Enter),
                    key: BrowserKey::Enter,
                    text: None,
                    modifiers: BrowserModifiers::default(),
                    repeat: false,
                    edit_command: None,
                },
            },
            HostDriverRequest::Input {
                input: BrowserInput::MouseMove {
                    x: 3.0,
                    y: 4.0,
                    buttons: 0,
                    modifiers: BrowserModifiers::default(),
                },
            },
            HostDriverRequest::Input {
                input: BrowserInput::Wheel {
                    x: 1.0,
                    y: 2.0,
                    delta_x: 0.0,
                    delta_y: -120.0,
                    modifiers: BrowserModifiers::default(),
                },
            },
            HostDriverRequest::Navigate {
                url: "https://example.test/".to_string(),
            },
            HostDriverRequest::Reload,
            HostDriverRequest::Back,
            HostDriverRequest::Forward,
            HostDriverRequest::Viewport {
                width: 1280,
                height: 800,
            },
            HostDriverRequest::FrameAck { seq: 7 },
            HostDriverRequest::Semantic {
                action_id: "a-1".to_string(),
                action: Box::new(BrowserControlAction::Snapshot { max_nodes: 100 }),
            },
            HostDriverRequest::Signals {
                signals: HostDriverSignals::default(),
            },
            HostDriverRequest::Close,
        ] {
            assert_eq!(round_trip_request(&request), request, "{request:?}");
        }
    }

    #[test]
    fn every_reply_of_this_wire_round_trips() {
        let welcome = HostDriverWelcome {
            version: HOST_DRIVER_VERSION,
            horizon_version: "1.2.3".to_string(),
            system: HostSystem::here(),
            engine: "firefox".to_string(),
            profile: "default".to_string(),
            page: HostDriverPageState::Same,
            width: 1280,
            height: 800,
            quality: DEFAULT_HOST_FRAME_QUALITY,
        };
        let page = HostDriverPage {
            url: "https://example.test/".to_string(),
            title: "Example".to_string(),
            resumed: true,
            loading: false,
            keeps_nothing: false,
        };
        for reply in [
            HostDriverReply::Welcome(welcome),
            HostDriverReply::Frame(HostDriverFrame {
                seq: 4,
                format: HostDriverFrameFormat::Jpeg,
                quality: DEFAULT_HOST_FRAME_QUALITY,
                width: 1280,
                height: 714,
                key: true,
                data: "AAAA".to_string(),
            }),
            HostDriverReply::Page(page),
            HostDriverReply::Action(HostDriverActionResult {
                action_id: "a-1".to_string(),
                result: AgentActionResult::completed("a-1", crate::BrowserControlValue::Accepted),
            }),
            HostDriverReply::Coordination(Box::new(HostDriverCoordination {
                owner: Some("agent".to_string()),
                handoff: Some(HostDriverHandoff {
                    request_id: "h-1".to_string(),
                    reason: "a code".to_string(),
                }),
                url: "https://example.test/".to_string(),
                title: "Example".to_string(),
                target_id: "t-1".to_string(),
                page_resumed: false,
                automation_disclosure: AutomationDisclosureStatus::default(),
                file_chooser: FileChooserStatus::default(),
                passkey_pending: false,
            })),
            HostDriverReply::HandoffAck {
                request_id: "h-1".to_string(),
            },
            HostDriverReply::Refusal {
                code: "no_firefox".to_string(),
                message: "there is no Firefox on this machine".to_string(),
                fatal: true,
            },
            HostDriverReply::Closed,
        ] {
            assert_eq!(round_trip_reply(&reply), reply, "{reply:?}");
        }
    }

    #[test]
    fn a_line_of_another_version_is_refused_with_words() {
        let reply = version_refusal(HOST_DRIVER_VERSION + 1);
        let HostDriverReply::Refusal { code, message, fatal } = reply else {
            panic!("a refusal");
        };
        assert_eq!(code, "protocol_version");
        assert!(fatal, "a version mismatch ends the link");
        assert!(
            message.contains(&format!("speaks browser host protocol version {HOST_DRIVER_VERSION}")),
            "{message}"
        );
        assert!(
            message.contains(&format!("speaks {}", HOST_DRIVER_VERSION + 1)),
            "{message}"
        );
    }

    #[test]
    fn a_line_larger_than_the_wire_allows_is_refused_with_its_size() {
        let long = vec![b'a'; 32];
        let mut reader = std::io::BufReader::new(long.as_slice());
        let error = read_line(&mut reader, 16).expect_err("a line of 32 bytes");
        let HostDriverLineError::TooLong { size, limit } = error else {
            panic!("a bound");
        };
        assert!(size > limit, "{size} > {limit}");
        assert_eq!(limit, 16);

        let refusal = line_refusal(32, 16);
        let HostDriverReply::Refusal { code, message, .. } = refusal else {
            panic!("a refusal");
        };
        assert_eq!(code, "line_too_long");
        assert!(
            message.contains("32 bytes") && message.contains("16 bytes"),
            "{message}"
        );

        // A frame that encodes to more than the bound is refused before it is
        // written, so no reader ever sees a partial line.
        let frame = HostDriverReply::Frame(HostDriverFrame {
            seq: 1,
            format: HostDriverFrameFormat::Jpeg,
            quality: DEFAULT_HOST_FRAME_QUALITY,
            width: 1,
            height: 1,
            key: true,
            data: "A".repeat(MAX_HOST_DRIVER_LINE_BYTES + 1),
        });
        assert!(matches!(encode_line(&frame), Err(HostDriverLineError::TooLong { .. })));
    }

    #[test]
    fn an_end_of_the_link_is_not_a_message() {
        let mut reader = std::io::BufReader::new(b"".as_slice());
        assert_eq!(read_line(&mut reader, 16).ok().flatten(), None);
        let mut partial = std::io::BufReader::new(b"{\"type\":\"close\"".as_slice());
        assert!(matches!(read_line(&mut partial, 64), Err(HostDriverLineError::Ended)));
    }

    #[test]
    fn a_line_that_is_not_a_message_of_this_wire_is_refused() {
        assert!(decode_request(b"{\"type\":\"fly\"}").is_err());
        assert!(decode_reply(b"not json").is_err());
        assert!(decode_request(b"").is_err());
    }

    #[test]
    fn a_frame_and_its_acknowledgement_share_one_sequence() {
        let frame = HostDriverReply::Frame(HostDriverFrame {
            seq: 12,
            format: HostDriverFrameFormat::Jpeg,
            quality: DEFAULT_HOST_FRAME_QUALITY,
            width: 1280,
            height: 714,
            key: false,
            data: "AAAA".to_string(),
        });
        let HostDriverReply::Frame(frame) = frame else {
            panic!("a frame");
        };
        let ack = round_trip_request(&HostDriverRequest::FrameAck { seq: frame.seq });
        assert_eq!(ack, HostDriverRequest::FrameAck { seq: 12 });
        assert_eq!(frame.format.as_str(), "jpeg");
    }

    #[test]
    fn a_copy_of_this_computer_binary_names_the_system_of_the_host() {
        let here = HostSystem::here();
        assert_eq!(here.copy_refusal(&here.clone()), None);
        let other = HostSystem {
            os: "other-os".to_string(),
            arch: "other-cpu".to_string(),
        };
        let words = here.copy_refusal(&other).expect("a refusal");
        assert!(words.contains("other-os") && words.contains("other-cpu"), "{words}");
        assert!(words.contains(&here.os) && words.contains(&here.arch), "{words}");
    }

    #[test]
    fn one_frame_is_in_flight_and_the_first_one_is_a_key_frame() {
        let mut gate = FrameGate::new();
        assert!(gate.may_send());
        assert!(gate.take_key(), "the first frame of a link is a key frame");
        assert!(!gate.take_key(), "only the first frame is one");

        gate.sent(1);
        assert!(!gate.may_send(), "the driver waits for the acknowledgement");
        assert!(!gate.acknowledge(0), "an older sequence acknowledges nothing");
        assert!(!gate.may_send(), "a frame is still in flight");
        assert!(gate.acknowledge(1), "the frame in flight came back");
        assert!(gate.may_send());
        assert!(!gate.take_key(), "a later frame of the same link is no key frame");

        // A new link of the same browser starts with a key frame again.
        gate.sent(2);
        gate.reattach();
        assert!(gate.may_send(), "no frame of the old link is in flight");
        assert!(gate.take_key());
        assert_eq!(gate.in_flight(), None);
    }

    #[test]
    fn the_quality_of_a_frame_stays_inside_the_range_of_the_setting() {
        assert_eq!(clamp_quality(0), MIN_HOST_FRAME_QUALITY);
        assert_eq!(clamp_quality(1_000), MAX_HOST_FRAME_QUALITY);
        assert_eq!(clamp_quality(60), DEFAULT_HOST_FRAME_QUALITY);
    }

    #[test]
    fn a_page_state_says_in_one_word_whether_the_page_came_back() {
        assert!(HostDriverPageState::Same.resumed());
        assert!(!HostDriverPageState::New.resumed());
        assert_eq!(HostDriverPageState::Same.as_str(), "same");
        assert_eq!(HostDriverPageState::New.as_str(), "new");
    }
}
