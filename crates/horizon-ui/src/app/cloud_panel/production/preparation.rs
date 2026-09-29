//! Everything a cloud's deployment needs before it may allocate a worker.
use super::{Confirmation, HorizonApp, Request, Settings, Stage, Store, cloud_runtime, deployment};

impl HorizonApp {
    /// Reads the cloud's record: whether it holds a requested or bound worker, and pinned
    /// siblings. `None` when it cannot be read; the card says why. A record that reads
    /// again after it could not is what the card shows from now on (its worker, stage and
    /// billing), so a later preflight failure is that failure, not the record's.
    fn read_record(&mut self, id: u32, state_root: &std::path::Path, started: bool) -> Option<(bool, bool)> {
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        let record = match Store::lock(state_root).and_then(|store| store.load()) {
            Ok(Some(state)) => Some(state),
            Ok(None) if !started => None,
            other => {
                runtime.state_unavailable = true;
                runtime.fail_preflight(
                    Stage::Validate,
                    other.err().map_or_else(
                        || "Deployment record is missing; reconcile its worker before continuing".into(),
                        |error| error.to_string(),
                    ),
                );
                return None;
            }
        };
        let found = record.as_ref().map_or((false, false), |state| {
            (
                matches!(
                    state.operation,
                    cloud_runtime::CreateState::Bound { .. } | cloud_runtime::CreateState::Requested
                ),
                state.siblings.is_some(),
            )
        });
        if std::mem::take(&mut runtime.state_unavailable)
            && let Some(record) = record
        {
            runtime.stage = Some(record.stage);
            runtime.state = Some(record);
            // The unreadable record's error is no longer true.
            runtime.error = None;
        }
        Some(found)
    }

    /// Saves the cloud and its prepared deployment record before any allocation, and
    /// returns what its deployment needs. `None` when refused; the card shows why.
    pub(super) fn prepare_production_deployment(
        &mut self,
        id: u32,
    ) -> Option<(Request, Vec<cloud_runtime::siblings::Binding>)> {
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(|runtime| runtime.recovery_receiver.is_some())
        {
            return None;
        }
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
            runtime.confirmation = Confirmation::None;
        }
        let group = self.cloud_prototype.groups.0.iter().find(|g| g.issue == id)?;
        let launch = group.remote.clone()?;
        let repository = group.cwd.clone();
        let siblings = group.siblings.clone();
        let root = self.cloud_prototype.root.clone()?;
        if !launch.deployment_started {
            self.save_cloud_prototype();
            if !self.persist_cloud_before_allocation(id) {
                return None;
            }
        }
        let state_root = match cloud_runtime::state::cloud_directory(&root, &launch.id) {
            Ok(path) => path,
            Err(error) => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.state_unavailable = true;
                runtime.fail_preflight(Stage::Validate, error.to_string());
                return None;
            }
        };
        let (existing, pinned) = self.read_record(id, &state_root, launch.deployment_started)?;
        let settings = match Settings::for_cloud(&root.join("settings.json"), &launch.placement) {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_prototype
                    .production
                    .runtimes
                    .entry(id)
                    .or_default()
                    .fail_preflight(
                        Stage::Validate,
                        format!("{error}. Configure {}", root.join("settings.json").display()),
                    );
                return None;
            }
        };
        let request = Request::new(
            launch.id.clone(),
            repository,
            launch.revision,
            launch.profile,
            state_root,
            settings,
        );
        if let Err(error) = deployment::prepare(&request) {
            self.cloud_prototype.error = Some(error.to_string());
            return None;
        }
        if let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter_mut()
            .find(|g| g.issue == id)
            .and_then(|g| g.remote.as_mut())
        {
            launch.deployment_started = true;
        }
        self.save_cloud_prototype();
        if !existing && !self.persist_cloud_before_allocation(id) {
            return None;
        }
        // A reconnect keeps the pinned siblings even when a checkout has since moved.
        Some((request, if pinned { Vec::new() } else { siblings }))
    }
}
