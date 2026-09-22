use egui::{Rect, pos2};
use horizon_core::browser::PageScrollState;

use super::{BrowserUiState, apply_zoom_gesture, vertical_scrollbar_geometry};

fn scroll_state(scroll_y: f32) -> PageScrollState {
    PageScrollState {
        scroll_x: 0.0,
        scroll_y,
        viewport_width: 1164.0,
        viewport_height: 608.0,
        client_width: 1152.0,
        client_height: 608.0,
        content_width: 1152.0,
        content_height: 3000.0,
    }
}

#[test]
fn page_scrollbar_overlay_tracks_native_gutter_and_scroll_position() {
    let image = Rect::from_min_max(pos2(0.0, 0.0), pos2(1164.0, 608.0));
    let Some((track, top_thumb)) = vertical_scrollbar_geometry(image, scroll_state(0.0)) else {
        panic!("scrollable page should have overlay geometry");
    };
    let Some((_, middle_thumb)) = vertical_scrollbar_geometry(image, scroll_state(1_196.0)) else {
        panic!("scrolled page should have overlay geometry");
    };

    assert!((track.width() - 12.0).abs() < f32::EPSILON);
    assert!((top_thumb.top() - image.top()).abs() < f32::EPSILON);
    assert!(middle_thumb.top() > top_thumb.top());
    assert!((top_thumb.height() - 123.2).abs() < 0.1);
}

fn zoom_menu() -> horizon_core::browser::NativeSelectPopup {
    use horizon_core::browser::{BrowserBounds, NativeSelectOption, NativeSelectPopup};
    NativeSelectPopup {
        css_path: "#menu".into(),
        name: "menu".into(),
        selected_index: 0,
        bounds: BrowserBounds {
            x: 20.0,
            y: 40.0,
            width: 80.0,
            height: 22.0,
        },
        options: (0..10)
            .map(|index| NativeSelectOption {
                index,
                value: index.to_string(),
                label: format!("Item {index}"),
                group: None,
                disabled: false,
                selected: index == 0,
            })
            .collect(),
    }
}

#[test]
fn native_select_zoom_uses_current_menu_bounds_and_keeps_its_first_owner() {
    for fullscreen in [false, true] {
        for fixed in [false, true] {
            for native in [false, true] {
                for starts_inside in [false, true] {
                    let event = if native {
                        egui::Event::Zoom(1.25)
                    } else {
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line,
                            delta: egui::vec2(0.0, 4.0),
                            phase: egui::TouchPhase::Move,
                            modifiers: egui::Modifiers::CTRL,
                        }
                    };
                    check_select_zoom(fullscreen, fixed, &event, starts_inside);
                }
            }
        }
    }
}

fn check_select_zoom(fullscreen: bool, fixed: bool, event: &egui::Event, starts_inside: bool) {
    use super::super::select_popup;
    use crate::{panel_zoom, test_egui::DiscardTextures};
    use egui::{Ui, vec2};
    use horizon_core::browser::BrowserPanelState;
    let native = matches!(event, egui::Event::Zoom(_));
    let popup = zoom_menu();
    let ctx = egui::Context::default();
    let browser = if fixed {
        BrowserPanelState::inert_remote("target", "provider")
    } else {
        BrowserPanelState::inert()
    };
    let mut state = BrowserUiState::default();
    let panel = egui::Id::new("browser-panel");
    let mut open = None;
    select_popup::sync_ui_state(&mut open, Some(&popup));
    for index in 0..5 {
        let active = index >= 3;
        // Chrome grows in the gesture's first frame. The old menu did not
        // cover y=360; the current layout does. Then the pointer crosses it.
        let header = if active { 90.0 } else { 0.0 };
        let inside = if index == 4 { !starts_inside } else { starts_inside };
        let pointer = pos2(if inside { 80.0 } else { 340.0 }, 360.0);
        let mut events = vec![egui::Event::PointerMoved(pointer)];
        if active {
            events.push(event.clone());
        }
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    time: Some(1.0 + f64::from(index) * 0.02),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let layer = if fullscreen { ui.layer_id().id } else { panel };
                    if active {
                        panel_zoom::gesture_owner(ui.ctx(), Some(panel_zoom::content_owner(layer, true)));
                    }
                    let mut draw = |ui: &mut Ui| {
                        ui.set_min_size(vec2(400.0, 500.0));
                        let menu = select_popup::show(
                            ui,
                            &browser,
                            Rect::from_min_size(pos2(0.0, header), vec2(400.0, 400.0)),
                            [400.0, 400.0],
                            &popup,
                            open.as_mut().expect("open menu"),
                        )
                        .expect("menu is rendered");
                        if !active {
                            return;
                        }
                        let hit = menu.contains(panel_zoom::local_pointer(ui).expect("pointer"));
                        assert_eq!(hit, inside);
                        let canvas = fixed && native && !fullscreen;
                        let dismiss = super::resolve_zoom_owner(ui, &browser, Some(menu.menu), fullscreen);
                        assert_eq!(dismiss, index == 3 && !starts_inside);
                        assert_eq!(panel_zoom::owns_gesture(ui), !starts_inside && !canvas);
                        let deferred = panel_zoom::take_deferred_canvas_zoom(ui.ctx());
                        assert_eq!(deferred.is_some(), index == 3 && !starts_inside && canvas);
                        if let Some(zoom) = deferred {
                            assert_eq!(zoom.anchor, pointer);
                            assert!((zoom.delta - 1.25).abs() < 0.001);
                        }
                        assert!(panel_zoom::take_deferred_canvas_zoom(ui.ctx()).is_none());
                        super::apply_zoom_gesture(ui, &browser, &mut state, panel_zoom::owns_gesture(ui));
                    };
                    if fullscreen {
                        draw(ui);
                    } else {
                        egui::Area::new(panel)
                            .fixed_pos(egui::Pos2::ZERO)
                            .constrain(false)
                            .order(egui::Order::Middle)
                            .show(ui.ctx(), draw);
                    }
                },
            )
            .discard_textures();
    }
    assert_eq!(state.zoom.factor() > 1.0, !fixed && !starts_inside);
}

#[test]
fn a_zoomed_viewport_stays_above_the_size_the_backend_sync_accepts() {
    use super::{MAX_FRAME_PIXELS, MIN_STABLE_VIEWPORT_SIDE, zoomed_viewport};
    let limit = 8192;
    // A roomy panel zooms exactly as asked.
    assert_eq!(zoomed_viewport(egui::vec2(800.0, 600.0), 4.0, limit), (200, 150));
    assert_eq!(zoomed_viewport(egui::vec2(800.0, 600.0), 0.5, limit), (1600, 1200));
    // Zooming out stops at the renderer's side limit and the pixel budget.
    let (wide, high) = zoomed_viewport(egui::vec2(3000.0, 2000.0), 0.25, limit);
    assert!(wide <= 8192 && high <= 8192, "{wide}x{high} passes the texture limit");
    assert!(
        u64::from(wide) * u64::from(high) <= u64::from(MAX_FRAME_PIXELS),
        "{wide}x{high} passes the pixel budget"
    );
    let (narrow, _) = zoomed_viewport(egui::vec2(3000.0, 200.0), 0.25, 2048);
    assert!(narrow <= 2048, "{narrow} passes a smaller renderer limit");
    // A short one caps the effective zoom instead of selecting a viewport
    // that would never be sent.
    // Conflicting constraints (a body far wider than tall) still produce a
    // viewport the backend sync will accept.
    let (long, short) = zoomed_viewport(egui::vec2(40000.0, 120.0), 4.0, limit);
    assert!(
        f32::from(u16::try_from(short).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE && long <= 8192,
        "{long}x{short} is not sendable"
    );
    let (width, height) = zoomed_viewport(egui::vec2(420.0, 100.0), 4.0, limit);
    assert!(
        f32::from(u16::try_from(height).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE
            && f32::from(u16::try_from(width).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE,
        "{width}x{height} is below the stable viewport floor"
    );
}

#[test]
fn rounded_viewports_stay_within_the_pixel_budget() {
    for size in [
        egui::vec2(254.0, 2041.0),
        egui::vec2(2041.0, 254.0),
        egui::vec2(960.0, 540.0),
        egui::vec2(3000.0, 2000.0),
    ] {
        for zoom in [0.25, 1.0, 4.0] {
            for side_limit in [2048, 4096, 16_384] {
                let (width, height) = super::zoomed_viewport(size, zoom, side_limit);
                assert!(width >= 33 && height >= 33);
                let limit = u32::try_from(side_limit).expect("small texture limit");
                assert!(width <= limit && height <= limit);
                assert!(
                    u64::from(width) * u64::from(height) <= u64::from(super::MAX_FRAME_PIXELS),
                    "{size:?} at {zoom} with side limit {side_limit} produced {width}x{height}"
                );
            }
        }
    }
    assert_eq!(
        super::zoomed_viewport(egui::vec2(960.0, 540.0), 0.25, 16_384),
        (3840, 2160),
        "a viewport exactly at the budget must remain unchanged"
    );
}

#[test]
fn a_changed_effective_zoom_refreshes_the_idle_selector_then_settles() {
    use crate::{panel_zoom::PanelZoom, test_egui::DiscardTextures};
    use std::time::Duration;

    let ctx = egui::Context::default();
    let mut state = BrowserUiState {
        zoom: PanelZoom::new(0.25),
        ..Default::default()
    };
    let frame = |state: &mut BrowserUiState, available| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::panel_zoom::dropdown(ui, "idle_zoom", &mut state.zoom, state.effective_zoom, true);
            super::sync_viewport_sizes(ui, state, available);
        })
        .discard_textures()
    };
    let repaint_delay = |output: &egui::FullOutput| {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .repaint_delay
    };
    let mut available = egui::vec2(800.0, 600.0);
    for next in [egui::vec2(3000.0, 2000.0), available] {
        for _ in 0..8 {
            let _ = frame(&mut state, available);
        }
        assert!(
            !repaint_delay(&frame(&mut state, available)).is_zero(),
            "unchanged layout should idle"
        );
        let previous = state.effective_zoom;
        let changed = frame(&mut state, next);
        assert_ne!(state.effective_zoom, previous);
        assert_eq!(
            repaint_delay(&changed),
            Duration::ZERO,
            "the already-painted selector needs another frame"
        );

        let refreshed = frame(&mut state, next);
        let label = state.effective_zoom.label();
        assert!(
            refreshed.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == label)
            }),
            "the follow-up frame must paint the applied percentage"
        );
        available = next;
    }
    for _ in 0..8 {
        let _ = frame(&mut state, available);
    }
    assert!(
        !repaint_delay(&frame(&mut state, available)).is_zero(),
        "the final layout should idle"
    );
}

#[test]
fn oversized_bodies_letterbox_at_the_reported_zoom_limit() {
    use crate::test_egui::DiscardTextures;
    use egui::vec2;
    let ctx = egui::Context::default();
    for available in [vec2(12_000.0, 12_000.0), vec2(40_000.0, 120.0)] {
        let mut state = BrowserUiState {
            zoom: crate::panel_zoom::PanelZoom::new(4.0),
            ..Default::default()
        };
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                let (viewport, _) = super::sync_viewport_sizes(ui, &mut state, available);
                let frame = vec2(
                    f32::from(u16::try_from(viewport.0).expect("bounded frame")),
                    f32::from(u16::try_from(viewport.1).expect("bounded frame")),
                );
                let scale = super::frame_scale(available, frame, true);
                assert!(scale <= crate::panel_zoom::MAX_ZOOM);
                assert!((scale - state.effective_zoom.factor()).abs() < 0.001);
                assert!(u64::from(viewport.0) * u64::from(viewport.1) <= u64::from(super::MAX_FRAME_PIXELS));
                assert!(frame.x * scale <= available.x && frame.y * scale <= available.y);
            })
            .discard_textures();
    }
    let fixed_scale = super::frame_scale(vec2(1000.0, 1000.0), vec2(100.0, 100.0), false);
    assert!((fixed_scale - 10.0).abs() < f32::EPSILON);
}

#[test]
fn host_dimensions_survive_responsive_frame_limits() {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            for (size, expected) in [
                (egui::vec2(12_000.4, 12_000.6), (12_000, 12_001)),
                (egui::vec2(24.0, 30.0), (24, 30)),
            ] {
                for zoom in [0.25, 1.0, 4.0] {
                    let mut state = BrowserUiState {
                        zoom: crate::panel_zoom::PanelZoom::new(zoom),
                        ..Default::default()
                    };
                    let (frame, host) = super::sync_viewport_sizes(ui, &mut state, size);
                    assert_eq!(host, expected);
                    assert_ne!(frame, host, "only the responsive frame should be capped");
                }
            }
        })
        .discard_textures();
}

#[test]
fn fixed_browser_keeps_its_zoom_preference_during_gestures() {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let remote = horizon_core::browser::BrowserPanelState::inert_remote("target", "provider");
    let mut state = BrowserUiState {
        zoom: crate::panel_zoom::PanelZoom::new(1.5),
        ..Default::default()
    };
    let _ = ctx
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::Zoom(1.25)],
                ..Default::default()
            },
            |ui| apply_zoom_gesture(ui, &remote, &mut state, true),
        )
        .discard_textures();
    assert_eq!(state.zoom, crate::panel_zoom::PanelZoom::new(1.5));
}

#[test]
fn a_capped_selection_still_responds_to_the_first_gesture_back() {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let gesture = |zoom: f32, effective: f32, delta: f32| {
        let mut state = BrowserUiState {
            zoom: crate::panel_zoom::PanelZoom::new(zoom),
            effective_zoom: crate::panel_zoom::PanelZoom::new(effective),
            ..BrowserUiState::default()
        };
        let output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Zoom(delta)],
                ..Default::default()
            },
            |ui| apply_zoom_gesture(ui, &horizon_core::browser::BrowserPanelState::inert(), &mut state, true),
        );
        let _ = output.discard_textures();
        state.zoom.factor()
    };
    // A selection the panel had to raise to 85% zooms in from there, not
    // from the hidden 25%.
    assert!((gesture(0.25, 0.85, 1.2) - 1.02).abs() < 0.01);
    // Pushing further past the cap keeps walking the saved selection.
    assert!(
        (gesture(0.25, 0.85, 0.5) - 0.25).abs() < 0.01,
        "clamped at the range end"
    );
    // The same from the other side: a 400% selection capped to 130%.
    assert!((gesture(4.0, 1.3, 0.5) - 0.65).abs() < 0.01);
    assert!((gesture(4.0, 1.3, 1.5) - 4.0).abs() < 0.01, "clamped at the range end");
}

#[test]
fn a_zoom_gesture_rescales_and_asks_for_the_frame_that_resends_the_viewport() {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let zoom_input = || egui::RawInput {
        events: vec![egui::Event::Zoom(1.25)],
        ..Default::default()
    };
    let mut state = BrowserUiState::default();
    let mut repaint_requested = false;
    let output = ctx.run_ui(zoom_input(), |ui| {
        apply_zoom_gesture(ui, &horizon_core::browser::BrowserPanelState::inert(), &mut state, true);
        repaint_requested = ui.ctx().has_requested_repaint();
    });
    let _ = output.discard_textures();
    assert!((state.zoom.factor() - 1.25).abs() < 0.001);
    assert!(repaint_requested, "a static page needs the follow-up frame");

    // Off the body, and at the end of the range, nothing changes.
    let mut untouched = BrowserUiState::default();
    let output = ctx.run_ui(zoom_input(), |ui| {
        apply_zoom_gesture(
            ui,
            &horizon_core::browser::BrowserPanelState::inert(),
            &mut untouched,
            false,
        );
    });
    let _ = output.discard_textures();
    assert_eq!(untouched.zoom, crate::panel_zoom::PanelZoom::ONE);
    let mut clamped = BrowserUiState {
        zoom: crate::panel_zoom::PanelZoom::new(crate::panel_zoom::MAX_ZOOM),
        ..BrowserUiState::default()
    };
    let output = ctx.run_ui(zoom_input(), |ui| {
        apply_zoom_gesture(
            ui,
            &horizon_core::browser::BrowserPanelState::inert(),
            &mut clamped,
            true,
        );
    });
    let _ = output.discard_textures();
    assert!((clamped.zoom.factor() - crate::panel_zoom::MAX_ZOOM).abs() <= f32::EPSILON);
}
