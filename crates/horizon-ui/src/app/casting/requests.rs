use super::{
    super::{HorizonApp, browser_requests::actor_panel},
    Session,
};
use horizon_cast::{CastSession, CastStatus, PairingStore};
use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::{
    self,
    cast::{self, CastOperation, CastOutcome, CastReceiver, CastSessionInfo, CastSource, CastSourceInfo},
};
use std::path::Path;

impl HorizonApp {
    pub(in crate::app) fn drain_cast_requests(&mut self, ctx: &egui::Context, root: Option<&Path>) {
        let requests = match root {
            Some(root) => cast::claim_at(root, manifest::host_instance()),
            None => cast::claim(manifest::host_instance()),
        };
        let Ok(requests) = requests else {
            return;
        };
        for request in requests {
            let outcome = if request.deadline_at_millis <= manifest::now_millis() {
                CastOutcome::failed("Request expired")
            } else if let Some(actor) = actor_panel(&self.board, &request.actor) {
                self.cast_operation(actor.workspace_id, &request.operation, ctx)
            } else {
                CastOutcome::failed("Calling agent is no longer available")
            };
            let result = match root {
                Some(root) => cast::complete_at(root, &request, outcome),
                None => cast::complete(&request, outcome),
            };
            if let Err(error) = result {
                tracing::warn!(%error,"could not publish casting response");
            }
        }
    }
    pub(in crate::app) fn cast_operation(
        &mut self,
        workspace: WorkspaceId,
        operation: &CastOperation,
        ctx: &egui::Context,
    ) -> CastOutcome {
        if self.casting.poll() {
            ctx.request_repaint();
        }
        let result = (|| -> Result<(), String> {
            match operation {
                CastOperation::Discover => self.casting.discover(),
                CastOperation::Sources | CastOperation::Status => {}
                CastOperation::Paired => {
                    self.casting.paired_refresh = None;
                    self.casting.paired_refresh_pending = false;
                    self.casting.paired_receivers =
                        PairingStore::list(&self.casting.pairing_directory).map_err(|error| error.to_string())?;
                }
                CastOperation::Forget { receiver_id } => self.casting.forget_pairing(receiver_id)?,
                CastOperation::Start {
                    receiver_id,
                    source,
                    orientation,
                    resolution,
                } => {
                    if self.casting.receiver_busy(receiver_id) {
                        return Err("This TV already has a casting or pairing session".into());
                    }
                    self.cast_source_rect(workspace, source, ctx)?;
                    let receiver = self
                        .casting
                        .receivers
                        .iter()
                        .find(|receiver| receiver.id == *receiver_id)
                        .ok_or("Discover and select an Apple TV first")?;
                    let store = PairingStore::new(
                        self.casting.pairing_directory.clone(),
                        receiver.id.clone(),
                        receiver.name.clone(),
                    );
                    let format = super::video_format(*orientation, *resolution);
                    let (width, height) = format.dimensions();
                    let scaling = super::scaling::Scaler::new((usize::from(width), usize::from(height)))
                        .map_err(|error| error.to_string())?;
                    let worker = CastSession::start_remembered(receiver.address, format, store)
                        .map_err(|error| error.to_string())?;
                    self.casting
                        .sessions
                        .retain(|session| session.receiver_id != *receiver_id);
                    self.casting.sessions.push(Session {
                        generation: std::time::Instant::now(),
                        receiver_id: receiver_id.clone(),
                        workspace,
                        source: source.clone(),
                        orientation: *orientation,
                        resolution: *resolution,
                        worker,
                        scaling: Some(scaling),
                        failure_notified: false,
                    });
                    self.casting.notice = None;
                }
                CastOperation::Pair { receiver_id, pin } => {
                    let session = self
                        .casting
                        .sessions
                        .iter()
                        .find(|session| session.receiver_id == *receiver_id && session.workspace == workspace)
                        .ok_or("No pairing session in this workspace")?;
                    session
                        .worker
                        .pair(zeroize::Zeroizing::new(pin.clone()))
                        .map_err(|error| error.to_string())?;
                }
                CastOperation::Stop { receiver_id } => {
                    if let Some(session) = self
                        .casting
                        .sessions
                        .iter_mut()
                        .find(|session| session.receiver_id == *receiver_id && session.workspace == workspace)
                    {
                        session.scaling = None;
                        session.worker.stop();
                    }
                }
            }
            Ok(())
        })();
        ctx.request_repaint();
        let mut outcome = self.cast_snapshot(workspace, ctx);
        outcome.error = result.err().or_else(|| {
            matches!(
                operation,
                CastOperation::Discover | CastOperation::Status | CastOperation::Sources
            )
            .then(|| self.casting.discovery_error.clone())
            .flatten()
        });
        outcome
    }
    pub(super) fn cast_snapshot(&self, workspace: WorkspaceId, ctx: &egui::Context) -> CastOutcome {
        let mut sources: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == workspace)
            .map(|panel| {
                let source = CastSource::Panel {
                    id: panel.local_id.clone(),
                };
                CastSourceInfo {
                    available: self.cast_source_rect(workspace, &source, ctx).is_ok(),
                    source,
                    name: panel.title.clone(),
                }
            })
            .collect();
        if let Some(value) = self.board.workspace(workspace) {
            let source = CastSource::Workspace {
                id: value.local_id.clone(),
            };
            sources.push(CastSourceInfo {
                available: self.cast_source_rect(workspace, &source, ctx).is_ok(),
                source,
                name: value.name.clone(),
            });
        }
        let sessions = self
            .casting
            .sessions
            .iter()
            .filter(|session| session.workspace == workspace)
            .map(|session| {
                let (state, frames, error) = match session.worker.status() {
                    CastStatus::Connecting => ("connecting", 0, None),
                    CastStatus::PinRequired => ("pin_required", 0, None),
                    CastStatus::Starting => ("starting", 0, None),
                    CastStatus::Streaming { frames } => ("streaming", frames, None),
                    CastStatus::Stopping => ("stopping", 0, None),
                    CastStatus::Stopped => ("stopped", 0, None),
                    CastStatus::Failed(error) => ("failed", 0, Some(error)),
                };
                let encoding = session.worker.encoding();
                CastSessionInfo {
                    receiver_id: session.receiver_id.clone(),
                    source: session.source.clone(),
                    orientation: session.orientation,
                    resolution: session.resolution,
                    state: state.into(),
                    frames,
                    encoder: encoding.as_ref().map(|selection| selection.backend.as_str().into()),
                    encoder_fallback: encoding.and_then(|selection| selection.fallback_reason),
                    error,
                }
            })
            .collect();
        CastOutcome {
            paired_receivers: self
                .casting
                .paired_receivers
                .iter()
                .map(|receiver| CastReceiver {
                    id: receiver.id.clone(),
                    name: receiver.name.clone(),
                })
                .collect(),
            receivers: self
                .casting
                .receivers
                .iter()
                .map(|receiver| CastReceiver {
                    id: receiver.id.clone(),
                    name: receiver.name.clone(),
                })
                .collect(),
            sources,
            sessions,
            discovering: self.casting.discovery.is_some(),
            error: self.casting.discovery_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        DeviceRequestBridge,
        test_support::{editor_workspace_state, test_app_with_startup},
    };
    use horizon_core::{PanelKind, RuntimeState, StartupDecision};
    use std::time::Duration;

    #[test]
    fn mcp_pump_answers_without_frames_and_resolves_current_workspace() {
        let state = RuntimeState {
            workspaces: vec![
                editor_workspace_state("first", [0.0, 0.0]),
                editor_workspace_state("second", [600.0, 0.0]),
            ],
            ..RuntimeState::default()
        };
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(state),
        });
        // Unit fixture supplies an agent identity without launching a real agent process.
        app.board.panels[0].kind = PanelKind::Codex;
        let actor = format!("horizon:{}", app.board.panels[0].local_id);
        let root = tempfile::tempdir().expect("root");
        let identity = manifest::AgentIdentity::new(&actor, Some(manifest::host_instance()));
        let request =
            cast::enqueue_at(root.path(), identity, CastOperation::Sources, Duration::from_secs(5)).expect("enqueue");
        let bridge = DeviceRequestBridge::with_root(root.path().to_path_buf());
        bridge.install(app, ctx);
        bridge.poll_on_ui_thread();
        let response = cast::take_result_at(root.path(), &request)
            .expect("result")
            .expect("answer without frame");
        assert!(response.error.is_none());
        assert!(
            response
                .sources
                .iter()
                .any(|s| s.source == CastSource::Workspace { id: "first".into() })
        );
        assert!(
            !response
                .sources
                .iter()
                .any(|s| s.source == CastSource::Workspace { id: "second".into() })
        );
        let outsider = cast::enqueue_at(
            root.path(),
            manifest::AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance())),
            CastOperation::Sources,
            Duration::from_secs(5),
        )
        .expect("enqueue outsider");
        bridge.poll_on_ui_thread();
        assert!(
            cast::take_result_at(root.path(), &outsider)
                .expect("result")
                .expect("answer")
                .error
                .is_some()
        );
    }
}
