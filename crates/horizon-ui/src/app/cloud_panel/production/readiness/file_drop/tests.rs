use super::*;
use crate::app::cloud_panel::production::park::tests::{member, ready_cloud};
use crate::app::test_support::dropped_file;
use egui::ViewportId;
use horizon_core::cloud_runtime::Stage;

/// The fixture cloud with its members attached to their worker sessions.
fn attached_cloud() -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = ready_cloud();
    app.board.focused = None;
    let one = member(&app, "one");
    app.panel_screen_rects
        .insert(one, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0)));
    app.sync_cloud_presentations();
    assert!(app.board.panel(one).unwrap().cloud_wait().is_none());
    (temp, app)
}

fn set_worker_id(app: &mut HorizonApp, id: &str) {
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    runtime.state.as_mut().unwrap().worker.as_mut().unwrap().id = id.into();
}

fn upload(temp: &tempfile::TempDir) -> egui::DroppedFileHandle {
    let path = temp.path().join("screen shot.png");
    std::fs::write(&path, b"\x89PNG").unwrap();
    dropped_file(path)
}

#[test]
fn a_drop_on_a_cloud_member_goes_to_its_session_on_the_bound_worker() {
    let (_temp, mut app) = attached_cloud();
    let one = member(&app, "one");
    let target = app.cloud_file_drop_target(one).expect("a cloud member");
    assert!(target.label.ends_with(" in Cloud"), "{}", target.label);
    let drop = target.drop.unwrap();
    assert_eq!((drop.cloud, drop.destination.session()), (1, "one"));
    assert_eq!(drop.destination.worker_id(), "fixture");

    let group = &mut app.cloud_prototype.groups.0[0];
    group.panels.retain(|local| local != "one");
    assert!(
        app.cloud_file_drop_target(one).is_none(),
        "a panel outside every cloud stays local"
    );
}

#[test]
fn a_cloud_that_cannot_take_files_says_why_instead_of_falling_back() {
    let (temp, mut app) = ready_cloud();
    let two = member(&app, "two");
    assert!(app.board.panel(two).unwrap().cloud_wait().is_some());
    let reason = app.cloud_file_drop_target(two).unwrap().drop.unwrap_err();
    assert!(reason.contains("not attached to its session on Cloud yet"), "{reason}");

    assert!(app.maybe_start_ssh_file_drop(two, &[upload(&temp)], ViewportId::ROOT));
    let flow = app.ssh_upload_flow.as_ref().expect("the reason is shown");
    assert!(flow.test_summary().starts_with("Failed: This panel is not attached"));

    let (_temp, mut app) = attached_cloud();
    let one = member(&app, "one");
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    runtime.state_unavailable = true;
    let reason = app.cloud_file_drop_target(one).unwrap().drop.unwrap_err();
    assert!(reason.starts_with("Cloud is not connected"), "{reason}");
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    runtime.state_unavailable = false;
    runtime.stage = Some(Stage::Stopped);
    let reason = app.cloud_file_drop_target(one).unwrap().drop.unwrap_err();
    assert!(reason.starts_with("Cloud is stopped. Resume the worker"), "{reason}");
}

#[test]
fn worker_paths_are_inserted_only_into_the_same_session_on_the_same_worker() {
    let (temp, mut app) = attached_cloud();
    let one = member(&app, "one");
    let drop = app.cloud_file_drop_target(one).unwrap().drop.unwrap();
    assert_eq!(app.cloud_file_drop_recipient(&drop), Some(one));

    let paths = vec!["/workspace/.horizon-drops/one/0123456789ab/screen shot.png".to_string()];
    assert!(app.maybe_start_ssh_file_drop(one, &[upload(&temp)], ViewportId::ROOT));
    app.ssh_upload_flow.as_mut().unwrap().test_finish(Ok(paths.clone()));
    app.poll_ssh_upload_flow();
    assert!(app.ssh_upload_flow.is_none(), "the paths went to the panel");

    assert!(app.maybe_start_ssh_file_drop(one, &[upload(&temp)], ViewportId::ROOT));
    set_worker_id(&mut app, "replacement");
    assert_eq!(app.cloud_file_drop_recipient(&drop), None);
    app.ssh_upload_flow.as_mut().unwrap().test_finish(Ok(paths.clone()));
    app.poll_ssh_upload_flow();
    let summary = app.ssh_upload_flow.as_ref().unwrap().test_summary();
    assert!(summary.starts_with("Finished: The panel of this session"), "{summary}");
    assert!(summary.ends_with(&paths[0]), "the person still gets the worker path");

    set_worker_id(&mut app, "fixture");
    app.cloud_prototype.groups.0[0].panels.retain(|local| local != "one");
    assert_eq!(app.cloud_file_drop_recipient(&drop), None);
}

#[test]
fn a_failed_cloud_upload_shows_the_failure_and_inserts_nothing() {
    let (temp, mut app) = attached_cloud();
    let one = member(&app, "one");
    assert!(app.maybe_start_ssh_file_drop(one, &[upload(&temp)], ViewportId::ROOT));
    app.ssh_upload_flow
        .as_mut()
        .unwrap()
        .test_finish(Err("Host key verification failed.".into()));
    app.poll_ssh_upload_flow();
    assert_eq!(
        app.ssh_upload_flow.as_ref().unwrap().test_summary(),
        "Failed: Host key verification failed."
    );
}

#[test]
fn finished_paths_keep_a_focus_the_person_moved_during_the_upload() {
    let (temp, mut app) = attached_cloud();
    let (one, two) = (member(&app, "one"), member(&app, "two"));
    let paths = vec!["/workspace/.horizon-drops/one/0123456789ab/notes.txt".to_string()];
    assert!(app.maybe_start_ssh_file_drop(one, &[upload(&temp)], ViewportId::ROOT));
    assert_eq!(app.board.focused, Some(one), "a drop focuses its panel");
    app.board.focus(two);
    app.ssh_upload_flow.as_mut().unwrap().test_finish(Ok(paths));
    app.poll_ssh_upload_flow();
    assert!(app.ssh_upload_flow.is_none(), "the paths went to the panel");
    assert_eq!(app.board.focused, Some(two));
}
