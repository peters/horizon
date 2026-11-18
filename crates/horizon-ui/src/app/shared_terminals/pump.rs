//! The share pump: the one thread that moves the bytes of shared terminals.
//!
//! A window that the desktop does not show can get no frame: on Wayland a
//! redraw waits for a frame callback, which the compositor does not send for a
//! surface that it does not present. So no byte of a shared terminal waits for a
//! frame. The pump starts and stops with the live link. It takes the events of
//! the link, sends the screens and the output of the watched panels, writes
//! what another computer typed, feeds the follow panels and sends what their
//! person typed. One thread takes the screen and the output of an outlet, as
//! the rule of the outlet asks, and that thread is the pump.
//!
//! The window and the pump share two values. The window writes [`Wanted`] and
//! the pump copies it. The pump swaps in a new [`Found`] and the window takes
//! the pointer. A lock is held for that copy only, and the pump takes it with
//! `try_lock`, so a window that stands still never holds the pump. The pump
//! holds no lock while it calls the link, a feed or a terminal.
//!
//! The pump sleeps while no panel is watched and no follow panel is open. An
//! event of the link and a notice of the window wake it. While it has a stream
//! it also runs every [`PERIOD`], because the reader of a terminal fills its
//! outlet and a person fills a feed with no notice.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use horizon_core::{FollowFeed, ShareSource};
use horizon_sync::{LiveComputer, LiveEvent, LivePanel, LiveState};

use super::follower::Follow;
use super::owner::Watched;
use super::{LiveLink, shown};

/// How often the pump runs while a panel is watched or a follow panel is open.
/// Output reaches a follower 25 ms after the program wrote it at the latest,
/// which a person does not see, and forty short passes a second cost no more
/// than the frames that did this work before.
const PERIOD: Duration = Duration::from_millis(25);

/// One terminal panel of this computer: its entry of the list, whose `id` is
/// the local id of the panel, and the source that shares its terminal.
#[derive(Clone)]
pub(super) struct OwnPanel {
    pub(super) entry: LivePanel,
    pub(super) source: ShareSource,
}

impl PartialEq for OwnPanel {
    /// A panel that started a new terminal is another value.
    fn eq(&self, other: &Self) -> bool {
        self.entry == other.entry && self.source.same_terminal(&other.source)
    }
}

/// One follow panel of this window, by its local id, with the computer and the
/// panel that it follows as the list names them.
#[derive(Clone)]
pub(super) struct FollowPanel {
    pub(super) local: String,
    pub(super) device: String,
    pub(super) id: String,
    pub(super) feed: FollowFeed,
}

/// What the window wants of the pump. A frame writes it, and the pump copies it.
#[derive(Clone, Default)]
pub(super) struct Wanted {
    /// Counts the changes, so the pump copies a new value only.
    pub(super) version: u64,
    /// The two switches of this computer.
    pub(super) sharing: bool,
    pub(super) typing: bool,
    pub(super) panels: Vec<OwnPanel>,
    pub(super) follows: Vec<FollowPanel>,
}

/// What the pump found, for the section of the sidebar and for the chips.
#[derive(Clone, Default)]
pub(in crate::app) struct Found {
    /// The other computers that hold a live link, with the panels they share.
    pub(in crate::app) computers: Vec<LiveComputer>,
    /// The chip text of each panel that is watched or that follows another, by
    /// the local id of the panel.
    pub(in crate::app) chips: HashMap<String, String>,
    /// Why the live link is down. It stays until the link opens.
    pub(in crate::app) down: Option<String>,
}

/// What the window and the pump share.
#[derive(Default)]
pub(super) struct Shared {
    pub(super) wanted: Mutex<Wanted>,
    pub(super) found: Mutex<Arc<Found>>,
    /// Whether somebody asked for a pass, with the condition that says so.
    asked: Mutex<bool>,
    wake: Condvar,
    stop: AtomicBool,
    /// The passes of the pump, which a test counts.
    pub(super) passes: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The lock while nobody holds it: the pump never waits for the window.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

impl Shared {
    /// Asks the pump for a pass now.
    pub(super) fn notify(&self) {
        *lock(&self.asked) = true;
        self.wake.notify_one();
    }

    /// Gives the pump what the window wants now.
    pub(super) fn want(&self, wanted: &Wanted) {
        lock(&self.wanted).clone_from(wanted);
        self.notify();
    }

    /// What the pump found last, unless the pump swaps it in at this instant.
    pub(super) fn found(&self) -> Option<Arc<Found>> {
        try_lock(&self.found).map(|found| Arc::clone(&found))
    }

    /// Sleeps until somebody asks for a pass, and for `period` at most.
    fn sleep(&self, period: Option<Duration>) {
        let until = period.map(|period| Instant::now() + period);
        let mut asked = lock(&self.asked);
        while !*asked && !self.stop.load(Ordering::Acquire) {
            let left = until.map(|until| until.saturating_duration_since(Instant::now()));
            asked = match left {
                Some(left) if left.is_zero() => break,
                Some(left) => {
                    self.wake
                        .wait_timeout(asked, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self.wake.wait(asked).unwrap_or_else(PoisonError::into_inner),
            };
        }
        *asked = false;
    }
}

/// The pump of one live link. Its drop ends the thread, which takes one pass
/// at most: no pass waits for I/O.
pub(super) struct Pump {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Pump {
    /// Starts the pump of `link`. `repaint` asks the window for a frame, and the
    /// pump calls it only when what it found changed. Its thread first ends the
    /// pump `before` it, so two pumps never take the output of one terminal.
    pub(super) fn start(
        shared: Arc<Shared>,
        link: Box<dyn LiveLink>,
        repaint: impl Fn() + Send + 'static,
        before: Option<Self>,
    ) -> std::io::Result<Self> {
        let run = Run {
            link,
            repaint: Box::new(repaint),
            wanted: Wanted::default(),
            sent: Vec::new(),
            watched: HashMap::new(),
            follows: Vec::new(),
            found: Found::default(),
            changed: false,
        };
        let state = Arc::clone(&shared);
        let thread = std::thread::Builder::new().name("share-pump".to_owned());
        let thread = thread.spawn(move || {
            drop(before);
            run.run(&state);
        })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub(super) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.notify();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The state of the pump, which only its thread touches.
pub(super) struct Run {
    pub(super) link: Box<dyn LiveLink>,
    repaint: Box<dyn Fn() + Send>,
    /// The copy of what the window wants.
    pub(super) wanted: Wanted,
    /// The list that the link has.
    pub(super) sent: Vec<LivePanel>,
    /// The panels that another computer watches, by their id in the list.
    pub(super) watched: HashMap<String, Watched>,
    pub(super) follows: Vec<Follow>,
    /// What the window reads, and whether the window does not have it yet.
    pub(super) found: Found,
    pub(super) changed: bool,
}

impl Run {
    fn run(mut self, shared: &Shared) {
        while !shared.stop.load(Ordering::Acquire) {
            let behind = self.pass(shared, Instant::now());
            shared.passes.fetch_add(1, Ordering::Release);
            let streams = !self.watched.is_empty() || !self.follows.is_empty();
            shared.sleep((behind || streams).then_some(PERIOD));
        }
        self.unwatch_all();
    }

    /// One pass of the pump. Answers whether the window held a lock, so the
    /// pump comes back for what it could not copy.
    fn pass(&mut self, shared: &Shared, now: Instant) -> bool {
        let events = self.link.take_events();
        // The copy follows the take, so an event is judged by the switches as
        // they are after it arrived, and by the list that the link had then.
        let wanted = try_lock(&shared.wanted);
        let behind = wanted.is_none();
        let fresh = wanted.is_some_and(|wanted| {
            wanted.version != self.wanted.version && {
                self.wanted.clone_from(&wanted);
                true
            }
        });
        for event in events {
            self.event(event, now);
        }
        if fresh {
            self.apply_wanted();
        }
        self.drain_watched(now);
        self.pump_follows(now);
        if self.changed
            && let Some(mut found) = try_lock(&shared.found)
        {
            *found = Arc::new(self.found.clone());
            drop(found);
            self.changed = false;
            (self.repaint)();
        }
        behind || self.changed
    }

    fn event(&mut self, event: LiveEvent, now: Instant) {
        match event {
            LiveEvent::State(LiveState::Open) => self.link_state(None, false),
            LiveEvent::State(LiveState::Connecting) => {}
            LiveEvent::State(LiveState::Unsupported) => self.link_state(None, true),
            LiveEvent::State(LiveState::Down(reason)) => self.link_state(Some(shown(&reason)), true),
            LiveEvent::Computers { computers } => self.computers(computers),
            LiveEvent::Watchers {
                panel,
                watchers,
                refresh,
            } => self.watchers(&panel, &watchers, refresh, now),
            LiveEvent::Typed {
                panel,
                from,
                label,
                data,
            } => self.typed(&panel, &data.0, &from, &label, now),
            LiveEvent::Resized {
                panel,
                from,
                label,
                cols,
                rows,
            } => self.resized(&panel, cols, rows, &from, &label),
            stream => self.stream(stream),
        }
    }

    /// Takes a state of the link. A link that is `lost` forgets what only an
    /// open link holds: who watches the panels of this computer, the other
    /// computers, and the streams that this window follows.
    fn link_state(&mut self, down: Option<String>, lost: bool) {
        (self.found.down, self.changed) = (down, true);
        if lost {
            self.unwatch_all();
            self.follows_lost();
        }
    }

    /// Takes a new copy of what the window wants: the list goes to the link
    /// when it differs, and each new follow panel starts its follow. The list
    /// is empty while this computer shares nothing.
    fn apply_wanted(&mut self) {
        let shared = self.wanted.panels.iter().filter(|_| self.wanted.sharing);
        let list: Vec<LivePanel> = shared.map(|own| own.entry.clone()).collect();
        if list != self.sent {
            self.sent.clone_from(&list);
            self.link.set_panels(list);
        }
        self.start_follows();
    }
}
