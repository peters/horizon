//! Nonsecret API status snapshots from background readers; no credential mutation UI.

use crate::theme;
pub(in crate::app) use horizon_core::cloud_runtime::tailnet::ApiCredentialObservation as Observation;
use horizon_core::cloud_runtime::tailnet::{self, ApiCredentialStatus, Catalog};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
};

type Update = Vec<(String, Result<ApiCredentialStatus, String>)>;
pub(super) type Reply = Result<Update, String>;

#[derive(Default)]
pub(super) struct State {
    statuses: BTreeMap<String, Result<ApiCredentialStatus, String>>,
    revision: u64,
    ids: BTreeSet<String>,
    receiver: Option<Receiver<Reply>>,
    barriers: Vec<Receiver<Reply>>,
    refresh: Option<(PathBuf, Vec<String>)>,
    message: Option<String>,
    failed: bool,
}

impl State {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn read_error(&self) -> Option<&str> {
        self.message.as_deref()
    }

    #[cfg(test)]
    pub(super) fn reply_fixture(ids: BTreeSet<String>, receiver: Receiver<Reply>) -> Self {
        Self {
            ids,
            receiver: Some(receiver),
            ..Self::default()
        }
    }

    pub(super) fn observation(&self, id: &str) -> Observation {
        if !self.ids.contains(id) {
            return Observation::Unavailable;
        }
        if self.row_error(id).is_some() {
            return Observation::Unavailable;
        }
        if self.busy() {
            return Observation::Pending;
        }
        match self.statuses.get(id) {
            Some(Ok(status)) => Observation::Ready(*status),
            Some(Err(_)) | None => Observation::Unavailable,
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some() || !self.barriers.is_empty()
    }

    pub(super) fn row_error(&self, id: &str) -> Option<&str> {
        if !self.ids.contains(id) {
            return None;
        }
        self.statuses
            .get(id)
            .and_then(|result| result.as_ref().err())
            .map(String::as_str)
    }

    fn start(&mut self, ctx: &egui::Context, task: impl FnOnce() -> Reply + Send + 'static) {
        let (sender, receiver) = channel();
        self.receiver = Some(receiver);
        self.revision = self.revision.wrapping_add(1);
        self.message = None;
        self.failed = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(task());
            ctx.request_repaint();
        });
    }

    pub(super) fn refresh(&mut self, ctx: &egui::Context, root: &Path, catalog: &Catalog) {
        self.ids = catalog.tailnets.iter().map(|network| network.id.clone()).collect();
        self.statuses.retain(|id, _| self.ids.contains(id));
        self.refresh = Some((root.into(), self.ids.iter().cloned().collect()));
        self.start_refresh(ctx);
    }

    fn start_refresh(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if let Some((root, ids)) = self.refresh.take() {
            let failure = self.failed.then(|| self.message.clone()).flatten();
            self.start(ctx, move || {
                let store = tailnet::store(&root);
                Ok(ids
                    .into_iter()
                    .map(|id| {
                        let status = store.api_credential_status(&id).map_err(|error| error.to_string());
                        (id, status)
                    })
                    .collect())
            });
            if let Some(error) = failure {
                self.failed = true;
                self.message = Some(error);
            }
        }
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        let mut failure = None;
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    self.revision = self.revision.wrapping_add(1);
                    match result {
                        Ok(update) => {
                            self.failed = false;
                            self.message = None;
                            for (id, status) in update {
                                if self.ids.contains(&id) {
                                    self.statuses.insert(id, status);
                                }
                            }
                        }
                        Err(error) => failure = Some(error),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.revision = self.revision.wrapping_add(1);
                    failure = Some("API credential read interrupted. Try again.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        self.barriers.retain(|receiver| match receiver.try_recv() {
            Err(TryRecvError::Empty) => true,
            Ok(Ok(_)) => false,
            result => {
                let error = match result {
                    Ok(Err(error)) => error,
                    _ => "API credential read interrupted. Try again.".into(),
                };
                failure = Some(error);
                false
            }
        });
        if let Some(error) = &failure {
            self.statuses.retain(|_, status| status.is_err());
            self.failed = true;
            self.message = Some(error.clone());
        }
        self.start_refresh(ctx);
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    pub(super) fn reload_after(&mut self, mut viewer: Self, catalog: Option<&Catalog>) {
        let ids = catalog.map_or_else(
            || viewer.ids.clone(),
            |catalog| catalog.tailnets.iter().map(|network| network.id.clone()).collect(),
        );
        viewer.statuses.retain(|id, status| ids.contains(id) && status.is_err());
        self.statuses.retain(|id, status| ids.contains(id) && status.is_err());
        self.statuses.extend(viewer.statuses);
        self.ids = self.statuses.keys().cloned().collect();
        self.refresh = None;
        self.revision = self.revision.wrapping_add(1);
        if let Some(receiver) = self.receiver.take() {
            self.barriers.push(receiver);
        }
        if let Some(receiver) = viewer.receiver.take() {
            self.barriers.push(receiver);
        }
        self.barriers.append(&mut viewer.barriers);
        if viewer.failed {
            self.failed = true;
            self.message = viewer.message.take();
        }
    }

    pub(super) fn row(&self, ui: &mut egui::Ui, id: &str, observation: Observation) {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("API credential").size(12.0).strong());
            ui.label(
                egui::RichText::new(status_text(observation))
                    .size(12.0)
                    .color(theme::FG_SOFT()),
            );
        });
        if let Some(error) = self.row_error(id) {
            ui.colored_label(theme::PALETTE_RED(), error);
        }
    }

    pub(super) fn status(&self, ui: &mut egui::Ui) {
        if self.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading API credential status…");
            });
        }
    }
}

fn status_text(observation: Observation) -> &'static str {
    use tailnet::ApiCredentialKind;
    match observation {
        Observation::Ready(ApiCredentialStatus::Missing) => "Not stored",
        Observation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)) => {
            "Personal API token stored"
        }
        Observation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)) => "OAuth client stored",
        Observation::Pending => "Reading…",
        Observation::Unavailable => "Status unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::tailnet::ApiCredentialKind;

    #[test]
    fn close_preserves_only_current_row_failures_until_a_fresh_reply_replaces_them() {
        let (old_sender, old_receiver) = channel();
        let mut state = State {
            ids: ["locked".into()].into(),
            statuses: [("locked".into(), Err("Earlier cached failure".into()))].into(),
            ..State::default()
        };
        state.reload_after(
            State {
                ids: ["locked".into(), "healthy".into()].into(),
                statuses: [
                    ("locked".into(), Err("Synthetic locked row".into())),
                    ("healthy".into(), Ok(ApiCredentialStatus::Missing)),
                    ("removed".into(), Err("Stale removed failure".into())),
                ]
                .into(),
                receiver: Some(old_receiver),
                ..State::default()
            },
            None,
        );
        assert_eq!(state.row_error("locked"), Some("Synthetic locked row"));
        assert_eq!(state.observation("locked"), Observation::Unavailable);
        assert!(state.row_error("removed").is_none());
        assert!(!state.statuses.contains_key("healthy"));
        old_sender
            .send(Ok(vec![("locked".into(), Ok(ApiCredentialStatus::Missing))]))
            .unwrap();
        let ctx = egui::Context::default();
        state.poll(&ctx);
        assert_eq!(state.row_error("locked"), Some("Synthetic locked row"));
        let (sender, receiver) = channel();
        state.ids = ["locked".into(), "healthy".into()].into();
        state.receiver = Some(receiver);
        assert_eq!(state.observation("locked"), Observation::Unavailable);
        assert_eq!(state.observation("healthy"), Observation::Pending);
        sender
            .send(Ok(vec![
                (
                    "locked".into(),
                    Ok(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)),
                ),
                ("healthy".into(), Ok(ApiCredentialStatus::Missing)),
            ]))
            .unwrap();
        state.poll(&ctx);
        assert!(state.row_error("locked").is_none());
        assert_eq!(
            state.observation("locked"),
            Observation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth))
        );
        let removed = Catalog::default();
        let root = tempfile::tempdir().unwrap();
        state.refresh(&ctx, root.path(), &removed);
        assert!(state.statuses.is_empty());
    }

    #[test]
    fn a_pending_settings_reader_cannot_erase_existing_current_cache_failures() {
        let (sender, receiver) = channel();
        let mut state = State {
            ids: ["locked".into(), "healthy".into(), "removed".into()].into(),
            statuses: [
                ("locked".into(), Err("Known cache failure".into())),
                ("healthy".into(), Ok(ApiCredentialStatus::Missing)),
                ("removed".into(), Err("Removed cache failure".into())),
            ]
            .into(),
            ..State::default()
        };
        state.reload_after(
            State {
                ids: ["locked".into(), "healthy".into()].into(),
                receiver: Some(receiver),
                ..State::default()
            },
            None,
        );
        assert!(state.busy());
        assert_eq!(state.row_error("locked"), Some("Known cache failure"));
        assert_eq!(state.observation("locked"), Observation::Unavailable);
        assert!(!state.statuses.contains_key("healthy"));
        assert!(state.row_error("removed").is_none());
        sender
            .send(Ok(vec![("locked".into(), Ok(ApiCredentialStatus::Missing))]))
            .unwrap();
        state.poll(&egui::Context::default());
        assert_eq!(state.row_error("locked"), Some("Known cache failure"));
    }

    #[test]
    fn a_closed_status_view_waits_for_the_read_before_accepting_fresh_status() {
        let (sender, receiver) = channel();
        let mut state = State {
            ids: ["work".into()].into(),
            statuses: [("work".into(), Ok(ApiCredentialStatus::Missing))].into(),
            refresh: Some((PathBuf::from("must-not-read-stale-catalog"), vec!["work".into()])),
            ..State::default()
        };
        state.reload_after(
            State {
                receiver: Some(receiver),
                ..State::default()
            },
            None,
        );
        assert!(state.read_error().is_none());
        assert!(state.busy());
        assert!(state.statuses.is_empty());
        assert!(state.ids.is_empty());
        assert!(
            state.refresh.is_none(),
            "The pre-close catalog cannot schedule another read"
        );
        state.poll(&egui::Context::default());
        sender
            .send(Ok(vec![(
                "work".into(),
                Ok(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)),
            )]))
            .unwrap();
        state.poll(&egui::Context::default());
        assert!(state.read_error().is_none());
        assert!(!state.busy());
        assert!(
            state.statuses.is_empty(),
            "A closed view's stale result is not a fresh observation"
        );
    }

    #[test]
    fn a_failed_or_disconnected_read_survives_settings_close() {
        for disconnected in [false, true] {
            let (sender, receiver) = channel();
            let mut state = State::default();
            state.reload_after(
                State {
                    receiver: Some(receiver),
                    ..State::default()
                },
                None,
            );
            if !disconnected {
                sender.send(Err("Synthetic locked store".into())).unwrap();
            }
            drop(sender);
            state.poll(&egui::Context::default());
            let error = state.read_error().unwrap();
            assert!(state.failed);
            assert!(!error.is_empty());
            assert!(!state.busy());
            assert!(state.statuses.is_empty());
        }
        let mut state = State::default();
        state.reload_after(
            State {
                failed: true,
                message: Some("Completed API read failed".into()),
                ..State::default()
            },
            None,
        );
        assert_eq!(state.read_error(), Some("Completed API read failed"));
        assert!(state.failed);
    }

    #[test]
    fn successful_retry_clears_interruption_but_preserves_row_errors() {
        for disconnected in [false, true] {
            let (sender, receiver) = channel();
            let mut state = State::reply_fixture(["work".into(), "locked".into()].into(), receiver);
            if !disconnected {
                sender.send(Err("Synthetic interrupted read".into())).unwrap();
            }
            drop(sender);
            let ctx = egui::Context::default();
            state.poll(&ctx);
            assert!(state.failed);
            assert!(state.message.is_some());

            let (sender, receiver) = channel();
            state.receiver = Some(receiver);
            state.poll(&ctx);
            assert!(state.message.is_some(), "A pending retry cannot claim recovery");
            sender
                .send(Ok(vec![
                    ("work".into(), Ok(ApiCredentialStatus::Missing)),
                    ("locked".into(), Err("Synthetic locked row".into())),
                ]))
                .unwrap();
            state.poll(&ctx);
            assert!(!state.failed);
            assert!(state.message.is_none());
            assert_eq!(
                state.observation("work"),
                Observation::Ready(ApiCredentialStatus::Missing)
            );
            assert_eq!(state.observation("locked"), Observation::Unavailable);
            assert_eq!(state.statuses["locked"].as_ref().unwrap_err(), "Synthetic locked row");
        }
    }

    #[test]
    fn delayed_status_does_not_restore_a_removed_network() {
        let (sender, receiver) = channel();
        let mut state = State {
            ids: ["retained".into()].into(),
            receiver: Some(receiver),
            ..State::default()
        };
        sender
            .send(Ok(vec![
                (
                    "removed".into(),
                    Ok(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)),
                ),
                ("retained".into(), Ok(ApiCredentialStatus::Missing)),
            ]))
            .unwrap();
        state.poll(&egui::Context::default());
        assert!(!state.statuses.contains_key("removed"));
        assert_eq!(state.statuses.get("retained"), Some(&Ok(ApiCredentialStatus::Missing)));
    }

    #[test]
    fn a_blocked_credential_reader_does_not_block_render_or_invent_status() {
        let (_sender, receiver) = channel();
        let mut state = State {
            receiver: Some(receiver),
            ..State::default()
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            state.poll(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| state.status(ui));
        });
        assert!(state.busy());
        assert!(state.statuses.is_empty());
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    #[test]
    fn one_unreadable_credential_does_not_hide_another_networks_kind() {
        let (sender, receiver) = channel();
        let mut state = State {
            ids: ["invalid".into(), "valid".into()].into(),
            receiver: Some(receiver),
            ..State::default()
        };
        sender
            .send(Ok(vec![
                ("invalid".into(), Err("Synthetic malformed stored data".into())),
                (
                    "valid".into(),
                    Ok(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth)),
                ),
            ]))
            .unwrap();
        state.poll(&egui::Context::default());
        assert!(state.statuses["invalid"].is_err());
        assert_eq!(
            state.statuses["valid"],
            Ok(ApiCredentialStatus::Stored(ApiCredentialKind::OAuth))
        );
        assert!(!state.busy());
    }

    #[test]
    fn cached_discovery_does_not_confuse_pending_or_unavailable_with_missing() {
        let mut state = State {
            ids: ["missing".into(), "stored".into(), "failed".into()].into(),
            statuses: [
                ("missing".into(), Ok(ApiCredentialStatus::Missing)),
                (
                    "stored".into(),
                    Ok(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)),
                ),
                ("failed".into(), Err("Synthetic locked store".into())),
            ]
            .into(),
            ..State::default()
        };
        assert_eq!(
            state.observation("missing"),
            Observation::Ready(ApiCredentialStatus::Missing)
        );
        assert_eq!(
            state.observation("stored"),
            Observation::Ready(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken))
        );
        assert_eq!(state.observation("failed"), Observation::Unavailable);
        assert_eq!(state.observation("unknown"), Observation::Unavailable);
        let (_sender, receiver) = channel();
        state.receiver = Some(receiver);
        assert_eq!(state.observation("missing"), Observation::Pending);
        assert_eq!(state.observation("stored"), Observation::Pending);
        assert_eq!(state.observation("unknown"), Observation::Unavailable);
    }
}
