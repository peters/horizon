use super::*;
use egui::{PointerButton, Pos2, Rect, epaint::Shape};

fn label_position(output: &egui::FullOutput, label: &str) -> Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing visible label: {label}"))
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

#[test]
fn reopening_keeps_cloud_controls_below_the_modal() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    let workspace = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let new_cloud = label_position(&output, "New cloud");
    let fit = label_position(&output, "Fit all");
    app.cloud_prototype.groups.0.push(CloudGroup::new(
        1,
        "Existing cloud".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().join("repository"),
        [4000.0, 4000.0],
    ));
    app.canvas_view.pan_offset[0] += 500.0;
    let original_view = app.canvas_view;
    app.cloud_overview(&ctx);
    assert_ne!(app.canvas_view, original_view, "Fit all must move this fixture");
    app.canvas_view = original_view;
    for _ in 0..2 {
        click(&ctx, &mut app, new_cloud);
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
        assert!(app.cloud_creation_open(), "toolbar click must open the dialog");
        assert_eq!(ctx.layer_id_at(fit).unwrap().id, Id::new("cloud-creation"));
        for _ in 0..16 {
            key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
            if let Some(response) = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id)) {
                assert_eq!(response.layer_id.id, Id::new("cloud-creation"));
            }
        }
        let view = app.canvas_view;
        click(&ctx, &mut app, fit);
        assert_eq!(app.canvas_view, view, "backdrop click must not fit the canvas");
        assert!(!app.cloud_creation_open(), "backdrop click dismisses the dialog");
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
}
