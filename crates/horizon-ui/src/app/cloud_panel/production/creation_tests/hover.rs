//! Hover text inside the New cloud dialog is drawn over the dialog.
use super::local_profiles::{finish_read, fixture};
use super::*;
use horizon_core::cloud_runtime::repository::launch::Configuration;

/// The hover text of the row that leaves quick start, as `repository_setup` words it.
const HOVER: &str = "Read the repository's own settings, from the working copy on the checked-out commit.";

/// Where in the paint order the text `text` is drawn, the last shape being the top one.
fn paint_position(output: &egui::FullOutput, text: &str) -> Option<usize> {
    output
        .shapes
        .iter()
        .position(|shape| matches!(&shape.shape, Shape::Text(text_shape) if text_shape.galley.job.text == text))
}

/// The New cloud dialog for a repository on quick start, in a window tall enough to show
/// the row under the title without scrolling.
fn quick_start_dialog(temp: &tempfile::TempDir, ctx: &egui::Context, app: &mut HorizonApp) {
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    fixture(temp.path());
    std::fs::remove_dir_all(temp.path().join(".horizon")).unwrap();
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.cloud_prototype.production.title = "Quick start".into();
    app.cloud_prototype.production.launch.configuration = Configuration::QuickStart;
    app.read_cloud_profiles(ctx);
    finish_read(ctx, app);
    // The dialog takes its height and its place over the first frames.
    for _ in 0..6 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
}

#[test]
fn the_hover_text_of_a_control_in_the_new_cloud_dialog_is_drawn_over_the_dialog() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    quick_start_dialog(&temp, &ctx, &mut app);
    ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let control = label_position(&output, "Use .horizon/cloud.yml");
    // A tooltip shows on the frame after the one that reports the hover.
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(control)], Modifiers::NONE);
    }
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = vec![Event::PointerMoved(control)];
    let output = run_app_frame_with_input(&ctx, &mut app, input);

    let label = paint_position(&output, "Use .horizon/cloud.yml").expect("the row under the title");
    let hover = paint_position(&output, HOVER).expect("the hover text of the row");
    assert!(
        hover > label,
        "the hover text draws over the dialog: hover at {hover}, its row at {label}"
    );
}
