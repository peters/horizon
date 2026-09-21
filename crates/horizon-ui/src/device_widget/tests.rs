use super::*;
use crate::test_egui::DiscardTextures;

fn fixture_device() -> DevicePanelState {
    DevicePanelState {
        target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
        identity: None,
        connect_on_start: false,
    }
}

fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos + text.galley.size() * 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing label {label}"))
}

fn click_events(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn patterned_desktop() -> ColorImage {
    ColorImage::new(
        [8, 4],
        (0..32).map(|value| egui::Color32::from_rgb(value, 40, 80)).collect(),
    )
}

fn disconnected_viewer() -> (egui::Context, DevicePanelState, DeviceUiState) {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let state = DeviceUiState {
        initialized: true,
        status: Status::Disconnected("The VNC client isn't started. Or it is already closed".into()),
        ..Default::default()
    };
    (ctx, fixture_device(), state)
}

fn show_viewer(ctx: &egui::Context, state: &mut DeviceUiState, device: &DevicePanelState, events: Vec<egui::Event>) {
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| {
                if state.source.is_none() {
                    state.update_texture(ui, patterned_desktop());
                }
                state.show(ui, device, true);
            },
        )
        .discard_textures();
}

fn click_label(ctx: &egui::Context, state: &mut DeviceUiState, device: &DevicePanelState, label: &str) {
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| state.show(ui, device, true),
        )
        .discard_textures();
    let pos = text_center(&output, label);
    for pressed in [true, false] {
        show_viewer(ctx, state, device, click_events(pos, pressed));
    }
}

fn presented_size(state: &DeviceUiState) -> Option<[usize; 2]> {
    state.texture.as_ref().map(TextureHandle::size)
}

#[test]
fn narrow_frame_exceeding_gpu_limit_is_rejected_before_texture_upload() {
    let mut state = DeviceUiState::default();
    state.controls.options.max_width = 8192;
    state.controls.options.max_height = 8192;
    let ctx = egui::Context::default();
    let output = ctx.run_ui(
        egui::RawInput {
            max_texture_side: Some(2048),
            ..Default::default()
        },
        |ui| {
            state.update_texture(ui, egui::ColorImage::filled([2049, 1], egui::Color32::BLACK));
        },
    );
    let _ = output.discard_textures();
    assert!(state.texture.is_none());
    assert!(matches!(state.status, Status::Disconnected(_)));
    assert_eq!(state.image.sequence, 0, "rejected images are not uploaded frames");
}

#[test]
fn image_limits_and_viewport_apply_without_a_live_session() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(presented_size(&state), Some([8, 4]));
    assert_eq!(state.image.sequence, 1);

    state.controls.options.max_width = 4;
    state.controls.options.max_height = 4;
    state.presented_options = None;
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(presented_size(&state), Some([4, 2]));
    assert_eq!(
        state.image.sequence, 1,
        "local presentation is not a new received frame"
    );

    state.controls.options.max_width = 2048;
    state.controls.options.max_height = 2048;
    state.controls.options.viewport = Some(horizon_core::DeviceViewport {
        x: 4,
        y: 0,
        width: 4,
        height: 4,
    });
    state.presented_options = None;
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(presented_size(&state), Some([4, 4]));
}

#[test]
fn desktop_shrink_lets_controls_clear_the_stale_viewport_draft() {
    let (ctx, device, mut state) = disconnected_viewer();
    let crop = horizon_core::DeviceViewport {
        x: 4,
        y: 0,
        width: 4,
        height: 4,
    };
    state.controls.options.viewport = Some(crop);
    state.controls.draft = Some(crop);
    show_viewer(&ctx, &mut state, &device, Vec::new());
    click_label(&ctx, &mut state, &device, "View controls");
    let _ = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| {
                state.update_texture(ui, ColorImage::filled([4, 2], egui::Color32::WHITE));
                state.show(ui, &device, true);
            },
        )
        .discard_textures();
    assert!(state.controls.options.viewport.is_none());
    assert_eq!(
        state.controls.draft.map(|draft| [draft.width, draft.height]),
        Some([4, 2])
    );
}

#[test]
fn zoom_and_whole_desktop_clicks_work_without_a_session() {
    let (ctx, device, mut state) = disconnected_viewer();
    state.controls.options.viewport = Some(horizon_core::DeviceViewport {
        x: 4,
        y: 0,
        width: 4,
        height: 4,
    });
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(presented_size(&state), Some([4, 4]));
    // The dropdown opens on its current selection, then picks a stop.
    click_label(&ctx, &mut state, &device, "Fit");
    click_label(&ctx, &mut state, &device, "100%");
    assert_eq!(state.controls.zoom, Some(PanelZoom::ONE));
    click_label(&ctx, &mut state, &device, "100%");
    click_label(&ctx, &mut state, &device, "Fit");
    assert_eq!(state.controls.zoom, None);
    click_label(&ctx, &mut state, &device, "View controls");
    click_label(&ctx, &mut state, &device, "Whole desktop");
    assert!(state.controls.options.viewport.is_none());
    assert_eq!(presented_size(&state), Some([8, 4]));
}

#[test]
fn a_pinch_over_the_image_zooms_the_panel_and_leaves_the_desktop_alone() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    let presented = presented_size(&state);
    // Hover the fitted image, then pinch over it.
    show_viewer(
        &ctx,
        &mut state,
        &device,
        vec![egui::Event::PointerMoved(egui::pos2(600.0, 500.0))],
    );
    show_viewer(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::PointerMoved(egui::pos2(600.0, 500.0)),
            egui::Event::Zoom(0.5),
        ],
    );
    let first = state.controls.zoom.expect("the pinch selects an explicit scale");
    show_viewer(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::PointerMoved(egui::pos2(600.0, 500.0)),
            egui::Event::Zoom(0.5),
        ],
    );
    let second = state.controls.zoom.expect("the gesture keeps an explicit scale");
    assert!(
        second.factor() < first.factor(),
        "each pinch out shrinks further: {first:?} then {second:?}"
    );
    assert_eq!(presented_size(&state), presented, "zoom never resamples the desktop");
    assert_eq!(state.image.sequence, 1, "local zoom is not a received frame");
}

#[test]
fn apply_viewport_and_reconnect_keep_the_last_desktop() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    click_label(&ctx, &mut state, &device, "View controls");
    state.controls.draft = Some(horizon_core::DeviceViewport {
        x: 0,
        y: 0,
        width: 2,
        height: 2,
    });
    click_label(&ctx, &mut state, &device, "Apply viewport");
    assert_eq!(
        state
            .controls
            .options
            .viewport
            .map(|viewport| [viewport.width, viewport.height]),
        Some([2, 2])
    );
    assert_eq!(presented_size(&state), Some([2, 2]));
    click_label(&ctx, &mut state, &device, "Reconnect");
    assert!(matches!(state.status, Status::Connecting));
    assert_eq!(presented_size(&state), Some([2, 2]));
    assert_eq!(state.image.sequence, 0);
    assert!(!state.image.received);
    let evidence = state.observation("panel".into(), &device, true, "agent");
    assert!(!evidence.image.image_received);
    assert!(!evidence.image.image_displayed);
    assert_eq!(evidence.image.frame_sequence, 0);
}

#[test]
fn reconnect_does_not_report_a_retained_texture_as_live_evidence() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert!(state.image.received);
    assert_eq!(state.image.sequence, 1);
    click_label(&ctx, &mut state, &device, "Reconnect");
    let _worker = state.session.take();
    state.status = Status::Connected;
    let _ = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| state.show(ui, &device, true),
        )
        .discard_textures();
    assert_eq!(presented_size(&state), Some([8, 4]));
    assert!(!state.image.displayed);
    state.begin_frame();
    let evidence = state.observation("panel".into(), &device, true, "agent");
    assert!(!evidence.image.image_received);
    assert!(!evidence.image.image_displayed);
    assert_eq!(evidence.image.frame_sequence, 0);
}

#[test]
fn fps_only_change_does_not_resample_the_retained_desktop() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    let presented = state.presented_options;
    assert!(presented.is_some());
    let previous = state.controls.options;
    state.controls.options.max_fps = 1;
    assert!(!state.apply_view_options(previous));
    assert_eq!(state.presented_options, presented);
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(state.presented_options, presented);
    assert_eq!(presented_size(&state), Some([8, 4]));
    assert_eq!(state.image.sequence, 1);
}

#[test]
fn discarded_stale_worker_frame_represents_the_latest_desktop() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(state.image.sequence, 1);
    let latest = ColorImage::filled([8, 4], egui::Color32::RED);
    let queued = ColorImage::filled([2, 2], egui::Color32::GREEN);
    state.session = Some(Session::pending_frame(
        latest,
        queued,
        DeviceViewOptions {
            viewport: Some(horizon_core::DeviceViewport {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }),
            ..Default::default()
        },
    ));
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_eq!(
        state.source.as_ref().map(|source| source.pixels[0]),
        Some(egui::Color32::RED)
    );
    assert_eq!(presented_size(&state), Some([8, 4]));
    assert!(state.image.received);
    assert_eq!(state.image.sequence, 2);
    assert!(matches!(state.status, Status::Connected));
}

#[test]
fn worker_frame_with_a_different_fps_is_not_treated_as_stale() {
    let (ctx, device, mut state) = disconnected_viewer();
    show_viewer(&ctx, &mut state, &device, Vec::new());
    let latest = ColorImage::filled([8, 4], egui::Color32::RED);
    let cropped = ColorImage::filled([2, 2], egui::Color32::GREEN);
    let crop = DeviceViewOptions {
        max_fps: 1,
        viewport: Some(horizon_core::DeviceViewport {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        }),
        ..Default::default()
    };
    state.controls.options = DeviceViewOptions {
        max_fps: 30,
        viewport: crop.viewport,
        ..Default::default()
    };
    state.session = Some(Session::pending_frame(latest, cropped, crop));
    show_viewer(&ctx, &mut state, &device, Vec::new());
    assert_ne!(
        state.source.as_ref().map(|source| source.pixels[0]),
        Some(egui::Color32::RED),
        "matching crop must keep the local source instead of replacing it"
    );
    assert_eq!(presented_size(&state), Some([2, 2]));
    assert_eq!(state.image.sequence, 2);
}

#[test]
fn connected_texture_is_not_display_proof_when_image_is_clipped() {
    let ctx = egui::Context::default();
    let device = DevicePanelState {
        target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
        identity: None,
        connect_on_start: false,
    };
    for zoom in [None, Some(PanelZoom::ONE)] {
        for (clip_height, expected) in [(20.0, false), (600.0, true)] {
            let mut state = DeviceUiState {
                initialized: true,
                status: Status::Connected,
                ..Default::default()
            };
            state.controls.zoom = zoom;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    state.update_texture(ui, egui::ColorImage::filled([100, 100], egui::Color32::WHITE));
                    ui.set_clip_rect(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, clip_height),
                    ));
                    state.show(ui, &device, false);
                },
            );
            let _ = output.discard_textures();
            assert_eq!(state.image.displayed, expected, "zoom={zoom:?}, clip={clip_height}");
            state.begin_frame();
            assert_eq!(
                state
                    .observation("panel".into(), &device, true, "agent")
                    .image
                    .image_displayed,
                expected
            );
        }
    }
}

#[test]
fn the_zoom_anchor_is_independent_of_the_canvas_transform() {
    // Panels paint through the canvas transform, so the anchor must be
    // computed in layer coordinates: the same visual pointer position has to
    // select the same source pixel however the canvas is panned or zoomed.
    let local = egui::pos2(600.0, 500.0);
    let anchored = |transform: Option<egui::emath::TSTransform>| {
        let (ctx, device, mut state) = disconnected_viewer();
        state.controls.zoom = Some(PanelZoom::ONE);
        let pointer = transform.map_or(local, |transform| transform * local);
        for zooming in [false, true] {
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if zooming {
                events.push(egui::Event::Zoom(1.5));
            }
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        events,
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                        ..Default::default()
                    },
                    |ui| {
                        if let Some(transform) = transform {
                            ui.ctx().set_transform_layer(ui.layer_id(), transform);
                        }
                        if state.source.is_none() {
                            state.update_texture(ui, patterned_desktop());
                        }
                        state.show(ui, &device, true);
                    },
                )
                .discard_textures();
        }
        state.pending_scroll.expect("the pinch anchors the scrolled view")
    };
    let plain = anchored(None);
    let transformed = anchored(Some(egui::emath::TSTransform::new(egui::vec2(37.0, 19.0), 1.5)));
    assert!(
        (plain - transformed).length() < 0.5,
        "canvas transform moved the anchor: {plain:?} vs {transformed:?}"
    );
}

/// Rectangle of the textured image painted this frame, if any.
fn painted_image_rect(output: &egui::FullOutput) -> Option<egui::Rect> {
    fn walk(shape: &egui::Shape, found: &mut Option<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.brush.is_some() => {
                *found = Some(found.map_or(rect.rect, |seen: egui::Rect| seen.union(rect.rect)));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = None;
    for shape in &output.shapes {
        walk(&shape.shape, &mut found);
    }
    found
}

#[test]
fn an_image_smaller_than_the_body_is_centered_rather_than_pinned() {
    // Zooming out below the body leaves no scroll range to hold a pixel in
    // place, so the leftover space is split instead of pushing the image into
    // the scroll origin.
    let (ctx, device, mut state) = disconnected_viewer();
    state.controls.zoom = Some(PanelZoom::new(2.0));
    state.pending_scroll = Some(egui::vec2(400.0, 300.0));
    show_viewer(&ctx, &mut state, &device, Vec::new());
    let mut body = egui::Rect::NOTHING;
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| {
                state.show(ui, &device, true);
                body = ui.min_rect();
            },
        )
        .discard_textures();
    let image = painted_image_rect(&output).expect("the desktop is painted at the selected scale");
    assert!(
        (image.center().x - body.center().x).abs() < 4.0,
        "image {image:?} is not centered in {body:?}"
    );
}

#[test]
fn a_latched_gesture_keeps_its_anchor_after_the_pointer_leaves() {
    // egui smooths a gesture for several frames after the input stops. The
    // anchor captured when the gesture started must survive that tail, so a
    // pointer that has moved on cannot drag the view somewhere else.
    let scroll_after = |wander: bool| {
        let (ctx, device, mut state) = disconnected_viewer();
        state.controls.zoom = Some(PanelZoom::new(2.0));
        let inside = egui::pos2(600.0, 500.0);
        let outside = egui::pos2(1190.0, 30.0);
        for step in 0..3 {
            let pointer = if wander && step == 2 { outside } else { inside };
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if step > 0 {
                events.push(egui::Event::Zoom(1.2));
            }
            show_viewer(&ctx, &mut state, &device, events);
        }
        state.pending_scroll.expect("the gesture anchors the view")
    };
    let steady = scroll_after(false);
    let wandered = scroll_after(true);
    assert!(
        (steady - wandered).length() < 0.5,
        "the anchor moved with the pointer: {steady:?} vs {wandered:?}"
    );
}

#[test]
fn switching_zoom_input_kind_captures_a_new_device_anchor() {
    let ctx = egui::Context::default();
    let mut state = DeviceUiState::default();
    state.controls.zoom = Some(PanelZoom::ONE);
    let first = egui::pos2(100.0, 100.0);
    let second = egui::pos2(300.0, 250.0);
    for (index, pointer) in [first, second, egui::pos2(200.0, 180.0)].into_iter().enumerate() {
        let event = if index == 1 {
            egui::Event::Zoom(1.25)
        } else {
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 4.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::CTRL,
            }
        };
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    time: Some(1.0 + f64::from(u32::try_from(index).expect("small index")) * 0.02),
                    events: vec![egui::Event::PointerMoved(pointer), event],
                    ..Default::default()
                },
                |ui| {
                    panel_zoom::gesture_owner(ui.ctx(), Some(ui.layer_id().id));
                    state.handle_zoom_gesture(
                        ui,
                        Some(ImageView {
                            scale: state.controls.zoom.expect("explicit scale").factor(),
                            image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2000.0, 1000.0)),
                            body: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)),
                        }),
                    );
                },
            )
            .discard_textures();
        assert_eq!(state.zoom_anchor.expect("active gesture").pointer, pointer);
    }
}

#[test]
fn changing_render_layers_discards_the_old_zoom_anchor() {
    let (ctx, device, mut state) = disconnected_viewer();
    state.controls.zoom = Some(PanelZoom::new(2.0));
    show_viewer(&ctx, &mut state, &device, Vec::new());
    show_viewer(
        &ctx,
        &mut state,
        &device,
        vec![
            egui::Event::PointerMoved(egui::pos2(600.0, 500.0)),
            egui::Event::Zoom(1.2),
        ],
    );
    assert!(state.zoom_anchor.is_some());
    let selection = state.controls.zoom;
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            ui.scope_builder(
                egui::UiBuilder::new().layer_id(egui::LayerId::new(egui::Order::Middle, egui::Id::new("fullscreen"))),
                |ui| state.show(ui, &device, false),
            );
        })
        .discard_textures();
    assert!(state.zoom_anchor.is_none());
    assert!(state.pending_scroll.is_none());
    assert_eq!(state.controls.zoom, selection);
}

#[test]
fn choosing_a_zoom_abandons_the_previous_gesture_state() {
    // A pending offset or anchor belongs to the scale it was computed for.
    let (ctx, device, mut state) = disconnected_viewer();
    state.controls.zoom = Some(PanelZoom::new(2.0));
    state.source = Some(ColorImage::filled([1600, 1000], egui::Color32::WHITE));
    let render = |state: &mut DeviceUiState| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| state.show(ui, &device, true),
        )
        .discard_textures()
    };
    let origin = painted_image_rect(&render(&mut state)).expect("initial image").min;
    state.pending_scroll = Some(egui::vec2(120.0, 90.0));
    let scrolled = painted_image_rect(&render(&mut state)).expect("scrolled image").min;
    assert!((origin - scrolled - egui::vec2(120.0, 90.0)).length() < 1.0);
    state.zoom_anchor = Some(ZoomAnchor {
        captured_at: 0.0,
        native_pinch: false,
        pointer: egui::pos2(600.0, 500.0),
        content: egui::vec2(4.0, 2.0),
    });
    click_label(&ctx, &mut state, &device, "200%");
    click_label(&ctx, &mut state, &device, "300%");
    assert_eq!(state.controls.zoom, Some(PanelZoom::new(3.0)));
    assert!(state.pending_scroll.is_none());
    assert!(state.zoom_anchor.is_none());
    let selected = painted_image_rect(&render(&mut state)).expect("selected image").min;
    assert!(
        (selected - origin).length() < 1.0,
        "persisted scroll survived selection: {origin:?} -> {selected:?}"
    );
    click_label(&ctx, &mut state, &device, "300%");
    click_label(&ctx, &mut state, &device, "Fit");
    assert_eq!(state.controls.zoom, None);
    assert!(state.pending_scroll.is_none(), "a stale offset survived the selection");
    assert!(state.zoom_anchor.is_none(), "a stale anchor survived the selection");
}

#[test]
fn changing_image_body_discards_pending_scroll_and_anchor_before_painting() {
    for next_body in [
        egui::Rect::from_min_size(egui::pos2(100.0, 80.0), egui::vec2(600.0, 400.0)),
        egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(500.0, 300.0)),
    ] {
        let ctx = egui::Context::default();
        let mut state = DeviceUiState::default();
        state.controls.zoom = Some(PanelZoom::ONE);
        let original = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(600.0, 400.0));
        for (index, body) in [original, original, next_body].into_iter().enumerate() {
            if index > 0 {
                state.pending_scroll = Some(if index == 1 {
                    egui::vec2(120.0, 90.0)
                } else {
                    egui::vec2(300.0, 200.0)
                });
                state.zoom_anchor = Some(ZoomAnchor {
                    captured_at: 1.0,
                    native_pinch: false,
                    pointer: egui::pos2(100.0, 100.0),
                    content: egui::vec2(80.0, 80.0),
                });
            }
            let _ = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                        if state.texture.is_none() {
                            state.update_texture(ui, ColorImage::filled([1600, 1000], egui::Color32::WHITE));
                        }
                        let view = state.show_image(ui).expect("image");
                        if index == 2 {
                            assert!(state.zoom_anchor.is_none(), "anchor outlived body geometry");
                            let offset = view.body.min - view.image_rect.min;
                            assert!(
                                (offset - egui::vec2(120.0, 90.0)).length() < 1.0,
                                "stale offset applied"
                            );
                        }
                    });
                })
                .discard_textures();
        }
    }
}

#[test]
fn changing_viewports_with_the_same_layer_discards_the_old_zoom_anchor() {
    let (ctx, device, mut state) = disconnected_viewer();
    state.controls.zoom = Some(PanelZoom::new(1.5));
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("panel"));
    let detached = egui::ViewportId::from_hash_of("detached");
    for (index, viewport_id) in [egui::ViewportId::ROOT, detached, egui::ViewportId::ROOT]
        .into_iter()
        .enumerate()
    {
        state.zoom_anchor = Some(ZoomAnchor {
            captured_at: 1.0,
            native_pinch: false,
            pointer: egui::pos2(100.0, 100.0),
            content: egui::vec2(50.0, 50.0),
        });
        state.pending_scroll = Some(egui::vec2(30.0, 40.0));
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    viewport_id,
                    viewports: [
                        (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                        (detached, egui::ViewportInfo::default()),
                    ]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                },
                |ui| {
                    ui.scope_builder(egui::UiBuilder::new().layer_id(layer), |ui| {
                        state.show(ui, &device, false);
                    });
                },
            )
            .discard_textures();
        if index > 0 {
            assert!(state.zoom_anchor.is_none());
            assert!(state.pending_scroll.is_none());
        }
        assert_eq!(state.controls.zoom, Some(PanelZoom::new(1.5)));
    }
}

#[test]
fn reversing_a_gesture_after_holding_a_zoom_limit_keeps_its_anchor() {
    for (scale, outward, reverse, expected) in [
        (panel_zoom::MAX_ZOOM, 1.1, 0.5, 2.0),
        (panel_zoom::MIN_ZOOM, 0.9, 2.0, 0.5),
    ] {
        let ctx = egui::Context::default();
        let mut state = DeviceUiState::default();
        state.controls.zoom = Some(PanelZoom::new(scale));
        let anchor_pointer = egui::pos2(200.0, 200.0);
        let outside = egui::pos2(900.0, 700.0);
        let body = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 300.0));
        for (time, pointer, delta) in [
            (1.0, anchor_pointer, outward),
            (1.1, outside, outward),
            (1.2, outside, outward),
            (1.3, outside, reverse),
        ] {
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        time: Some(time),
                        events: vec![egui::Event::PointerMoved(pointer), egui::Event::Zoom(delta)],
                        ..Default::default()
                    },
                    |ui| {
                        panel_zoom::gesture_owner(ui.ctx(), Some(ui.layer_id().id));
                        state.handle_zoom_gesture(
                            ui,
                            Some(ImageView {
                                scale,
                                image_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4000.0, 3200.0)),
                                body,
                            }),
                        );
                    },
                )
                .discard_textures();
            let anchor = state.zoom_anchor.expect("active samples retain the initial anchor");
            assert_eq!(anchor.pointer, anchor_pointer);
            assert!((anchor.captured_at - time).abs() < f64::EPSILON);
        }
        assert_eq!(state.controls.zoom, Some(PanelZoom::new(expected)));
        let offset = (anchor_pointer.to_vec2() / scale * expected - (anchor_pointer - body.min)).max(egui::Vec2::ZERO);
        assert_eq!(state.pending_scroll, Some(offset));
    }
}

#[test]
fn presentation_changes_discard_anchors_but_ordinary_frames_and_fps_do_not() {
    let (ctx, _device, mut state) = disconnected_viewer();
    let seed_anchor = |state: &mut DeviceUiState| {
        state.zoom_anchor = Some(ZoomAnchor {
            captured_at: 1.0,
            native_pinch: false,
            pointer: egui::pos2(50.0, 40.0),
            content: egui::vec2(25.0, 20.0),
        });
        state.pending_scroll = Some(egui::vec2(10.0, 20.0));
    };
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            state.update_texture(ui, patterned_desktop());
            seed_anchor(&mut state);
            state.update_texture(ui, egui::ColorImage::filled([8, 4], egui::Color32::RED));
            assert!(state.zoom_anchor.is_some());
            assert!(state.pending_scroll.is_some());
            let previous = state.controls.options;
            state.controls.options.max_fps = 1;
            assert!(!state.apply_view_options(previous));
            assert!(state.zoom_anchor.is_some());
            assert!(state.pending_scroll.is_some());
            for crop in [false, true] {
                seed_anchor(&mut state);
                let previous = state.controls.options;
                if crop {
                    state.controls.options.viewport = Some(horizon_core::DeviceViewport {
                        x: 1,
                        y: 0,
                        width: 4,
                        height: 4,
                    });
                } else {
                    state.controls.options.max_width = 4;
                }
                assert!(state.apply_view_options(previous));
                assert!(state.zoom_anchor.is_none());
                assert!(state.pending_scroll.is_none());
            }
            state.controls.options = DeviceViewOptions::default();
            state.update_texture(ui, patterned_desktop());
            seed_anchor(&mut state);
            state.update_texture(ui, egui::ColorImage::filled([16, 8], egui::Color32::RED));
            assert!(state.zoom_anchor.is_none());
            assert!(state.pending_scroll.is_none());
        })
        .discard_textures();
}

#[test]
fn connection_details_show_supplied_and_observed_values_separately() {
    use horizon_core::browser::manifest::device::DeviceIdentity;
    let ctx = egui::Context::default();
    let mut device = fixture_device();
    device.identity = Some(DeviceIdentity {
        machine_name: Some("Lab workstation".into()),
        hostname: Some("lab-host".into()),
        tailscale_name: Some("lab-host.example.ts.net".into()),
        ip_addresses: vec!["192.0.2.10".parse().unwrap()],
    });
    let mut state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        server: DeviceServerDetails {
            name: Some("Fixture desktop".into()),
            desktop_size: Some([640, 360]),
        },
        ..Default::default()
    };
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    click_label(&ctx, &mut state, &device, "Connection details");
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                ..Default::default()
            },
            |ui| state.show(ui, &device, true),
        )
        .discard_textures();
    for label in [
        "Lab workstation",
        "Supplied by session creator",
        "lab-host",
        "lab-host.example.ts.net",
        "192.0.2.10",
        "Local endpoint",
        "127.0.0.1:5900",
        "Server-reported",
        "Fixture desktop",
        "640 × 360",
    ] {
        text_center(&output, label);
    }
    state.status = Status::Disconnected("fixture ended".into());
    let observation = state.observation("fixture".into(), &device, true, "actor");
    assert_eq!(observation.server.name.as_deref(), Some("Fixture desktop"));
    assert_eq!(observation.connection, Connection::Disconnected);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    device.target = horizon_core::DeviceViewTarget::parse(&listener.local_addr().unwrap().to_string()).unwrap();
    state.reconnect(&ctx, &device);
    let observation = state.observation("fixture".into(), &device, true, "actor");
    assert_eq!(observation.server, DeviceServerDetails::default());
    assert_eq!(observation.identity, device.identity);
}
