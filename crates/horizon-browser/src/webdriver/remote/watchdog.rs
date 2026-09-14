//! Independent lifetime enforcement for one allocated remote session. The
//! driver loop can sit inside a classic command for most of a minute and has
//! queued commands behind it, so the hard deadline and the idle policy are
//! judged on their own thread, which releases the session itself the moment
//! either elapses. The driver learns the verdict on its next poll and reads
//! the settled release outcome instead of deleting twice.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{RemoteExpiry, RemoteReleaseOutcome, release_session};
use crate::webdriver::remote_http::RemoteHttpClient;

/// Longest the thread sleeps between looks at the clock.
const POLL: Duration = Duration::from_millis(100);

pub(super) struct Watchdog {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    deadline: Instant,
    idle_release: Duration,
    last_activity: Mutex<Instant>,
    expired: Mutex<Option<RemoteExpiry>>,
    released: Mutex<Option<RemoteReleaseOutcome>>,
    stop: AtomicBool,
}

impl Watchdog {
    /// Start judging from `now`, the instant allocation succeeded; nothing
    /// that happened during allocation counts against the session.
    ///
    /// # Errors
    /// The thread could not be spawned. The session then has no enforcement
    /// and the caller must release it at once.
    pub(super) fn start(
        transport: Arc<RemoteHttpClient>,
        session_id: String,
        max_session: Duration,
        idle_release: Duration,
        now: Instant,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            deadline: now + max_session,
            idle_release,
            last_activity: Mutex::new(now),
            expired: Mutex::new(None),
            released: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("remote-session-watchdog".into())
            .spawn(move || run(&worker, &transport, &session_id))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// A user or agent command happened; frame polling never reports here.
    pub(super) fn note_activity(&self, now: Instant) {
        *lock(&self.shared.last_activity) = now;
    }

    /// The thread's verdict, sticky once given.
    pub(super) fn expired(&self) -> Option<RemoteExpiry> {
        *lock(&self.shared.expired)
    }

    /// Stop judging and return the release the thread already settled, if
    /// any. Joins the thread, which is asleep for at most one poll or inside
    /// the bounded release.
    pub(super) fn finish(mut self) -> Option<RemoteReleaseOutcome> {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        lock(&self.shared.released).take()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn run(shared: &Shared, transport: &RemoteHttpClient, session_id: &str) {
    let reason = loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let now = Instant::now();
        if now >= shared.deadline {
            break RemoteExpiry::HardDeadline;
        }
        let last_activity = *lock(&shared.last_activity);
        if now.duration_since(last_activity) >= shared.idle_release {
            break RemoteExpiry::Idle;
        }
        let next = shared.deadline.min(last_activity + shared.idle_release);
        std::thread::sleep(next.saturating_duration_since(now).min(POLL));
    };
    *lock(&shared.expired) = Some(reason);
    let outcome = release_session(transport, session_id);
    *lock(&shared.released) = Some(outcome);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
