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
