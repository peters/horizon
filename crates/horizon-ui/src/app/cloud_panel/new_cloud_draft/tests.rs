//! What a dismissal does to the New cloud dialog: the draft stays, the workspace stays, and
//! its card brings the dialog back with everything the person had chosen.
use super::*;
use crate::app::sidebar::NewWorkspace;
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use egui::{Event, Modifiers, PointerButton, Pos2, epaint::Shape};
use horizon_core::{RuntimeState, StartupDecision};

fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>, modifiers: Modifiers) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = std::iter::once(Event::ModifiersChanged(modifiers))
        .chain(events)
        .collect();
    run_app_frame_with_input(ctx, app, input);
}

/// Frames until the worker threads that read and write the records of the active session
/// settle: the frames apply their answers.
fn wait_for_cloud_drafts(ctx: &egui::Context, app: &mut HorizonApp) {
    for _ in 0..200 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
        if app.cloud_drafts_settled() {
            return;
        }
    }
    panic!("the records of the active session are not read");
}

fn label_position(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.job.text == label => {
            Some(Rect::from_min_size(text.pos, text.galley.size()).center())
        }
        _ => None,
    })
}

fn click(ctx: &egui::Context, app: &mut HorizonApp, position: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(position)], Modifiers::NONE);
    for pressed in [true, false] {
        frame(
            ctx,
            app,
            vec![Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
    }
}

/// Parks the open draft the way the frame does: the workspace that holds it leaves the canvas,
/// so the surface cannot show it any more and its card becomes the way back.
fn park_off_the_canvas(ctx: &egui::Context, app: &mut HorizonApp) {
    app.canvas_view.set_pan_offset([-100_000.0, -100_000.0]);
    frame(ctx, app, Vec::new(), Modifiers::NONE);
    assert!(
        app.cloud_prototype.production.draft.is_some(),
        "the draft is parked in its workspace"
    );
}

/// An app that can start a cloud, with the workspace **+ New cloud** makes for one.
fn parked_draft(ctx: &egui::Context, app: &mut HorizonApp, temp: &std::path::Path, workspaces: usize) {
    app.cloud_prototype.root = Some(temp.join("clouds"));
    app.cloud_prototype.ready = true;
    for _ in 0..2 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
    app.create_new_workspace(ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
    assert!(app.cloud_creation_open());
    assert_eq!(
        app.board.workspaces.len(),
        workspaces + 1,
        "a workspace holds the draft"
    );
    app.cloud_prototype.production.set_draft_title("Atlas cloud");
    park_off_the_canvas(ctx, app);
    // Parking takes the view to the workspace that holds the card; its pan settles first.
    for _ in 0..20 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
}

fn ready_app() -> (tempfile::TempDir, egui::Context, HorizonApp, usize) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    // The board never removes its last workspace, so there is one already.
    let _existing = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let workspaces = app.board.workspaces.len();
    (temp, ctx, app, workspaces)
}

#[test]
fn a_parked_draft_stays_in_its_workspace_with_its_choices() {
    let (temp, ctx, mut app, workspaces) = ready_app();
    parked_draft(&ctx, &mut app, temp.path(), workspaces);
    assert!(!app.cloud_creation_open(), "the form is out of view");
    assert!(
        app.cloud_prototype.production.creating,
        "a parked draft is not a cancelled one"
    );
    assert_eq!(
        app.cloud_prototype.production.draft_card().0,
        "Atlas cloud",
        "what was typed stays with the draft"
    );
    assert_eq!(app.board.workspaces.len(), workspaces + 1, "its workspace stays");
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    for label in ["Atlas cloud", "Continue", "Cancel"] {
        assert!(
            label_position(&output, label).is_some(),
            "the parked draft's card offers {label}"
        );
    }
}

#[test]
fn continue_on_the_card_opens_the_dialog_again_with_what_it_kept() {
    let (temp, ctx, mut app, workspaces) = ready_app();
    parked_draft(&ctx, &mut app, temp.path(), workspaces);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));

    let continue_at = label_position(&output, "Continue").expect("the card offers Continue");
    click(&ctx, &mut app, continue_at);
    assert!(app.cloud_creation_open(), "Continue brings the dialog back");
    assert_eq!(
        app.cloud_prototype.production.draft_card().0,
        "Atlas cloud",
        "the dialog shows the draft it kept"
    );
    assert!(app.cloud_prototype.production.draft.is_none(), "no card is left behind");
    assert_eq!(app.board.workspaces.len(), workspaces + 1, "one workspace, not two");
}

#[test]
fn cancel_on_the_card_ends_the_draft_and_takes_its_empty_workspace() {
    let (temp, ctx, mut app, workspaces) = ready_app();
    parked_draft(&ctx, &mut app, temp.path(), workspaces);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel_at = label_position(&output, "Cancel").expect("the card offers Cancel");
    click(&ctx, &mut app, cancel_at);
    assert!(!app.cloud_prototype.production.creating, "Cancel ends the draft");
    assert!(app.cloud_prototype.production.draft.is_none(), "no card is left behind");
    assert_eq!(
        app.board.workspaces.len(),
        workspaces,
        "the workspace New cloud made goes with the draft"
    );
}

/// A parked draft, its workspace, and the folder its choices were saved in.
fn parked_draft_beside_a_root() -> (tempfile::TempDir, egui::Context, HorizonApp, String, std::path::PathBuf) {
    let (temp, ctx, mut app, _workspaces) = ready_app();
    let root = temp.path().join("clouds");
    app.cloud_prototype.use_root(Some(root.clone()));
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.cloud_prototype.production.set_draft_title("Atlas cloud");
    app.cloud_prototype
        .production
        .set_draft_repository("/synthetic/smoke/atlas-app");
    park_off_the_canvas(&ctx, &mut app);
    for _ in 0..20 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let workspace = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to the workspace + New cloud made");
    app.settle_cloud_drafts();
    (temp, ctx, app, workspace, root)
}

/// An app of the next start, whose board is the saved one and whose cloud root is `root`.
fn restarted_app(root: &std::path::Path, saved: RuntimeState) -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(saved),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.use_root(Some(root.to_path_buf()));
    wait_for_cloud_drafts(&ctx, &mut app);
    (temp, ctx, app)
}

/// The frame in which the card shows, once the start's pan has settled and the view is on the
/// workspace that holds it, as a saved view finds it.
fn card_frame(ctx: &egui::Context, app: &mut HorizonApp, workspace: &str) -> egui::FullOutput {
    let id = app
        .board
        .workspace_id_by_local_id(workspace)
        .expect("the card's workspace");
    assert!(app.focus_workspace_visible(ctx, id, false));
    let mut last = run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
    for _ in 0..60 {
        let placed = label_position(&last, "Continue");
        let next = run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
        // The start's pan moves the card; a click lands only on a frame that left it in place.
        if placed.is_some() && placed == label_position(&next, "Continue") {
            return next;
        }
        last = next;
    }
    panic!("the card of the saved draft does not show");
}

/// The board as the next start reads it, with a fresh view so that the card shows.
fn saved_board(app: &HorizonApp) -> RuntimeState {
    let mut saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    saved.canvas_view = None;
    saved
}

/// The choices the saved draft of `workspace` keeps, as `drafts.json` has them.
fn saved_draft(root: &std::path::Path, workspace: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(root.join("drafts.json")).ok()?;
    let saved: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    saved.get(workspace).cloned()
}

#[test]
fn a_parked_draft_keeps_its_choices_in_the_cloud_settings_folder() {
    let (_temp, _ctx, app, workspace, root) = parked_draft_beside_a_root();
    let saved = saved_draft(&root, &workspace).expect("the parked draft is saved beside the cloud settings");
    assert_eq!(saved["title"], "Atlas cloud");
    assert_eq!(saved["repository"], "/synthetic/smoke/atlas-app");
    // A clone, a token, a check, a sibling or a pending creation is never part of a choice.
    let mut fields: Vec<&str> = saved
        .as_object()
        .expect("one record")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "configuration",
            "new_workspace",
            "profile",
            "repository",
            "revision",
            "session",
            "title"
        ],
        "only choices, their owner, and where the draft sits are saved"
    );
    assert_eq!(saved["new_workspace"], true, "New workspace made that workspace");
    assert!(
        app.cloud_prototype.production.creating,
        "the draft still belongs to this session"
    );
}

#[test]
fn the_next_start_shows_the_card_and_continue_shows_the_choices() {
    let (_temp, _ctx, app, workspace, root) = parked_draft_beside_a_root();
    let saved = saved_board(&app);
    drop(app);

    let (_restart, ctx, mut app) = restarted_app(&root, saved);
    assert!(app.board.workspace_id_by_local_id(&workspace).is_some());
    assert!(app.cloud_prototype.production.draft.is_some(), "the card came back");
    assert!(
        !app.cloud_prototype.production.creating,
        "a restored draft rents nothing until Continue"
    );
    let output = card_frame(&ctx, &mut app, &workspace);
    for label in ["Atlas cloud", "atlas-app · nothing rented yet", "Continue", "Cancel"] {
        assert!(
            label_position(&output, label).is_some(),
            "the restored card shows {label}"
        );
    }

    let continue_at = label_position(&output, "Continue").expect("the card offers Continue");
    click(&ctx, &mut app, continue_at);
    assert!(app.cloud_creation_open(), "Continue brings the dialog back");
    assert_eq!(app.cloud_prototype.production.draft_card().0, "Atlas cloud");
    assert!(
        app.cloud_prototype.production.draft_card().1.starts_with("atlas-app"),
        "the repository came back with the title"
    );
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    assert!(
        label_position(&output, "Atlas cloud").is_some(),
        "the dialog shows the kept title"
    );
    app.settle_cloud_drafts();
    assert!(
        saved_draft(&root, &workspace).is_some(),
        "opening the dialog again keeps the record until the draft ends"
    );
}

#[test]
fn cancel_on_a_restored_card_takes_its_record_away() {
    let (_temp, _ctx, app, workspace, root) = parked_draft_beside_a_root();
    let saved = saved_board(&app);
    drop(app);

    let (_restart, ctx, mut app) = restarted_app(&root, saved);
    let output = card_frame(&ctx, &mut app, &workspace);
    let cancel_at = label_position(&output, "Cancel").expect("the card offers Cancel");
    click(&ctx, &mut app, cancel_at);
    assert!(!app.cloud_prototype.production.creating, "Cancel ends the draft");
    assert!(app.cloud_prototype.production.draft.is_none());
    app.settle_cloud_drafts();
    assert!(
        saved_draft(&root, &workspace).is_none(),
        "an ended draft keeps no record"
    );
}

#[test]
fn a_record_of_a_workspace_that_went_is_dropped() {
    let (_temp, _ctx, app, workspace, root) = parked_draft_beside_a_root();
    let mut app = app;
    let card = app
        .board
        .workspace_id_by_local_id(&workspace)
        .expect("the draft's workspace");
    assert!(
        app.board.remove_empty_workspace(card),
        "the person removed the workspace"
    );
    let saved = saved_board(&app);
    drop(app);

    let (_restart, _ctx, mut app) = restarted_app(&root, saved);
    assert!(
        app.cloud_prototype.production.draft.is_none(),
        "a card has no place on the board that came back"
    );
    app.settle_cloud_drafts();
    assert!(
        saved_draft(&root, &workspace).is_none(),
        "the record of the removed workspace is dropped"
    );
}

/// A persistent session whose board holds `name`'s one editor workspace.
fn persistent_session(app: &HorizonApp, name: &str) -> horizon_core::ResolvedSession {
    app.session_store
        .create_session_from_runtime(RuntimeState {
            workspaces: vec![editor_workspace_state(name, [0.0, 0.0])],
            ..RuntimeState::default()
        })
        .expect("a persistent session")
}

/// Switches to `session` and lets the switch, the board and the cloud state settle.
fn switch_to(ctx: &egui::Context, app: &mut HorizonApp, session: &horizon_core::ResolvedSession) {
    app.root_viewport_stabilizer = None;
    app.activate_runtime_session(ctx, session);
    for _ in 0..40 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
        if app.pending_session_switch.is_none() {
            break;
        }
    }
    assert!(app.pending_session_switch.is_none(), "the session switch settles");
    wait_for_cloud_drafts(ctx, app);
}

#[test]
fn a_draft_survives_a_switch_away_and_back() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let root = temp.path().join("clouds");
    app.cloud_prototype.use_root(Some(root.clone()));
    let session_a = persistent_session(&app, "atlas-session");
    app.activate_persistent_session(&session_a);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    // A draft parked in session A.
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.cloud_prototype.production.set_draft_title("Atlas cloud");
    park_off_the_canvas(&ctx, &mut app);
    for _ in 0..5 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let workspace = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to the workspace + New cloud made");
    app.settle_cloud_drafts();
    assert!(saved_draft(&root, &workspace).is_some(), "the parked draft is saved");

    // Session B shows no card of A, and A's record waits for A.
    let session_b = persistent_session(&app, "beta-session");
    switch_to(&ctx, &mut app, &session_b);
    assert!(
        app.cloud_prototype.production.draft.is_none(),
        "another session shows no card of the park it does not own"
    );
    assert!(
        saved_draft(&root, &workspace).is_some(),
        "the record of the other session stays"
    );

    // Back in session A, the card comes back with its choices.
    let resumed = app
        .session_store
        .resume_session(&session_a.session_id)
        .expect("session A");
    switch_to(&ctx, &mut app, &resumed);
    assert!(
        app.board.workspace_id_by_local_id(&workspace).is_some(),
        "its workspace came back"
    );
    assert_eq!(
        app.cloud_prototype.production.draft_card().0,
        "Atlas cloud",
        "the card kept its title"
    );
    let output = card_frame(&ctx, &mut app, &workspace);
    assert!(
        label_position(&output, "Atlas cloud").is_some(),
        "session A shows the card again"
    );
}

#[test]
fn a_frame_that_parks_a_draft_writes_no_file_and_the_worker_saves_it() {
    let (temp, ctx, mut app, _workspaces) = ready_app();
    let root = temp.path().join("clouds");
    app.cloud_prototype.use_root(Some(root.clone()));
    // The writer thread holds every write, so a frame that wrote a file would wait for it here.
    let release = app.hold_cloud_draft_saves();
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.cloud_prototype.production.set_draft_title("Atlas cloud");
    let workspace = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to the workspace + New cloud made");

    park_off_the_canvas(&ctx, &mut app);
    assert_eq!(
        app.cloud_draft_record(&workspace).map(|kept| kept.title.as_str()),
        Some("Atlas cloud"),
        "the frame keeps the record in memory"
    );
    assert!(
        saved_draft(&root, &workspace).is_none(),
        "the frame that parks a draft opens no file, because the write waits for the test"
    );

    release.send(()).expect("the writer thread holds the save");
    for _ in 0..200 {
        if saved_draft(&root, &workspace).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        saved_draft(&root, &workspace).map(|saved| saved["title"].clone()),
        Some(serde_json::json!("Atlas cloud")),
        "the worker writes the record that the frame kept"
    );
}

#[test]
fn a_restored_card_ends_on_a_switch_so_the_next_session_shows_its_own() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let root = temp.path().join("clouds");
    app.cloud_prototype.use_root(Some(root.clone()));
    let session_a = persistent_session(&app, "atlas-session");
    app.activate_persistent_session(&session_a);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    // Session A parks a draft.
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.cloud_prototype.production.set_draft_title("Atlas cloud");
    park_off_the_canvas(&ctx, &mut app);
    for _ in 0..5 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let atlas = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to the workspace + New cloud made");
    // Session B parks its own draft.
    let session_b = persistent_session(&app, "beta-session");
    switch_to(&ctx, &mut app, &session_b);
    assert!(
        app.cloud_prototype.production.draft.is_none(),
        "another session shows no card of the park it does not own"
    );
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.cloud_prototype.production.set_draft_title("Beta cloud");
    park_off_the_canvas(&ctx, &mut app);
    for _ in 0..5 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let beta = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to the workspace + New cloud made");
    // Back in session A, its card is restored, without Continue.
    let resumed = app
        .session_store
        .resume_session(&session_a.session_id)
        .expect("session A");
    switch_to(&ctx, &mut app, &resumed);
    assert!(
        app.cloud_prototype
            .production
            .draft
            .as_ref()
            .is_some_and(|draft| draft.restored),
        "session A shows the card its record brought back"
    );
    assert_eq!(app.cloud_prototype.production.draft_card().0, "Atlas cloud");

    // A restored card belongs to the session whose record it came from, so the switch to B ends
    // it and keeps its record, and B's own card comes back in its place.
    let resumed = app
        .session_store
        .resume_session(&session_b.session_id)
        .expect("session B");
    switch_to(&ctx, &mut app, &resumed);
    assert_eq!(
        app.cloud_prototype.production.draft_card().0,
        "Beta cloud",
        "session B shows its own card, not the one session A parked"
    );
    assert!(
        !app.cloud_prototype.production.creating,
        "the card comes back without Continue"
    );
    app.settle_cloud_drafts();
    assert!(
        saved_draft(&root, &atlas).is_some(),
        "the switch ends session A's card and keeps its record"
    );
    assert!(saved_draft(&root, &beta).is_some(), "session B's record stays");
    let output = card_frame(&ctx, &mut app, &beta);
    assert!(
        label_position(&output, "Beta cloud").is_some(),
        "session B's card is on its board"
    );
}
