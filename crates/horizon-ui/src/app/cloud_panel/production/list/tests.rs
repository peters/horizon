use super::super::park::tests::{member, ready_cloud, ready_cloud_in};
use super::super::{Runtime, Stage};
use super::*;
use crate::app::test_support::{raw_input, test_app};
use crate::test_egui::DiscardTextures;
use egui::{Event, PointerButton, Pos2, Rect, epaint::Shape};
use horizon_core::cloud_list::{Dot, Group, Row};
use horizon_core::cloud_runtime::github::requests::Request;
use horizon_core::cloud_runtime::session_status::SessionStatus;

fn runtime(app: &mut HorizonApp) -> &mut Runtime {
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap()
}

/// A ready cloud with its connection watch running, as when it is in use.
fn connected() -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = ready_cloud();
    // The sender is dropped: the watch reports nothing in these tests.
    runtime(&mut app).receiver = Some(std::sync::mpsc::channel().1);
    (temp, app)
}

fn row(app: &HorizonApp) -> Row {
    let workspace = app.board.panel(member(app, "one")).unwrap().workspace_id;
    app.read_sidebar_rows().remove(&workspace).unwrap()
}

/// The status of the session of the member `local`, keyed as Horizon keeps it.
fn session(local: &str, activity: SessionActivity, lines: &[&str]) -> SessionStatus {
    SessionStatus {
        id: local.to_owned(),
        activity,
        quiet_for: None,
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
    }
}

#[test]
fn an_attached_ready_cloud_is_in_the_cloud_group() {
    let (_temp, app) = connected();
    let attached = row(&app);
    assert_eq!(attached.group, Group::Cloud);
    assert_eq!(attached.dot, Dot::Idle);
    // Without its watch the cloud is disconnected, still in the cloud group.
    let (_temp, app) = ready_cloud();
    assert_eq!(row(&app).group, Group::Cloud);
}

#[test]
fn a_parked_cloud_shows_its_workers_last_line_and_whether_an_agent_works() {
    let (_temp, mut app) = connected();
    runtime(&mut app).parking.park_with(vec![
        session("one", SessionActivity::Working, &["● Running cargo test", "╰──╯"]),
        session("two", SessionActivity::Idle, &["$ ls"]),
    ]);
    let row = row(&app);
    assert_eq!(row.group, Group::Parked);
    assert_eq!(row.dot, Dot::Parked { working: true });
    assert_eq!(row.line, "Running cargo test");
}

#[test]
fn a_parked_cloud_whose_connection_is_down_stays_parked() {
    // Without its watch the card shows the cloud as disconnected.
    let (_temp, mut app) = ready_cloud();
    runtime(&mut app)
        .parking
        .park_with(vec![session("one", SessionActivity::Idle, &["synthetic output 12"])]);
    let row = row(&app);
    assert_eq!((row.group, row.dot), (Group::Parked, Dot::Parked { working: false }));
    assert_eq!(row.line, "synthetic output 12");
}

#[test]
fn a_parked_session_that_ended_needs_the_user() {
    let (_temp, mut app) = connected();
    runtime(&mut app).parking.park_with(vec![
        session("one", SessionActivity::Idle, &["waiting"]),
        session("two", SessionActivity::Exited(Some(1)), &[]),
    ]);
    let row = row(&app);
    assert_eq!(row.group, Group::NeedsYou);
    assert_eq!(row.dot, Dot::Attention);
    assert_eq!(row.line, "Ended with status 1");
}

#[test]
fn a_stopped_worker_is_parked_and_a_failure_needs_the_user() {
    let (_temp, mut app) = ready_cloud();
    let state = runtime(&mut app).state.as_mut().unwrap();
    state.stage = Stage::Stopped;
    runtime(&mut app).stage = Some(Stage::Stopped);
    let stopped = row(&app);
    assert_eq!((stopped.group, stopped.dot), (Group::Parked, Dot::Stopped));
    assert_eq!(stopped.hourly_rate, None, "a stopped worker bills no compute");

    let (_temp, mut app) = ready_cloud();
    runtime(&mut app).error = Some("Worker did not answer".to_owned());
    let failed = row(&app);
    assert_eq!((failed.group, failed.dot), (Group::NeedsYou, Dot::Failed));
    assert!(failed.line.contains("Worker did not answer"), "{}", failed.line);
}

#[test]
fn an_agent_waiting_for_github_access_needs_the_user() {
    let (_temp, mut app) = ready_cloud();
    runtime(&mut app).github_requests.list.push(Request {
        id: "1".to_owned(),
        repository: "example/project".to_owned(),
        access: "push".to_owned(),
        reason: "open a pull request".to_owned(),
        session: String::new(),
        agent: "claude".to_owned(),
    });
    let row = row(&app);
    assert_eq!(row.group, Group::NeedsYou);
    assert_eq!(row.line, "GitHub push access requested for example/project");
}

#[test]
fn a_cloud_without_a_runtime_is_not_deployed_and_a_plain_workspace_is_on_this_pc() {
    let (_temp, mut app) = ready_cloud();
    app.cloud_prototype.production.runtimes.clear();
    let row = row(&app);
    assert_eq!((row.group, row.line.as_str()), (Group::Cloud, "Not deployed"));
    let local = app.board.create_workspace("local");
    assert_eq!(app.read_sidebar_rows()[&local].group, Group::ThisPc);
}

/// Where each text of `output` is drawn.
fn texts(output: &egui::FullOutput) -> Vec<(String, Pos2)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) => Some((
                text.galley.job.text.clone(),
                Rect::from_min_size(text.pos, text.galley.size()).center(),
            )),
            _ => None,
        })
        .collect()
}

fn position(texts: &[(String, Pos2)], label: &str) -> Pos2 {
    texts
        .iter()
        .find(|(text, _)| text == label)
        .map_or_else(|| panic!("{label} is not drawn"), |(_, position)| *position)
}

/// Draws only the sidebar, with `events`.
fn sidebar(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>) -> egui::FullOutput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = events;
    ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
}

#[test]
fn the_sidebar_groups_workspaces_and_a_click_attaches_a_parked_cloud() {
    let (temp, app) = test_app();
    let (_temp, mut app) = ready_cloud_in(temp, app, [40_000.0, 40_000.0]);
    app.template_config.features.sidebar_accordion = true;
    let parking = &mut runtime(&mut app).parking;
    parking.park_with(vec![session(
        "one",
        SessionActivity::Working,
        &["● Running cargo test"],
    )]);
    runtime(&mut app).receiver = Some(std::sync::mpsc::channel().1);
    runtime(&mut app).needs_attach = false;
    let local = app.board.create_workspace("Local");
    app.board.active_workspace = Some(local);
    app.board.focused = None;
    let ctx = egui::Context::default();
    // An area shows from its second frame on.
    sidebar(&ctx, &mut app, Vec::new());
    let texts = texts(&sidebar(&ctx, &mut app, Vec::new()));
    let (parked, cloud, this_pc, local_row) = (
        position(&texts, "PARKED"),
        position(&texts, "Fixture"),
        position(&texts, "THIS PC"),
        position(&texts, "Local"),
    );
    assert!(parked.y < cloud.y && cloud.y < this_pc.y && this_pc.y < local_row.y);
    // Each summary is on the line of its header, at the right.
    for (header, summary) in [(parked, "no local cost"), (this_pc, "live")] {
        let at = position(&texts, summary);
        assert!((at.y - header.y).abs() < 2.0 && at.x > header.x, "{summary}");
    }
    assert!(
        texts.iter().all(|(text, _)| text != "Running cargo test"),
        "a parked row is compact"
    );

    for pressed in [true, false] {
        let click = Event::PointerButton {
            pos: cloud,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        sidebar(&ctx, &mut app, vec![Event::PointerMoved(cloud), click]);
    }
    let workspace = app.board.panel(member(&app, "one")).unwrap().workspace_id;
    assert_eq!(app.board.active_workspace, Some(workspace));
    app.sync_cloud_parking();
    assert!(
        !runtime(&mut app).parking.is_parked(),
        "the cloud attaches when it is in use"
    );
}
