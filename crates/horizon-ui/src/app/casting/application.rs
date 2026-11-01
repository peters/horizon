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
            || self.application_cast_running()
    }

    /// One click opens the Cast picker of the whole window; a click while a whole-window cast
    /// runs stops that cast. `chooser` opens the picker also while a cast runs, for another
    /// TV, another quality, or that cast's own Stop and settings.
    pub(in crate::app) fn toggle_application_cast(&mut self, button: Rect, ctx: &Context, chooser: bool) {
        if self
            .casting
            .picker
            .as_ref()
            .is_some_and(|picker| picker.anchor == Anchor::Application)
        {
            self.casting.close_picker(ctx);
            return;
        }
        if chooser {
            self.toggle_application_cast_picker(button, ctx);
            return;
        }
        if self.application_cast_running() {
            self.stop_application_cast();
            return;
        }
        self.toggle_application_cast_picker(button, ctx);
    }

    /// Opens the Cast picker under the toolbar `button`, or closes it when that button opened
    /// it. The picker keeps its session bookkeeping and its consent for agents in the active
    /// workspace, so it does not open without one.
    pub(in crate::app) fn toggle_application_cast_picker(&mut self, button: Rect, ctx: &Context) {
        let Some(workspace) = self.application_target_workspace() else {
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

    /// The workspace that this button uses for session records and consent: the workspace of
    /// a running whole-window cast, so the button's picker can stop that cast after the person
    /// switched to another workspace, else the active workspace.
    fn application_target_workspace(&self) -> Option<horizon_core::WorkspaceId> {
        self.casting
            .sessions
            .iter()
            .rev()
            .find(|session| matches!(session.source, CastSource::Application {}) && !session.worker.finished())
            .map(|session| session.workspace)
            .or(self.board.active_workspace)
    }

    fn application_cast_running(&self) -> bool {
        self.casting
            .sessions
            .iter()
            .any(|session| matches!(session.source, CastSource::Application {}) && !session.worker.finished())
    }

    fn stop_application_cast(&mut self) {
        for session in &mut self.casting.sessions {
            if matches!(session.source, CastSource::Application {}) {
                session.scaling = None;
                session.worker.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::casting::{
        Session,
        requests::tests::{await_pairing, synthetic_pairing_receiver},
    };
    use crate::app::root_chrome::{
        ROOT_TOOLBAR_BUTTON_GAP, ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_CAST_WIDTH, ROOT_TOOLBAR_FPS_WIDTH,
        ROOT_TOOLBAR_MENU_WIDTH, ROOT_TOOLBAR_NEW_CLOUD_COMPACT_WIDTH, ROOT_TOOLBAR_NEW_CLOUD_WIDTH, ToolbarItem,
        root_toolbar_layout,
    };
    use crate::app::test_support::{
        editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
    use horizon_cast::CastSession;
    use horizon_core::browser::manifest::cast::{CastOperation, CastOrientation, CastResolution};
    use horizon_core::{RuntimeState, StartupDecision};
    use std::{os::unix::fs::DirBuilderExt, sync::mpsc, time::Duration};

    const VIEWPORT: [f32; 2] = [1600.0, 1000.0];

    fn app_with_one_workspace() -> (tempfile::TempDir, Context, HorizonApp) {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![editor_workspace_state("synthetic", [0.0, 0.0])],
                active_workspace_local_id: Some("synthetic".into()),
                ..RuntimeState::default()
            }),
        });
        // A session reserves its pairing directory before it connects, and the store
        // refuses a directory that other users can read.
        let pairings = temp.path().join("cast-pairings");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&pairings)
            .expect("pairing directory");
        app.casting.pairing_directory = pairings;
        (temp, ctx, app)
    }

    /// The cast button that the root toolbar lays out for `viewport`.
    fn laid_out_toolbar_button(viewport: Rect) -> Rect {
        let layout = root_toolbar_layout(viewport);
        let mut left = layout.actions_rect.min.x;
        let mut cast = None;
        for item in &layout.visible_items {
            let width = match item {
                ToolbarItem::FpsMeter => ROOT_TOOLBAR_FPS_WIDTH,
                ToolbarItem::NewCloud { compact: true } => ROOT_TOOLBAR_NEW_CLOUD_COMPACT_WIDTH,
                ToolbarItem::NewCloud { compact: false } => ROOT_TOOLBAR_NEW_CLOUD_WIDTH,
                ToolbarItem::Cast => ROOT_TOOLBAR_CAST_WIDTH,
                ToolbarItem::Menu => ROOT_TOOLBAR_MENU_WIDTH,
            };
            if matches!(item, ToolbarItem::Cast) {
                cast = Some(Rect::from_min_size(
                    Pos2::new(left, layout.actions_rect.min.y),
                    Vec2::new(width, ROOT_TOOLBAR_BUTTON_HEIGHT),
                ));
            }
            left += width + ROOT_TOOLBAR_BUTTON_GAP;
        }
        cast.expect("the root toolbar holds the cast button")
    }

    fn toolbar_button() -> Rect {
        laid_out_toolbar_button(Rect::from_min_size(Pos2::ZERO, Vec2::new(VIEWPORT[0], VIEWPORT[1])))
    }

    /// One press and one release with the pointer on `position`, each in its own frame.
    fn click(ctx: &Context, app: &mut HorizonApp, position: Pos2) {
        for pressed in [true, false] {
            let mut input = raw_input(VIEWPORT, None);
            if pressed {
                input.events.push(Event::PointerMoved(position));
            }
            input.events.push(Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            run_app_frame_with_input(ctx, app, input);
        }
    }

    /// A receiver that discovery reports, with a synthetic receiver behind its address.
    /// Each test uses its own receiver address, because a process-wide reservation holds a
    /// receiver IP while a session uses it.
    fn discovered_receiver(app: &mut HorizonApp, id: &str, ip: &str) -> std::thread::JoinHandle<()> {
        let (address, peer) = synthetic_pairing_receiver(ip);
        app.casting.receivers.push(horizon_cast::Receiver {
            id: id.to_owned(),
            name: format!("Synthetic {id}"),
            address,
        });
        peer
    }

    fn pair_only_tv(app: &mut HorizonApp, id: &str) {
        app.casting.paired_receivers.push(horizon_cast::PairedDevice {
            id: id.to_owned(),
            name: format!("Synthetic {id}"),
        });
    }

    /// Waits until the newest session has connected and asked for its PIN, so a later stop
    /// cannot close the socket before the synthetic receiver accepted it.
    fn await_newest_pairing(app: &HorizonApp) {
        let worker = &app.casting.sessions.last().expect("a session").worker;
        await_pairing(worker);
    }

    fn pending_discovery(app: &mut HorizonApp) {
        let (_send, receive) = mpsc::channel();
        app.casting.discovery = Some(receive);
    }

    fn application_session(app: &mut HorizonApp, ip: &str) -> std::thread::JoinHandle<()> {
        let workspace = app.board.active_workspace.expect("the active workspace");
        let (address, peer) = synthetic_pairing_receiver(ip);
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
        pending_discovery(&mut app);
        let active = app.board.active_workspace.expect("the active workspace");
        let button = toolbar_button();
        assert!(!app.application_cast_highlighted());
        app.toggle_application_cast(button, &ctx, true);
        let picker = app.casting.picker.as_ref().expect("the button opens the picker");
        assert_eq!(picker.source, CastSource::Application {});
        assert_eq!(picker.anchor, Anchor::Application);
        assert_eq!(picker.workspace, active);
        assert_eq!(picker.anchor_rect, Some(button));
        assert!(app.application_cast_highlighted());
        app.toggle_application_cast(button, &ctx, true);
        assert!(app.casting.picker.is_none(), "the same button closes it");
        assert!(!app.application_cast_highlighted());
    }

    #[test]
    fn a_plain_click_opens_the_picker_and_starts_nothing() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.11");
        pair_only_tv(&mut app, "living-room");
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        let picker = app.casting.picker.as_ref().expect("a click opens the picker");
        assert_eq!(picker.anchor, Anchor::Application);
        assert!(
            app.casting.sessions.is_empty(),
            "a click alone starts no cast, not even on a known TV"
        );
        assert!(app.application_cast_highlighted(), "the button shows its picker");
        drop(peer);
    }

    #[test]
    fn the_root_toolbar_button_clicked_with_the_pointer_reopens_its_picker() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        // The reported case: one paired TV that discovery already reports. Each click must
        // open or close the picker, however often it repeats.
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.12");
        pair_only_tv(&mut app, "living-room");
        pending_discovery(&mut app);
        run_app_frame_with_input(&ctx, &mut app, raw_input(VIEWPORT, None));
        let position =
            laid_out_toolbar_button(Rect::from_min_size(Pos2::ZERO, Vec2::new(VIEWPORT[0], VIEWPORT[1]))).center();
        // A pointer that rests on the button first, as it does for a person.
        let mut hover = raw_input(VIEWPORT, None);
        hover.events.push(Event::PointerMoved(position));
        run_app_frame_with_input(&ctx, &mut app, hover);
        for round in 0..4 {
            click(&ctx, &mut app, position);
            assert!(
                app.casting.picker.is_some(),
                "round {round}: the click opens the picker"
            );
            click(&ctx, &mut app, position);
            assert!(
                app.casting.picker.is_none(),
                "round {round}: the click closes the picker"
            );
        }
        assert!(app.casting.sessions.is_empty(), "no click started a cast");
        drop(peer);
    }

    #[test]
    fn a_click_while_casting_stops_the_whole_window_session() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = application_session(&mut app, "127.245.13.13");
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(app.casting.picker.is_none(), "stopping needs no picker");
        assert!(
            app.casting.stop_and_wait(Duration::from_secs(5)),
            "the click stops the cast"
        );
        assert!(!app.application_cast_running());
        peer.join().expect("synthetic receiver");
    }

    #[test]
    fn a_click_opens_the_picker_again_after_a_cast_stopped() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.19");
        pair_only_tv(&mut app, "living-room");
        pending_discovery(&mut app);
        let workspace = app.board.active_workspace.expect("the active workspace");
        let start = CastOperation::Start {
            receiver_id: "living-room".into(),
            source: CastSource::Application {},
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
        };
        assert!(
            app.cast_user_operation(workspace, &start, &ctx).error.is_none(),
            "the picker starts the whole-window cast"
        );
        await_newest_pairing(&app);
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(
            app.casting.stop_and_wait(Duration::from_secs(5)),
            "the click stops the cast"
        );
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(
            app.casting.picker.is_some(),
            "a stopped cast leaves the next click with its picker"
        );
        peer.join().expect("synthetic receiver");
    }

    #[test]
    fn the_chooser_click_opens_the_picker_on_a_remembered_tv() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.14");
        pair_only_tv(&mut app, "living-room");
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, true);
        let picker = app.casting.picker.as_ref().expect("the chooser opens the picker");
        assert_eq!(picker.anchor, Anchor::Application);
        assert!(app.casting.sessions.is_empty(), "the chooser starts nothing");
        drop(peer);
    }

    #[test]
    fn the_picker_binds_to_the_workspace_of_a_running_cast() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![
                    editor_workspace_state("first", [0.0, 0.0]),
                    editor_workspace_state("second", [600.0, 0.0]),
                ],
                active_workspace_local_id: Some("first".into()),
                ..RuntimeState::default()
            }),
        });
        let peer = application_session(&mut app, "127.245.13.17");
        let casting_workspace = app.casting.sessions[0].workspace;
        let other = app
            .board
            .workspaces
            .iter()
            .find(|workspace| workspace.id != casting_workspace)
            .expect("a second workspace")
            .id;
        app.board.focus_workspace(other);
        assert_eq!(app.board.active_workspace, Some(other));
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, true);
        let picker = app.casting.picker.as_ref().expect("the chooser opens the picker");
        assert_eq!(
            picker.workspace, casting_workspace,
            "the picker of a running cast stays on its own workspace"
        );
        assert!(picker.binding.is_some(), "the picker can stop the running cast");
        app.casting.close_picker(&ctx);
        app.casting.stop_and_wait(Duration::from_secs(5));
        peer.join().expect("synthetic receiver");
    }

    #[test]
    fn the_highlight_follows_the_root_toolbar_anchor_and_application_sessions() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        pending_discovery(&mut app);
        let workspace = app.board.active_workspace.expect("the active workspace");
        app.toggle_workspace_cast_picker(workspace, &ctx);
        assert!(
            !app.application_cast_highlighted(),
            "a workspace picker is not the root toolbar button's"
        );
        app.toggle_workspace_cast_picker(workspace, &ctx);
        let peer = application_session(&mut app, "127.245.13.16");
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
