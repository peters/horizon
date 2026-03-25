use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::runtime_state::RuntimeState;

#[derive(Clone, Debug)]
pub(super) struct ProfileSnapshot {
    pub last_session_id: Option<String>,
    pub sessions: Vec<StoredSession>,
}

#[derive(Clone, Debug)]
pub(super) struct StoredSession {
    pub summary: SessionSummary,
    pub is_live: bool,
    pub has_stale_lease: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedSession {
    pub session_id: String,
    pub runtime_state: RuntimeState,
    pub runtime_state_path: PathBuf,
    pub transcript_root: PathBuf,
    pub meta: SessionMeta,
}

#[derive(Clone, Debug)]
pub enum StartupDecision {
    Open {
        disposition: SessionOpenDisposition,
        session: Box<ResolvedSession>,
    },
    Ephemeral {
        runtime_state: Box<RuntimeState>,
    },
    Choose(StartupChooser),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOpenDisposition {
    New,
    Resume,
    Recover,
}

#[derive(Clone, Debug)]
pub struct StartupChooser {
    pub reason: StartupPromptReason,
    pub config_path: String,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPromptReason {
    LiveConflict,
    MultipleRecoverable,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionSummary {
    pub session_id: String,
    pub label: String,
    pub workspace_count: usize,
    pub panel_count: usize,
    pub last_active_at: i64,
    pub config_path: String,
    pub is_live: bool,
}

impl SessionSummary {
    pub(super) fn from_meta(meta: &SessionMeta, is_live: bool) -> Self {
        Self {
            session_id: meta.session_id.clone(),
            label: meta.label.clone().unwrap_or_else(|| "Horizon session".to_string()),
            workspace_count: meta.workspace_count,
            panel_count: meta.panel_count,
            last_active_at: meta.last_active_at,
            config_path: meta.config_path.clone(),
            is_live,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionMeta {
    pub version: u32,
    pub session_id: String,
    pub profile_id: String,
    pub config_path: String,
    pub label: Option<String>,
    pub workspace_count: usize,
    pub panel_count: usize,
    pub started_at: i64,
    pub last_active_at: i64,
}

impl SessionMeta {
    pub(super) fn new(
        session_id: String,
        profile_id: String,
        config_path: String,
        runtime_state: &RuntimeState,
        now: i64,
    ) -> Self {
        Self {
            version: super::SESSION_META_VERSION,
            session_id,
            profile_id,
            config_path,
            label: super::derive_session_label(runtime_state),
            workspace_count: runtime_state.workspaces.len(),
            panel_count: runtime_state.panel_count(),
            started_at: now,
            last_active_at: now,
        }
    }

    pub(super) fn updated(&self, runtime_state: &RuntimeState, now: i64) -> Self {
        Self {
            version: super::SESSION_META_VERSION,
            session_id: self.session_id.clone(),
            profile_id: self.profile_id.clone(),
            config_path: self.config_path.clone(),
            label: super::derive_session_label(runtime_state),
            workspace_count: runtime_state.workspaces.len(),
            panel_count: runtime_state.panel_count(),
            started_at: self.started_at,
            last_active_at: now,
        }
    }
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            version: super::SESSION_META_VERSION,
            session_id: String::new(),
            profile_id: String::new(),
            config_path: String::new(),
            label: None,
            workspace_count: 0,
            panel_count: 0,
            started_at: 0,
            last_active_at: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionLease {
    pub version: u32,
    pub session_id: String,
    pub pid: u32,
    pub hostname: String,
    pub started_at: i64,
    pub last_heartbeat_at: i64,
}

impl SessionLease {
    pub(super) fn new(session_id: String) -> Self {
        let now = super::current_unix_millis();
        Self {
            version: super::SESSION_LEASE_VERSION,
            session_id,
            pid: std::process::id(),
            hostname: super::hostname(),
            started_at: now,
            last_heartbeat_at: now,
        }
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        let now = super::current_unix_millis();
        now.saturating_sub(self.last_heartbeat_at) <= super::LEASE_STALE_AFTER_MILLIS
            && super::process_is_alive(self.pid)
    }
}

impl Default for SessionLease {
    fn default() -> Self {
        Self {
            version: super::SESSION_LEASE_VERSION,
            session_id: String::new(),
            pid: 0,
            hostname: String::new(),
            started_at: 0,
            last_heartbeat_at: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct SessionIndex {
    pub version: u32,
    pub profiles: Vec<SessionProfileIndex>,
}

impl SessionIndex {
    pub(super) fn profile(&self, profile_id: &str) -> Option<&SessionProfileIndex> {
        self.profiles.iter().find(|profile| profile.profile_id == profile_id)
    }

    pub(super) fn touch_profile_session(&mut self, profile_id: &str, session_id: &str) {
        self.version = super::SESSION_INDEX_VERSION;
        let profile = if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.profile_id == profile_id)
        {
            profile
        } else {
            self.profiles.push(SessionProfileIndex::new(profile_id.to_string()));
            let len = self.profiles.len();
            &mut self.profiles[len - 1]
        };

        profile.last_session_id = Some(session_id.to_string());
        profile.recent_session_ids.retain(|candidate| candidate != session_id);
        profile.recent_session_ids.insert(0, session_id.to_string());
        profile.recent_session_ids.truncate(12);
    }

    pub(super) fn remove_profile_session(&mut self, profile_id: &str, session_id: &str) {
        self.version = super::SESSION_INDEX_VERSION;
        if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.profile_id == profile_id)
        {
            profile.remove_session(session_id);
        }
        self.profiles.retain(|profile| !profile.is_empty());
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct SessionProfileIndex {
    pub profile_id: String,
    pub last_session_id: Option<String>,
    pub recent_session_ids: Vec<String>,
}

impl SessionProfileIndex {
    fn new(profile_id: String) -> Self {
        Self {
            profile_id,
            last_session_id: None,
            recent_session_ids: Vec::new(),
        }
    }

    fn remove_session(&mut self, session_id: &str) {
        self.recent_session_ids.retain(|candidate| candidate != session_id);
        if self.last_session_id.as_deref() == Some(session_id) {
            self.last_session_id = self.recent_session_ids.first().cloned();
        }
    }

    fn is_empty(&self) -> bool {
        self.last_session_id.is_none() && self.recent_session_ids.is_empty()
    }
}
