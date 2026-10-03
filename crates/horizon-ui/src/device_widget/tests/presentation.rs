use super::*;

#[test]
fn empty_or_edge_touching_image_never_counts_as_displayed() {
    let ctx = egui::Context::default();
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            let texture = ui
                .ctx()
                .load_texture("fixture", patterned_desktop(), TextureOptions::LINEAR);
            assert!(!visible_image(ui, &texture, egui::Vec2::ZERO, false).0);
            let top = ui.next_widget_position();
            ui.set_clip_rect(egui::Rect::from_min_max(top - egui::vec2(50.0, 50.0), top));
            assert!(!visible_image(ui, &texture, egui::vec2(100.0, 100.0), false).0);
        })
        .discard_textures();
}

#[test]
fn invisible_sizing_ui_never_counts_as_displayed_image() {
    let ctx = egui::Context::default();
    let device = fixture_device();
    let mut state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        ..Default::default()
    };
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            state.update_texture(ui.ctx(), patterned_desktop());
            ui.set_invisible();
            state.show(ui, &device, true);
            assert!(state.image.received);
            assert!(!state.image.displayed);
            assert!(state.image.last_displayed.is_none());
        })
        .discard_textures();
}

const EMPTY_HINT: &str = EMPTY_DESKTOP_HINT;

fn painted_texts(ctx: &egui::Context, state: &mut DeviceUiState) -> Vec<String> {
    let device = fixture_device();
    ctx.run_ui(egui::RawInput::default(), |ui| state.show(ui, &device, true))
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_flat_picture_is_an_empty_desktop_and_anything_drawn_on_it_is_not() {
    let flat = |size: [usize; 2]| ColorImage::filled(size, egui::Color32::BLACK);
    assert!(frame::looks_empty(&flat([1920, 1080])));
    assert!(frame::looks_empty(&flat([3, 2])), "a small picture too");
    assert!(
        !frame::looks_empty(&ColorImage::default()),
        "no picture is not an empty desktop"
    );

    // A pointer is not content.
    let mut with_pointer = flat([1920, 1080]);
    for y in 500..512 {
        for x in 960..972 {
            with_pointer.pixels[y * 1920 + x] = egui::Color32::WHITE;
        }
    }
    assert!(frame::looks_empty(&with_pointer));
    // Not even at the very corner the background colour was once read from.
    let mut at_the_corner = flat([1920, 1080]);
    for y in 0..14 {
        for x in 0..14 {
            at_the_corner.pixels[y * 1920 + x] = egui::Color32::WHITE;
        }
    }
    assert!(frame::looks_empty(&at_the_corner));

    // A window, a panel or a wallpaper is.
    let mut with_window = flat([1920, 1080]);
    for y in 200..700 {
        for x in 300..900 {
            with_window.pixels[y * 1920 + x] = egui::Color32::from_rgb(40, 60, 90);
        }
    }
    assert!(!frame::looks_empty(&with_window));
    assert!(!frame::looks_empty(&patterned_desktop()));
}

#[test]
fn a_connected_empty_desktop_says_so_and_a_busy_one_does_not() {
    let ctx = egui::Context::default();
    let mut state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        ..Default::default()
    };
    state.update_texture(&ctx, ColorImage::filled([64, 48], egui::Color32::BLACK));
    assert!(painted_texts(&ctx, &mut state).iter().any(|text| text == EMPTY_HINT));

    state.update_texture(&ctx, patterned_desktop());
    assert!(
        !painted_texts(&ctx, &mut state).iter().any(|text| text == EMPTY_HINT),
        "the hint goes as soon as something is drawn"
    );

    state.update_texture(&ctx, ColorImage::filled([64, 48], egui::Color32::BLACK));
    state.status = Status::Disconnected("lost".into());
    assert!(
        !painted_texts(&ctx, &mut state).iter().any(|text| text == EMPTY_HINT),
        "a lost connection is shown as that, not as an empty desktop"
    );
}

#[test]
fn the_hint_stays_inside_the_part_of_the_picture_that_is_on_screen() {
    let ctx = egui::Context::default();
    let room = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(300.0, 200.0));
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            // A desktop far larger than the room it has, scrolled to its top-left, as in 1:1 mode.
            let image = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(4000.0, 3000.0));
            ui.set_clip_rect(room);
            paint_empty_hint(ui, image);
        })
        .discard_textures();
    let (text, clip) = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some((text.visual_bounding_rect(), shape.clip_rect)),
            _ => None,
        })
        .expect("the hint is painted");
    assert_eq!(clip, room, "it cannot paint over what is around the picture");
    assert!(room.contains(text.center()), "and it is where the picture can be seen");
}

#[test]
fn only_a_non_discarded_presented_image_latches_navigation_protection() {
    use horizon_core::browser::manifest::device::HostViewport;
    let mut state = DeviceUiState::default();
    state.image.received = true;
    state.image.displayed = true;
    state.host.record(HostViewport::Root, None, None);
    state.host.finish(None, true, 1, true);
    state.commit_pass();
    assert!(!state.presented_once());
    state.host.finish(None, true, 2, false);
    state.commit_pass();
    assert!(state.presented_once());
    state.image.displayed = false;
    state.commit_pass();
    assert!(state.presented_once());
}
