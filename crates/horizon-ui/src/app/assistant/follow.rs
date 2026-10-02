//! Keeps the feed in step with the real agent's transcript.

use std::time::{Duration, Instant, SystemTime};

use super::HorizonApp;
use super::events::{self, Event, Tail};

/// How often to look for a transcript that has not shown up yet.
const LOOK_EVERY: Duration = Duration::from_millis(1500);
/// How often to read what the agent has written.
const READ_EVERY: Duration = Duration::from_millis(350);

pub(super) struct Follow {
    tail: Option<Tail>,
    /// The session the tail belongs to, so a restarted agent is followed afresh.
    session: Option<String>,
    looked: Option<Instant>,
    read: Option<Instant>,
    /// When this app began, so a Codex rollout from an earlier run is not mistaken for this one.
    since: SystemTime,
}

impl Default for Follow {
    fn default() -> Self {
        Self {
            tail: None,
            session: None,
            looked: None,
            read: None,
            since: SystemTime::now(),
        }
    }
}

impl Follow {
    /// Whether a real transcript is being followed; then it, not the host, tells what the person asked.
    pub(super) fn active(&self) -> bool {
        self.tail.is_some()
    }
}

impl HorizonApp {
    /// Reads new events from the assistant agent's transcript into the feed.
    pub(super) fn follow_assistant_transcript(&mut self) {
        let now = Instant::now();
        let Some(id) = self.board.assistant_panel() else {
            return;
        };
        let Some(panel) = self.board.panel(id) else {
            return;
        };
        let kind = panel.kind;
        let session = panel.session_id().map(str::to_string);
        let cwd = panel.launch_cwd.clone();

        let follow = &mut self.assistant.follow;
        if follow.session != session {
            follow.tail = None;
            follow.session.clone_from(&session);
        }
        if follow.tail.is_none() {
            if follow.looked.is_some_and(|at| now.duration_since(at) < LOOK_EVERY) {
                return;
            }
            follow.looked = Some(now);
            let user_home = horizon_core::user_home_dir();
            let codex_home = horizon_core::codex_home_dir();
            follow.tail = events::locate(
                kind,
                user_home.as_deref(),
                codex_home.as_deref(),
                session.as_deref(),
                cwd.as_deref(),
                follow.since,
            )
            .map(|path| Tail::open(path, kind));
        }
        if follow.read.is_some_and(|at| now.duration_since(at) < READ_EVERY) {
            return;
        }
        follow.read = Some(now);
        let Some(tail) = follow.tail.as_mut() else {
            return;
        };
        for event in tail.poll() {
            match event {
                Event::You(text) => self.assistant.feed.you(&text, false),
                Event::Said(text) => self.assistant.feed.said(&text),
                Event::Did(text) => self.assistant.feed.did(text),
                Event::Plan(text) => self.assistant.feed.plan(&text),
            }
        }
    }
}
