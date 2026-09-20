use egui::{Context, Event, Key, Modifiers};
use horizon_core::ShortcutBinding;

use crate::app::HorizonApp;
use crate::app::test_support::test_app;
use crate::test_egui::DiscardTextures;

fn key(key: Key, pressed: bool, repeat: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers,
    }
}

fn filtered(ctx: &Context, app: &mut HorizonApp, events: Vec<Event>, focused: bool, time: f64) -> Vec<Event> {
    let mut input = egui::RawInput {
        events,
        time: Some(time),
        focused,
        ..Default::default()
    };
    input.events.insert(0, Event::WindowFocused(focused));
    input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
    let mut remaining = Vec::new();
    let _ = ctx
        .run_ui(input, |_ui| {
            app.filter_held_navigation_keys(ctx);
            app.handle_fullscreen_toggle(ctx);
            remaining = ctx.input(|i| i.events.clone());
        })
        .discard_textures();
    remaining
}

#[test]
fn printable_navigation_consumes_its_text_but_preserves_other_typing() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.shortcuts.fullscreen_window = ShortcutBinding::parse("Shift+F").unwrap();
    let result = filtered(
        &ctx,
        &mut app,
        vec![
            key(Key::F, true, false, Modifiers::SHIFT),
            Event::Text("F".into()),
            key(Key::X, true, false, Modifiers::NONE),
            Event::Text("x".into()),
        ],
        true,
        0.0,
    );
    assert!(result.contains(&Event::Text("x".into())));
    assert!(!result.contains(&Event::Text("F".into())));
    let released = filtered(
        &ctx,
        &mut app,
        vec![key(Key::F, false, false, Modifiers::NONE)],
        true,
        0.1,
    );
    assert!(!released.iter().any(|event| matches!(event, Event::Key { .. })));
}

#[test]
fn navigation_owns_late_release_after_refocus_but_not_a_new_press() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.shortcuts.fullscreen_window = ShortcutBinding::parse("F11").unwrap();
    filtered(
        &ctx,
        &mut app,
        vec![key(Key::F11, true, false, Modifiers::NONE)],
        true,
        0.0,
    );
    filtered(&ctx, &mut app, Vec::new(), false, 0.1);
    assert!(
        filtered(
            &ctx,
            &mut app,
            vec![key(Key::F11, false, false, Modifiers::NONE)],
            true,
            0.2
        )
        .iter()
        .all(|event| !matches!(event, Event::Key { .. }))
    );
    filtered(
        &ctx,
        &mut app,
        vec![key(Key::F11, true, false, Modifiers::NONE)],
        true,
        0.3,
    );
    filtered(&ctx, &mut app, Vec::new(), false, 0.4);
    app.shortcuts.fullscreen_window = ShortcutBinding::parse("F12").unwrap();
    app.shortcuts.fullscreen_panel = ShortcutBinding::parse("F10").unwrap();
    let fresh = key(Key::F11, true, false, Modifiers::NONE);
    assert!(filtered(&ctx, &mut app, vec![fresh.clone()], true, 0.5).contains(&fresh));
}

#[test]
fn unfocused_detached_viewport_does_not_expire_root_navigation() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.shortcuts.fullscreen_window = ShortcutBinding::parse("F11").unwrap();
    filtered(
        &ctx,
        &mut app,
        vec![key(Key::F11, true, false, Modifiers::NONE)],
        true,
        0.0,
    );
    let viewport = egui::ViewportId::from_hash_of("unfocused-detached");
    let mut input = egui::RawInput {
        viewport_id: viewport,
        focused: false,
        time: Some(0.1),
        ..Default::default()
    };
    input.viewports.entry(viewport).or_default().focused = Some(false);
    let _ = ctx
        .run_ui(input, |_ui| app.filter_held_navigation_keys(&ctx))
        .discard_textures();
    let repeated = filtered(
        &ctx,
        &mut app,
        vec![key(Key::F11, true, true, Modifiers::NONE)],
        true,
        5.0,
    );
    assert!(!repeated.iter().any(|event| matches!(event, Event::Key { .. })));
}
