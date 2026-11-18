//! A terminal that follows a stream: it shows a terminal that runs on another
//! computer, and it starts no program and owns no process of this computer.
//!
//! [`Terminal::spawn_follow`] gives the event loop one end of a socket pair as the
//! link of a holder, so the VT parser and the status reader run as for any panel.
//! The other end belongs to the [`FollowFeed`], which writes the output of the
//! stream to it as `output` frames. Nothing ever reads that end: what the person
//! types goes to a queue of the feed and never to the link, and every answer that
//! this terminal would give the program is dropped, because the terminal of the
//! owner answers it.
//!
//! Every byte of a stream is untrusted. The feed bounds the grid, the output that
//! the link has not taken, the bytes of the person and the texts that it keeps,
//! and no call of it waits: the link is non-blocking and takes the rest later.

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use horizon_pty_tap::frames::{self, Frame};

use super::lifecycle::{PtyOutput, window_size_of};
use super::wake::OutputWakeState;
use super::{
    Error, FairMutex, Result, Term, TermMode, Terminal, TerminalDimensions, TerminalEventProxy, TerminalSpawnOptions,
};
use crate::panel::{DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, PANEL_SCROLLBACK_LIMIT, PanelKind};

/// The largest grid that a stream can ask for.
pub const MAX_FOLLOW_COLS: u16 = 1000;
pub const MAX_FOLLOW_ROWS: u16 = 500;
/// The output that waits for the link. Output above it drops what waits.
pub const MAX_WAITING_OUTPUT: usize = 8 * 1024 * 1024;
/// The bytes of the person that wait for [`FollowFeed::take_typed`]. Bytes above
/// it drop the oldest.
pub const MAX_TYPED: usize = 1024 * 1024;
/// The characters that the feed keeps of a name, a title or a reason.
const MAX_TEXT: usize = 200;
/// The payload of one frame of the link.
const FRAME: usize = 64 * 1024;
/// `CAN` ends a sequence that a cut of the stream left open.
const CAN: u8 = 0x18;
/// The start of a new screen: end the open sequence and the open synchronized
/// update, then reset the terminal, which clears the screen, the history and
/// the modes.
const SCREEN_START: &[u8] = b"\x18\x1b[?2026l\x1bc";
/// The start of a screen that a follower takes while it keeps its history: it
/// ends what was open and leaves the screen and the history as they are.
const SCREEN_AGAIN: &[u8] = b"\x18\x1b[?2026l";

/// The terminal that a follower shows, and how the person may use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowSpec {
    /// The computer that runs the terminal, as the server names it.
    pub device: String,
    /// The name of that computer for the person.
    pub label: String,
    /// The panel of that computer that holds the terminal.
    pub panel: String,
    /// The title of that panel.
    pub title: String,
    /// The kind of that panel, which reads its title and its status reports.
    pub kind: PanelKind,
    pub cols: u16,
    pub rows: u16,
    /// Whether the bytes of the person go back to the stream.
    pub typing: bool,
}

/// What the terminal of a follower and its feed share.
struct Shared {
    /// The grid, which the feed resizes. It is gone with its terminal.
    term: Weak<FairMutex<Term<TerminalEventProxy>>>,
    state: Mutex<State>,
    /// The person closed the panel of the follower.
    closed: AtomicBool,
    /// The wake of the terminal, which the feed arms: the stream arrives on a
    /// thread of its own, and the frame that shows it follows its first byte.
    wake: Arc<OutputWakeState>,
}

struct State {
    /// The end of the link that the feed writes.
    link: UnixStream,
    /// The frames that the link has not taken, and their bytes.
    waiting: VecDeque<Vec<u8>>,
    waiting_bytes: usize,
    /// The bytes of the first frame that the link took.
    sent: usize,
    typed: Vec<u8>,
    typing: bool,
    /// Columns and rows of the newest screen.
    grid: (u16, u16),
    /// Why the stream ended, while it has no newer screen.
    ended: Option<String>,
    /// Output was dropped since the newest screen.
    lost: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Adds `payload` as one frame behind the frames that wait.
    fn queue(&mut self, payload: &[u8]) {
        let mut frame = Vec::with_capacity(payload.len() + 8);
        if frames::write(&mut frame, &Frame::Output(payload.to_vec())).is_ok() {
            self.waiting_bytes += frame.len();
            self.waiting.push_back(frame);
        }
    }

    /// Drops the frames that wait. A frame that the link took a part of stays,
    /// because the link would lose the bounds of its frames without the rest.
    fn drop_waiting(&mut self) {
        self.waiting.truncate(usize::from(self.sent > 0));
        self.waiting_bytes = self.waiting.front().map_or(0, Vec::len);
    }

    /// Writes what the link takes now. A link that asks to wait takes the rest
    /// with a later call, and a link whose terminal is gone takes nothing.
    fn flush(&mut self) {
        while let Some(frame) = self.waiting.front() {
            match self.link.write(&frame[self.sent..]) {
                // A link that takes nothing makes no progress, so this pass ends.
                Ok(0) => return,
                Ok(written) => self.sent += written,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.sent = 0;
                    self.drop_waiting();
                    return;
                }
            }
            if self.sent == frame.len() {
                self.waiting_bytes -= frame.len();
                self.sent = 0;
                self.waiting.pop_front();
            }
        }
    }
}

/// The stream side of a follower: it takes the screens and the output of the
/// stream, and it gives what the person typed. No call waits for the terminal
/// beyond the short lock of its grid, and a clone feeds the same follower.
#[derive(Clone)]
pub struct FollowFeed {
    shared: Arc<Shared>,
}

impl FollowFeed {
    /// A new screen starts with this grid: the terminal takes the grid, and the
    /// output that follows draws the new screen. A `fresh` screen makes the
    /// terminal forget its screen, its history and its modes first; one that is
    /// not fresh keeps its history, and the screen draws each row at its place.
    ///
    /// The grid changes here, in this call and under the lock of the grid, and
    /// only then the link gets the first byte of the new screen. So the event
    /// loop can parse no byte of the new screen on the old grid. Output of the
    /// old screen that the link still holds is parsed before the start that
    /// begins the new screen, and output that waited for the link is dropped.
    pub fn screen(&self, fresh: bool, cols: u16, rows: u16) {
        let Some(term) = self.shared.term.upgrade() else {
            return;
        };
        let (cols, rows) = bounded_grid(cols, rows);
        {
            let mut state = self.shared.state();
            state.drop_waiting();
            state.grid = (cols, rows);
            state.ended = None;
            state.lost = false;
        }
        term.lock().resize(TerminalDimensions::new(rows, cols));
        self.shared.wake.arm();
        let mut state = self.shared.state();
        state.queue(if fresh { SCREEN_START } else { SCREEN_AGAIN });
        state.flush();
    }

    /// Output of the stream, which the terminal shows.
    pub fn output(&self, bytes: &[u8]) {
        if self.closed() {
            return;
        }
        self.shared.wake.arm();
        let mut state = self.shared.state();
        let mut bytes = bytes;
        if state.waiting_bytes + bytes.len() > MAX_WAITING_OUTPUT {
            // The terminal is behind by the whole bound. The newest output is
            // what the person reads, so what waits goes, and `CAN` ends the
            // sequence that the cut may have left open.
            state.drop_waiting();
            state.lost = true;
            state.queue(&[CAN]);
            bytes = &bytes[bytes.len().saturating_sub(MAX_WAITING_OUTPUT)..];
        }
        for chunk in bytes.chunks(FRAME) {
            state.queue(chunk);
        }
        state.flush();
    }

    /// Says whether the bytes of the person go back to the stream now. The list of
    /// the owner gives this value at each change, so an open panel takes a typing
    /// that the owner turns on or off. The bytes that wait leave when it is off.
    pub fn set_typing(&self, typing: bool) {
        let mut state = self.shared.state();
        if state.typing != typing {
            state.typing = typing;
            state.typed.clear();
        }
    }

    /// The stream ended. The terminal keeps its last screen, the panel says the
    /// reason in one line, and the bytes of the person are dropped until a new
    /// screen arrives.
    pub fn ended(&self, reason: &str) {
        let mut state = self.shared.state();
        state.typed.clear();
        state.ended = Some(plain_text(reason));
    }

    /// What the person typed since the last call: keys, pastes and mouse reports.
    #[must_use]
    pub fn take_typed(&self) -> Vec<u8> {
        let mut state = self.shared.state();
        state.flush();
        std::mem::take(&mut state.typed)
    }

    /// Whether output was dropped since the newest screen and the last call, so
    /// the screen of the follower may differ from the screen of the stream until
    /// the stream sends a new screen.
    #[must_use]
    pub fn take_lost(&self) -> bool {
        std::mem::take(&mut self.shared.state().lost)
    }

    /// Whether the person closed the panel of this follower.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Whether the terminal of this follower shows the alternate screen now.
    #[must_use]
    pub fn on_alternate_screen(&self) -> bool {
        let term = self.shared.term.upgrade();
        term.is_some_and(|term| term.lock().mode().contains(TermMode::ALT_SCREEN))
    }
}

/// The part of a follower that its terminal holds.
pub(super) struct Follower {
    shared: Arc<Shared>,
    spec: FollowSpec,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
    }
}

impl Terminal {
    /// Starts a terminal that shows the stream of `spec`, with the feed that
    /// gives it that stream. The texts and the grid of `spec` are bounded here.
    ///
    /// # Errors
    ///
    /// Returns why the link or the event loop of the terminal cannot start.
    pub fn spawn_follow(mut spec: FollowSpec) -> Result<(Self, FollowFeed)> {
        (spec.cols, spec.rows) = bounded_grid(spec.cols, spec.rows);
        for text in [&mut spec.device, &mut spec.label, &mut spec.panel, &mut spec.title] {
            *text = plain_text(text);
        }
        let link_error = |error| Error::Pty(format!("failed to open the link of a follower: {error}"));
        let (link, loop_end) = UnixStream::pair().map_err(link_error)?;
        link.set_nonblocking(true).map_err(link_error)?;
        let options = TerminalSpawnOptions {
            program: String::new(),
            args: Vec::new(),
            cwd: None,
            rows: spec.rows,
            cols: spec.cols,
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            scrollback_limit: PANEL_SCROLLBACK_LIMIT,
            window_id: 0,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
            // The keys of the person take the encoding that the terminal of the
            // owner gives a panel of this kind.
            kitty_keyboard: crate::panel::spawn::kitty_keyboard_for_kind(spec.kind),
        };
        let pty = horizon_pty_tap::PanelPty::attached(loop_end, window_size_of(&options)).map_err(link_error)?;
        let mut terminal = Self::on_pty(
            options,
            super::TerminalSshTrust::default(),
            PtyOutput::Shown,
            pty,
            None,
            Arc::default(),
            None,
        )?;
        let shared = Arc::new(Shared {
            term: Arc::downgrade(&terminal.term),
            state: Mutex::new(State {
                link,
                waiting: VecDeque::new(),
                waiting_bytes: 0,
                sent: 0,
                typed: Vec::new(),
                typing: spec.typing,
                grid: (spec.cols, spec.rows),
                ended: None,
                lost: false,
            }),
            closed: AtomicBool::new(false),
            wake: Arc::clone(&terminal.output_wake),
        });
        let feed = FollowFeed {
            shared: Arc::clone(&shared),
        };
        terminal.follow = Some(Follower { shared, spec });
        Ok((terminal, feed))
    }

    /// The terminal that this terminal follows, or `None` for a terminal that
    /// runs a program of this computer.
    #[must_use]
    pub fn follow_spec(&self) -> Option<&FollowSpec> {
        self.follow.as_ref().map(|follow| &follow.spec)
    }

    /// The feed of this follower.
    #[must_use]
    pub fn follow_feed(&self) -> Option<FollowFeed> {
        let shared = Arc::clone(&self.follow.as_ref()?.shared);
        Some(FollowFeed { shared })
    }

    /// Why the stream of this follower ended, while it has no newer screen.
    #[must_use]
    pub fn follow_ended(&self) -> Option<String> {
        self.follow.as_ref()?.shared.state().ended.clone()
    }

    /// Keeps the bytes of the person for the feed of a follower, and answers
    /// whether this terminal is one. The bytes of a follower never reach its
    /// link: a follower without typing and a stream that ended drop them.
    pub(super) fn follow_typed(&self, bytes: &[u8]) -> bool {
        let Some(follow) = &self.follow else {
            return false;
        };
        let mut state = follow.shared.state();
        if state.typing && state.ended.is_none() {
            state.typed.extend_from_slice(bytes);
            let above = state.typed.len().saturating_sub(MAX_TYPED);
            state.typed.drain(..above);
        }
        true
    }

    /// Takes the grid of the newest screen of a follower into the size that
    /// this terminal reports, and gives the link the output that waited.
    pub(super) fn sync_follow(&mut self) {
        if let Some(follow) = &self.follow {
            let mut state = follow.shared.state();
            state.flush();
            (self.cols, self.rows) = state.grid;
        }
    }
}

/// The grid of a stream inside the bounds of a follower.
fn bounded_grid(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(2, MAX_FOLLOW_COLS), rows.clamp(1, MAX_FOLLOW_ROWS))
}

/// A text of a stream as one bounded line without control characters.
fn plain_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()
}

#[cfg(all(test, unix))]
mod tests;
