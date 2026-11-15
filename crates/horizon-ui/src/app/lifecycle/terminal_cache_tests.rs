use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::Shape;
use horizon_core::{PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision, WindowConfig};

use super::super::DetachedWorkspaceViewportState;
use super::super::HorizonApp;
use super::super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::terminal_widget::TerminalGridCache;

const OFFSCREEN_POSITION: [f32; 2] = [100_000.0, 100_000.0];

fn offscreen_test_app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("cache-test", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.root_viewport_stabilizer = None;
    (temp, ctx, app)
}

fn spawn_script(app: &mut HorizonApp, temp: &tempfile::TempDir, script: &str) -> PanelId {
    let workspace = app.board.workspaces[0].id;
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), script.into()],
                cwd: Some(temp.path().to_path_buf()),
                transcript_root: Some(temp.path().join("transcripts")),
                position: Some(OFFSCREEN_POSITION),
                size: Some([600.0, 300.0]),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("synthetic terminal")
}

fn panel_text(app: &HorizonApp, panel_id: PanelId) -> String {
    app.board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("terminal")
        .last_lines_text(20)
}

/// Drains until the panel shows `marker`. Returns whether any drain asked
/// for a fast repaint.
fn drain_until_text(app: &mut HorizonApp, panel_id: PanelId, marker: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut fast_repaint = false;
    loop {
        fast_repaint |= app.drain_panel_output();
        if panel_text(app, panel_id).contains(marker) {
            // The marker can arrive in a later event batch than the first
            // drain that saw output; drain once more to settle the flag.
            fast_repaint |= app.drain_panel_output();
            return fast_repaint;
        }
        assert!(Instant::now() < deadline, "terminal must print {marker}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn append_text(shape: &Shape, text: &mut String) {
    match shape {
        Shape::Text(shape) => text.push_str(&shape.galley.job.text),
        Shape::Vec(shapes) => {
            for shape in shapes {
                append_text(shape, text);
            }
        }
        _ => {}
    }
}

#[test]
fn offscreen_output_invalidates_terminal_cache_without_a_fast_repaint() {
    let (temp, ctx, mut app) = offscreen_test_app();
    let panel_id = spawn_script(
        &mut app,
        &temp,
        r"stty -echo; printf 'READY'; read -r line; printf 'OFFSCREEN-UPDATE'; read -r line; printf 'ONSCREEN-UPDATE'; read -r line",
    );
    // Wait until terminal setup is finished before sending input or retaining a grid.
    drain_until_text(&mut app, panel_id, "READY");
    app.panel_render_caches
        .terminal_grid_cache
        .insert(panel_id, TerminalGridCache::default());
    app.board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("terminal")
        .write_input(b"update\n");
    assert!(
        !drain_until_text(&mut app, panel_id, "OFFSCREEN-UPDATE"),
        "output from a culled panel must not request a fast repaint"
    );
    assert!(
        !app.panel_render_caches.terminal_grid_cache.contains_key(&panel_id),
        "offscreen output must invalidate the retained grid"
    );
    app.board.panel_mut(panel_id).expect("panel").layout.position = [40.0, 60.0];
    app.board.focused = Some(panel_id);
    app.canvas_view = horizon_core::CanvasViewState::default();
    let mut text = String::new();
    for _ in 0..3 {
        for shape in run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None)).shapes {
            append_text(&shape.shape, &mut text);
        }
    }
    assert!(
        text.contains("OFFSCREEN-UPDATE"),
        "returning to the panel must paint its latest output"
    );
    assert!(app.panel_screen_rects.contains_key(&panel_id));
    app.board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("terminal")
        .write_input(b"update\n");
    assert!(
        drain_until_text(&mut app, panel_id, "ONSCREEN-UPDATE"),
        "output from a drawn panel must request a fast repaint"
    );
}

#[test]
fn fullscreen_and_detached_panels_count_as_drawn() {
    let (temp, _ctx, mut app) = offscreen_test_app();
    let script = |marker: &str| format!("stty -echo; printf 'READY'; read -r line; printf '{marker}'; sleep 30");
    let fullscreen = spawn_script(&mut app, &temp, &script("FULLSCREEN-OUTPUT"));
    let detached = spawn_script(&mut app, &temp, &script("DETACHED-OUTPUT"));
    // Both terminals must be quiet before each one gets its own output phase,
    // or one drain could consume the other panel's activity.
    drain_until_text(&mut app, fullscreen, "READY");
    drain_until_text(&mut app, detached, "READY");
    // A stale root rect must not count while another panel covers the window.
    app.panel_screen_rects.insert(detached, egui::Rect::EVERYTHING);
    app.fullscreen_panel = Some(fullscreen);
    let mut detached_state = DetachedWorkspaceViewportState::new(WindowConfig::default());
    detached_state
        .panel_screen_rects
        .insert(detached, egui::Rect::EVERYTHING);

    assert!(app.panel_drawn_last_frame(fullscreen));
    assert!(!app.panel_drawn_last_frame(detached));
    app.detached_workspaces.insert("detached-test".into(), detached_state);
    assert!(app.panel_drawn_last_frame(detached));

    for (panel_id, marker, message) in [
        (
            fullscreen,
            "FULLSCREEN-OUTPUT",
            "output from the fullscreen panel must request a fast repaint",
        ),
        (
            detached,
            "DETACHED-OUTPUT",
            "output from a panel in a detached window must request a fast repaint",
        ),
    ] {
        app.board
            .panel(panel_id)
            .expect("panel")
            .terminal()
            .expect("terminal")
            .write_input(b"go\n");
        assert!(drain_until_text(&mut app, panel_id, marker), "{message}");
    }
}

#[test]
fn offscreen_terminal_query_requests_a_fast_repaint() {
    let (temp, _ctx, mut app) = offscreen_test_app();
    // A cursor position query (DSR 6) waits for an answer from the terminal.
    let panel_id = spawn_script(
        &mut app,
        &temp,
        r"stty -echo -icanon min 0 time 50; printf 'ASK\033[6n'; dd bs=1 count=6 2>/dev/null >/dev/null; printf 'ANSWERED'; sleep 30",
    );
    assert!(
        drain_until_text(&mut app, panel_id, "ANSWERED"),
        "an answered terminal query must request a fast repaint"
    );
}

#[test]
fn the_window_asks_for_its_own_frame_when_the_answer_of_a_key_press_arrives() {
    let (temp, ctx, mut app) = offscreen_test_app();
    // The wake of this window counts its own calls and asks for the frame, as the
    // wake of a window does. It replaces the wake of the start before the first
    // frame, because a panel keeps the first wake that it takes.
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let wake: horizon_core::OutputWake = {
        let ctx = ctx.clone();
        Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            ctx.request_repaint();
        })
    };
    app.output_wake = wake;
    // The far shell answers every line well after the idle poll of the window fell
    // due, so only a wake of its panel can bring the frame that shows the answer.
    let panel_id = spawn_script(
        &mut app,
        &temp,
        r#"stty -echo; printf 'READY'; while IFS= read -r line; do sleep 1.5; printf 'ECHO-%s' "$line"; done"#,
    );
    drain_until_text(&mut app, panel_id, "READY");
    app.board.panel_mut(panel_id).expect("panel").layout.position = [40.0, 60.0];
    app.board.focused = Some(panel_id);
    app.canvas_view = horizon_core::CanvasViewState::default();
    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    }
    assert!(
        app.panel_screen_rects.contains_key(&panel_id),
        "the panel of the key press must be on screen"
    );

    let mut input = raw_input([1400.0, 900.0], None);
    input.events.push(egui::Event::Text("a\n".to_owned()));
    run_app_frame_with_input(&ctx, &mut app, input);
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        0,
        "no awake of the window before the answer"
    );
    assert!(
        !panel_text(&app, panel_id).contains("ECHO-a"),
        "the answer must still be on its way: {}",
        panel_text(&app, panel_id)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while wakes.load(Ordering::Relaxed) == 0 {
        assert!(
            Instant::now() < deadline,
            "the answer of a key press must ask for its own frame instead of the idle poll: grid={:?}",
            panel_text(&app, panel_id)
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    let mut text = String::new();
    for shape in run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None)).shapes {
        append_text(&shape.shape, &mut text);
    }
    assert!(
        text.contains("ECHO-a"),
        "the frame that the wake asked for paints the answer of the key press:\n{text}"
    );
}

/// The shapes of one frame of the window that paint the text `wanted`: the colour of the
/// text, and the pictures of the font atlas that each glyph of it points into.
fn painted_text(
    ctx: &egui::Context,
    app: &mut HorizonApp,
    wanted: &str,
) -> Vec<(egui::Color32, Vec<([u16; 2], [u16; 2])>)> {
    let output = run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
    let mut found = Vec::new();
    for clipped in output.shapes {
        collect_painted(&clipped.shape, wanted, &mut found);
    }
    found
}

fn collect_painted(shape: &Shape, wanted: &str, found: &mut Vec<(egui::Color32, Vec<([u16; 2], [u16; 2])>)>) {
    match shape {
        Shape::Text(text) if text.galley.job.text.contains(wanted) => {
            let glyphs = text
                .galley
                .rows
                .iter()
                .flat_map(|row| row.row.glyphs.iter().map(|glyph| (glyph.uv_rect.min, glyph.uv_rect.max)))
                .collect();
            // The window fades a panel in, and the fade scales the alpha of its text
            // only: the colour of the theme is its hue with no alpha.
            found.push((text.fallback_color.to_opaque(), glyphs));
        }
        Shape::Vec(shapes) => {
            for shape in shapes {
                collect_painted(shape, wanted, found);
            }
        }
        _ => {}
    }
}

/// A panel that prints nothing keeps its text through a change of the theme: the next
/// frames paint it with the colours and the font atlas of the new theme, and not with the
/// shapes that the cache of the grid built while the atlas of the old theme was current.
#[test]
fn a_theme_change_repaints_an_idle_terminal_with_the_atlas_of_that_theme() {
    const MARKER: &str = "IDLE-THEME-TEXT";
    let (temp, ctx, mut app) = offscreen_test_app();
    let panel_id = spawn_script(&mut app, &temp, "stty -echo; printf 'IDLE-THEME-TEXT'; sleep 60");
    drain_until_text(&mut app, panel_id, MARKER);
    app.board.panel_mut(panel_id).expect("panel").layout.position = [40.0, 60.0];
    app.board.focused = Some(panel_id);
    app.canvas_view = horizon_core::CanvasViewState::default();
    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    }
    let dark = painted_text(&ctx, &mut app, MARKER);
    assert!(!dark.is_empty(), "the idle terminal paints its text in the theme of the start");

    // The person picks the light theme in Settings, Appearance: the live preview of the
    // tab applies the draft, as `apply_live_preview` does.
    let mut config = app.template_config.clone();
    config.appearance.theme = horizon_core::AppearanceTheme::Light;
    app.apply_runtime_config(&config);
    // The frame that applies the theme, and the frame that builds the font atlas of it.
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    // A panel fades in, and the fade scales the colour of its text: this lane judges the
    // colour itself.
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let painted = painted_text(&ctx, &mut app, MARKER);

    // The same panel painted fresh in this theme, without the cache of its grid.
    app.panel_render_caches.terminal_grid_cache.remove(&panel_id);
    let fresh = painted_text(&ctx, &mut app, MARKER);
    assert_eq!(
        painted, fresh,
        "an idle terminal paints the shapes that the theme of this frame builds, and not a \
         cached mesh of the theme before it"
    );
    assert_ne!(dark, fresh, "the light theme paints other colours and other atlas pictures");

    // The frames between changes keep the cache of the grid, which paints the panel:
    // `terminal_widget::cache_tests::mouse_reporting_hover_and_canvas_pan_reuse_unchanged_grid`
    // holds the count of the rebuilds that those frames do not make.
    let again = painted_text(&ctx, &mut app, MARKER);
    assert_eq!(
        again, fresh,
        "a frame between changes paints the same terminal, from the cache of its grid"
    );
}
