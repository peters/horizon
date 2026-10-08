use super::*;
use crate::app::cloud_panel::production::rebuild::tests::{Phase, deployment};
use crate::app::cloud_panel::scroll_bar_tests::{assert_press_reaches_lower_control, input};
use crate::app::test_support::test_app_with_startup;
use egui::{Context, Event, LayerId};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

const ID: u32 = 901;

#[test]
fn management_wheel_scrolls_with_the_complete_canvas_render_path() {
    let (temp, ctx, mut app) = ready_card(1.0);
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.cloud_prototype.root = Some(temp.path().into());
    let workspace = app.board.create_workspace("Cloud fixture");
    app.cloud_prototype.groups.0[0].workspace = app.board.workspace(workspace).unwrap().local_id.clone();
    let mut time = 0.0;
    let mut render = |position, events| {
        time += 0.05;
        ctx.run_ui(input(Vec2::new(1600.0, 1000.0), time, position, events), |ui| {
            app.render_active_view(ui, false);
        })
        .discard_textures()
    };
    for _ in 0..4 {
        render(Pos2::ZERO, Vec::new());
    }
    let center = ctx
        .memory(|memory| memory.area_rect(Id::new(("cloud-drawer", ID))))
        .unwrap()
        .center();
    let before = render(center, Vec::new());
    let marker = |output: &egui::FullOutput| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "View" => Some(text.pos.y),
            _ => None,
        })
    };
    let first = marker(&before).unwrap();
    render(
        center,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -7.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let mut after = render(center, Vec::new());
    for _ in 0..10 {
        after = render(center, Vec::new());
    }
    assert!(
        marker(&after).is_none_or(|last| last < first - 10.0),
        "wheel moves the Manage tab's content"
    );
}

fn frame(ctx: &Context, app: &mut HorizonApp, time: f64, position: Pos2, events: Vec<Event>) {
    let _ = ctx
        .run_ui(input(Vec2::new(3000.0, 2200.0), time, position, events), |ui| {
            app.handle_canvas_pan(ui.ctx());
            app.render_production_runtimes(ui.ctx());
        })
        .discard_textures();
}

fn ready_card(zoom: f32) -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.cloud_prototype.ready = true;
    app.canvas_view = CanvasViewState::new([0.0, 0.0], zoom);
    let state = deployment(std::path::Path::new("/synthetic"), true, Phase::None);
    let mut group = CloudGroup::new(
        ID,
        "Hover fixture".into(),
        "workspace".into(),
        "/synthetic".into(),
        [10.0, 10.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "hover-fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: state.profile.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    let runtime = app.cloud_prototype.production.runtimes.entry(ID).or_default();
    runtime.stage = Some(Stage::Ready);
    runtime.state = Some(state);
    runtime.drawer = Some(super::super::Tab::Manage);
    // The long confirmation keeps Manage taller than the drawer at every zoom.
    runtime.confirmation = super::super::super::Confirmation::Delete;
    (temp, ctx, app)
}

#[test]
fn entering_the_card_scroll_area_keeps_lower_controls_under_the_pointer() {
    for zoom in [1.0_f32, 1.797] {
        let (_temp, ctx, mut app) = ready_card(zoom);
        let layer = LayerId::new(Order::Foreground, Id::new(("cloud-drawer", ID)));
        let mut time = 0.0;
        for _ in 0..3 {
            time += 0.05;
            frame(&ctx, &mut app, time, Pos2::new(2900.0, 2100.0), Vec::new());
        }
        let body = ctx
            .memory(|memory| memory.area_rect(layer.id))
            .expect("drawer")
            .center();
        assert_press_reaches_lower_control(
            &ctx,
            layer,
            body,
            Pos2::new(2900.0, 2100.0),
            &format!("runtime card at zoom {zoom}"),
            |position, events| {
                time += 0.05;
                frame(&ctx, &mut app, time, position, events);
            },
        );
    }
}

/// The texts that `output` draws.
fn drawn(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_stage_segment_shows_its_own_hover_text_over_the_card_hint() {
    use crate::app::cloud_panel::render::CARD_HINT;
    let (temp, ctx, mut app) = ready_card(1.0);
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.cloud_prototype.root = Some(temp.path().into());
    let workspace = app.board.create_workspace("Cloud fixture");
    app.cloud_prototype.groups.0[0].workspace = app.board.workspace(workspace).unwrap().local_id.clone();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&ID).unwrap();
    runtime.drawer = None;
    runtime.confirmation = super::super::super::Confirmation::None;
    // An image-only cloud, so the hover text of Build locally ends with `skipped`, a
    // text that only the hover text draws.
    runtime.state.as_mut().unwrap().profile.build = None;
    let size = Vec2::new(1600.0, 1000.0);
    let mut time = 0.0;
    // One frame; a pointer that does not move sends no event, as on a real screen.
    let mut render = |moved: Option<Pos2>| {
        time += 0.05;
        let input = match moved {
            Some(position) => input(size, time, position, Vec::new()),
            None => egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(time),
                ..egui::RawInput::default()
            },
        };
        ctx.run_ui(input, |ui| app.render_active_view(ui, false))
            .discard_textures()
    };
    for _ in 0..4 {
        render(Some(Pos2::ZERO));
    }
    let layer = LayerId::new(Order::Middle, Id::new(("cloud-header", ID)));
    let header = ctx
        .memory(|memory| memory.area_rect(layer.id))
        .expect("the card header");
    let to_screen = ctx.layer_transform_to_global(layer).unwrap_or_default();
    // Rests the pointer at `at` for a second, longer than the hover text delay.
    let mut rest = |at: Pos2| {
        let mut output = render(Some(to_screen * at));
        for _ in 0..20 {
            output = render(None);
        }
        drawn(&output)
    };
    // The track spans the header 1 pt in from each side along its bottom edge, one segment
    // for each deployment stage with 2 pt between them (`strip::paint_track`).
    let stages = crate::app::util::usize_to_f32(Stage::ALL.len());
    let index = crate::app::util::usize_to_f32(Stage::ALL.iter().position(|stage| *stage == Stage::Build).unwrap());
    let segment = (header.width() - 2.0 - 2.0 * (stages - 1.0)) / stages;
    let build = Pos2::new(
        header.left() + 1.0 + index * (segment + 2.0) + segment / 2.0,
        header.bottom() - 2.0,
    );
    let on_segment = rest(build);
    assert!(
        on_segment.iter().any(|text| text == "Build locally · skipped"),
        "the segment names its stage: {on_segment:?}"
    );
    assert!(!on_segment.iter().any(|text| text == CARD_HINT), "{on_segment:?}");
    let away = rest(Pos2::new(header.center().x, header.bottom() + 400.0));
    assert!(!away.iter().any(|text| text == CARD_HINT || text.ends_with("· skipped")));
    let on_title = rest(Pos2::new(header.left() + 140.0, header.top() + 22.0));
    assert!(
        on_title.iter().any(|text| text == CARD_HINT),
        "the card keeps its hint elsewhere: {on_title:?}"
    );
    assert!(!on_title.iter().any(|text| text.ends_with("· skipped")));
}

#[test]
fn a_production_cloud_renames_from_a_click_on_its_title() {
    let (temp, ctx, mut app) = ready_card(1.0);
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.cloud_prototype.root = Some(temp.path().into());
    let workspace = app.board.create_workspace("Cloud fixture");
    app.cloud_prototype.groups.0[0].workspace = app.board.workspace(workspace).unwrap().local_id.clone();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&ID).unwrap();
    runtime.drawer = None;
    runtime.confirmation = super::super::super::Confirmation::None;
    let size = Vec2::new(1600.0, 1000.0);
    let mut time = 0.0;
    let mut render = |app: &mut HorizonApp, position: Pos2, events: Vec<Event>| {
        time += 0.05;
        ctx.run_ui(input(size, time, position, events), |ui| {
            app.render_active_view(ui, false);
        })
        .discard_textures()
    };
    for _ in 0..4 {
        render(&mut app, Pos2::ZERO, Vec::new());
    }
    let layer = LayerId::new(Order::Middle, Id::new(("cloud-header", ID)));
    let header = ctx
        .memory(|memory| memory.area_rect(layer.id))
        .expect("the card header");
    let to_screen = ctx.layer_transform_to_global(layer).unwrap_or_default();
    let beside = to_screen * Pos2::new(header.left() + 100.0, header.top() + 58.0);
    for pressed in [true, false] {
        let event = Event::PointerButton {
            pos: beside,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(&mut app, beside, vec![event]);
    }
    render(&mut app, beside, Vec::new());
    assert_eq!(
        app.cloud_prototype.renaming, None,
        "a click on the subtitle leaves the title"
    );
    let title = to_screen * Pos2::new(header.left() + 100.0, header.top() + 30.0);
    let button = |pressed| Event::PointerButton {
        pos: title,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for pressed in [true, false] {
        render(&mut app, title, vec![button(pressed)]);
    }
    render(&mut app, title, Vec::new());
    assert_eq!(app.cloud_prototype.renaming, Some(ID), "a click on the title edits it");
    let key = |key, pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: if key == egui::Key::A {
            egui::Modifiers::COMMAND
        } else {
            egui::Modifiers::NONE
        },
    };
    render(&mut app, title, vec![key(egui::Key::A, true), key(egui::Key::A, false)]);
    render(&mut app, title, vec![Event::Text("Renamed cloud".into())]);
    render(
        &mut app,
        title,
        vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
    );
    render(&mut app, title, Vec::new());
    assert_eq!(app.cloud_prototype.renaming, None);
    assert_eq!(app.cloud_prototype.groups.0[0].title, "Renamed cloud");
}
