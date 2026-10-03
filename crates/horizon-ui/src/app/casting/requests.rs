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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CastCaller {
    WorkspaceAgent,
    User,
}

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
        self.cast_operation_for(workspace, operation, ctx, CastCaller::WorkspaceAgent)
    }
    pub(super) fn cast_user_operation(
        &mut self,
        workspace: WorkspaceId,
        operation: &CastOperation,
        ctx: &egui::Context,
    ) -> CastOutcome {
        self.cast_operation_for(workspace, operation, ctx, CastCaller::User)
    }
    pub(super) fn application_cast_approved(&self, workspace: WorkspaceId) -> bool {
        self.board
            .workspace(workspace)
            .is_some_and(|workspace| self.casting.application_approval.permits(&workspace.local_id))
    }
    pub(super) fn set_application_cast_approval(&mut self, workspace: WorkspaceId, approved: bool) {
        // A new grant replaces the old one. Revoke the old controller's sessions first.
        for session in &mut self.casting.sessions {
            if session.agent_controlled && matches!(session.source, CastSource::Application { .. }) {
                session.scaling = None;
                session.worker.stop();
            }
        }
        self.casting.application_approval.revoke();
        if approved && let Some(workspace) = self.board.workspace(workspace) {
            self.casting.application_approval.grant(workspace.local_id.clone());
        }
    }
    fn cast_operation_for(
        &mut self,
        workspace: WorkspaceId,
        operation: &CastOperation,
        ctx: &egui::Context,
        caller: CastCaller,
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
                    self.validate_cast_start(workspace, source, receiver_id, ctx, caller)?;
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
                    let repaint = ctx.clone();
                    let scaling = super::scaling::Scaler::new((usize::from(width), usize::from(height)), move || {
                        repaint.request_repaint();
                    })
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
                        agent_controlled: caller == CastCaller::WorkspaceAgent,
                    });
                    self.casting.notice = None;
                    if caller == CastCaller::WorkspaceAgent && matches!(source, CastSource::Application { .. }) {
                        self.close_application_cast_controls(ctx);
                    }
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
    fn close_application_cast_controls(&mut self, ctx: &egui::Context) {
        self.casting.picker = None;
        if let Some(menus) = self.casting.control_menus {
            for menu in menus {
                egui::Popup::close_id(ctx, menu.id);
            }
        }
    }
    fn validate_cast_start(
        &self,
        workspace: WorkspaceId,
        source: &CastSource,
        receiver_id: &str,
        ctx: &egui::Context,
        caller: CastCaller,
    ) -> Result<(), String> {
        if matches!(source, CastSource::Application { .. })
            && caller == CastCaller::WorkspaceAgent
            && !self.application_cast_approved(workspace)
        {
            return Err("Entire Horizon requires user approval in the Cast picker for this workspace".into());
        }
        if self.casting.receiver_busy(receiver_id) {
            return Err("This TV already has a casting or pairing session".into());
        }
        self.cast_source_rect(workspace, source, ctx)?;
        Ok(())
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
                    requires_user_approval: false,
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
                requires_user_approval: false,
            });
        }
        sources.push(CastSourceInfo {
            source: CastSource::Application {},
            name: "Entire Horizon".into(),
            available: self
                .cast_source_rect(workspace, &CastSource::Application {}, ctx)
                .is_ok(),
            requires_user_approval: !self.application_cast_approved(workspace),
        });
        let sessions = self
            .casting
            .sessions
            .iter()
            .filter(|session| session.workspace == workspace)
            .map(|session| {
                let (state, error) = match session.worker.status() {
                    CastStatus::Connecting => ("connecting", None),
                    CastStatus::PinRequired => ("pin_required", None),
                    CastStatus::Starting => ("starting", None),
                    CastStatus::Streaming { .. } => ("streaming", None),
                    CastStatus::Stopping => ("stopping", None),
                    CastStatus::Stopped => ("stopped", None),
                    CastStatus::Failed(error) => ("failed", Some(error)),
                };
                let encoding = session.worker.encoding();
                CastSessionInfo {
                    receiver_id: session.receiver_id.clone(),
                    source: session.source.clone(),
                    orientation: session.orientation,
                    resolution: session.resolution,
                    state: state.into(),
                    frames: session.worker.frames_sent(),
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
    use crate::test_egui::DiscardTextures;
    use horizon_core::{PanelKind, RuntimeState, StartupDecision};
    use std::time::Duration;

    fn synthetic_pairing_receiver(ip: &str) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind((ip, 0)).expect("private receiver");
        let address = listener.local_addr().expect("address");
        listener.set_nonblocking(true).expect("nonblocking accept");
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline, "receiver never connected");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("bounded receiver");
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).expect("pair request");
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"POST /pair-pin-start "));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                .expect("pair response");
            assert_eq!(socket.read(&mut byte).expect("session closes"), 0);
        });
        (address, worker)
    }

    fn await_pairing(worker: &CastSession) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while worker.status() != CastStatus::PinRequired {
            assert!(
                std::time::Instant::now() < deadline,
                "pairing did not start: {:?}",
                worker.status()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn open_application_picker(app: &mut HorizonApp, workspace: WorkspaceId, receiver: &str) {
        app.casting.picker = Some(super::super::Picker {
            workspace,
            source: CastSource::Application {},
            receiver: Some(receiver.into()),
            orientation: cast::CastOrientation::default(),
            resolution: cast::CastResolution::default(),
            pin: zeroize::Zeroizing::new(String::new()),
        });
        app.casting.control_menus = Some([egui::LayerId::background(); 2]);
    }

    #[test]
    fn replacing_and_revoking_consent_stops_only_agent_application_sessions() {
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
        let _ = ctx
            .run_ui(crate::app::test_support::raw_input([1600.0, 1000.0], None), |_| {})
            .discard_textures();
        let first = app.board.workspaces[0].id;
        let second = app.board.workspaces[1].id;
        let mut peers = Vec::new();
        for (id, ip) in [
            ("manual", "127.251.123.1"),
            ("first-agent", "127.251.123.2"),
            ("second-agent", "127.251.123.3"),
        ] {
            let (address, peer) = synthetic_pairing_receiver(ip);
            peers.push(peer);
            app.casting.receivers.push(horizon_cast::Receiver {
                id: id.into(),
                name: id.into(),
                address,
            });
        }
        let start = |id: &str| CastOperation::Start {
            receiver_id: id.into(),
            source: CastSource::Application {},
            orientation: cast::CastOrientation::default(),
            resolution: cast::CastResolution::default(),
        };
        assert!(app.cast_user_operation(first, &start("manual"), &ctx).error.is_none());
        await_pairing(&app.casting.sessions[0].worker);
        assert!(
            !app.application_cast_approved(first),
            "manual casting must not grant agent permission"
        );
        app.set_application_cast_approval(first, true);
        open_application_picker(&mut app, first, "first-agent");
        assert!(app.cast_operation(first, &start("first-agent"), &ctx).error.is_none());
        assert!(
            app.casting.picker.is_none(),
            "the approved start must allow its first safe image"
        );
        assert!(
            app.casting.control_menus.is_some(),
            "retain known layers through the next frame"
        );
        await_pairing(&app.casting.sessions[1].worker);
        app.set_application_cast_approval(second, true);
        let replacement_deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !app.casting.sessions[1].worker.finished() {
            assert!(
                std::time::Instant::now() < replacement_deadline,
                "replaced agent session did not stop"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !app.casting.sessions[0].worker.finished(),
            "replacement must preserve the manual session"
        );
        assert!(!app.application_cast_approved(first));
        assert!(app.application_cast_approved(second));
        assert!(app.cast_operation(second, &start("second-agent"), &ctx).error.is_none());
        await_pairing(&app.casting.sessions[2].worker);
        app.set_application_cast_approval(second, false);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while app
            .casting
            .sessions
            .iter()
            .filter(|session| session.agent_controlled)
            .any(|session| !session.worker.finished())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "revoked agent sessions did not stop"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !app.casting.sessions[0].worker.finished(),
            "manual session must remain independent"
        );
        assert!(app.casting.stop_and_wait(Duration::from_secs(3)));
        for peer in peers {
            peer.join().expect("clean receiver closure");
        }
    }

    #[test]
    fn removing_the_approved_workspace_stops_its_application_session() {
        let state = RuntimeState {
            workspaces: vec![
                editor_workspace_state("controller", [0.0, 0.0]),
                editor_workspace_state("remaining", [600.0, 0.0]),
            ],
            ..RuntimeState::default()
        };
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(state),
        });
        let workspace = app.board.workspaces[0].id;
        let (address, peer) = synthetic_pairing_receiver("127.251.123.4");
        app.casting.receivers.push(horizon_cast::Receiver {
            id: "removed-controller".into(),
            name: "Synthetic TV".into(),
            address,
        });
        app.set_application_cast_approval(workspace, true);
        let start = CastOperation::Start {
            receiver_id: "removed-controller".into(),
            source: CastSource::Application {},
            orientation: cast::CastOrientation::default(),
            resolution: cast::CastResolution::default(),
        };
        assert!(app.cast_operation(workspace, &start, &ctx).error.is_none());
        await_pairing(&app.casting.sessions[0].worker);
        app.board.remove_workspace(workspace);
        assert!(!app.application_cast_approved(workspace));
        let _ = ctx
            .run_ui(crate::app::test_support::raw_input([1600.0, 1000.0], None), |ui| {
                assert!(
                    app.cast_source_rect(workspace, &CastSource::Application {}, ui.ctx())
                        .is_err()
                );
                app.cast_frame(ui.ctx());
            })
            .discard_textures();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !app.casting.sessions[0].worker.finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "removing consent scope must stop capture automatically"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.casting.sessions[0].worker.status(), CastStatus::Stopped);
        assert!(app.casting.stop_and_wait(Duration::from_secs(3)));
        peer.join().expect("removed workspace closes its receiver");
    }

    #[test]
    fn application_start_requires_the_users_workspace_grant() {
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
        let first = app.board.workspaces[0].id;
        let second = app.board.workspaces[1].id;
        let start = CastOperation::Start {
            receiver_id: "not-discovered".into(),
            source: CastSource::Application {},
            orientation: cast::CastOrientation::default(),
            resolution: cast::CastResolution::default(),
        };
        assert!(
            app.cast_operation(first, &start, &ctx)
                .error
                .expect("denied")
                .contains("user approval")
        );
        app.set_application_cast_approval(first, true);
        assert!(app.application_cast_approved(first));
        assert!(!app.application_cast_approved(second));
        assert!(
            !app.cast_operation(first, &start, &ctx)
                .error
                .expect("unavailable receiver")
                .contains("user approval")
        );
        assert!(
            app.cast_operation(second, &start, &ctx)
                .error
                .expect("other workspace")
                .contains("user approval")
        );
        app.set_application_cast_approval(first, false);
        assert!(!app.application_cast_approved(first));
        app.set_application_cast_approval(first, true);
        app.casting.reset_for_session_switch();
        assert!(!app.application_cast_approved(first));
        let source = app
            .cast_snapshot(first, &ctx)
            .sources
            .into_iter()
            .find(|source| matches!(source.source, CastSource::Application { .. }))
            .expect("application capability");
        assert!(source.requires_user_approval);
    }

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
