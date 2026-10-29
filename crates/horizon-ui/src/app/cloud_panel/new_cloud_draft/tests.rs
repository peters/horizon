//! What a dismissal does to the New cloud dialog: the draft stays, the workspace stays, and
//! its card brings the dialog back with everything the person had chosen.
use super::*;
use crate::app::sidebar::NewWorkspace;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, epaint::Shape};
use horizon_core::{RuntimeState, StartupDecision};

fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>, modifiers: Modifiers) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = std::iter::once(Event::ModifiersChanged(modifiers))
        .chain(events)
        .collect();
    run_app_frame_with_input(ctx, app, input);
}

fn key(ctx: &egui::Context, app: &mut HorizonApp, key: Key) {
    for pressed in [true, false] {
        frame(
            ctx,
            app,
            vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
    }
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
    key(ctx, app, Key::Escape);
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
fn escape_parks_the_draft_in_its_workspace_with_its_choices() {
    let (temp, ctx, mut app, workspaces) = ready_app();
    parked_draft(&ctx, &mut app, temp.path(), workspaces);
    assert!(!app.cloud_creation_open(), "the dialog is out of view");
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

#[test]
fn a_backdrop_click_parks_instead_of_ending_the_draft() {
    let (temp, ctx, mut app, workspaces) = ready_app();
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    // A control of the board, well outside the dialog.
    let outside = label_position(&output, "Menu").expect("the toolbar's Menu");
    click(&ctx, &mut app, outside);
    assert!(!app.cloud_creation_open(), "a click outside dismisses the dialog");
    assert!(app.cloud_prototype.production.creating, "and parks its draft");
    assert_eq!(app.board.workspaces.len(), workspaces + 1, "its workspace stays");
}
