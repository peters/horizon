use super::{MAX_ZOOM, MIN_ZOOM, PanelZoom, dropdown_with_fit, gesture_delta, gesture_target};
use crate::test_egui::DiscardTextures;

#[test]
fn cached_labels_preserve_button_typography_and_current_theme() {
    let ctx = egui::Context::default();
    for (spacing, color) in [
        (0.0, egui::Color32::LIGHT_BLUE),
        (0.0, egui::Color32::LIGHT_RED),
        (3.0, egui::Color32::LIGHT_GREEN),
    ] {
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                ui.style_mut().spacing.extra_text_line_spacing = spacing;
                ui.style_mut().override_text_style = Some(egui::TextStyle::Heading);
                ui.style_mut().visuals.override_text_color = Some(color);
                let layout = |text: egui::WidgetText| {
                    text.into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Button,
                    )
                };
                let plain = layout("150%".into());
                let cached = layout(super::dropdown_label(ui, Some(PanelZoom::new(1.5))).into());
                assert_eq!(plain.job, cached.job);
                assert_eq!(plain.size(), cached.size());
            })
            .discard_textures();
    }
}

#[test]
fn zoom_labels_share_text_until_the_displayed_percentage_changes() {
    let ctx = egui::Context::default();
    let first = super::cached_label(&ctx, Some(PanelZoom::new(0.666)), 14.0);
    let unchanged = super::cached_label(&ctx, Some(PanelZoom::new(0.667)), 14.0);
    assert!(std::sync::Arc::ptr_eq(&first, &unchanged));
    assert_eq!(first.text(), "67%");
    let changed = super::cached_label(&ctx, Some(PanelZoom::new(0.68)), 14.0);
    assert!(!std::sync::Arc::ptr_eq(&first, &changed));
    assert_eq!(changed.text(), "68%");
    let fit = super::cached_label(&ctx, None, 14.0);
    assert_eq!(fit.text(), "Fit");
    assert!(std::sync::Arc::ptr_eq(&fit, &super::cached_label(&ctx, None, 14.0)));
    assert_eq!(super::cached_label(&ctx, Some(PanelZoom::ONE), 14.0).text(), "100%");
    let resized = super::cached_label(&ctx, Some(PanelZoom::new(0.666)), 18.0);
    assert!(!std::sync::Arc::ptr_eq(&first, &resized));
    assert_eq!(resized.text(), "67%");
}

#[test]
fn scales_are_clamped_and_labeled_as_whole_percentages() {
    assert_eq!(PanelZoom::default(), PanelZoom::ONE);
    assert!((PanelZoom::new(f32::NAN).factor() - 1.0).abs() <= f32::EPSILON);
    assert!((PanelZoom::new(99.0).factor() - MAX_ZOOM).abs() <= f32::EPSILON);
    assert!((PanelZoom::new(0.01).factor() - MIN_ZOOM).abs() <= f32::EPSILON);
    assert_eq!(PanelZoom::ONE.label(), "100%");
    assert_eq!(PanelZoom::new(0.666).label(), "67%");
}

#[test]
fn a_gesture_never_moves_the_image_against_its_own_direction() {
    // Inside the range a gesture simply scales.
    assert_eq!(gesture_target(1.0, 1.25), Some(PanelZoom::new(1.25)));
    assert_eq!(gesture_target(1.0, 0.5), Some(PanelZoom::new(0.5)));
    // A desktop fitted below the range stays fitted on a pinch out, and
    // grows to the nearest supported scale on a pinch in.
    assert_eq!(gesture_target(0.1, 0.5), None);
    assert_eq!(gesture_target(0.1, 1.5), Some(PanelZoom::new(MIN_ZOOM)));
    // A tiny desktop fitted above the range behaves symmetrically.
    assert_eq!(gesture_target(8.0, 1.5), None);
    assert_eq!(gesture_target(8.0, 0.5), Some(PanelZoom::new(MAX_ZOOM)));
    // The ends of the range absorb gestures that push past them.
    assert_eq!(gesture_target(MIN_ZOOM, 0.5), None);
    assert_eq!(gesture_target(MAX_ZOOM, 1.5), None);
    // A degenerate layout still accepts the gesture from 100%.
    assert_eq!(gesture_target(0.0, 1.25), Some(PanelZoom::new(1.25)));
}

#[test]
fn one_gesture_keeps_one_owner_until_it_goes_idle() {
    use super::gesture_owner;
    let ctx = egui::Context::default();
    let panel = egui::Id::new("panel-a");
    let other = egui::Id::new("panel-b");
    let owner_at = |time: f64, zooming: bool, candidate: Option<egui::Id>| {
        let mut owner = None;
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    time: Some(time),
                    events: if zooming {
                        vec![egui::Event::Zoom(1.1)]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                },
                |ui| owner = gesture_owner(ui.ctx(), candidate),
            )
            .discard_textures();
        owner
    };
    // The pointer's panel claims the gesture, and keeps it while the
    // smoothed delta continues even after the pointer moves elsewhere.
    assert_eq!(owner_at(1.0, true, Some(panel)), Some(panel));
    assert_eq!(owner_at(1.05, true, Some(other)), Some(panel));
    assert_eq!(owner_at(1.1, true, None), Some(panel));
    // A gesture that starts on the canvas keeps the canvas the same way.
    assert_eq!(owner_at(5.0, true, None), None);
    assert_eq!(owner_at(5.05, true, Some(panel)), None);
    // Once the gesture goes idle the next one is claimed afresh.
    assert_eq!(owner_at(9.0, true, Some(other)), Some(other));
    // With no gesture in flight the pointer's own answer is returned.
    assert_eq!(owner_at(9.02, false, Some(panel)), Some(panel));
}

#[test]
fn retired_dialog_hit_testing_keeps_underlying_popups_and_panel_occlusion() {
    for panel_above in [false, true] {
        for offset in [0.0, 300.0] {
            let ctx = egui::Context::default();
            let popup = egui::Id::new("live-popup");
            let panel = egui::Id::new("panel");
            let retired = egui::Id::new("retired-dialog");
            for frame in 0..4 {
                let _ = ctx
                    .run_ui(
                        egui::RawInput {
                            events: vec![egui::Event::PointerMoved(egui::pos2(100.0 + offset, 100.0))],
                            ..Default::default()
                        },
                        |ui| {
                            for (id, order) in [
                                (popup, egui::Order::Foreground),
                                (
                                    panel,
                                    if panel_above {
                                        egui::Order::Foreground
                                    } else {
                                        egui::Order::Middle
                                    },
                                ),
                                (retired, egui::Order::Debug),
                            ] {
                                let layer = egui::LayerId::new(order, id);
                                ui.ctx().set_transform_layer(
                                    layer,
                                    egui::emath::TSTransform::from_translation(egui::vec2(offset, 0.0)),
                                );
                                egui::Area::new(id)
                                    .order(order)
                                    .fixed_pos(egui::pos2(40.0, 40.0))
                                    .constrain(false)
                                    .show(ui.ctx(), |ui| {
                                        ui.set_min_size(egui::vec2(200.0, 200.0));
                                    });
                            }
                            if frame == 3 {
                                assert_eq!(
                                    ui.ctx()
                                        .layer_id_at(egui::pos2(100.0 + offset, 100.0))
                                        .map(|layer| layer.id),
                                    Some(retired)
                                );
                                assert_eq!(
                                    super::blocking_layer(ui.ctx(), std::iter::once(panel), |id| id == retired),
                                    (!panel_above).then_some(popup)
                                );
                            }
                        },
                    )
                    .discard_textures();
            }
        }
    }
}

#[test]
fn unresolved_native_menu_gesture_waits_for_an_active_sample() {
    let ctx = egui::Context::default();
    for step in 0..3 {
        let mut events = vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))];
        if step != 1 {
            events.push(egui::Event::Zoom(1.25));
        }
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    time: Some(1.0 + f64::from(step) * 0.02),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let owner = super::content_owner(ui.layer_id().id, true);
                    if step != 1 {
                        super::gesture_owner(ui.ctx(), Some(owner));
                    }
                    if step == 0 {
                        return;
                    } // temporarily noninteractive rendering
                    let dismiss = super::resolve_content_owner(ui, false, super::is_native_pinch(ui.ctx()));
                    assert_eq!(dismiss, step == 2);
                    assert!(!super::owns_gesture(ui));
                    assert_eq!(super::take_deferred_canvas_zoom(ui.ctx()).is_some(), step == 2);
                },
            )
            .discard_textures();
    }
}

#[test]
fn fullscreen_transition_discards_deferred_canvas_sample() {
    let ctx = egui::Context::default();
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(egui::pos2(20.0, 20.0)),
                    egui::Event::Zoom(1.25),
                ],
                ..Default::default()
            },
            |ui| {
                super::gesture_owner(ui.ctx(), Some(super::content_owner(ui.layer_id().id, true)));
                assert!(super::resolve_content_owner(ui, false, true));
                assert!(super::synchronize_fullscreen(
                    ui.ctx(),
                    Some(egui::Id::new("fullscreen"))
                ));
                assert!(super::take_deferred_canvas_zoom(ui.ctx()).is_none());
            },
        )
        .discard_textures();
}

#[test]
fn wheel_and_native_pinch_claim_separate_gestures() {
    let ctx = egui::Context::default();
    let panel = egui::Id::new("fixed-browser");
    let other = egui::Id::new("another-panel");
    let wheel = || {
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 4.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::CTRL,
        }]
    };
    let mut mixed = wheel();
    mixed.push(egui::Event::Zoom(1.25));
    for (index, (events, candidate, expected)) in [
        (wheel(), Some(panel), Some(panel)),
        (Vec::new(), Some(other), Some(panel)),
        (vec![egui::Event::Zoom(1.25)], None, None),
        (Vec::new(), Some(panel), Some(panel)),
        (mixed, None, None),
        (wheel(), Some(panel), Some(panel)),
    ]
    .into_iter()
    .enumerate()
    {
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    time: Some(1.0 + f64::from(u32::try_from(index).expect("small index")) * 0.02),
                    events,
                    ..Default::default()
                },
                |ui| {
                    assert!(ui.input(|input| (input.zoom_delta() - 1.0).abs() > f32::EPSILON));
                    assert_eq!(super::gesture_owner(ui.ctx(), candidate), expected, "frame {index}");
                },
            )
            .discard_textures();
    }
}

#[test]
fn gestures_are_ignored_off_content_and_without_a_zoom_event() {
    let ctx = egui::Context::default();
    let mut deltas = Vec::new();
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::Zoom(1.25)],
                ..Default::default()
            },
            |ui| deltas = vec![gesture_delta(ui, false), gesture_delta(ui, true)],
        )
        .discard_textures();
    assert_eq!(deltas[0], None);
    assert!(deltas[1].is_some_and(|delta| (delta - 1.25).abs() < 0.001));
    let mut idle = None;
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| idle = gesture_delta(ui, true))
        .discard_textures();
    assert_eq!(idle, None);
}

#[test]
fn the_dropdown_reports_only_an_actual_change() {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let render = |events: Vec<egui::Event>| {
        let mut changed = None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                    ..Default::default()
                },
                |ui| {
                    let mut selection = Some(PanelZoom::ONE);
                    changed = dropdown_with_fit(ui, "zoom", &mut selection, true).then_some(selection);
                },
            )
            .discard_textures();
        (changed, output)
    };
    let (unchanged, output) = render(Vec::new());
    assert_eq!(unchanged, None);
    let selected = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "100%" => Some(text.pos),
            _ => None,
        })
        .expect("the current zoom is displayed");
    for pressed in [true, false] {
        render(vec![
            egui::Event::PointerMoved(selected),
            egui::Event::PointerButton {
                pos: selected,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    let (_, output) = render(Vec::new());
    let fit = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Fit" => Some(text.pos + text.galley.size() * 0.5),
            _ => None,
        })
        .expect("the open dropdown lists Fit");
    let mut chosen = None;
    for pressed in [true, false] {
        let (changed, _) = render(vec![
            egui::Event::PointerMoved(fit),
            egui::Event::PointerButton {
                pos: fit,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        chosen = chosen.or(changed);
    }
    assert_eq!(chosen, Some(None), "choosing Fit reports the new selection");
}
