//! Restore saved cloud membership and reconnect its original recorded workers.
use super::{HorizonApp, Runtime, Store, cards, cloud_runtime, resize, session_record};
#[cfg(debug_assertions)]
use super::{failure_preview, log_preview, stopped_preview};

impl HorizonApp {
    pub(in crate::app::cloud_panel) fn cloud_state_matches_session(&self) -> bool {
        self.cloud_prototype.production.session_id == self.active_session.as_ref().map(|s| s.session_id.clone())
    }

    pub(super) fn restore_cloud_state(&mut self, ctx: &egui::Context) {
        let session = self.active_session.as_ref().map(|s| s.session_id.clone());
        if !self.cloud_prototype.initialized || self.cloud_prototype.production.session_id != session {
            // Clouds that do not survive this restore never paint again, so their
            // row-height caches would otherwise stay in egui temp data.
            let kept: Vec<u32> = self.board.cloud_groups.0.iter().map(|group| group.issue).collect();
            let mut dropped: Vec<u32> = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .map(|group| group.issue)
                .chain(self.cloud_prototype.production.runtimes.keys().copied())
                .filter(|id| !kept.contains(id))
                .collect();
            dropped.sort_unstable();
            dropped.dedup();
            for id in dropped {
                cards::forget_log_heights(ctx, id);
            }
            self.cloud_prototype.initialized = true;
            self.cloud_prototype.production.pending_creation = None;
            self.cloud_prototype.production.close.reset();
            self.cloud_prototype.production.session_id = session;
            self.cloud_prototype.root = Some(horizon_core::HorizonHome::resolve().root().join("cloud"));
            self.cloud_prototype.groups = self.board.cloud_groups.clone();
            self.cloud_prototype.production.runtimes.clear();
            self.cloud_prototype.ready = true;
            let reconnect: Vec<_> = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .filter_map(|group| {
                    let launch = group.remote.as_ref()?;
                    let directory =
                        cloud_runtime::state::cloud_directory(self.cloud_prototype.root.as_ref()?, &launch.id);
                    // A save that outlived the runtimes of the previous session holds the
                    // newest record and its lock; this runtime joins it.
                    let sessions = directory
                        .as_deref()
                        .map_or_else(|_| session_record::Sessions::default(), session_record::Sessions::of);
                    let result = directory.and_then(|root| match sessions.saving_record() {
                        Some(record) => Ok(Some(record)),
                        None => Store::lock(&root).and_then(|store| store.load()),
                    });
                    let state = match result {
                        Ok(Some(state)) => state,
                        Ok(None) if !launch.deployment_started => return None,
                        other => {
                            let message = other.err().map_or_else(
                                || "Deployment record is missing; reconcile its worker before continuing".into(),
                                |error| error.to_string(),
                            );
                            self.cloud_prototype.production.runtimes.insert(
                                group.issue,
                                Runtime {
                                    error: Some(message),
                                    state_unavailable: true,
                                    resize: cloud_runtime::state::cloud_directory(
                                        self.cloud_prototype.root.as_ref()?,
                                        &launch.id,
                                    )
                                    .map_or_else(|_| resize::State::default(), |root| resize::restored(&root)),
                                    sessions,
                                    ..Runtime::default()
                                },
                            );
                            return None;
                        }
                    };
                    self.cloud_prototype.production.runtimes.insert(
                        group.issue,
                        Runtime {
                            stage: Some(state.stage),
                            state: Some(state.clone()),
                            sessions,
                            ..Runtime::default()
                        },
                    );
                    Runtime::reconnects_on_restore(&state).then_some(group.issue)
                })
                .collect();
            self.sync_bound_profiles();
            for id in reconnect {
                self.start_production_deployment(id, ctx);
            }
            // A debug build can show a synthetic deploy log. Release builds omit it.
            #[cfg(debug_assertions)]
            log_preview::seed(self);
            #[cfg(debug_assertions)]
            stopped_preview::seed(self, ctx);
            #[cfg(debug_assertions)]
            failure_preview::seed(self);
        }
    }
}
