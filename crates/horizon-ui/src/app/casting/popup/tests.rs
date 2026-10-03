use super::*;
use crate::app::casting::{
    Session,
    requests::tests::{await_pairing, synthetic_pairing_receiver},
};
use horizon_cast::{CastSession, CastStatus};
use horizon_core::browser::manifest::cast::{CastOrientation, CastResolution, CastSource};
use std::time::Duration;

fn session(receiver: &str, workspace: u64, ip: &str) -> (Session, std::thread::JoinHandle<()>) {
    let (address, peer) = synthetic_pairing_receiver(ip);
    let worker = CastSession::start(address, horizon_cast::VideoFormat::default()).expect("isolated worker");
    await_pairing(&worker);
    (
        Session {
            generation: Instant::now(),
            receiver_id: receiver.into(),
            workspace: WorkspaceId(workspace),
            source: CastSource::Application {},
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
            worker,
            scaling: None,
            failure_notified: false,
            agent_controlled: false,
        },
        peer,
    )
}
fn picker(session: &Session) -> Picker {
    Picker {
        anchor: horizon_core::PanelId(1),
        workspace: session.workspace,
        source: session.source.clone(),
        receiver: Some(session.receiver_id.clone()),
        orientation: session.orientation,
        resolution: session.resolution,
        pin: zeroize::Zeroizing::new(String::new()),
        position: None,
        binding: Some(SessionBinding::from_session(session)),
    }
}

#[test]
fn close_stops_only_bound_tv_generation_in_its_workspace() {
    let ctx = Context::default();
    let mut state = CastState::default();
    let mut peers = Vec::new();
    for (receiver, workspace, ip) in [
        ("first-tv", 1, "127.245.12.1"),
        ("other-tv", 1, "127.245.12.2"),
        ("first-tv", 2, "127.245.12.3"),
    ] {
        let (session, peer) = session(receiver, workspace, ip);
        state.sessions.push(session);
        peers.push(peer);
    }
    state.picker = Some(picker(&state.sessions[0]));
    // An externally replaced session on the same TV cannot be stopped by old controls.
    state.sessions[0].generation += Duration::from_nanos(1);
    state.close_picker(&ctx);
    assert!(
        state
            .sessions
            .iter()
            .all(|session| session.worker.status() == CastStatus::PinRequired)
    );
    state.picker = Some(picker(&state.sessions[0]));
    state.close_picker(&ctx);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !state.sessions[0].worker.finished() {
        assert!(Instant::now() < deadline, "bound worker must finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        state.sessions[1..]
            .iter()
            .all(|session| session.worker.status() == CastStatus::PinRequired)
    );
    assert!(state.stop_and_wait(Duration::from_secs(3)));
    for peer in peers {
        peer.join().expect("receiver closed normally");
    }
}

#[test]
fn placement_avoids_source_or_uses_bounded_movable_fallback() {
    let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0));
    let size = egui::vec2(300.0, 380.0);
    let ctx = Context::default();
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let shadow = control_shadow_margin(&ctx);
    for source in [
        Rect::from_min_size(egui::pos2(400.0, 140.0), egui::vec2(400.0, 300.0)),
        canvas,
    ] {
        let popup = Rect::from_min_size(initial_position(canvas, source, size, shadow), size);
        assert!(canvas.contains_rect(popup));
        if source != canvas {
            assert!(!source.intersects(popup + shadow));
        }
    }
}

#[test]
fn whole_window_dismissal_preserves_the_worker_until_explicit_stop() {
    let ctx = Context::default();
    let mut state = CastState::default();
    let (session, peer) = session("application-tv", 1, "127.245.12.4");
    state.picker = Some(picker(&session));
    state.sessions.push(session);
    state.dismiss_picker(&ctx);
    assert!(state.picker.is_none());
    assert_eq!(state.sessions[0].worker.status(), CastStatus::PinRequired);
    assert!(state.stop_and_wait(Duration::from_secs(3)));
    peer.join().expect("receiver normal teardown");
}

#[test]
fn successful_application_start_or_pair_hides_controls_but_other_sources_keep_them() {
    let start = CastOperation::Start {
        receiver_id: "synthetic-tv".into(),
        source: CastSource::Application {},
        orientation: CastOrientation::Landscape,
        resolution: CastResolution::default(),
    };
    let pair = CastOperation::Pair {
        receiver_id: "synthetic-tv".into(),
        pin: "1234".into(),
    };
    let stop = CastOperation::Stop {
        receiver_id: "synthetic-tv".into(),
    };
    for source in [
        CastSource::Application {},
        CastSource::Panel {
            id: "synthetic-panel".into(),
        },
        CastSource::Workspace {
            id: "synthetic-workspace".into(),
        },
    ] {
        let application = matches!(source, CastSource::Application {});
        assert_eq!(hide_after_action(&source, &start, true), application);
        assert_eq!(hide_after_action(&source, &pair, false), application);
        assert!(
            !hide_after_action(&source, &start, false),
            "unpaired start must retain PIN controls"
        );
        assert!(!hide_after_action(&source, &stop, true));
    }
}
