//! Everything a cloud's deployment needs before it may allocate a worker.
use super::{Confirmation, HorizonApp, Request, Settings, Store, cloud_runtime, deployment};

impl HorizonApp {
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
                runtime.fail_preflight(error.to_string());
                return None;
            }
        };
        let loaded = Store::lock(&state_root).and_then(|store| store.load());
        let (existing, pinned) = match loaded {
            Ok(Some(state)) => (
                matches!(
                    state.operation,
                    cloud_runtime::CreateState::Bound { .. } | cloud_runtime::CreateState::Requested
                ),
                state.siblings.is_some(),
            ),
            Ok(None) if !launch.deployment_started => (false, false),
            other => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.state_unavailable = true;
                runtime.fail_preflight(other.err().map_or_else(
                    || "Deployment record is missing; reconcile its worker before continuing".into(),
                    |error| error.to_string(),
                ));
                return None;
            }
        };
        let settings = match Settings::for_cloud(&root.join("settings.json"), &launch.placement) {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_prototype
                    .production
                    .runtimes
                    .entry(id)
                    .or_default()
                    .fail_preflight(format!("{error}. Configure {}", root.join("settings.json").display()));
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
