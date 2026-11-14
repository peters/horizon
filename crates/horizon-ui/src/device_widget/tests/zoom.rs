//! The text of the viewer is made for the screen size of the canvas zoom.
use super::*;

/// The words of every text that a pass of the viewer painted, in paint order.
///
/// The lines of each of them are the lines of zoom 1: the galley of a text is
/// laid out for the screen size of the zoom, and its rows are the rows that the
/// same text has at zoom 1.
fn painted_rows(output: &egui::FullOutput) -> Vec<(String, Vec<String>)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text),
            _ => None,
        })
        .map(|text| {
            let rows = text
                .galley
                .rows
                .iter()
                .map(|placed| {
                    placed
                        .row
                        .glyphs
                        .iter()
                        .map(|glyph| glyph.chr)
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect();
            (text.galley.text().to_owned(), rows)
        })
        .collect()
}

/// The labels, the rows, the badges and the buttons of the viewer are made for
/// the screen size of the canvas zoom. A stock label there paints a picture of
/// zoom 1 that the layer transform stretches, and `Reconnect` blurs at a zoom of
/// 67 percent.
#[test]
fn the_text_of_the_device_panel_is_made_for_the_screen_size_of_the_zoom() {
    use horizon_core::browser::manifest::device::DeviceIdentity;

    for display_scale in [1.0_f32, 2.0] {
        for zoom in [0.67_f32, 1.0, 1.5] {
            let (quads, worst_phase, worst_stretch) =
                crate::zoom_text::measure_canvas_quads(display_scale, zoom, |ui, _| {
                    let mut device = fixture_device();
                    device.identity = Some(DeviceIdentity {
                        machine_name: Some("Lab workstation".into()),
                        hostname: Some("lab-host".into()),
                        tailscale_name: Some("lab-host.example.ts.net".into()),
                        ip_addresses: vec!["192.0.2.10".parse().expect("a fixture address")],
                    });
                    let mut native = native_metadata();
                    native.run_id = Some("synthetic-run".into());
                    let server = DeviceServerDetails {
                        name: Some("Fixture desktop".into()),
                        desktop_size: Some([640, 360]),
                        native_session: Some(native.clone()),
                    };
                    let labels = super::details::NativeLabels::new(&native);
                    let status = Status::disconnected("The desktop closed the connection");
                    let _ = super::details::header(ui, &device, &server, &labels, &status, true, false);
                    super::details::show(ui, &device, &server, true);
                    let mut controls = super::controls::Controls::default();
                    let _ = controls.show(ui, Some([1280, 720]), Some([1280, 720]));
                    let mut viewer = DeviceUiState::default();
                    viewer.interact_toggle(ui, horizon_core::DeviceInput::Accepted);
                    viewer.interact_toggle(ui, horizon_core::DeviceInput::ViewOnly);
                    viewer.recording_controls(ui, true);
                });
            assert!(quads > 150, "the viewer draws its labels, rows and buttons: {quads}");
            assert!(
                worst_phase <= 0.01,
                "at zoom {zoom} and display scale {display_scale} a glyph corner sits {worst_phase} device pixels from a whole pixel"
            );
            assert!(
                worst_stretch <= 0.01,
                "at zoom {zoom} and display scale {display_scale} a glyph picture is stretched by {worst_stretch} device pixels"
            );
        }
    }
}

/// A caption of the viewer keeps the line breaks of zoom 1: a long reason of a
/// lost link breaks at the same words at every zoom, so a person reads the same
/// sentence at 67 percent as at 100.
#[test]
fn a_wrapped_line_of_the_viewer_keeps_the_breaks_of_zoom_one() {
    const REASON: &str = "The link to the desktop is lost; Horizon keeps trying the same endpoint";
    let rows_at = |zoom: f32| {
        let ctx = egui::Context::default();
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                ui.ctx()
                    .set_transform_layer(ui.layer_id(), egui::emath::TSTransform::from_scaling(zoom));
                ui.scope(|ui| {
                    ui.set_max_width(200.0);
                    super::details::caption(ui, REASON);
                });
            })
            .discard_textures();
        painted_rows(&output)
    };
    let expected: Vec<Vec<String>> = rows_at(1.0).into_iter().map(|(_, rows)| rows).collect();
    let rows: Vec<String> = expected
        .iter()
        .flat_map(Clone::clone)
        .filter(|row| !row.is_empty())
        .collect();
    assert!(rows.len() > 1, "the caption wraps in a narrow viewer: {rows:?}");
    assert_eq!(rows.join(" "), REASON, "the caption paints its whole sentence");
    for zoom in [0.5_f32, 0.67, 0.94, 1.5, 2.0] {
        let rows: Vec<Vec<String>> = rows_at(zoom).into_iter().map(|(_, rows)| rows).collect();
        assert_eq!(rows, expected, "the caption breaks at the same words at zoom {zoom}");
    }
}

/// The controls of the viewer draw at the size of the screen at every zoom.
///
/// A drag value draws its own number through the fonts of the context, so a drag
/// value under the canvas transform paints a picture of zoom 1 that the layer
/// stretches: the number of `Maximum fps` blurs beside captions that are sharp.
/// The rows of the controls take a pane of the screen size instead.
#[test]
fn the_numbers_of_the_controls_are_made_for_the_screen_size_of_the_zoom() {
    for display_scale in [1.0_f32, 2.0] {
        for zoom in [0.67_f32, 1.0, 1.5] {
            let (quads, worst_phase, worst_stretch) =
                crate::zoom_text::measure_canvas_quads(display_scale, zoom, |ui, _| {
                    ui.ctx().memory_mut(|memory| memory.set_everything_is_visible(true));
                    let mut controls = super::controls::Controls::default();
                    let _ = controls.show(ui, Some([1280, 720]), Some([1280, 720]));
                });
            assert!(quads > 150, "the controls draw their numbers: {quads}");
            assert!(
                worst_phase <= 0.01,
                "at zoom {zoom} and display scale {display_scale} a glyph corner sits {worst_phase} device pixels from a whole pixel"
            );
            assert!(
                worst_stretch <= 0.01,
                "at zoom {zoom} and display scale {display_scale} a glyph picture is stretched by {worst_stretch} device pixels"
            );
        }
    }
}

/// The number of a drag value is laid out at the size that it has on the screen,
/// so its picture is not stretched by the zoom and it stands as large as the
/// caption beside it.
#[test]
fn a_number_of_the_controls_keeps_the_size_of_zoom_one_times_the_zoom() {
    let number_at = |zoom: f32| {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::bootstrap::configure_fonts());
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("controls"));
        let output = ctx
            .run_ui(egui::RawInput::default(), |_| {
                ctx.set_transform_layer(layer, egui::emath::TSTransform::from_scaling(zoom));
                let builder = egui::UiBuilder::new()
                    .layer_id(layer)
                    .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)));
                let mut child = egui::Ui::new(ctx.clone(), egui::Id::unique("controls ui"), builder);
                child.set_clip_rect(child.max_rect());
                let mut controls = super::controls::Controls::default();
                controls.limits(&mut child);
            })
            .discard_textures();
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "20" => Some((
                    text.pos,
                    text.galley.size(),
                    text.galley.rows[0].row.glyphs[0].font_height,
                )),
                _ => None,
            })
            .expect("the number of Maximum fps is drawn")
    };

    let (base_pos, base_size, base_height) = number_at(1.0);
    assert!(base_height > 0.0, "the number takes a line");
    for zoom in [0.67_f32, 0.94, 1.5] {
        let (pos, size, height) = number_at(zoom);
        assert!(
            (height - base_height * zoom).abs() < 0.1,
            "at zoom {zoom} the number is made for the screen: {height} against {}",
            base_height * zoom
        );
        assert!(
            (size.x - base_size.x * zoom).abs() < 0.6 && (size.y - base_size.y * zoom).abs() < 0.6,
            "at zoom {zoom} the number takes the size of zoom 1 times the zoom: {size:?} against {:?}",
            base_size * zoom
        );
        assert!(
            (pos.x - base_pos.x * zoom).abs() < 0.6 && (pos.y - base_pos.y * zoom).abs() < 0.6,
            "at zoom {zoom} the number keeps its place in the row: {pos:?} against {:?}",
            base_pos * zoom
        );
    }
}

/// A row of the controls takes the size of zoom 1 times the zoom in the layout of
/// the panel, so the panel keeps its lines and its height at every zoom, and the
/// screen draws every row at the size that the panel gave it times the zoom.
#[test]
fn the_rows_of_the_controls_take_the_size_of_zoom_one_times_the_zoom() {
    let row_at = |zoom: f32, viewport: bool| {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::bootstrap::configure_fonts());
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("rows"));
        let transform = egui::emath::TSTransform::from_scaling(zoom);
        let mut row = egui::Rect::NOTHING;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |_| {
                ctx.set_transform_layer(layer, transform);
                let builder = egui::UiBuilder::new()
                    .layer_id(layer)
                    .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0)));
                let mut child = egui::Ui::new(ctx.clone(), egui::Id::unique("rows ui"), builder);
                child.set_clip_rect(child.max_rect());
                let mut controls = super::controls::Controls::default();
                if viewport {
                    controls.viewport(&mut child, [1280, 720]);
                } else {
                    controls.limits(&mut child);
                }
                row = transform * child.min_rect();
            })
            .discard_textures();
        row
    };

    for viewport in [false, true] {
        let base = row_at(1.0, viewport);
        assert!(base.width() > 100.0, "the row draws its controls: {base:?}");
        for zoom in [0.5_f32, 0.67, 0.94, 1.5, 2.0] {
            let row = row_at(zoom, viewport);
            assert!(
                (row.min.x - base.min.x * zoom).abs() < 0.6 && (row.min.y - base.min.y * zoom).abs() < 0.6,
                "at zoom {zoom} the row starts where zoom 1 times the zoom starts: {row:?} against {base:?}"
            );
            // The glyph advances of a number that is laid out at the screen size
            // are rounded to the whole device pixel, so a row of a zoom can be a
            // couple of points wider than the row of zoom 1 times the zoom.
            assert!(
                (row.width() - base.width() * zoom).abs() < 2.5,
                "at zoom {zoom} the row takes the width of zoom 1 times the zoom: {row:?} against {base:?}"
            );
            assert!(
                (row.height() - base.height() * zoom).abs() < 2.5,
                "at zoom {zoom} the row takes the height of zoom 1 times the zoom: {row:?} against {base:?}"
            );
        }
    }
}

/// One pass of the rows of the controls of a zoom, in a layer under the canvas
/// transform, and the events that the pass reads.
fn draw_controls(
    ctx: &egui::Context,
    layer: egui::LayerId,
    zoom: f32,
    controls: &mut super::controls::Controls,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |_| {
            ctx.set_transform_layer(layer, egui::emath::TSTransform::from_scaling(zoom));
            let builder = egui::UiBuilder::new()
                .layer_id(layer)
                .max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)));
            let mut child = egui::Ui::new(ctx.clone(), egui::Id::unique("controls ui"), builder);
            child.set_clip_rect(child.max_rect());
            controls.limits(&mut child);
        },
    )
    .discard_textures()
}

/// The middle of the text `text` that a pass painted, in the coordinates of the
/// screen, where the events of a pass are.
fn painted_text_center(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text_shape) if text_shape.galley.text() == text => {
                Some(text_shape.pos + text_shape.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the text {text} of the controls is drawn"))
}

/// A drag value that draws in a pane of the screen size takes a drag and a click
/// to type as it did under the transform: the pane draws over the place that the
/// row of the panel gave the widget, and the widget is the one that the pointer
/// meets.
#[test]
fn a_number_of_the_controls_still_takes_a_drag_and_a_click() {
    use egui::Event;

    const ZOOM: f32 = 0.67;
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };

    let ctx = egui::Context::default();
    ctx.set_fonts(crate::app::bootstrap::configure_fonts());
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("interact"));
    let mut controls = super::controls::Controls::default();
    let draw = |controls: &mut super::controls::Controls, events| draw_controls(&ctx, layer, ZOOM, controls, events);

    let fps = painted_text_center(&draw(&mut controls, Vec::new()), "20");
    let before = controls.options.max_fps;
    // A drag of eight points to the right raises the number of frames per second.
    draw(&mut controls, vec![Event::PointerMoved(fps), button(fps, true)]);
    let moved = egui::pos2(fps.x + 8.0, fps.y);
    draw(&mut controls, vec![Event::PointerMoved(moved)]);
    let dragged = controls.options.max_fps;
    draw(&mut controls, vec![button(moved, false)]);
    assert!(
        dragged != before,
        "a drag on the number changes it: {before} then {dragged}"
    );

    // A click opens the text of the number, and a typed number takes its place.
    let fps = painted_text_center(&draw(&mut controls, Vec::new()), &dragged.to_string());
    draw(&mut controls, vec![Event::PointerMoved(fps), button(fps, true)]);
    draw(&mut controls, vec![button(fps, false)]);
    let editing = draw(&mut controls, Vec::new());
    assert!(
        editing.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Text(text) if text.galley.text() == dragged.to_string()
        )),
        "a click opens the text of the number"
    );
    let typed = Event::Text("3".into());
    let enter = Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    draw(&mut controls, vec![typed, enter]);
    draw(&mut controls, Vec::new());
    assert_eq!(
        controls.options.max_fps, 3,
        "a typed number takes the place of the old one"
    );
}

/// The captions of a row of numbers stand on one line: the first caption of the
/// row has the height of the row to stand in, as the captions after it have.
#[test]
fn the_captions_of_a_row_of_numbers_stand_on_one_line() {
    for zoom in [0.67_f32, 1.0, 1.5] {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::bootstrap::configure_fonts());
        // The padding of a button of the application: a number is taller than a
        // caption by twice that padding.
        ctx.all_styles_mut(|style| style.spacing.button_padding = egui::vec2(12.0, 6.0));
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("captions"));
        let mut controls = super::controls::Controls::default();
        let _ = draw_controls(&ctx, layer, zoom, &mut controls, Vec::new());
        let output = draw_controls(&ctx, layer, zoom, &mut controls, Vec::new());
        let first = painted_text_center(&output, "Maximum fps");
        let second = painted_text_center(&output, "Image limits");
        assert!(
            (first.y - second.y).abs() < 0.6,
            "at zoom {zoom} the first caption stands on the line of the second: {first:?} against {second:?}"
        );
    }
}
