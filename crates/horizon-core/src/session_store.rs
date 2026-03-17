use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::runtime_state::RuntimeState;

const SESSION_INDEX_VERSION: u32 = 1;
const SESSION_META_VERSION: u32 = 1;
const SESSION_LEASE_VERSION: u32 = 1;
const LEASE_STALE_AFTER_MILLIS: i64 = 15_000;

#[derive(Clone, Debug)]
pub struct SessionStore {
    home: HorizonHome,
    config_path: PathBuf,
    profile_id: String,
}

impl SessionStore {
    #[must_use]
    pub fn new(home: HorizonHome, config_path: PathBuf) -> Self {
        let profile_id = profile_id_for_config_path(&config_path);
        Self {
            home,
            config_path,
            profile_id,
        }
    }

    #[must_use]
    pub fn home(&self) -> &HorizonHome {
        &self.home
    }

    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn prepare_startup(&self, config: &Config) -> Result<StartupDecision> {
        let profile = self.load_profile_snapshot()?;

        if profile.sessions.is_empty() {
            let session = self.create_new_session(config)?;
            return Ok(StartupDecision::Open {
                disposition: SessionOpenDisposition::New,
                session: Box::new(session),
            });
        }

        let selected = profile
            .last_session_id
            .as_ref()
            .and_then(|session_id| {
                profile
                    .sessions
                    .iter()
                    .find(|session| session.summary.session_id == *session_id)
            })
            .or_else(|| profile.sessions.first());

        if let Some(session) = selected {
            if session.is_live {
                return Ok(StartupDecision::Choose(StartupChooser {
                    reason: StartupPromptReason::LiveConflict,
                    config_path: self.config_path.display().to_string(),
                    sessions: profile.sessions.into_iter().map(|session| session.summary).collect(),
                }));
            }

            let disposition = if session.has_stale_lease {
                SessionOpenDisposition::Recover
            } else {
                SessionOpenDisposition::Resume
            };
            let resolved = self.load_existing_session(&session.summary.session_id)?;
            return Ok(StartupDecision::Open {
                disposition,
                session: Box::new(resolved),
            });
        }

        if profile.sessions.len() > 1 {
            return Ok(StartupDecision::Choose(StartupChooser {
                reason: StartupPromptReason::MultipleRecoverable,
                config_path: self.config_path.display().to_string(),
                sessions: profile.sessions.into_iter().map(|session| session.summary).collect(),
            }));
        }

        let session = self.load_existing_session(&profile.sessions[0].summary.session_id)?;
        Ok(StartupDecision::Open {
            disposition: SessionOpenDisposition::Resume,
            session: Box::new(session),
        })
    }

    pub fn create_new_session(&self, config: &Config) -> Result<ResolvedSession> {
        let runtime_state = RuntimeState::from_config(config);
        self.create_session_from_runtime(runtime_state)
    }

    pub fn duplicate_session(&self, source_session_id: &str) -> Result<ResolvedSession> {
        let source_runtime_path = self.home.session_runtime_path(source_session_id);
        let runtime_state = RuntimeState::load(&source_runtime_path)?
            .ok_or_else(|| Error::State(format!("missing runtime state for session {source_session_id}")))?;
        let session = self.create_session_from_runtime(runtime_state)?;
        copy_directory_recursive(
            &self.home.session_transcripts_dir(source_session_id),
            &self.home.session_transcripts_dir(&session.session_id),
        )?;
        Ok(session)
    }

    pub fn resume_session(&self, session_id: &str) -> Result<ResolvedSession> {
        self.load_existing_session(session_id)
    }

    pub fn take_over_session(&self, session_id: &str) -> Result<ResolvedSession> {
        self.load_existing_session(session_id)
    }

    pub fn save_runtime_state(&self, session_id: &str, runtime_state: &RuntimeState) -> Result<()> {
        let runtime_path = self.home.session_runtime_path(session_id);
        let meta_path = self.home.session_meta_path(session_id);

        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let runtime_yaml = runtime_state.to_yaml()?;
        atomic_write(&runtime_path, runtime_yaml.as_bytes())?;

        let existing_meta = self.load_session_meta(session_id).unwrap_or_else(|_| {
            SessionMeta::new(
                session_id.to_string(),
                self.profile_id.clone(),
                self.config_path.display().to_string(),
                runtime_state,
                current_unix_millis(),
            )
        });
        let meta = existing_meta.updated(runtime_state, current_unix_millis());
        let meta_yaml = serde_yaml::to_string(&meta).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&meta_path, meta_yaml.as_bytes())?;

        let mut index = self.load_session_index()?;
        index.touch_profile_session(&self.profile_id, session_id);
        self.save_session_index(&index)?;
        Ok(())
    }

    pub fn acquire_lease(&self, session_id: &str) -> Result<SessionLease> {
        let lease_path = self.home.session_lease_path(session_id);
        if let Some(parent) = lease_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let lease = SessionLease::new(session_id.to_string());
        let json = serde_json::to_vec_pretty(&lease).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&lease_path, &json)?;
        Ok(lease)
    }

    pub fn refresh_lease(&self, lease: &mut SessionLease) -> Result<()> {
        lease.last_heartbeat_at = current_unix_millis();
        let json = serde_json::to_vec_pretty(lease).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_lease_path(&lease.session_id), &json)?;
        Ok(())
    }

    pub fn release_lease(&self, session_id: &str) -> Result<()> {
        let path = self.home.session_lease_path(session_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn create_session_from_runtime(&self, mut runtime_state: RuntimeState) -> Result<ResolvedSession> {
        runtime_state.ensure_local_ids();
        let session_id = Uuid::new_v4().to_string();
        let now = current_unix_millis();
        let runtime_path = self.home.session_runtime_path(&session_id);
        let transcript_root = self.home.session_transcripts_dir(&session_id);
        let meta = SessionMeta::new(
            session_id.clone(),
            self.profile_id.clone(),
            self.config_path.display().to_string(),
            &runtime_state,
            now,
        );

        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&transcript_root)?;

        let runtime_yaml = runtime_state.to_yaml()?;
        atomic_write(&runtime_path, runtime_yaml.as_bytes())?;
        let meta_yaml = serde_yaml::to_string(&meta).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_meta_path(&session_id), meta_yaml.as_bytes())?;

        let mut index = self.load_session_index()?;
        index.touch_profile_session(&self.profile_id, &session_id);
        self.save_session_index(&index)?;

        Ok(ResolvedSession {
            session_id,
            runtime_state,
            runtime_state_path: runtime_path,
            transcript_root,
            meta,
        })
    }

    fn load_existing_session(&self, session_id: &str) -> Result<ResolvedSession> {
        let runtime_path = self.home.session_runtime_path(session_id);
        let runtime_state = RuntimeState::load(&runtime_path)?
            .ok_or_else(|| Error::State(format!("missing runtime state for session {session_id}")))?;
        let meta = self.load_session_meta(session_id)?;

        Ok(ResolvedSession {
            session_id: session_id.to_string(),
            runtime_state,
            runtime_state_path: runtime_path,
            transcript_root: self.home.session_transcripts_dir(session_id),
            meta,
        })
    }

    fn load_profile_snapshot(&self) -> Result<ProfileSnapshot> {
        let index = self.load_session_index()?;
        let last_session_id = index
            .profile(&self.profile_id)
            .and_then(|profile| profile.last_session_id.clone());
        let sessions_dir = self.home.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(ProfileSnapshot {
                last_session_id,
                sessions: Vec::new(),
            });
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = self.load_session_meta(&session_id) else {
                continue;
            };
            if meta.profile_id != self.profile_id {
                continue;
            }

            let lease = self.load_session_lease(&session_id)?;
            let is_live = lease.as_ref().is_some_and(SessionLease::is_live);
            let has_stale_lease = lease.is_some() && !is_live;

            sessions.push(StoredSession {
                summary: SessionSummary::from_meta(&meta, is_live),
                is_live,
                has_stale_lease,
            });
        }

        sessions.sort_by(|left, right| {
            if Some(left.summary.session_id.as_str()) == last_session_id.as_deref() {
                return std::cmp::Ordering::Less;
            }
            if Some(right.summary.session_id.as_str()) == last_session_id.as_deref() {
                return std::cmp::Ordering::Greater;
            }
            right.summary.last_active_at.cmp(&left.summary.last_active_at)
        });

        Ok(ProfileSnapshot {
            last_session_id,
            sessions,
        })
    }

    fn load_session_index(&self) -> Result<SessionIndex> {
        let path = self.home.session_index_path();
        if !path.exists() {
            return Ok(SessionIndex::default());
        }

        let contents = fs::read_to_string(&path)?;
        let mut index =
            serde_yaml::from_str::<SessionIndex>(&contents).map_err(|error| Error::State(error.to_string()))?;
        index.version = SESSION_INDEX_VERSION;
        Ok(index)
    }

    fn save_session_index(&self, index: &SessionIndex) -> Result<()> {
        let yaml = serde_yaml::to_string(index).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_index_path(), yaml.as_bytes())?;
        Ok(())
    }

    fn load_session_meta(&self, session_id: &str) -> Result<SessionMeta> {
        let contents = fs::read_to_string(self.home.session_meta_path(session_id))?;
        let mut meta =
            serde_yaml::from_str::<SessionMeta>(&contents).map_err(|error| Error::State(error.to_string()))?;
        meta.version = SESSION_META_VERSION;
        Ok(meta)
    }

    fn load_session_lease(&self, session_id: &str) -> Result<Option<SessionLease>> {
        let path = self.home.session_lease_path(session_id);
        if !path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(path)?;
        let mut lease =
            serde_json::from_str::<SessionLease>(&contents).map_err(|error| Error::State(error.to_string()))?;
        lease.version = SESSION_LEASE_VERSION;
        Ok(Some(lease))
    }
}

#[derive(Clone, Debug)]
struct ProfileSnapshot {
    last_session_id: Option<String>,
    sessions: Vec<StoredSession>,
}

#[derive(Clone, Debug)]
struct StoredSession {
    summary: SessionSummary,
    is_live: bool,
    has_stale_lease: bool,
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
    fn from_meta(meta: &SessionMeta, is_live: bool) -> Self {
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
    fn new(
        session_id: String,
        profile_id: String,
        config_path: String,
        runtime_state: &RuntimeState,
        now: i64,
    ) -> Self {
        Self {
            version: SESSION_META_VERSION,
            session_id,
            profile_id,
            config_path,
            label: derive_session_label(runtime_state),
            workspace_count: runtime_state.workspaces.len(),
            panel_count: runtime_state.panel_count(),
            started_at: now,
            last_active_at: now,
        }
    }

    fn updated(&self, runtime_state: &RuntimeState, now: i64) -> Self {
        Self {
            version: SESSION_META_VERSION,
            session_id: self.session_id.clone(),
            profile_id: self.profile_id.clone(),
            config_path: self.config_path.clone(),
            label: derive_session_label(runtime_state),
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
            version: SESSION_META_VERSION,
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
    fn new(session_id: String) -> Self {
        let now = current_unix_millis();
        Self {
            version: SESSION_LEASE_VERSION,
            session_id,
            pid: std::process::id(),
            hostname: hostname(),
            started_at: now,
            last_heartbeat_at: now,
        }
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        let now = current_unix_millis();
        now.saturating_sub(self.last_heartbeat_at) <= LEASE_STALE_AFTER_MILLIS && process_is_alive(self.pid)
    }
}

impl Default for SessionLease {
    fn default() -> Self {
        Self {
            version: SESSION_LEASE_VERSION,
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
struct SessionIndex {
    version: u32,
    profiles: Vec<SessionProfileIndex>,
}

impl SessionIndex {
    fn profile(&self, profile_id: &str) -> Option<&SessionProfileIndex> {
        self.profiles.iter().find(|profile| profile.profile_id == profile_id)
    }

    fn touch_profile_session(&mut self, profile_id: &str, session_id: &str) {
        self.version = SESSION_INDEX_VERSION;
        let profile = if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.profile_id == profile_id)
        {
            profile
        } else {
            self.profiles.push(SessionProfileIndex::new(profile_id.to_string()));
            self.profiles.last_mut().expect("session index profile was just pushed")
        };

        profile.last_session_id = Some(session_id.to_string());
        profile.recent_session_ids.retain(|candidate| candidate != session_id);
        profile.recent_session_ids.insert(0, session_id.to_string());
        profile.recent_session_ids.truncate(12);
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct SessionProfileIndex {
    profile_id: String,
    last_session_id: Option<String>,
    recent_session_ids: Vec<String>,
}

impl SessionProfileIndex {
    fn new(profile_id: String) -> Self {
        Self {
            profile_id,
            last_session_id: None,
            recent_session_ids: Vec::new(),
        }
    }
}

fn derive_session_label(runtime_state: &RuntimeState) -> Option<String> {
    runtime_state
        .workspaces
        .iter()
        .find(|workspace| !workspace.name.is_empty())
        .map(|workspace| workspace.name.clone())
}

fn profile_id_for_config_path(config_path: &Path) -> String {
    let stable_path = fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    stable_state_key(&stable_path.to_string_lossy())
}

fn stable_state_key(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn current_unix_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/proc").join(pid.to_string()).exists()
    }

    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, bytes)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }

    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision};

    #[test]
    fn empty_store_creates_new_session() {
        let root = test_root("empty-store");
        let home = HorizonHome::from_root(root.clone());
        let store = SessionStore::new(home.clone(), home.config_path());

        let decision = store.prepare_startup(&Config::default()).expect("startup decision");

        match decision {
            StartupDecision::Open {
                disposition: SessionOpenDisposition::New,
                session,
            } => {
                assert!(session.runtime_state_path.exists());
                assert!(session.transcript_root.starts_with(root.join("sessions")));
            }
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn second_startup_resumes_previous_session() {
        let root = test_root("resume-store");
        let home = HorizonHome::from_root(root.clone());
        let store = SessionStore::new(home.clone(), home.config_path());
        let created = store.create_new_session(&Config::default()).expect("create session");
        store
            .save_runtime_state(&created.session_id, &RuntimeState::from_config(&Config::default()))
            .expect("save state");

        let decision = store.prepare_startup(&Config::default()).expect("startup decision");

        match decision {
            StartupDecision::Open {
                disposition: SessionOpenDisposition::Resume,
                session,
            } => assert_eq!(session.session_id, created.session_id),
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    fn test_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("horizon-session-store-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
