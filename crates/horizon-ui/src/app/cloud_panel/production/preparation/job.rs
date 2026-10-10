//! The file work of a deployment's preparation, done on a worker thread: the session is
//! synced, the record read, prepared and saved, and each step reported back in order.
use super::super::{Request, Settings, Store, cloud_runtime, deployment, lifecycle};
use cloud_runtime::state::Deployment;
use horizon_core::SessionStore;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

const MISSING_RECORD: &str = "Deployment record is missing; reconcile its worker before continuing";

/// A saved session that a worker thread makes durable before anything is allocated for it.
pub(in crate::app::cloud_panel::production) struct Durability {
    store: SessionStore,
    session_id: String,
}

impl Durability {
    pub(super) fn new(store: SessionStore, session_id: String) -> Self {
        Self { store, session_id }
    }

    fn sync(&self) -> Result<(), String> {
        self.sync_until_unchanged(|| {})
    }

    /// Syncs until no save replaced a session file meanwhile. The UI may save the session
    /// again while this runs, and a file it replaced after the sync opened the old one is
    /// not durable; the snapshot reported durable must be the one on disk. `synced` runs
    /// after each pass.
    fn sync_until_unchanged(&self, mut synced: impl FnMut()) -> Result<(), String> {
        loop {
            let before = self.identities();
            self.store
                .sync_runtime_state(&self.session_id)
                .map_err(|error| error.to_string())?;
            synced();
            if self.identities() == before {
                return Ok(());
            }
        }
    }

    /// Which file each session path names. A save replaces a file with a new one, which
    /// has another inode and change time.
    fn identities(&self) -> [Option<(u64, u64, i64, i64)>; 3] {
        let home = self.store.home();
        [
            home.session_runtime_path(&self.session_id),
            home.session_meta_path(&self.session_id),
            home.session_index_path(),
        ]
        .map(|path| identity(&path))
    }
}

#[cfg(unix)]
fn identity(path: &std::path::Path) -> Option<(u64, u64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec()))
}

/// Syncing a session refuses hosts other than Unix, so there is nothing to compare.
#[cfg(not(unix))]
fn identity(_: &std::path::Path) -> Option<(u64, u64, i64, i64)> {
    None
}

/// What a preparation needs from the cloud as the UI holds it.
pub(super) struct Input {
    /// The cloud as launched. Once its deployment started, its record must exist.
    pub launch: horizon_core::cloud_panel::CloudLaunch,
    pub repository: PathBuf,
    pub state_root: PathBuf,
    pub settings_path: PathBuf,
    /// The session to make durable first, for a cloud that never started.
    pub first: Option<Durability>,
    /// A new panel's session save that holds the record's lock.
    pub fence: super::super::session_record::Fence,
}

pub(super) enum Report {
    /// The record as read, before the settings or the preparation could refuse.
    Record(Option<Box<Deployment>>),
    /// The record is prepared and saved. `existing` says that it already holds a requested
    /// or bound worker, and `pinned` that it already pinned its siblings.
    Prepared {
        request: Box<Request>,
        existing: bool,
        pinned: bool,
    },
    /// The session that names the started deployment is durable.
    Durable(Box<Request>),
    Failed(Failure),
}

/// Why a preparation stopped; each is shown where the UI showed it before.
pub(super) enum Failure {
    /// The session could not be saved or made durable.
    Unsaved(String),
    /// The record could not be read, or is missing for a started cloud.
    Unreadable(String),
    /// The machine settings refused this cloud.
    Unconfigured(String),
    /// Core refused to prepare the deployment.
    Refused(String),
}

/// A preparation running for a cloud; its deployment starts once it is reported.
pub(in crate::app::cloud_panel::production) struct Pending {
    reports: Receiver<Report>,
    pub(super) siblings: Vec<cloud_runtime::siblings::Binding>,
    /// The operation the started deployment continues, such as a resume.
    pub(super) then: Option<lifecycle::Action>,
    pub(super) pinned: bool,
}

impl Pending {
    pub(super) fn new(
        reports: Receiver<Report>,
        siblings: Vec<cloud_runtime::siblings::Binding>,
        then: Option<lifecycle::Action>,
    ) -> Self {
        Self {
            reports,
            siblings,
            then,
            pinned: false,
        }
    }

    /// The next report, if one arrived. A thread that ended without its last report
    /// fails the preparation.
    pub(super) fn next(&self) -> Option<Report> {
        match self.reports.try_recv() {
            Ok(report) => Some(report),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Report::Failed(Failure::Refused(
                "Deployment preparation ended without a result".into(),
            ))),
        }
    }

    /// Makes the session durable before the deployment starts.
    pub(super) fn make_durable(&mut self, request: Box<Request>, durability: Durability, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.reports = rx;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let report = match durability.sync() {
                Ok(()) => Report::Durable(request),
                Err(error) => Report::Failed(Failure::Unsaved(error)),
            };
            let _ = tx.send(report);
            ctx.request_repaint();
        });
    }
}

/// Starts preparing on a worker thread and returns where it reports.
pub(super) fn prepare(input: Input, ctx: &egui::Context) -> Receiver<Report> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let send = |report| {
            let _ = tx.send(report);
            ctx.request_repaint();
        };
        if let Err(failure) = input.run(&send) {
            send(Report::Failed(failure));
        }
    });
    rx
}

impl Input {
    fn run(self, send: &dyn Fn(Report)) -> Result<(), Failure> {
        // Nothing is allocated before the record holds every running panel's session.
        self.fence.wait();
        if let Some(first) = &self.first {
            first.sync().map_err(Failure::Unsaved)?;
        }
        let record = match Store::lock(&self.state_root).and_then(|store| store.load()) {
            Ok(Some(state)) => Some(state),
            Ok(None) if !self.launch.deployment_started => None,
            other => {
                return Err(Failure::Unreadable(
                    other
                        .err()
                        .map_or_else(|| MISSING_RECORD.into(), |error| error.to_string()),
                ));
            }
        };
        let (existing, pinned) = record.as_ref().map_or((false, false), |state| {
            (
                matches!(
                    state.operation,
                    cloud_runtime::CreateState::Bound { .. } | cloud_runtime::CreateState::Requested
                ),
                state.siblings.is_some(),
            )
        });
        send(Report::Record(record.map(Box::new)));
        let settings = Settings::for_cloud(&self.settings_path, &self.launch.placement)
            .map_err(|error| Failure::Unconfigured(format!("{error}. Configure {}", self.settings_path.display())))?;
        let request = Request::new(
            self.launch.id,
            self.repository,
            self.launch.revision,
            self.launch.profile,
            self.state_root,
            settings,
        );
        deployment::prepare(&request).map_err(|error| Failure::Refused(error.to_string()))?;
        send(Report::Prepared {
            request: Box::new(request),
            existing,
            pinned,
        });
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_core::{HorizonHome, RuntimeState};

    #[test]
    fn a_save_that_replaces_the_session_during_the_sync_is_synced_again() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(
            HorizonHome::from_root(temp.path().join("home")),
            temp.path().join("config.yaml"),
        );
        store.save_runtime_state("session", &RuntimeState::default()).unwrap();
        let durability = Durability::new(store.clone(), "session".into());
        let mut passes = 0;
        durability
            .sync_until_unchanged(|| {
                passes += 1;
                if passes == 1 {
                    // The UI saves the session while the first sync runs.
                    store.save_runtime_state("session", &RuntimeState::default()).unwrap();
                }
            })
            .unwrap();
        assert_eq!(passes, 2, "the replaced files are synced once more");
    }
}
