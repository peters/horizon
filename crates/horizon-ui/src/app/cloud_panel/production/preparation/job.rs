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
/// How long the preparation waits for the session save that names the cloud.
const DURABLE_WITHIN: std::time::Duration = std::time::Duration::from_secs(60);

/// A session save that a worker thread waits for before anything is allocated for it.
pub(in crate::app::cloud_panel::production) struct Durability {
    store: SessionStore,
    session_id: String,
    /// The save that holds the cloud; the session writer makes it durable.
    generation: u64,
}

impl Durability {
    pub(super) fn new(store: SessionStore, session_id: String, generation: u64) -> Self {
        Self {
            store,
            session_id,
            generation,
        }
    }

    /// Waits until the session writer made that save, or a newer one, durable. Every save
    /// the writer makes is durable, so a later save cannot replace it with one that is not.
    fn sync(&self) -> Result<(), String> {
        self.store
            .wait_runtime_state_durable(&self.session_id, self.generation, DURABLE_WITHIN)
            .map_err(|failure| {
                failure.map_or_else(
                    || "Horizon could not save this workspace in time; try again".into(),
                    |error| {
                        format!(
                            "Could not save this workspace before allocating a worker: {error}; retry after fixing session storage"
                        )
                    },
                )
            })
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
    /// The deployment signs in to GitHub again; see [`Request::renew_github`].
    pub renew_github: bool,
    /// See [`Request::working_copy`].
    pub working_copy: Option<cloud_runtime::repository::uncommitted::Confirmed>,
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

    /// A preparation that has reported nothing yet, for tests that do not poll it.
    #[cfg(test)]
    pub(in crate::app::cloud_panel::production) fn running_for_test() -> Self {
        Self::new(channel().1, Vec::new(), None)
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
        // Nothing is allocated before the record holds every running panel's session, unless
        // a save failed so long that it released the cloud; that save adds it later.
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
        // An unstarted legacy launch retains an explicit recipe until a durable record exists.
        let publishing_selection = self.launch.publishing_selection.or_else(|| {
            (record.is_none() && !self.launch.profile.automatic_image()).then(|| {
                cloud_runtime::publishing::Selection::Recipe {
                    image: self.launch.profile.image.clone(),
                }
            })
        });
        let new_record = record.is_none();
        send(Report::Record(record.map(Box::new)));
        let settings = Settings::for_cloud(&self.settings_path, &self.launch.placement)
            .map_err(|error| Failure::Unconfigured(format!("{error}. Configure {}", self.settings_path.display())))?;
        if new_record
            && settings.portable_image_publishing
            && let Some(shown) = &publishing_selection
        {
            shown
                .confirm_current(
                    &settings,
                    &self.repository,
                    &self.launch.profile_name,
                    &self.launch.profile,
                    &horizon_core::cloud_runtime::Cancellation::default(),
                )
                .map_err(|error| Failure::Refused(error.to_string()))?;
        }
        let mut request = Request::new(
            self.launch.id,
            self.repository,
            self.launch.revision,
            self.launch.profile,
            self.state_root,
            settings,
        );
        request.renew_github = self.renew_github;
        request.profile_name = Some(self.launch.profile_name);
        request.publishing_selection = publishing_selection;
        request.working_copy = self.working_copy;
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

    fn board(active: &str) -> RuntimeState {
        RuntimeState {
            active_workspace_local_id: Some(active.into()),
            ..RuntimeState::default()
        }
    }

    #[test]
    fn a_ui_save_that_races_the_allocation_is_durable_too() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(
            HorizonHome::from_root(temp.path().join("home")),
            temp.path().join("config.yaml"),
        );
        store.save_runtime_state("session", &RuntimeState::default()).unwrap();
        assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
        // A named pipe holds the session index, so the writer waits in the save of the
        // cloud board until the test answers it.
        let index = store.home().session_index_path();
        let saved_index = std::fs::read_to_string(&index).unwrap();
        std::fs::remove_file(&index).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&index)
                .status()
                .unwrap()
                .success()
        );
        let generation = store.queue_runtime_state("session", board("cloud"));
        let durability = Durability::new(store.clone(), "session".into(), generation);
        let allocation = std::thread::spawn(move || durability.sync());
        // The UI saves while the allocation waits for the board that names the cloud.
        for step in 0..20 {
            let _ = store.queue_runtime_state("session", board(&format!("later-{step}")));
        }
        assert!(!allocation.is_finished(), "the cloud board is not durable yet");
        std::fs::write(&index, saved_index).unwrap();
        allocation.join().unwrap().unwrap();
        assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
        // Every save went through the writer, so what is on disk is whole and the newest.
        let saved = std::fs::read_to_string(store.home().session_runtime_path("session")).unwrap();
        let saved: RuntimeState = serde_yaml::from_str(&saved).unwrap();
        assert_eq!(saved.active_workspace_local_id.as_deref(), Some("later-19"));
    }

    #[test]
    fn a_session_that_cannot_be_saved_is_refused_in_time() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(
            HorizonHome::from_root(temp.path().join("home")),
            temp.path().join("config.yaml"),
        );
        store.save_runtime_state("session", &RuntimeState::default()).unwrap();
        // A folder where the board goes refuses the rename, even to root.
        let runtime = store.home().session_runtime_path("session");
        std::fs::remove_file(&runtime).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        let generation = store.queue_runtime_state("session", board("cloud"));
        let durable = store.wait_runtime_state_durable("session", generation, std::time::Duration::from_secs(30));
        assert!(
            matches!(durable, Err(Some(_))),
            "a save that cannot be written ends the wait with its reason: {durable:?}"
        );
        std::fs::remove_dir(&runtime).unwrap();
        // The failure stays known until a retry succeeds.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while store.wait_runtime_state_durable("session", generation, std::time::Duration::ZERO) != Ok(()) {
            assert!(std::time::Instant::now() < deadline, "it is written once it can be");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn publishing_settings(root: &std::path::Path) -> Settings {
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":root.join("missing-key"),"ssh_identity_file":root.join("missing-identity"),
            "docker_config":root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        settings.portable_image_publishing = true;
        settings.registries = Some(
            serde_json::from_value(serde_json::json!({
                "root":root.join("registry"),"bindings":[
                    {"repository":"registry.example/team/first","generation":"first","read_only_confirmed":true,
                     "pull":{"username":"reader","secret_file":root.join("first-pull"),"expires_at":null},
                     "publish":{"username":"writer","secret_file":root.join("first-push"),"expires_at":null}},
                    {"repository":"registry.example/team/second","generation":"second","read_only_confirmed":true,
                     "pull":{"username":"reader","secret_file":root.join("second-pull"),"expires_at":null},
                     "publish":{"username":"writer","secret_file":root.join("second-push"),"expires_at":null}}
                ]
            }))
            .unwrap(),
        );
        settings
    }

    #[test]
    fn a_saved_reviewed_choice_cannot_be_replaced_by_a_new_local_preference_during_ui_preparation() {
        use super::super::super::session_record;
        use horizon_core::{
            cloud_panel::Placement,
            cloud_runtime::{Cancellation, publishing, setup},
        };
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("source");
        let root = temporary.path().join("cloud");
        std::fs::create_dir(&repository).unwrap();
        std::fs::create_dir(&root).unwrap();
        let repository = repository.canonicalize().unwrap();
        let settings = publishing_settings(&root);
        let choices = publishing::choices(&settings).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
        setup::save_image_destination(
            &root,
            &repository,
            "cpu",
            None,
            Some(&choices[1]),
            &Cancellation::default(),
        )
        .unwrap();
        for (publishing_selection, image) in [
            (Some(choices[0].clone()), ""),
            (None, "registry.example/team/first"),
            (None, ""),
            (Some(choices[0].clone()), ""),
        ] {
            let automatic = publishing_selection.is_none() && image.is_empty();
            if automatic {
                std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
            }
            let before = std::fs::read(&settings_path).unwrap();
            let profile = serde_json::from_value(serde_json::json!({
                "provider":"runpod", "image":image, "cpu":2, "memory_gb":4,
                "build":{"context":".","dockerfile":"Dockerfile"}
            }))
            .unwrap();
            let launch = horizon_core::cloud_panel::CloudLaunch {
                publishing_selection,
                deployment_started: false,
                id: "fixture".into(),
                revision: "a".repeat(40),
                profile_name: "cpu".into(),
                placement: Placement::default(),
                profile,
            };
            let launch = serde_json::from_value(serde_json::to_value(launch).unwrap()).unwrap();
            let state_root = root.join(if automatic { "automatic" } else { "fixture" });
            let input = Input {
                launch,
                repository: repository.clone(),
                state_root: state_root.clone(),
                settings_path: settings_path.clone(),
                first: None,
                fence: session_record::fence(&state_root),
                renew_github: false,
                working_copy: None,
            };
            let prepared = std::cell::RefCell::new(None);
            let result = input.run(&|report| {
                if let Report::Prepared { request, .. } = report {
                    prepared.replace(Some(request));
                }
            });
            if automatic {
                assert!(
                    result.is_ok(),
                    "an automatic profile keeps automatic intent on first Start"
                );
                let request = prepared.into_inner().unwrap();
                assert!(request.profile.automatic_image());
                assert!(request.publishing_selection.is_none(), "no empty recipe confirmation");
                assert!(Store::snapshot(&state_root).unwrap().unwrap().publication.is_none());
            } else {
                assert!(
                    matches!(result,Err(Failure::Refused(message)) if message.contains("changed since it was shown"))
                );
                assert!(!state_root.join("deployment.json").exists());
            }
            assert_eq!(std::fs::read(&settings_path).unwrap(), before);
            assert!(!root.join("registry").exists());
            assert!(!settings.docker_config.exists());
        }
    }
}
