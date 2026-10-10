use super::super::park::tests::ready_cloud;
use super::super::{Runtime, Stage};
use super::*;
use horizon_core::cloud_runtime::session_status::{SessionActivity, SessionStatus};

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
    crate::test_egui::accesskit_texts(|ui| app.render_idle_stop_confirmation(&ui.ctx().clone()))
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
    app.cloud_prototype.production.bulk_stop.unchecked.insert(1);
    let texts = dialog(&mut app);
    assert!(texts.iter().any(|entry| entry == &("Stop 0 workers".to_owned(), true)), "{texts:?}");
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
    runtime(&mut app).parking.park_with(vec![session(SessionActivity::Idle)]);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(2));
    assert_eq!(app.sidebar_idle_clouds(Group::Parked).len(), 1, "a parked idle worker still bills");
    runtime(&mut app).parking.park_with(vec![session(SessionActivity::Working)]);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(4));
    assert!(app.sidebar_idle_clouds(Group::Parked).is_empty(), "an agent works on the worker");
    app.request_idle_stop(Group::Parked);
    assert!(!app.idle_stop_open(), "nothing to choose from");

    let (_temp, mut app) = connected(Some(0.5));
    runtime(&mut app).stage = Some(Stage::Stopped);
    app.refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(2));
    assert!(app.sidebar_idle_clouds(Group::Cloud).is_empty());
    assert!(app.sidebar_idle_clouds(Group::Parked).is_empty());
}

#[test]
fn a_confirmed_stop_skips_a_cloud_that_is_no_longer_idle() {
    let (_temp, mut app) = connected(Some(0.5));
    app.request_idle_stop(Group::Cloud);
    assert!(app.cloud_can_stop_now(1));
    runtime(&mut app).stage = Some(Stage::Stopping);
    assert!(!app.cloud_can_stop_now(1));
}
