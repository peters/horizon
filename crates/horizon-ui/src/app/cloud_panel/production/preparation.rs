//! Everything a cloud's deployment needs before it may allocate a worker. The UI saves the
//! session; a worker thread makes it durable and reads, prepares and saves the record, so a
//! slow disk never stalls a frame. The deployment starts once each of them is reported.
mod job;
#[cfg(all(test, unix))]
mod tests;

use super::{Confirmation, HorizonApp, Runtime, Stage, cloud_runtime, lifecycle};
pub(super) use job::Pending;
use job::{Durability, Failure, Report};

impl Runtime {
    /// Ends a preparation that could not start its deployment. A refused reconnect after
    /// a resume is still that resume's failure, so the card offers Resume worker again.
    fn abandon_preparation(&mut self) {
        if let Some(then) = self.preparation.take().and_then(|pending| pending.then) {
            self.operation = Some(then);
        }
    }

    /// A record that reads again after it could not is what the card shows from now on
    /// (its worker, stage and billing), so a later preflight failure is that failure, not
    /// the record's.
    fn adopt_read_record(&mut self, record: Option<Box<cloud_runtime::state::Deployment>>) {
        if std::mem::take(&mut self.state_unavailable)
            && let Some(record) = record
        {
            self.stage = Some(record.stage);
            self.state = Some(*record);
            // The unreadable record's error is no longer true.
            self.error = None;
        }
    }
}

impl HorizonApp {
    /// Saves the cloud and starts preparing its deployment record before any allocation.
    /// Nothing starts while a provider check or another preparation holds the cloud; a
    /// refusal shows on the card. `then` is the operation the deployment continues.
    pub(super) fn prepare_production_deployment(
        &mut self,
        id: u32,
        then: Option<lifecycle::Action>,
        ctx: &egui::Context,
    ) {
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .is_some_and(|runtime| runtime.recovery_receiver.is_some() || runtime.preparation.is_some())
        {
            return;
        }
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
            runtime.confirmation = Confirmation::None;
        }
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|g| g.issue == id) else {
            return;
        };
        let Some(launch) = group.remote.clone() else { return };
        let repository = group.cwd.clone();
        let siblings = group.siblings.clone();
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let first = if launch.deployment_started {
            None
        } else {
            self.save_cloud_prototype();
            let Some(durability) = self.save_cloud_before_allocation(id) else {
                return;
            };
            Some(durability)
        };
        let state_root = match cloud_runtime::state::cloud_directory(&root, &launch.id) {
            Ok(path) => path,
            Err(error) => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.state_unavailable = true;
                runtime.fail_preflight(Stage::Validate, error.to_string());
                return;
            }
        };
        let fence = super::session_record::fence(&state_root);
        let reports = job::prepare(
            job::Input {
                launch,
                repository,
                state_root,
                settings_path: root.join("settings.json"),
                first,
                fence,
            },
            ctx,
        );
        self.cloud_prototype
            .production
            .runtimes
            .entry(id)
            .or_default()
            .preparation = Some(Pending::new(reports, siblings, then));
    }

    /// Applies what each preparation reported; a prepared deployment starts here.
    pub(super) fn poll_cloud_preparations(&mut self, ctx: &egui::Context) {
        let preparing: Vec<u32> = self
            .cloud_prototype
            .production
            .runtimes
            .iter()
            .filter(|(_, runtime)| runtime.preparation.is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in preparing {
            while let Some(report) = self
                .cloud_prototype
                .production
                .runtimes
                .get(&id)
                .and_then(|runtime| runtime.preparation.as_ref()?.next())
            {
                self.apply_preparation(id, report, ctx);
            }
        }
    }

    fn apply_preparation(&mut self, id: u32, report: Report, ctx: &egui::Context) {
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        match report {
            Report::Record(record) => runtime.adopt_read_record(record),
            Report::Prepared {
                request,
                existing,
                pinned,
            } => {
                if let Some(pending) = &mut runtime.preparation {
                    pending.pinned = pinned;
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
                if existing {
                    self.start_prepared_deployment(id, *request, ctx);
                    return;
                }
                let durability = self.save_cloud_before_allocation(id);
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                match (durability, &mut runtime.preparation) {
                    (Some(durability), Some(pending)) => pending.make_durable(request, durability, ctx),
                    _ => runtime.abandon_preparation(),
                }
            }
            Report::Durable(request) => self.start_prepared_deployment(id, *request, ctx),
            Report::Failed(failure) => {
                match failure {
                    Failure::Unsaved(error) => self.show_unsaved_cloud(id, error),
                    Failure::Unreadable(error) => {
                        runtime.state_unavailable = true;
                        runtime.fail_preflight(Stage::Validate, error);
                    }
                    Failure::Unconfigured(error) => runtime.fail_preflight(Stage::Validate, error),
                    Failure::Refused(error) => self.cloud_prototype.error = Some(error),
                }
                self.cloud_prototype
                    .production
                    .runtimes
                    .entry(id)
                    .or_default()
                    .abandon_preparation();
            }
        }
    }

    /// Starts the deployment a finished preparation made ready. A reconnect keeps the
    /// pinned siblings even when a checkout has since moved.
    fn start_prepared_deployment(&mut self, id: u32, request: cloud_runtime::deployment::Request, ctx: &egui::Context) {
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        let Some(pending) = runtime.preparation.take() else {
            return;
        };
        let siblings = if pending.pinned { Vec::new() } else { pending.siblings };
        runtime.start_deployment(request, siblings, ctx);
        if pending.then.is_some() {
            runtime.operation = pending.then;
        }
    }

    /// Saves the cloud in its persistent session before any allocation and returns what a
    /// worker thread makes durable. `None` when refused; the card says why.
    fn save_cloud_before_allocation(&mut self, id: u32) -> Option<Durability> {
        let result = (|| {
            let session = self
                .active_session
                .as_ref()
                .filter(|session| session.persistent)
                .ok_or_else(|| {
                    "Save this workspace in a persistent Horizon session before allocating a worker".to_string()
                })?;
            if !self.auto_save_runtime_state() {
                return Err(
                    "Could not save this workspace before allocating a worker; retry after fixing session storage"
                        .into(),
                );
            }
            Ok(Durability::new(self.session_store.clone(), session.session_id.clone()))
        })();
        result.map_err(|error| self.show_unsaved_cloud(id, error)).ok()
    }

    fn show_unsaved_cloud(&mut self, id: u32, error: String) {
        self.cloud_prototype.production.runtimes.entry(id).or_default().error = Some(error.clone());
        self.cloud_prototype.error = Some(error);
    }

    /// Applies every report until no preparation runs. Tests use it where the UI would
    /// apply them on later frames.
    #[cfg(test)]
    pub(super) fn finish_cloud_preparations(&mut self, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self
            .cloud_prototype
            .production
            .runtimes
            .values()
            .any(|runtime| runtime.preparation.is_some())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "a cloud preparation did not finish"
            );
            self.poll_cloud_preparations(ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
