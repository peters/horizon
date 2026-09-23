use super::*;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, epaint::Shape};
use horizon_core::{RuntimeState, StartupDecision};
use std::time::{Duration, Instant};

mod profiles;
mod reopening;
mod repository_picker;

fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<Event>, modifiers: Modifiers) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = std::iter::once(Event::ModifiersChanged(modifiers))
        .chain(events)
        .collect();
    run_app_frame_with_input(ctx, app, input);
}

fn key(ctx: &egui::Context, app: &mut HorizonApp, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        frame(
            ctx,
            app,
            vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
    }
}

fn label_position(output: &egui::FullOutput, label: &str) -> Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing visible label: {label}"))
}

fn click(ctx: &egui::Context, app: &mut HorizonApp, position: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(position)], Modifiers::NONE);
    for pressed in [true, false] {
        frame(
            ctx,
            app,
            vec![Event::PointerButton {
                pos: position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
    }
}

#[test]
fn opening_workspace_cloud_focuses_title_without_an_extra_click() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    let workspace = app.board.create_workspace("Sample project");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.pending_preset_pick = Some((Some(workspace), [400.0, 300.0], Instant::now()));
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Cloud" && text.pos.y > 200.0 => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap();
    for pressed in [true, false] {
        frame(
            &ctx,
            &mut app,
            vec![
                Event::PointerMoved(cloud),
                Event::PointerButton {
                    pos: cloud,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            Modifiers::NONE,
        );
    }
    assert!(app.cloud_creation_open());
    frame(
        &ctx,
        &mut app,
        vec![Event::Text("Feature workspace".into())],
        Modifiers::NONE,
    );
    assert_eq!(app.cloud_prototype.production.title, "Feature workspace");
}

#[test]
fn creation_tab_navigation_never_activates_the_toolbar() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    let _ = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.add_mock_cloud(&ctx);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    for _ in 0..16 {
        key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
        if let Some(response) = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id)) {
            assert_eq!(
                response.layer_id.id,
                Id::new("cloud-creation"),
                "modal focus escaped to another layer"
            );
        }
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Cancel" => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .unwrap();
    for _ in 0..32 {
        key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
        let focused = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id));
        assert!(app.command_palette.is_none(), "modal focus reached Quick Nav");
        assert!(app.settings.is_none(), "modal focus reached Settings");
        if focused.is_some_and(|response| response.rect.contains(cancel)) {
            key(&ctx, &mut app, Key::Space, Modifiers::NONE);
            assert!(!app.cloud_creation_open());
            return;
        }
    }
    panic!("keyboard navigation did not reach the dialog's Cancel action");
}

#[test]
fn creation_traversal_and_shortcuts_never_reach_the_focused_terminal() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let ready = temp.path().join("capture-ready");
    let received = temp.path().join("pty-input");
    let workspace = app.board.create_workspace("input isolation");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "stty raw -echo; : > \"$1\"; cat > \"$2\"".into(),
                    "capture".into(),
                    ready.to_string_lossy().into(),
                    received.to_string_lossy().into(),
                ],
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    app.board.focus(panel);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() || !received.exists() {
        assert!(Instant::now() < deadline, "PTY capture did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.add_mock_cloud(&ctx);
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    ctx.memory_mut(|memory| memory.request_focus(Id::new("cloud-title")));
    frame(&ctx, &mut app, vec![Event::Text("Deployment".into())], Modifiers::NONE);
    let repository = temp.path().join("committed-repository");
    std::fs::create_dir(&repository).unwrap();
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    frame(
        &ctx,
        &mut app,
        vec![Event::Text(repository.to_string_lossy().into())],
        Modifiers::NONE,
    );
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    frame(&ctx, &mut app, vec![Event::Text("HEAD".into())], Modifiers::NONE);
    assert_eq!(app.cloud_prototype.production.title, "Deployment");
    assert_eq!(
        std::path::Path::new(&app.cloud_prototype.production.repository),
        repository
    );
    assert_eq!(app.cloud_prototype.production.revision, "HEAD");
    key(&ctx, &mut app, Key::Tab, Modifiers::SHIFT);
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(
        app.dir_picker.is_some(),
        "Shift+Tab must return to the repository field"
    );
    key(&ctx, &mut app, Key::F11, Modifiers::NONE);
    assert!(app.fullscreen_panel.is_none());
    assert!(app.cloud_creation_open());
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(app.dir_picker.is_none());
    assert!(app.cloud_creation_open(), "Escape closes only the picker");
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.cloud_creation_open());
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        std::fs::read(&received).unwrap().is_empty(),
        "dialog input reached the PTY"
    );
    app.board.panel_mut(panel).unwrap().write_input(b"capture-check");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read(&received).unwrap() == b"capture-check" {
            break;
        }
        assert!(Instant::now() < deadline, "PTY capture must detect actual input");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "speech")]
#[test]
fn creation_keeps_speech_release_processing_but_blocks_new_recordings() {
    use crate::app::speech::{SpeechSink, SpeechSystem};
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let workspace = app.board.create_workspace("speech isolation");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    app.board.focus(panel);
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let (mut speech, channels) = SpeechSystem::with_test_bindings(&["F8"]);
    speech.start(SpeechSink::Panel(panel), 0);
    assert!(channels.capture_start_requested());
    app.speech = Some(speech);
    app.speech_engaged_profile = Some(0);
    app.speech_global_hotkeys_tried = true;
    app.cloud_prototype.production.creating = true;
    frame(
        &ctx,
        &mut app,
        vec![Event::Key {
            key: Key::F8,
            physical_key: Some(Key::F8),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );
    assert_eq!(app.speech.as_ref().unwrap().recording_target(), None);
    assert_eq!(app.speech_engaged_profile, None);
    app.speech.as_mut().unwrap().cancel();
    key(&ctx, &mut app, Key::F8, Modifiers::NONE);
    assert_eq!(app.speech.as_ref().unwrap().recording_target(), None);
    assert!(!channels.capture_start_requested());
    assert!(app.cloud_creation_open());
}
