//! The Cast button in the root toolbar, which casts the whole Horizon window.
use super::super::HorizonApp;
use super::Anchor;
use egui::{Context, Rect};
use horizon_core::browser::manifest::cast::CastSource;

impl HorizonApp {
    /// Whether the root toolbar's cast button shows as on: a whole-window cast runs, or the
    /// button opened the picker.
    pub(in crate::app) fn application_cast_highlighted(&self) -> bool {
        self.casting
            .picker
            .as_ref()
            .is_some_and(|picker| picker.anchor == Anchor::Application)
            || self
                .casting
                .sessions
                .iter()
                .any(|session| matches!(session.source, CastSource::Application {}) && !session.worker.finished())
    }

    /// Opens the Cast picker under the toolbar `button`, or closes it when that button opened
    /// it. The picker keeps its session bookkeeping and its consent for agents in the active
    /// workspace, so it does not open without one.
    pub(in crate::app) fn toggle_application_cast_picker(&mut self, button: Rect, ctx: &Context) {
        let Some(workspace) = self.board.active_workspace else {
            return;
        };
        self.casting.toggle_picker(
            Anchor::Application,
            workspace,
            CastSource::Application {},
            Some(button),
            ctx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::casting::{
        Session,
        requests::tests::{await_pairing, synthetic_pairing_receiver},
    };
    use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
    use horizon_cast::CastSession;
    use horizon_core::{
        RuntimeState, StartupDecision,
        browser::manifest::cast::{CastOrientation, CastResolution},
    };
    use std::{sync::mpsc, time::Duration};

    fn app_with_one_workspace() -> (tempfile::TempDir, Context, HorizonApp) {
        let (temp, ctx, app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![editor_workspace_state("synthetic", [0.0, 0.0])],
                active_workspace_local_id: Some("synthetic".into()),
                ..RuntimeState::default()
            }),
        });
        (temp, ctx, app)
    }

    fn toolbar_button() -> Rect {
        Rect::from_min_size(egui::pos2(1480.0, 8.0), egui::vec2(34.0, 30.0))
    }

    fn application_session(app: &mut HorizonApp) -> std::thread::JoinHandle<()> {
        let workspace = app.board.active_workspace.expect("the active workspace");
        let (address, peer) = synthetic_pairing_receiver("127.245.13.1");
        let worker = CastSession::start(address, horizon_cast::VideoFormat::default()).expect("isolated worker");
        await_pairing(&worker);
        app.casting.sessions.push(Session {
            generation: std::time::Instant::now(),
            receiver_id: "synthetic".into(),
            workspace,
            source: CastSource::Application {},
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
            worker,
            scaling: None,
            failure_notified: false,
            agent_controlled: false,
        });
        peer
    }

    #[test]
    fn the_root_toolbar_button_opens_and_closes_an_application_picker() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        // A pending discovery keeps the picker from searching the real network.
        let (_send, receive) = mpsc::channel();
        app.casting.discovery = Some(receive);
        let active = app.board.active_workspace.expect("the active workspace");
        let button = toolbar_button();
        assert!(!app.application_cast_highlighted());
        app.toggle_application_cast_picker(button, &ctx);
        let picker = app.casting.picker.as_ref().expect("the button opens the picker");
        assert_eq!(picker.source, CastSource::Application {});
        assert_eq!(picker.anchor, Anchor::Application);
        assert_eq!(picker.workspace, active);
        assert_eq!(picker.anchor_rect, Some(button));
        assert!(app.application_cast_highlighted());
        app.toggle_application_cast_picker(button, &ctx);
        assert!(app.casting.picker.is_none(), "the same button closes it");
        assert!(!app.application_cast_highlighted());
    }

    #[test]
    fn the_highlight_follows_the_root_toolbar_anchor_and_application_sessions() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        // A pending discovery keeps the picker from searching the real network.
        let (_send, receive) = mpsc::channel();
        app.casting.discovery = Some(receive);
        let workspace = app.board.active_workspace.expect("the active workspace");
        app.toggle_workspace_cast_picker(workspace, &ctx);
        assert!(
            !app.application_cast_highlighted(),
            "a workspace picker is not the root toolbar button's"
        );
        app.toggle_workspace_cast_picker(workspace, &ctx);
        let peer = application_session(&mut app);
        assert!(app.application_cast_highlighted(), "an application cast runs");
        app.casting.sessions[0].source = CastSource::Workspace {
            id: app.board.workspaces[0].local_id.clone(),
        };
        assert!(
            !app.application_cast_highlighted(),
            "a workspace cast is not an application cast"
        );
        app.casting.stop_and_wait(Duration::from_secs(3));
        assert!(!app.application_cast_highlighted(), "the stopped cast is off");
        peer.join().expect("synthetic receiver");
    }
}
