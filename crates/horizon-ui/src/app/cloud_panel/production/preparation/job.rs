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
        self.store
            .sync_runtime_state(&self.session_id)
            .map_err(|error| error.to_string())
    }
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
