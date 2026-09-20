use super::*;
use crate::test_egui::DiscardTextures;

fn fixture_device() -> DevicePanelState {
    DevicePanelState {
        target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
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
