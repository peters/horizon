use super::*;
#[test]
fn a_closing_cloud_shows_its_disposal_over_its_panels() {
    let mut group = CloudGroup::new(101, "test".into(), "ws".into(), ".".into(), [0.0, 0.0]);
    assert!(
        !view::body_visible(&group, true),
        "a local cloud has no disposal to show"
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "test".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: serde_json::from_value(serde_json::json!({
            "provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8,
        }))
        .unwrap(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    assert!(view::body_visible(&group, false), "an empty cloud shows its steps");
    group.panels.push("member".into());
    assert!(
        !view::body_visible(&group, false),
        "its panels take the place of the steps"
    );
    assert!(
        view::body_visible(&group, true),
        "a cloud being closed shows its disposal"
    );
    group.collapsed = true;
    assert!(!view::body_visible(&group, true), "a collapsed cloud shows nothing");
}

#[test]
fn the_card_shows_the_disposal_steps_and_says_it_closes_by_itself() {
    use super::scrolling::{frame, label_pos, verbose_card};
    use horizon_core::cloud_runtime::Cancellation;
    const HINT: &str =
        "Closing this cloud. Its worker and storage are being deleted, and the card closes once they are gone.";
    let (_temp, ctx, mut app) = verbose_card();
    // The cloud holds a panel, as it does until a close shows its disposal instead.
    app.cloud_prototype.groups.0[0].panels.push("member".into());
    let (_sender, receiver) = std::sync::mpsc::channel();
    {
        let runtime = app.cloud_prototype.production.runtimes.entry(901).or_default();
        runtime.receiver = Some(receiver);
        runtime.cancel = Some(Cancellation::default());
        runtime.stage = Some(Stage::DeleteWorker);
        runtime.progress.stage(Stage::DeleteWorker, std::time::Instant::now());
    }
    let mut output = frame(&ctx, &mut app, 0.0, Pos2::ZERO, 0.0);
    for step in 1..5 {
        output = frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    assert!(
        label_pos(&output, "Steps").is_none(),
        "its panels own the body until it closes"
    );

    app.cloud_prototype.production.close.start_closing(901);
    for step in 5..10 {
        output = frame(&ctx, &mut app, f64::from(step) * 0.02, Pos2::ZERO, 0.0);
    }
    assert!(
        label_pos(&output, "Steps").is_some(),
        "the disposal steps take the body"
    );
    assert!(label_pos(&output, "Output").is_some(), "with their output");
    assert!(
        label_pos(&output, HINT).is_some(),
        "and the card says it closes by itself"
    );
    for step in ["Release hosted devices", "Delete worker", "Delete workspace storage"] {
        assert!(
            output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::Shape::Text(text) if text.galley.text().starts_with(step)
            )),
            "{step} is a visible step"
        );
    }
}
