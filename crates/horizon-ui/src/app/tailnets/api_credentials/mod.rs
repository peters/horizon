//! Nonsecret background snapshots and native owner credential removal.
mod removal;
use super::requests::{Requests, Target};
use horizon_core::browser::manifest::{self, provider_usage::CredentialOperation as Operation};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Read,
    Write,
}
struct Pending {
    kind: TaskKind,
    receiver: Receiver<Reply>,
}

#[derive(Default)]
pub(super) struct State {
    statuses: BTreeMap<String, Result<ApiCredentialStatus, String>>,
    revision: u64,
    ids: BTreeSet<String>,
    receiver: Option<Pending>,
    barriers: Vec<Pending>,
    refresh: Option<(PathBuf, Vec<String>)>,
    message: Option<String>,
    failed: bool,
    write_result: Option<Result<(), String>>,
}

impl State {
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn read_error(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub(super) fn write_error(&self) -> Option<&str> {
        self.write_result
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .map(String::as_str)
    }

    #[cfg(test)]
    pub(super) fn reply_fixture(ids: BTreeSet<String>, receiver: Receiver<Reply>) -> Self {
        Self {
            ids,
            receiver: Some(Pending {
                kind: TaskKind::Read,
                receiver,
            }),
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

    fn start(&mut self, ctx: &egui::Context, kind: TaskKind, task: impl FnOnce() -> Reply + Send + 'static) {
        let (sender, receiver) = channel();
        self.receiver = Some(Pending { kind, receiver });
        self.revision = self.revision.wrapping_add(1);
        if kind == TaskKind::Write {
            self.write_result = None;
        }
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

    pub(super) fn suspend_refresh(&mut self) {
        self.refresh = None;
    }

    fn start_refresh(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if let Some((root, ids)) = self.refresh.take() {
            self.start(ctx, TaskKind::Read, move || {
                let store = tailnet::store(&root);
                Ok(ids
                    .into_iter()
                    .map(|id| {
                        let status = store.api_credential_status(&id).map_err(|error| error.to_string());
                        (id, status)
                    })
                    .collect())
            });
        }
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.receiver {
            let kind = pending.kind;
            match pending.receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    self.revision = self.revision.wrapping_add(1);
                    match result {
                        Ok(update) => {
                            self.completed(kind);
                            for (id, status) in update {
                                if self.ids.contains(&id) {
                                    self.statuses.insert(id, status);
                                }
                            }
                        }
                        Err(error) => self.record_failure(kind, error),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.revision = self.revision.wrapping_add(1);
                    self.record_failure(kind, interrupted(kind).into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut failures = Vec::new();
        let mut write_completed = false;
        self.barriers.retain(|pending| match pending.receiver.try_recv() {
            Err(TryRecvError::Empty) => true,
            Ok(Ok(_)) => {
                write_completed |= pending.kind == TaskKind::Write;
                false
            }
            result => {
                let error = match result {
                    Ok(Err(error)) => error,
                    _ => interrupted(pending.kind).into(),
                };
                failures.push((pending.kind, error));
                false
            }
        });
        if write_completed {
            self.completed(TaskKind::Write);
        }
        for (kind, error) in failures {
            self.record_failure(kind, error);
        }
        self.start_refresh(ctx);
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn completed(&mut self, kind: TaskKind) {
        match kind {
            TaskKind::Read => {
                self.failed = false;
                self.message = None;
            }
            TaskKind::Write => self.write_result = Some(Ok(())),
        }
    }
    fn record_failure(&mut self, kind: TaskKind, error: String) {
        self.statuses.retain(|_, status| status.is_err());
        match kind {
            TaskKind::Read => {
                self.failed = true;
                self.message = Some(error);
            }
            TaskKind::Write => self.write_result = Some(Err(error)),
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
        if let Some(result) = viewer.write_result.take() {
            self.write_result = Some(result);
        }
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

    pub(super) fn manage(&mut self, ui: &mut egui::Ui, target: &Target, requests: &mut Requests) {
        if self.busy() {
            return;
        }
        let stored = matches!(
            self.observation(&target.id),
            Observation::Ready(ApiCredentialStatus::Stored(_))
        );
        if ui
            .add_enabled(stored, egui::Button::new("Remove API credential"))
            .clicked()
        {
            self.remove(ui.ctx(), target.clone(), None);
        }
        ui.label(
            egui::RichText::new(format!("Saved tailnet ID: {} · shared across workspaces", target.id))
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
        for (id, actor, _) in requests.pending(target) {
            ui.label(format!("API credential removal requested by {actor}"));
            ui.horizontal(|ui| {
                if ui.button("Approve API credential removal").clicked()
                    && let Some(sender) = requests.approve(&id, target, Operation::Remove, manifest::now_millis())
                {
                    self.remove(ui.ctx(), target.clone(), Some(sender));
                }
                if ui.button("Decline request").clicked() {
                    requests.reject(&id, target);
                }
            });
            if self.busy() {
                return;
            }
        }
    }

    fn remove(&mut self, ctx: &egui::Context, target: Target, result: Option<std::sync::mpsc::Sender<bool>>) {
        self.start(ctx, TaskKind::Write, move || {
            let reply = (|| {
                removal::checked_store(&target)?
                    .remove_api_credential(&target.id)
                    .map_err(|error| error.to_string())?;
                Ok(vec![(target.id, Ok(ApiCredentialStatus::Missing))])
            })();
            if let Some(sender) = result {
                let _ = sender.send(reply.is_ok());
            }
            reply
        });
    }

    pub(super) fn status(&self, ui: &mut egui::Ui) {
        if self.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    if self
                        .receiver
                        .as_ref()
                        .is_some_and(|pending| pending.kind == TaskKind::Write)
                        || self.barriers.iter().any(|pending| pending.kind == TaskKind::Write)
                    {
                        "Updating API credential…"
                    } else {
                        "Reading API credential status…"
                    },
                );
            });
        }
    }
}

fn interrupted(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Read => "API credential read interrupted. Try again.",
        TaskKind::Write => "API credential write interrupted. Check its status before another write.",
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

    mod reopen;

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
                receiver: Some(Pending {
                    kind: TaskKind::Read,
                    receiver: old_receiver,
                }),
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
        state.receiver = Some(Pending {
            kind: TaskKind::Read,
            receiver,
        });
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
            write_result: Some(Err("Independent removal failure".into())),
            ..State::default()
        };
        state.reload_after(
            State {
                ids: ["locked".into(), "healthy".into()].into(),
                receiver: Some(Pending {
                    kind: TaskKind::Read,
                    receiver,
                }),
                ..State::default()
            },
            None,
        );
        assert!(state.busy());
        assert_eq!(state.write_error(), Some("Independent removal failure"));
        assert_eq!(state.row_error("locked"), Some("Known cache failure"));
        assert_eq!(state.observation("locked"), Observation::Unavailable);
        assert!(!state.statuses.contains_key("healthy"));
        assert!(state.row_error("removed").is_none());
        sender
            .send(Ok(vec![("locked".into(), Ok(ApiCredentialStatus::Missing))]))
            .unwrap();
        state.poll(&egui::Context::default());
        assert_eq!(state.write_error(), Some("Independent removal failure"));
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
                receiver: Some(Pending {
                    kind: TaskKind::Read,
                    receiver,
                }),
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
                    receiver: Some(Pending {
                        kind: TaskKind::Read,
                        receiver,
                    }),
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
            state.receiver = Some(Pending {
                kind: TaskKind::Read,
                receiver,
            });
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
            receiver: Some(Pending {
                kind: TaskKind::Read,
                receiver,
            }),
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
            receiver: Some(Pending {
                kind: TaskKind::Read,
                receiver,
            }),
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
            receiver: Some(Pending {
                kind: TaskKind::Read,
                receiver,
            }),
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
        state.receiver = Some(Pending {
            kind: TaskKind::Read,
            receiver,
        });
        assert_eq!(state.observation("missing"), Observation::Pending);
        assert_eq!(state.observation("stored"), Observation::Pending);
        assert_eq!(state.observation("unknown"), Observation::Unavailable);
    }

    fn queued(kind: TaskKind) -> (std::sync::mpsc::Sender<Reply>, Pending) {
        let (sender, receiver) = channel();
        (sender, Pending { kind, receiver })
    }
    fn missing() -> Update {
        vec![("work".into(), Ok(ApiCredentialStatus::Missing))]
    }

    #[test]
    fn a_successful_read_preserves_write_failure_until_a_later_write_succeeds() {
        let ctx = egui::Context::default();
        for disconnected in [false, true] {
            let (sender, pending) = queued(TaskKind::Write);
            let mut state = State {
                ids: ["work".into()].into(),
                receiver: Some(pending),
                ..State::default()
            };
            if !disconnected {
                sender.send(Err("Synthetic API write failed".into())).unwrap();
            }
            drop(sender);
            state.poll(&ctx);
            let error = state.write_error().unwrap().to_owned();
            assert!(state.read_error().is_none());
            let (sender, pending) = queued(TaskKind::Read);
            state.receiver = Some(pending);
            sender.send(Ok(missing())).unwrap();
            state.poll(&ctx);
            assert_eq!(
                state.observation("work"),
                Observation::Ready(ApiCredentialStatus::Missing)
            );
            assert_eq!(state.write_error(), Some(error.as_str()));
            assert!(state.read_error().is_none());
            let (sender, pending) = queued(TaskKind::Write);
            state.receiver = Some(pending);
            sender.send(Ok(missing())).unwrap();
            state.poll(&ctx);
            assert!(state.write_error().is_none());
        }
    }

    #[test]
    fn a_successful_write_does_not_clear_an_unrelated_read_failure() {
        let ctx = egui::Context::default();
        let (sender, pending) = queued(TaskKind::Read);
        let mut state = State {
            ids: ["work".into()].into(),
            receiver: Some(pending),
            ..State::default()
        };
        sender.send(Err("Synthetic API read failed".into())).unwrap();
        state.poll(&ctx);
        let (sender, pending) = queued(TaskKind::Write);
        state.receiver = Some(pending);
        sender.send(Ok(missing())).unwrap();
        state.poll(&ctx);
        assert_eq!(state.read_error(), Some("Synthetic API read failed"));
        assert!(state.write_error().is_none());
        let (sender, pending) = queued(TaskKind::Read);
        state.receiver = Some(pending);
        sender.send(Ok(missing())).unwrap();
        state.poll(&ctx);
        assert!(state.read_error().is_none());
    }

    #[test]
    fn closed_write_failures_keep_their_own_error_after_fresh_read_recovery() {
        for disconnected in [false, true] {
            let (sender, pending) = queued(TaskKind::Write);
            let mut production = State::default();
            production.reload_after(
                State {
                    receiver: Some(pending),
                    ..State::default()
                },
                None,
            );
            if !disconnected {
                sender.send(Err("Synthetic closed write failed".into())).unwrap();
            }
            drop(sender);
            let ctx = egui::Context::default();
            production.poll(&ctx);
            let error = production.write_error().unwrap().to_owned();
            assert!(production.read_error().is_none());
            let (sender, pending) = queued(TaskKind::Read);
            production.ids = ["work".into()].into();
            production.receiver = Some(pending);
            sender.send(Ok(missing())).unwrap();
            production.poll(&ctx);
            assert_eq!(production.write_error(), Some(error.as_str()));
            assert_eq!(
                production.observation("work"),
                Observation::Ready(ApiCredentialStatus::Missing)
            );
        }
        let mut production = State::default();
        production.reload_after(
            State {
                write_result: Some(Err("Completed write failed".into())),
                ..State::default()
            },
            None,
        );
        assert_eq!(production.write_error(), Some("Completed write failed"));
        production.reload_after(State::default(), None);
        assert_eq!(production.write_error(), Some("Completed write failed"));
        production.reload_after(
            State {
                write_result: Some(Ok(())),
                ..State::default()
            },
            None,
        );
        assert!(production.write_error().is_none());
    }
}
