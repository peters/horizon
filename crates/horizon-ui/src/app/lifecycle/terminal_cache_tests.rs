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
    let fullscreen = spawn_script(&mut app, &temp, "printf 'FULLSCREEN-OUTPUT'; sleep 30");
    let detached = spawn_script(&mut app, &temp, "printf 'DETACHED-OUTPUT'; sleep 30");
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

    assert!(
        drain_until_text(&mut app, fullscreen, "FULLSCREEN-OUTPUT"),
        "output from the fullscreen panel must request a fast repaint"
    );
    assert!(
        drain_until_text(&mut app, detached, "DETACHED-OUTPUT"),
        "output from a panel in a detached window must request a fast repaint"
    );
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
