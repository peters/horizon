use super::super::park::tests::ready_cloud;
use super::super::{Runtime, Stage};
use super::*;
use horizon_core::cloud_runtime::session_status::{SessionActivity, SessionStatus};
use std::time::{Duration, Instant};

fn runtime(app: &mut HorizonApp) -> &mut Runtime {
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap()
}

/// A ready cloud with its connection watch running, whose worker bills `rate` each hour.
fn connected(rate: Option<f64>) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = ready_cloud();
    // The sender is dropped: the watch reports nothing in these tests.
    runtime(&mut app).receiver = Some(std::sync::mpsc::channel().1);
    let worker = runtime(&mut app).state.as_mut().unwrap().worker.as_mut().unwrap();
    worker.cost_per_hr = rate;
    app.refresh_sidebar_rows(std::time::Instant::now());
    (temp, app)
}

/// What a screen reader reads in the bulk stop dialog of `app`.
fn dialog(app: &mut HorizonApp) -> Vec<(String, bool)> {
    crate::test_egui::accesskit_texts_after_sizing(|ui| app.render_idle_stop_confirmation(&ui.ctx().clone()))
}

#[test]
fn an_idle_ready_cloud_is_offered_with_what_its_stop_saves() {
    let (_temp, mut app) = connected(Some(0.0121));
    let idle = app.sidebar_idle_clouds(Group::Cloud);
    assert_eq!(idle.len(), 1);
    assert_eq!((idle[0].cloud.id, idle[0].workspace.as_str()), (1, "Fixture"));
    assert!(app.sidebar_idle_clouds(Group::Parked).is_empty());

    app.request_idle_stop(Group::Cloud);
    assert!(app.idle_stop_open());
    let texts = dialog(&mut app);
    let has = |text: &str, disabled: bool| texts.iter().any(|entry| entry == &(text.to_owned(), disabled));
    assert!(has("Cloud", false), "{texts:?}");
    assert!(has("Saves $0.012/h", false), "{texts:?}");
    assert!(has("Stop 1 worker", false), "{texts:?}");

    // Taking the only cloud out leaves nothing to stop.
    app.cloud_prototype
        .production
        .bulk_stop
        .dialog
        .as_mut()
        .unwrap()
        .unchecked
        .insert(1);
    let texts = dialog(&mut app);
    assert!(
        texts.iter().any(|entry| entry == &("Stop 0 workers".to_owned(), true)),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|(text, _)| text.starts_with("Saves")), "{texts:?}");
}

#[test]
fn a_parked_cloud_is_offered_unless_an_agent_works_or_it_cannot_stop() {
    let (_temp, mut app) = connected(Some(0.5));
    let session = |activity| SessionStatus {
        id: "one".to_owned(),
        activity,
        quiet_for: None,
        lines: vec!["synthetic output 12".to_owned()],
    };
    runtime(&mut app)
        .parking
        .park_with(vec![session(SessionActivity::Idle)]);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(2));
    assert_eq!(
        app.sidebar_idle_clouds(Group::Parked).len(),
        1,
        "a parked idle worker still bills"
    );
    runtime(&mut app)
        .parking
        .park_with(vec![session(SessionActivity::Working)]);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(4));
    assert!(
        app.sidebar_idle_clouds(Group::Parked).is_empty(),
        "an agent works on the worker"
    );
    app.request_idle_stop(Group::Parked);
    assert!(!app.idle_stop_open(), "nothing to choose from");

    let (_temp, mut app) = connected(Some(0.5));
    runtime(&mut app).stage = Some(Stage::Stopped);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(2));
    assert!(app.sidebar_idle_clouds(Group::Cloud).is_empty());
    assert!(app.sidebar_idle_clouds(Group::Parked).is_empty());
}

#[test]
fn a_confirmed_stop_stops_the_chosen_clouds_that_are_still_idle() {
    let ctx = egui::Context::default();
    let (_temp, mut app) = connected(Some(0.5));
    app.request_idle_stop(Group::Cloud);
    let mut dialog = app.cloud_prototype.production.bulk_stop.dialog.take().unwrap();
    dialog.unchecked.insert(1);
    app.stop_chosen(&dialog, &ctx, Instant::now());
    assert_eq!(runtime(&mut app).operation, None, "a cloud taken out keeps running");

    dialog.unchecked.clear();
    app.stop_chosen(&dialog, &ctx, Instant::now());
    assert_eq!(runtime(&mut app).operation, Some(Action::Stop));
    assert_eq!(runtime(&mut app).stage, Some(Stage::Stopping));

    // A cloud that is busy by the time of the confirmation keeps what it does.
    let (_temp, mut app) = connected(Some(0.5));
    app.request_idle_stop(Group::Cloud);
    let dialog = app.cloud_prototype.production.bulk_stop.dialog.take().unwrap();
    runtime(&mut app).stage = Some(Stage::Provision);
    app.stop_chosen(&dialog, &ctx, Instant::now());
    assert_eq!(runtime(&mut app).operation, None);
}

/// The session `one` of a parked cloud, doing `activity`.
fn session(activity: SessionActivity) -> Vec<SessionStatus> {
    vec![SessionStatus {
        id: "one".to_owned(),
        activity,
        quiet_for: None,
        lines: Vec::new(),
    }]
}

/// A parked idle cloud whose stop the user confirmed at `confirmed`.
fn confirmed_parked_stop(ctx: &egui::Context, confirmed: Instant) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = connected(Some(0.5));
    runtime(&mut app).parking.park_with(session(SessionActivity::Idle));
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(2));
    app.request_idle_stop(Group::Parked);
    let dialog = app.cloud_prototype.production.bulk_stop.dialog.take().unwrap();
    app.stop_chosen(&dialog, ctx, confirmed);
    (temp, app)
}

#[test]
fn a_parked_cloud_stops_only_when_a_read_after_the_confirmation_shows_it_idle() {
    let ctx = egui::Context::default();
    let confirmed = Instant::now();
    let (_temp, mut app) = confirmed_parked_stop(&ctx, confirmed);
    app.finish_waiting_stops(&ctx);
    assert_eq!(
        runtime(&mut app).operation,
        None,
        "the last read is older than the confirmation"
    );
    assert_eq!(app.cloud_prototype.production.bulk_stop.waiting.len(), 1);
    runtime(&mut app)
        .parking
        .read_with(session(SessionActivity::Idle), confirmed);
    app.finish_waiting_stops(&ctx);
    assert_eq!(runtime(&mut app).operation, Some(Action::Stop));
    assert!(app.cloud_prototype.production.bulk_stop.waiting.is_empty());

    // An agent that started to work since the last read keeps its worker.
    let (_temp, mut app) = confirmed_parked_stop(&ctx, confirmed);
    runtime(&mut app)
        .parking
        .read_with(session(SessionActivity::Working), confirmed);
    app.finish_waiting_stops(&ctx);
    assert_eq!(runtime(&mut app).operation, None);
    assert!(app.cloud_prototype.production.bulk_stop.waiting.is_empty());

    // Without a new read in time, the cloud keeps running.
    let long_ago = Instant::now()
        .checked_sub(STATUS_WAIT + Duration::from_secs(1))
        .unwrap();
    let (_temp, mut app) = confirmed_parked_stop(&ctx, long_ago);
    app.finish_waiting_stops(&ctx);
    assert_eq!(runtime(&mut app).operation, None);
    assert!(app.cloud_prototype.production.bulk_stop.waiting.is_empty());
}

#[test]
fn a_parked_cloud_is_offered_only_when_a_read_covers_each_parked_terminal() {
    let (_temp, mut app) = connected(Some(0.5));
    for local in ["one", "two"] {
        let id = app.board.panel_id_by_local_id(local).unwrap();
        app.board.panel_mut(id).unwrap().park_cloud().unwrap();
    }
    let idle = |id: &str| SessionStatus {
        id: id.to_owned(),
        activity: SessionActivity::Idle,
        quiet_for: None,
        lines: Vec::new(),
    };
    runtime(&mut app).parking.park_with(vec![idle("one")]);
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(2));
    assert!(
        app.sidebar_idle_clouds(Group::Parked).is_empty(),
        "nothing shows yet that no agent works in two"
    );
    runtime(&mut app).parking.park_with(vec![idle("one"), idle("two")]);
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(4));
    assert_eq!(app.sidebar_idle_clouds(Group::Parked).len(), 1);
}

#[test]
fn a_parked_cloud_is_not_offered_while_a_terminal_reports_work_for_itself() {
    let (_temp, mut app) = connected(Some(0.5));
    let one = app.board.panel_id_by_local_id("one").unwrap();
    let panel = app.board.panel_mut(one).unwrap();
    panel.park_cloud().unwrap();
    // The terminal's own status says that an agent works; the worker's last read does not.
    panel.set_parked_agent_status(horizon_core::AgentStatus::Working);
    let idle = |id: &str| SessionStatus {
        id: id.to_owned(),
        activity: SessionActivity::Idle,
        quiet_for: None,
        lines: Vec::new(),
    };
    runtime(&mut app).parking.park_with(vec![idle("one"), idle("two")]);
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(2));
    assert!(app.sidebar_idle_clouds(Group::Parked).is_empty());
}
