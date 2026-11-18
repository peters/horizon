//! The live link of one computer: the second link to its server, for the terminal
//! panels that this computer shares and the ones that it follows.
//!
//! [`LiveSupervisor`] opens it with the transport and the greeting of the sync
//! link, on a thread of its own, and opens it again on the delays of a lost link.
//! It sends a live frame only to a server that answered a version with live
//! links: an older server keeps the sync link of this computer and reads no live
//! frame. No call of [`LiveHandle`] does I/O or waits for the link, so the UI
//! thread may call it. The label, the panels and the follows are kept as state
//! and sent again on every new link. Screens, output and typed bytes wait in a
//! queue under a ceiling, and a call that would pass it is refused. Events wait
//! for the application under a ceiling too: over it the reader of the link
//! waits, so the server holds the output back and nothing is lost here.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::connect::Halt;
use super::{End, Greeted, LINK_SLICE, SupervisorConfig};
use crate::error::SyncError;
use crate::frame::write_frame;
use crate::live::protocol::{ID_CHARS, LABEL_CHARS, clean, clean_panels};
use crate::live::{
    Alive, LiveBytes, LiveEvent, LivePanel, LiveRequest, LiveState, MAX_LIVE_OUTPUT_BYTES, MAX_LIVE_PANELS,
    MAX_LIVE_TYPED_BYTES,
};
use crate::protocol::{Frame, Hello, LIVE_PROTOCOL_VERSION, LinkErrorCode, PROTOCOL_VERSION};
use crate::refusal::RefusalCell;

/// The most bytes of screens, output and typed input that wait for the link.
const QUEUE_CEILING_BYTES: usize = 1024 * 1024;
/// The bytes of events over which the reader of the link waits for the
/// application to take them.
const EVENT_CEILING_BYTES: usize = 4 * 1024 * 1024;
/// What one waiting frame or event counts beside its terminal bytes.
const FRAME_BYTES: usize = 64;
/// The most frames that one turn of the writer takes from the queue.
const BATCH_FRAMES: usize = 32;
/// How long this computer waits before it asks an older server again.
const UNSUPPORTED_RETRY: Duration = Duration::from_secs(300);
/// How long this computer waits before it tries again after the server ended its
/// live link for another Horizon of the same computer, and the wait after that:
/// the two programs never make progress while each one replaces the other.
const REPLACED_RETRY: Duration = Duration::from_secs(60);
const REPLACED_RETRY_LONG: Duration = Duration::from_secs(300);

/// Whether another attempt follows a live link that ended, and how long it waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Again {
    /// No attempt follows: the server removed this computer or logged it out.
    Never,
    /// The delays of the sync link.
    Soon,
    /// A long wait: another Horizon of this computer holds the live link.
    Held,
}

impl Again {
    /// The wait before the next attempt. `held` counts the links that another
    /// Horizon of this computer ended, since this link started its attempts.
    fn wait(self, attempt: u32, held: u32) -> Duration {
        match self {
            Self::Held if held <= 1 => REPLACED_RETRY,
            Self::Held => REPLACED_RETRY_LONG,
            Self::Never | Self::Soon => horizon_core::reconnect_delay(attempt),
        }
    }
}

type Follow = (String, String);

#[derive(Default)]
struct Inner {
    state: LiveState,
    label: String,
    device: String,
    /// The panels that this computer shares, and whether the link has them.
    panels: Vec<LivePanel>,
    panels_sent: bool,
    /// The panels that this computer follows, and the ones that the link has.
    follows: BTreeSet<Follow>,
    sent: BTreeSet<Follow>,
    /// Whether a link is open for frames, and why the last one ended, with
    /// whether another attempt follows.
    up: bool,
    end: Option<(String, Again)>,
    queue: VecDeque<LiveRequest>,
    queued: usize,
    events: VecDeque<LiveEvent>,
    event_bytes: usize,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Told when the writer has work, when events were taken and at a stop.
    changed: Condvar,
    stop: AtomicBool,
    /// The sign of life of the open link: a heartbeat frame each 10 seconds and
    /// no link that reads nothing for 30 seconds.
    alive: Alive,
    wake: Box<dyn Fn() + Send + Sync>,
}

/// The live link of this computer, for the application. A clone is cheap.
#[derive(Clone)]
pub struct LiveHandle {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for LiveHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LiveHandle").finish_non_exhaustive()
    }
}

impl LiveHandle {
    fn with<T>(&self, work: impl FnOnce(&mut Inner) -> T) -> T {
        let answer = work(&mut self.shared.lock());
        self.shared.changed.notify_all();
        answer
    }

    /// Replaces the list of the panels that this computer shares.
    pub fn set_panels(&self, panels: Vec<LivePanel>) {
        let panels = clean_panels(panels);
        self.with(|inner| {
            inner.panels_sent &= inner.panels == panels;
            inner.panels = panels;
        });
    }

    /// Starts to follow one panel of the computer `owner`, or asks for a new
    /// screen of a panel that this computer follows. The server answers a follow
    /// that it does not start with [`LiveEvent::Ended`]. At most 64 follows wait.
    pub fn follow(&self, owner: &str, panel: &str) {
        let follow = (owner.to_owned(), clean(panel, ID_CHARS));
        self.with(|inner| {
            inner.sent.remove(&follow);
            inner.follows.len() < MAX_LIVE_PANELS && inner.follows.insert(follow)
        });
    }

    /// Stops to follow one panel of the computer `owner`.
    pub fn unfollow(&self, owner: &str, panel: &str) {
        self.with(|inner| inner.follows.remove(&(owner.to_owned(), clean(panel, ID_CHARS))));
    }

    /// Starts a new screen of a panel of this computer. `fresh` says that the
    /// follower forgets its screen, its history and its modes, and `everyone`
    /// says whether every follower of the panel takes this screen. `false` says
    /// that the link is away or full: nothing was sent, and the caller sends a
    /// new screen later.
    #[must_use]
    pub fn screen(&self, panel: &str, cols: u16, rows: u16, fresh: bool, everyone: bool) -> bool {
        let panel = panel.to_owned();
        let screen = LiveRequest::Screen {
            panel,
            cols,
            rows,
            fresh,
            everyone,
        };
        self.queue(vec![screen], 0)
    }

    /// Sends output bytes of a panel of this computer, split into frames.
    /// `replay` says that they draw the screen that came before them. `false`
    /// says that the link is away or that the bytes would pass the ceiling of the
    /// queue: nothing was sent, and the caller sends a new screen later.
    #[must_use]
    pub fn output(&self, panel: &str, bytes: &[u8], replay: bool) -> bool {
        let frame = |chunk: &[u8]| LiveRequest::Output {
            panel: panel.to_owned(),
            data: LiveBytes(chunk.into()),
            replay,
        };
        self.queue(bytes.chunks(MAX_LIVE_OUTPUT_BYTES).map(frame).collect(), bytes.len())
    }

    /// A panel of this computer is gone: it leaves the list that the link has.
    pub fn ended(&self, panel: &str) {
        self.with(|inner| {
            let shared = inner.panels.len();
            inner.panels.retain(|listed| listed.id != panel);
            inner.panels_sent &= inner.panels.len() == shared;
        });
    }

    /// Sends what the person typed in a panel that this computer follows. Typed
    /// bytes never wait for a later link: they are dropped while the link is
    /// away, while the link does not have the follow, and over the ceiling.
    pub fn typed(&self, owner: &str, panel: &str, bytes: &[u8]) {
        let follow = (owner.to_owned(), clean(panel, ID_CHARS));
        if self.shared.lock().sent.contains(&follow) {
            let frame = |chunk: &[u8]| LiveRequest::Typed {
                owner: follow.0.clone(),
                panel: follow.1.clone(),
                data: LiveBytes(chunk.into()),
            };
            let _ = self.queue(bytes.chunks(MAX_LIVE_TYPED_BYTES).map(frame).collect(), bytes.len());
        }
    }

    /// Asks the owner of a panel that this computer follows for a new screen. The
    /// server passes one request a second for one panel and drops the rest, and
    /// nothing is asked while the link is away or does not have the follow.
    pub fn refresh(&self, owner: &str, panel: &str) {
        let follow = (owner.to_owned(), clean(panel, ID_CHARS));
        if self.shared.lock().sent.contains(&follow) {
            let (owner, panel) = follow;
            let _ = self.queue(vec![LiveRequest::Refresh { owner, panel }], 0);
        }
    }

    /// The events that arrived since the last call, in their order.
    #[must_use]
    pub fn take_events(&self) -> Vec<LiveEvent> {
        self.with(|inner| {
            inner.event_bytes = 0;
            inner.events.drain(..).collect()
        })
    }

    /// The state of the link now.
    #[must_use]
    pub fn state(&self) -> LiveState {
        self.shared.lock().state.clone()
    }

    /// Queues the frames of one call when the link is open and they fit.
    fn queue(&self, frames: Vec<LiveRequest>, bytes: usize) -> bool {
        let cost = bytes + FRAME_BYTES * frames.len();
        self.with(|inner| {
            let fits = inner.up && inner.queued + cost <= QUEUE_CEILING_BYTES;
            if fits {
                inner.queued += cost;
                inner.queue.extend(frames);
            }
            fits
        })
    }
}

impl Inner {
    fn event(&mut self, event: LiveEvent) {
        let bytes = match &event {
            LiveEvent::Output { data, .. } | LiveEvent::Typed { data, .. } => data.0.len(),
            _ => 0,
        };
        self.event_bytes += bytes + FRAME_BYTES;
        self.events.push_back(event);
    }

    fn set_state(&mut self, state: LiveState) {
        if self.state != state {
            self.state = state.clone();
            self.event(LiveEvent::State(state));
        }
    }

    /// Keeps one frame of the server for the application, with the state that
    /// it changes.
    fn accept(&mut self, mut event: LiveEvent) {
        if let LiveEvent::Opened { device, computers } = &mut event {
            let computers = std::mem::take(computers);
            self.device = std::mem::take(device);
            self.set_state(LiveState::Open);
            event = LiveEvent::Computers { computers };
        }
        match &mut event {
            LiveEvent::Computers { computers } => {
                // The newest list is the list: an older one that still waits leaves.
                self.events
                    .retain(|event| !matches!(event, LiveEvent::Computers { .. }));
                for computer in computers {
                    computer.own = computer.device == self.device;
                }
            }
            LiveEvent::Screen { owner, panel, .. } | LiveEvent::Output { owner, panel, .. } => {
                if !self.follows.iter().any(|(of, id)| of == owner && id == panel) {
                    return;
                }
            }
            LiveEvent::Ended { owner, panel, .. } => {
                let follow = (owner.clone(), panel.clone());
                self.follows.remove(&follow);
                self.sent.remove(&follow);
            }
            // A heartbeat is the sign of life of the server, and no event of the
            // application.
            LiveEvent::Heartbeat => return,
            _ => {}
        }
        self.event(event);
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn set_state(&self, state: LiveState) {
        self.lock().set_state(state);
        (self.wake)();
    }

    /// Ends the frames of the open link. The first reason of a link stays.
    fn link_down(&self, end: Option<(String, Again)>) {
        let mut inner = self.lock();
        inner.up = false;
        inner.end = inner.end.take().or(end);
        self.changed.notify_all();
    }

    /// Takes one frame of the server. Answers why the link ends, when it does,
    /// and whether another attempt follows.
    fn receive(&self, frame: Frame) -> Option<(String, Again)> {
        let event = match frame {
            Frame::LiveEvent(event) => event,
            Frame::Error(error) => {
                let (reason, again) = match error.code {
                    LinkErrorCode::Removed | LinkErrorCode::NotLoggedIn => {
                        (format!("{}: {}", error.code.as_str(), error.message), Again::Never)
                    }
                    LinkErrorCode::LiveReplaced => (
                        "another Horizon of this computer holds the live link".to_owned(),
                        Again::Held,
                    ),
                    _ => (format!("{}: {}", error.code.as_str(), error.message), Again::Soon),
                };
                return Some((reason, again));
            }
            _ => {
                return Some((
                    "the server sent a frame that a live link does not carry".to_owned(),
                    Again::Soon,
                ));
            }
        };
        let mut inner = self.lock();
        while inner.event_bytes > EVENT_CEILING_BYTES && inner.up && !self.stopped() {
            inner = self.changed.wait(inner).unwrap_or_else(PoisonError::into_inner);
        }
        inner.accept(event);
        drop(inner);
        (self.wake)();
        None
    }

    /// The frames that the link sends now: first what the server does not have
    /// of the wanted state, then the queue. Waits for work, and answers `None`
    /// when the link ended.
    fn next_batch(&self) -> Option<Vec<LiveRequest>> {
        let mut inner = self.lock();
        loop {
            if !inner.up || self.stopped() {
                return None;
            }
            let mut batch = Vec::new();
            if !std::mem::replace(&mut inner.panels_sent, true) {
                let panels = inner.panels.clone();
                batch.push(LiveRequest::Panels { panels });
            }
            let left: Vec<Follow> = inner.sent.difference(&inner.follows).cloned().collect();
            let new: Vec<Follow> = inner.follows.difference(&inner.sent).cloned().collect();
            for (owner, panel) in left {
                inner.sent.remove(&(owner.clone(), panel.clone()));
                batch.push(LiveRequest::Unfollow { owner, panel });
            }
            for (owner, panel) in new {
                inner.sent.insert((owner.clone(), panel.clone()));
                batch.push(LiveRequest::Follow { owner, panel });
            }
            while batch.len() < BATCH_FRAMES {
                let Some(request) = inner.queue.pop_front() else { break };
                let bytes = match &request {
                    LiveRequest::Output { data, .. } | LiveRequest::Typed { data, .. } => data.0.len(),
                    _ => 0,
                };
                inner.queued = inner.queued.saturating_sub(bytes + FRAME_BYTES);
                batch.push(request);
            }
            if !batch.is_empty() {
                return Some(batch);
            }
            if self.alive.beat_due() {
                return Some(vec![LiveRequest::Heartbeat]);
            }
            inner = self
                .changed
                .wait_timeout(inner, self.alive.tick())
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits for `delay`, or for a stop.
    fn pause(&self, delay: Duration) {
        let deadline = Instant::now() + delay;
        let mut inner = self.lock();
        while !self.stopped() && Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            inner = self
                .changed
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// The supervisor of the live link: its thread, the transport that it owns, and
/// the handle of the application.
#[derive(Debug)]
pub struct LiveSupervisor {
    handle: LiveHandle,
    halt: Arc<Mutex<Option<Halt>>>,
    thread: Option<JoinHandle<()>>,
}

impl LiveSupervisor {
    /// Starts the live link of one computer with the settings of its sync link.
    /// `label` is the readable name of this computer, and `wake` is called when
    /// events wait.
    ///
    /// # Errors
    /// Returns why the thread of the link cannot start.
    pub fn start(
        config: SupervisorConfig,
        label: &str,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, SyncError> {
        let inner = Inner {
            label: clean(label, LABEL_CHARS),
            device: config.device.clone(),
            ..Inner::default()
        };
        let shared = Arc::new(Shared {
            inner: Mutex::new(inner),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            alive: Alive::default(),
            wake: Box::new(wake),
        });
        let halt = Arc::new(Mutex::new(None));
        let thread = {
            let (shared, halt) = (Arc::clone(&shared), Arc::clone(&halt));
            thread::Builder::new()
                .name("horizon-sync-live-link".to_owned())
                .spawn(move || run(&config, &shared, &halt))
                .map_err(|error| SyncError::Queue(format!("the thread of the live link: {error}")))?
        };
        Ok(Self {
            handle: LiveHandle { shared },
            halt,
            thread: Some(thread),
        })
    }

    /// The handle of the application.
    #[must_use]
    pub fn handle(&self) -> LiveHandle {
        self.handle.clone()
    }

    /// Ends the link and its thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.handle.shared.stop.store(true, Ordering::SeqCst);
        self.handle.shared.link_down(None);
        super::halt(&self.halt);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        super::reap(&self.halt);
    }
}

impl Drop for LiveSupervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The life of the live link: a link, then the wait of a lost one, until a stop
/// or until the server does not accept the login of this computer.
fn run(config: &SupervisorConfig, shared: &Shared, halt_slot: &Arc<Mutex<Option<Halt>>>) {
    let mut attempt: u32 = 1;
    let mut held: u32 = 0;
    while !shared.stopped() {
        shared.set_state(LiveState::Connecting);
        let started = Instant::now();
        let hello = |token: Option<&str>| {
            Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: config.device.clone(),
                store: None,
                token: token.map(str::to_owned),
            })
        };
        let greeted = super::greet(config, &shared.stop, halt_slot, &RefusalCell::default(), hello, |_| {});
        let (state, again) = match greeted {
            Ok((_, answer)) if answer.protocol < LIVE_PROTOCOL_VERSION => (LiveState::Unsupported, Again::Soon),
            Ok((greeted, _)) => {
                let (reason, again) = serve(shared, greeted, halt_slot);
                (LiveState::Down(reason), again)
            }
            Err(failed) => {
                let again = if failed.end == End::LoggedOut {
                    Again::Never
                } else {
                    Again::Soon
                };
                (LiveState::Down(failed.reason), again)
            }
        };
        super::halt(halt_slot);
        super::reap(halt_slot);
        if shared.stopped() || again == Again::Never {
            shared.set_state(state);
            return;
        }
        tracing::info!(?state, "the live link of this computer is not open");
        if started.elapsed() >= horizon_core::RECONNECT_RESET_AFTER {
            (attempt, held) = (1, 0);
        }
        if again == Again::Held {
            held = held.saturating_add(1);
        }
        let delay = if matches!(state, LiveState::Unsupported) {
            UNSUPPORTED_RETRY
        } else {
            again.wait(attempt, held)
        };
        attempt = attempt.saturating_add(1);
        shared.set_state(state);
        shared.pause(delay);
    }
}

/// Serves one greeted link until it ends: this thread writes, and a second one
/// takes the frames of the server. Answers why the link ended, and whether
/// another attempt follows.
fn serve(shared: &Shared, greeted: Greeted, halt_slot: &Arc<Mutex<Option<Halt>>>) -> (String, Again) {
    let Greeted { mut input, answers, .. } = greeted;
    // A new link has nothing of this computer: the wanted state goes again, and
    // what waited for the last link is not for this one.
    let open = {
        let mut inner = shared.lock();
        (inner.up, inner.end, inner.panels_sent, inner.queued) = (true, None, false, 0);
        inner.sent.clear();
        inner.queue.clear();
        let label = inner.label.clone();
        Frame::Live(LiveRequest::Open { label })
    };
    thread::scope(|scope| {
        scope.spawn(move || {
            // The reader of the transport may stay in a read that nothing ends, so
            // this thread looks at the link between frames and never waits for it.
            let end = loop {
                match answers.recv_timeout(LINK_SLICE) {
                    Ok(frame) => {
                        shared.alive.read();
                        if let Some(end) = shared.receive(frame) {
                            break Some(end);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) if shared.lock().up => {
                        if shared.alive.quiet() {
                            break Some((shared.alive.quiet_reason(), Again::Soon));
                        }
                    }
                    Err(_) => break None,
                }
            };
            shared.link_down(end);
        });
        let mut sent = write_frame(&mut input, &open).is_ok();
        shared.alive.sent();
        while sent {
            let Some(batch) = shared.next_batch() else { break };
            sent = batch
                .into_iter()
                .all(|request| write_frame(&mut input, &Frame::Live(request)).is_ok());
            shared.alive.sent();
        }
        shared.link_down(None);
        super::halt(halt_slot);
    });
    let end = shared.lock().end.take();
    end.unwrap_or_else(|| ("the live link ended".to_owned(), Again::Soon))
}

#[cfg(test)]
mod tests {
    use super::{Again, REPLACED_RETRY, REPLACED_RETRY_LONG, Shared};
    use crate::live::alive::Limits;
    use crate::live::{Alive, LiveRequest};
    use crate::protocol::{Frame, LinkError, LinkErrorCode};
    use std::sync::{Condvar, Mutex, atomic::AtomicBool};
    use std::time::Duration;

    fn shared() -> Shared {
        Shared {
            inner: Mutex::default(),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            alive: Alive::default(),
            wake: Box::new(|| {}),
        }
    }

    #[test]
    fn a_link_that_another_horizon_replaced_says_so_and_waits_long() {
        let shared = shared();
        let error = LinkError::new(
            LinkErrorCode::LiveReplaced,
            "Another live link of this computer opened.",
        );
        let (reason, again) = shared.receive(Frame::Error(error)).expect("the link ends");
        assert_eq!(again, Again::Held, "{reason}");
        assert!(reason.contains("another Horizon of this computer"), "{reason}");
        assert_eq!(again.wait(1, 1), REPLACED_RETRY);
        assert_eq!(again.wait(2, 2), REPLACED_RETRY_LONG);
        assert_eq!(again.wait(3, 9), REPLACED_RETRY_LONG);
        // A computer that the server removed makes no attempt, and one that fell
        // away keeps the delays of the sync link.
        let removed = LinkError::new(LinkErrorCode::Removed, "gone");
        assert_eq!(
            shared.receive(Frame::Error(removed)).expect("the link ends").1,
            Again::Never
        );
        let failed = LinkError::new(LinkErrorCode::StoreFailed, "busy");
        assert_eq!(
            shared.receive(Frame::Error(failed)).expect("the link ends").1,
            Again::Soon
        );
    }

    #[test]
    fn an_idle_live_link_sends_a_heartbeat_frame() {
        let shared = Shared {
            alive: Alive::new(Limits {
                tick: Duration::from_millis(5),
                heartbeat: Duration::from_millis(20),
                silence: Duration::from_secs(30),
            }),
            ..shared()
        };
        {
            let mut inner = shared.lock();
            (inner.up, inner.panels_sent) = (true, true);
        }
        let batch = shared.next_batch().expect("a batch of an open link");
        assert_eq!(batch, vec![LiveRequest::Heartbeat], "the sign of life of an idle link");
    }
}
