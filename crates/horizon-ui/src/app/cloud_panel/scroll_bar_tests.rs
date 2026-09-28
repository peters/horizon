//! A press aimed at the right end of a full-width control inside a cloud scroll area must reach
//! the control, not the area's scroll bar.
use super::HorizonApp;
use crate::app::test_support::test_app_with_startup;
use crate::test_egui::DiscardTextures;
use egui::{Context, Event, Id, LayerId, Modifiers, MouseWheelUnit, Order, Pos2, RawInput, Rect, TouchPhase, Vec2};
use horizon_core::{CanvasViewState, RuntimeState, StartupDecision, cloud_panel::CloudGroup};

/// The layer's focusable click targets: id, visible rect in layer coordinates and whether the
/// whole control is visible, in paint order.
fn controls(ctx: &Context, layer: LayerId) -> Vec<(Id, Rect, bool)> {
    ctx.viewport(|viewport| {
        viewport
            .this_pass
            .widgets
            .get_layer(layer)
            .filter(|widget| {
                widget.sense.senses_click() && widget.sense.is_focusable() && widget.interact_rect.is_positive()
            })
            .map(|widget| (widget.id, widget.interact_rect, widget.interact_rect == widget.rect))
            .collect()
    })
}

/// Scrolls the area under `body` (layer coordinates) part way down, then aims 2 px inside the right
/// end of the lowest whole full-width control that scrolled. Hovering and pressing there must move
/// nothing and the press must reach that control. No scrolled control may reach further right,
/// under the bar. `frame` runs one frame with the pointer at a screen position and more events.
pub(super) fn assert_press_reaches_lower_control(
    ctx: &Context,
    layer: LayerId,
    body: Pos2,
    outside: Pos2,
    scenario: &str,
    mut frame: impl FnMut(Pos2, Vec<Event>),
) {
    for _ in 0..3 {
        frame(outside, Vec::new());
    }
    let to_screen = ctx.layer_transform_to_global(layer).unwrap_or_default();
    let top = controls(ctx, layer);
    // Scroll part way down, so a jump in either direction would show.
    let wheel = Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -120.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    frame(to_screen * body, vec![wheel]);
    // Wheel scrolling is smoothed over the next frames, which reach only a hovered area.
    for _ in 0..40 {
        frame(to_screen * body, Vec::new());
    }
    for _ in 0..40 {
        frame(outside, Vec::new());
    }
    let before = controls(ctx, layer);
    let scrolled: Vec<_> = before.iter().filter(|control| !top.contains(control)).collect();
    let widest = scrolled.iter().map(|(_, rect, _)| rect.width()).fold(0.0, f32::max);
    let (target, rect, _) = **scrolled
        .iter()
        .filter(|(_, rect, whole)| *whole && rect.width() > widest * 0.9)
        .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
        .unwrap_or_else(|| panic!("{scenario}: the scroll area shows a whole full-width control once scrolled"));
    for (_, other, _) in &scrolled {
        assert!(
            other.right() <= rect.right() + 0.5,
            "{scenario}: a control reaches under the scroll bar: {other:?} beside {rect:?}"
        );
    }
    let aim = to_screen * Pos2::new(rect.right() - 2.0, rect.center().y);
    for _ in 0..20 {
        frame(aim, Vec::new());
        assert_eq!(controls(ctx, layer), before, "{scenario}: hovering moved the controls");
    }
    frame(
        aim,
        vec![Event::PointerButton {
            pos: aim,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    frame(aim, Vec::new());
    assert_eq!(controls(ctx, layer), before, "{scenario}: pressing moved the controls");
    assert!(
        ctx.read_response(target)
            .is_some_and(|response| response.is_pointer_button_down_on()),
        "{scenario}: the press must land on the control the pointer aimed at"
    );
}

/// One frame's input on a `size` screen with the pointer at `position`.
pub(super) fn input(size: Vec2, time: f64, position: Pos2, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        time: Some(time),
        events: std::iter::once(Event::PointerMoved(position)).chain(events).collect(),
        ..RawInput::default()
    }
}

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.cloud_prototype.root = Some(temp.path().join("cloud"));
    app.cloud_prototype.ready = true;
    (temp, ctx, app)
}

#[test]
fn demo_card_press_on_a_lower_control_reaches_it() {
    const ID: u32 = 902;
    let (_temp, ctx, mut app) = app();
    // A long image name wraps over several lines, so the card body scrolls.
    let image = format!("example.invalid/{}worker", "team/".repeat(30));
    app.cloud_prototype.profiles = Some(
        horizon_core::cloud_panel::CloudConfig::parse(&format!(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: {image}\n    cpu: 4\n    memory_gb: 8\n"
        ))
        .unwrap(),
    );
    app.canvas_view = CanvasViewState::new([0.0, 0.0], 1.0);
    let group = CloudGroup::new(
        ID,
        "Demo fixture".into(),
        "workspace".into(),
        "/synthetic".into(),
        [10.0, 10.0],
    );
    let body = Pos2::from(group.runtime_bounds().0) + Vec2::new(100.0, 300.0);
    app.cloud_prototype.groups.0.push(group);
    let layer = LayerId::new(Order::Middle, Id::new(("cloud-runtime", ID)));
    let mut time = 0.0;
    assert_press_reaches_lower_control(
        &ctx,
        layer,
        body,
        Pos2::new(2900.0, 2100.0),
        "demo card",
        |position, events| {
            time += 0.05;
            let _ = ctx
                .run_ui(input(Vec2::new(3000.0, 2200.0), time, position, events), |ui| {
                    app.render_cloud_runtimes(ui.ctx());
                })
                .discard_textures();
        },
    );
}

#[test]
fn new_cloud_dialog_press_on_a_lower_control_reaches_it() {
    let (_temp, ctx, mut app) = app();
    app.cloud_prototype.production.creating = true;
    // A short screen leaves the dialog body too little height for its fields.
    let size = Vec2::new(900.0, 520.0);
    let mut time = 0.0;
    let mut frame = |app: &mut HorizonApp, position: Pos2, events: Vec<Event>| {
        time += 0.05;
        let _ = ctx
            .run_ui(input(size, time, position, events), |ui| {
                app.render_cloud_creation(ui.ctx())
            })
            .discard_textures();
    };
    frame(&mut app, Pos2::ZERO, Vec::new());
    let body = ctx
        .read_response(Id::new("cloud-title"))
        .expect("the dialog shows its title field")
        .rect
        .center();
    let layer = LayerId::new(Order::Tooltip, Id::new("cloud-creation"));
    assert_press_reaches_lower_control(
        &ctx,
        layer,
        body,
        Pos2::new(4.0, 4.0),
        "New cloud",
        |position, events| {
            frame(&mut app, position, events);
        },
    );
}

#[test]
fn cloud_settings_press_on_a_lower_control_reaches_it() {
    let (_temp, ctx, mut app) = app();
    app.open_cloud_accounts(&ctx, false);
    // A short screen leaves the Accounts section too little height once Hetzner adds its fields.
    let size = Vec2::new(900.0, 460.0);
    let mut time = 0.0;
    let mut frame = |app: &mut HorizonApp, position: Pos2, events: Vec<Event>| {
        time += 0.05;
        ctx.run_ui(input(size, time, position, events), |ui| {
            app.render_cloud_accounts(ui.ctx())
        })
        .discard_textures()
    };
    let label = |output: &egui::FullOutput, label: &str| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let hetzner = loop {
        let output = frame(&mut app, Pos2::ZERO, Vec::new());
        if let Some(hetzner) = label(&output, "Hetzner Cloud (CPU only)") {
            break hetzner;
        }
        assert!(std::time::Instant::now() < deadline, "the settings did not load");
        std::thread::yield_now();
    };
    for pressed in [true, false] {
        let button = Event::PointerButton {
            pos: hetzner,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&mut app, hetzner, vec![button]);
    }
    let token = label(&frame(&mut app, Pos2::ZERO, Vec::new()), "Hetzner Cloud API token")
        .expect("enabling Hetzner shows its token field");
    let layer = LayerId::new(Order::Tooltip, Id::new("cloud-accounts"));
    assert_press_reaches_lower_control(
        &ctx,
        layer,
        token,
        Pos2::new(4.0, 4.0),
        "Cloud settings",
        |position, events| {
            frame(&mut app, position, events);
        },
    );
}
