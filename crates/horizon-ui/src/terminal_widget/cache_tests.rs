use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use egui::{Context, Event, Pos2, RawInput, Rect, Vec2};
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::{TerminalGridCache, TerminalKeyboardContext, TerminalSelectionDragState, TerminalView};
use crate::primary_selection::PrimarySelection;
use crate::test_egui::DiscardTextures;

struct TerminalHarness {
    ctx: Context,
    panel: Panel,
    cache: TerminalGridCache,
    primary_selection: PrimarySelection,
    selection_drag: TerminalSelectionDragState,
    _state: tempfile::TempDir,
}

impl TerminalHarness {
    fn new() -> Self {
        let state = tempfile::tempdir().expect("private terminal state");
        let panel = Panel::spawn(
            PanelId(42),
            WorkspaceId(7),
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    r"stty -echo; printf '\033[?1003h\033[?1006hREADY'; i=0; while IFS= read -r line; do i=$((i+1)); printf '\rUPDATED-%s' $i; done".into(),
                ],
                cwd: Some(state.path().to_path_buf()),
                transcript_root: Some(state.path().to_path_buf()),
                rows: 16,
                cols: 60,
                ..PanelOptions::default()
            },
        )
        .expect("spawn isolated test terminal");
        let mut harness = Self {
            ctx: Context::default(),
            panel,
            cache: TerminalGridCache::default(),
            primary_selection: PrimarySelection::new(),
            selection_drag: TerminalSelectionDragState::default(),
            _state: state,
        };
        harness.wait_for_text("READY");
        assert!(
            harness
                .panel
                .terminal()
                .expect("terminal")
                .with_renderable_content(|content| { content.mode.intersects(TermMode::MOUSE_MODE) })
        );
        harness
    }

    fn wait_for_text(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.panel.process_output();
            if self.panel.had_recent_output()
                && self
                    .panel
                    .terminal()
                    .expect("terminal")
                    .last_lines_text(20)
                    .contains(text)
            {
                return;
            }
            assert!(Instant::now() < deadline, "terminal did not produce {text}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn frame(&mut self, pointer: Pos2, interactive: bool) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0))),
            events: vec![Event::PointerMoved(pointer)],
            ..RawInput::default()
        };
        let _ = self
            .ctx
            .run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    TerminalView::new(&mut self.panel, Some(&mut self.cache)).show(
                        ui,
                        true,
                        interactive,
                        &mut self.selection_drag,
                        TerminalKeyboardContext {
                            keyboard_events: &[],
                            primary_selection: &self.primary_selection,
                            local_ssh_reconnect_enabled: false,
                            reconnect_requested: &mut false,
                        },
                    );
                });
            })
            .discard_textures();
    }
}

#[test]
fn mouse_reporting_hover_and_canvas_pan_reuse_unchanged_grid() {
    let mut harness = TerminalHarness::new();
    // Warm geometry and drain the resize wakeup before measuring quiet frames.
    for _ in 0..5 {
        harness.panel.process_output();
        harness.frame(Pos2::new(40.0, 40.0), true);
    }
    // A PTY round trip acknowledges startup and resize before counting reuse.
    harness.panel.terminal().expect("terminal").write_input(b"ready\n");
    harness.wait_for_text("UPDATED-1");
    harness.frame(Pos2::new(40.0, 40.0), true);
    harness.panel.process_output();
    let before = harness.cache.rebuilds;
    assert!(before > 0);
    for interactive in [true, false] {
        for x in [50.0, 80.0, 110.0] {
            harness.panel.process_output();
            harness.frame(Pos2::new(x, 40.0), interactive);
        }
    }
    assert_eq!(
        harness.cache.rebuilds, before,
        "pointer motion must not rebuild unchanged terminal text"
    );

    harness.panel.terminal().expect("terminal").write_input(b"update\n");
    harness.wait_for_text("UPDATED-2");
    harness.frame(Pos2::new(110.0, 40.0), true);
    assert!(
        harness.cache.rebuilds > before,
        "real terminal output must refresh the cached grid"
    );
}

#[test]
fn tab_does_not_steal_focus_from_another_text_field() {
    let mut harness = TerminalHarness::new();
    let field = egui::Id::new("synthetic-pin");
    let mut pin = String::new();
    let mut pair_button = None;
    for (frame, events) in [
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ]
    .into_iter()
    .enumerate()
    {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..RawInput::default()
        };
        let _ = harness
            .ctx
            .run_ui(input, |ui| {
                TerminalView::new(&mut harness.panel, Some(&mut harness.cache)).show(
                    ui,
                    true,
                    true,
                    &mut harness.selection_drag,
                    TerminalKeyboardContext {
                        keyboard_events: &[],
                        primary_selection: &harness.primary_selection,
                        local_ssh_reconnect_enabled: false,
                        reconnect_requested: &mut false,
                    },
                );
                egui::Window::new("Synthetic pairing")
                    .fixed_pos(Pos2::new(30.0, 30.0))
                    .show(ui.ctx(), |ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut pin).id(field).password(true));
                        if !ui.input(|input| input.key_pressed(egui::Key::Tab)) {
                            response.request_focus();
                        }
                        pair_button = Some(ui.button("Pair").id);
                    });
            })
            .discard_textures();
        if frame >= 2 {
            assert_eq!(
                harness.ctx.memory(egui::Memory::focused),
                if frame == 2 { Some(field) } else { pair_button }
            );
        }
    }
}
