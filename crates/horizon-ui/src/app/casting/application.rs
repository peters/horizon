//! The Cast button in the root toolbar, which casts the whole Horizon window.
use super::super::HorizonApp;
use super::{Anchor, PendingApplicationStart};
use egui::{Context, Rect};
use horizon_core::browser::manifest::cast::{CastOperation, CastOrientation, CastResolution, CastSource};
use std::time::{Duration, Instant};

/// How long a toolbar click waits for discovery to find its TV before it gives up.
const APPLICATION_START_WAIT: Duration = Duration::from_secs(8);

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

    /// One click on the root toolbar starts a whole-window cast on the remembered TV, and a
    /// click while it runs stops that cast. `chooser` opens the picker instead, for another
    /// TV, another quality or a first pairing.
    pub(in crate::app) fn toggle_application_cast(&mut self, button: Rect, ctx: &Context, chooser: bool) {
        if self
            .casting
            .picker
            .as_ref()
            .is_some_and(|picker| picker.anchor == Anchor::Application)
        {
            self.casting.close_picker(ctx);
            self.casting.pending_application_start = None;
            return;
        }
        if self.application_cast_running() {
            self.stop_application_cast();
            return;
        }
        let Some(workspace) = self.board.active_workspace else {
            return;
        };
        if !chooser && let Some(receiver_id) = self.application_start_receiver() {
            if self.application_start_target().is_some_and(|id| id == receiver_id) {
                let orientation = self.application_cast_orientation();
                let resolution = self.application_cast_resolution();
                if self.start_application_cast(workspace, &receiver_id, orientation, resolution, ctx) {
                    return;
                }
            } else {
                self.casting.pending_application_start = Some(PendingApplicationStart {
                    workspace,
                    receiver_id: receiver_id.clone(),
                    orientation: self.application_cast_orientation(),
                    resolution: self.application_cast_resolution(),
                    deadline: Instant::now() + APPLICATION_START_WAIT,
                });
            }
        }
        self.toggle_application_cast_picker(button, ctx);
    }

    /// Starts the whole-window cast that a toolbar click is waiting for, once discovery
    /// either finds its TV or ends without it.
    pub(in crate::app) fn resume_pending_application_cast(&mut self, ctx: &Context) {
        let Some(pending) = self.casting.pending_application_start.clone() else {
            return;
        };
        if self.casting.discovery.is_some() {
            if Instant::now() < pending.deadline {
                return;
            }
            self.casting.pending_application_start = None;
            self.casting
                .notify("No Apple TV answered discovery. Choose a TV in the Cast picker.".into());
            return;
        }
        self.casting.pending_application_start = None;
        if self.casting.receiver_available(&pending.receiver_id) {
            let started = self.start_application_cast(
                pending.workspace,
                &pending.receiver_id,
                pending.orientation,
                pending.resolution,
                ctx,
            );
            if started {
                self.casting.dismiss_picker(ctx);
            }
        } else {
            self.casting.notify(format!(
                "{} did not answer discovery. Choose a TV in the Cast picker.",
                self.application_receiver_name(&pending.receiver_id)
            ));
        }
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

    fn application_cast_running(&self) -> bool {
        self.casting
            .sessions
            .iter()
            .any(|session| matches!(session.source, CastSource::Application {}) && !session.worker.finished())
    }

    /// The TV that discovery can start on now: the TV of the last whole-window cast, else the
    /// only paired TV that discovery sees.
    fn application_start_target(&self) -> Option<String> {
        if let Some(id) = self.casting.last_application_receiver.as_deref()
            && self.casting.receiver_available(id)
        {
            return Some(id.to_owned());
        }
        let mut paired = self.casting.receivers.iter().filter(|receiver| {
            self.casting
                .paired_devices()
                .iter()
                .any(|paired| paired.id == receiver.id)
        });
        match (paired.next(), paired.next()) {
            (Some(only), None) => Some(only.id.clone()),
            _ => None,
        }
    }

    /// The TV that a click waits for when discovery has not found it yet. A TV that the
    /// person never used has no remembered id, and the picker pairs it.
    fn application_start_receiver(&self) -> Option<String> {
        self.application_start_target().or_else(|| {
            self.casting
                .last_application_receiver
                .clone()
                .or_else(|| match self.casting.paired_devices() {
                    [only] => Some(only.id.clone()),
                    _ => None,
                })
        })
    }

    fn application_receiver_name(&self, receiver_id: &str) -> String {
        self.casting
            .receivers
            .iter()
            .find(|receiver| receiver.id == receiver_id)
            .map(|receiver| receiver.name.clone())
            .or_else(|| {
                self.casting
                    .paired_devices()
                    .iter()
                    .find(|paired| paired.id == receiver_id)
                    .map(|paired| paired.name.clone())
            })
            .unwrap_or_else(|| "The Apple TV".to_owned())
    }

    fn application_cast_orientation(&self) -> CastOrientation {
        self.last_application_session()
            .map_or(CastOrientation::Landscape, |session| session.orientation)
    }

    fn application_cast_resolution(&self) -> CastResolution {
        self.last_application_session()
            .map_or(CastResolution::default(), |session| session.resolution)
    }

    /// The settings of the last whole-window cast, which a later click starts again.
    fn last_application_session(&self) -> Option<&super::Session> {
        self.casting
            .sessions
            .iter()
            .rev()
            .find(|session| matches!(session.source, CastSource::Application {}))
    }

    fn start_application_cast(
        &mut self,
        workspace: horizon_core::WorkspaceId,
        receiver_id: &str,
        orientation: CastOrientation,
        resolution: CastResolution,
        ctx: &Context,
    ) -> bool {
        let action = CastOperation::Start {
            receiver_id: receiver_id.to_owned(),
            source: CastSource::Application {},
            orientation,
            resolution,
        };
        let outcome = self.cast_user_operation(workspace, &action, ctx);
        if let Some(error) = outcome.error {
            self.casting.notify(error);
            return false;
        }
        self.casting.notice = None;
        true
    }

    fn stop_application_cast(&mut self) {
        for session in &mut self.casting.sessions {
            if matches!(session.source, CastSource::Application {}) {
                session.scaling = None;
                session.worker.stop();
            }
        }
        self.casting.pending_application_start = None;
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
    use horizon_core::{RuntimeState, StartupDecision};
    use std::{os::unix::fs::DirBuilderExt, sync::mpsc, time::Duration};

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

    fn toolbar_button() -> Rect {
        Rect::from_min_size(egui::pos2(1480.0, 8.0), egui::vec2(34.0, 30.0))
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
    fn a_click_starts_on_the_tv_of_the_last_whole_window_cast() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.11");
        app.casting.last_application_receiver = Some("living-room".into());
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(app.casting.picker.is_none(), "a remembered TV needs no picker");
        let session = app
            .casting
            .sessions
            .last()
            .expect("the click starts the cast of the remembered TV");
        assert_eq!(session.receiver_id, "living-room");
        assert!(matches!(session.source, CastSource::Application {}));
        assert!(app.application_cast_highlighted());
        await_newest_pairing(&app);
        app.casting.stop_and_wait(Duration::from_secs(5));
        peer.join().expect("synthetic receiver");
    }

    #[test]
    fn the_only_paired_tv_needs_no_picker_either() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "only-tv", "127.245.13.12");
        app.casting.paired_receivers.push(horizon_cast::PairedDevice {
            id: "only-tv".into(),
            name: "Synthetic only-tv".into(),
        });
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(app.casting.picker.is_none());
        assert_eq!(
            app.casting.sessions.last().map(|session| session.receiver_id.as_str()),
            Some("only-tv")
        );
        await_newest_pairing(&app);
        app.casting.stop_and_wait(Duration::from_secs(5));
        peer.join().expect("synthetic receiver");
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
    fn the_chooser_click_opens_the_picker_on_a_remembered_tv() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.14");
        app.casting.last_application_receiver = Some("living-room".into());
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, true);
        let picker = app.casting.picker.as_ref().expect("the chooser opens the picker");
        assert_eq!(picker.anchor, Anchor::Application);
        assert!(app.casting.sessions.is_empty(), "the chooser starts nothing");
        drop(peer);
    }

    #[test]
    fn a_click_waits_for_discovery_then_starts_on_the_remembered_tv() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        app.casting.last_application_receiver = Some("living-room".into());
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        assert!(app.casting.picker.is_some(), "the picker shows the wait");
        assert!(app.casting.pending_application_start.is_some());
        // Discovery ends and finds the remembered TV.
        app.casting.discovery = None;
        let peer = discovered_receiver(&mut app, "living-room", "127.245.13.15");
        app.resume_pending_application_cast(&ctx);
        assert!(app.casting.pending_application_start.is_none());
        assert!(app.casting.picker.is_none(), "the started cast closes the picker");
        assert_eq!(
            app.casting.sessions.last().map(|session| session.receiver_id.as_str()),
            Some("living-room")
        );
        await_newest_pairing(&app);
        app.casting.stop_and_wait(Duration::from_secs(5));
        peer.join().expect("synthetic receiver");
    }

    #[test]
    fn a_click_reports_a_tv_that_does_not_answer_discovery() {
        let (_temp, ctx, mut app) = app_with_one_workspace();
        app.casting.last_application_receiver = Some("gone-tv".into());
        pending_discovery(&mut app);
        app.toggle_application_cast(toolbar_button(), &ctx, false);
        app.casting.discovery = None;
        app.resume_pending_application_cast(&ctx);
        assert!(app.casting.pending_application_start.is_none());
        assert!(app.casting.sessions.is_empty(), "no TV means no session");
        assert!(
            app.casting
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("did not answer discovery")),
            "{:?}",
            app.casting.notice
        );
        assert!(app.casting.picker.is_some(), "the person can still choose a TV");
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
