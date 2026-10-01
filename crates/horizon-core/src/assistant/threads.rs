//! Threads of the assistant: the agent sessions it has run, remembered so the
//! drawer can switch between them. A thread is a session of the hosted agent;
//! switching resumes that session with the agent's own resume flag.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{directory, write_private};
use crate::{Error, HorizonHome, PanelKind, Result};

/// Threads kept; the oldest are forgotten first.
const MAX_THREADS: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// The agent's own session id, which is what a resume needs.
    pub session_id: String,
    pub agent: PanelKind,
    /// What the session is about, from the agent's own history. Empty until known.
    pub title: String,
    /// The workspace the assistant ran in, which groups threads in the menu.
    pub space: String,
    pub cwd: Option<String>,
    /// Unix milliseconds of the last time the person used this thread.
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Threads {
    threads: Vec<Thread>,
}

impl Threads {
    /// Reads the stored threads, starting empty when absent or unreadable.
    #[must_use]
    pub fn load(home: &HorizonHome) -> Self {
        std::fs::read_to_string(path(home))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Persists the threads.
    ///
    /// # Errors
    /// Returns an I/O error when the file cannot be written.
    pub fn save(&self, home: &HorizonHome) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|error| Error::State(error.to_string()))?;
        write_private(&path(home), text.as_bytes())
    }

    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&Thread> {
        self.threads.iter().find(|thread| thread.session_id == session_id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// Adds a thread, or refreshes the title of a known one. Returns whether anything changed.
    pub fn upsert(&mut self, thread: Thread) -> bool {
        let Some(existing) = self
            .threads
            .iter_mut()
            .find(|known| known.session_id == thread.session_id && known.agent == thread.agent)
        else {
            self.threads.push(thread);
            self.trim();
            return true;
        };
        if thread.title.is_empty() || existing.title == thread.title {
            return false;
        }
        existing.title = thread.title;
        true
    }

    /// Marks a thread as just used, so it sorts first.
    pub fn touch(&mut self, session_id: &str, now: i64) -> bool {
        match self.threads.iter_mut().find(|thread| thread.session_id == session_id) {
            Some(thread) if thread.updated_at != now => {
                thread.updated_at = now;
                true
            }
            _ => false,
        }
    }

    /// Forgets a thread. The agent's own history is left alone.
    pub fn forget(&mut self, session_id: &str) -> bool {
        let before = self.threads.len();
        self.threads.retain(|thread| thread.session_id != session_id);
        self.threads.len() != before
    }

    /// Threads grouped by space: the space with the most recently used thread
    /// first, and the newest thread first within each.
    #[must_use]
    pub fn by_space(&self) -> Vec<(&str, Vec<&Thread>)> {
        let mut sorted: Vec<&Thread> = self.threads.iter().collect();
        sorted.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
        let mut groups: Vec<(&str, Vec<&Thread>)> = Vec::new();
        for thread in sorted {
            match groups.iter_mut().find(|(space, _)| *space == thread.space) {
                Some((_, members)) => members.push(thread),
                None => groups.push((thread.space.as_str(), vec![thread])),
            }
        }
        groups
    }

    fn trim(&mut self) {
        while self.threads.len() > MAX_THREADS {
            let Some(oldest) = self
                .threads
                .iter()
                .enumerate()
                .min_by_key(|(_, thread)| thread.updated_at)
                .map(|(index, _)| index)
            else {
                break;
            };
            self.threads.remove(oldest);
        }
    }
}

fn path(home: &HorizonHome) -> std::path::PathBuf {
    let directory: &Path = &directory(home);
    directory.join("threads.json")
}

#[cfg(test)]
mod tests;
