use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use egui::{Context, Event, Pos2, RawInput, Rect, Vec2};
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::{TerminalGridCache, TerminalKeyboardContext, TerminalSelectionDragState, TerminalView};
use crate::primary_selection::PrimarySelection;
use crate::test_egui::DiscardTextures;

// Canonical reads wait for a newline, so mouse reports sit in the slave input
// queue. A full queue can block the PTY writer, and the event-loop join then
// keeps the test process alive. Non-canonical reads drain every byte and exit on
// EOF.
const MOUSE_CHILD_ARGS: &str = r#"stty -echo -icanon min 1 time 0 || exit 1
printf '\033[?1003h\033[?1006hREADY'
i=0
while IFS= read -r -n 1 byte; do
  # bash read drops the delimiter, so a newline arrives as an empty byte.
  if [ -z "$byte" ]; then
    i=$((i + 1))
    printf '\rUPDATED-%s' "$i"
  fi
done
exit 0
"#;

// Enough output to fill the scrollback, then a read loop that exits on EOF and
// drains every byte, so a chord that reaches the PTY cannot block the join.
const SCROLLBACK_CHILD_ARGS: &str = r#"i=0
while [ "$i" -lt 200 ]; do
  printf 'LINE-%s\n' "$i"
  i=$((i + 1))
done
printf 'TAIL-MARKER\n'
while IFS= read -r byte; do :; done
exit 0
"#;

struct TerminalHarness {
    ctx: Context,
    panel: Panel,
    cache: TerminalGridCache,
    primary_selection: PrimarySelection,
    clipboard_images: crate::clipboard_image::ClipboardImages,
    selection_drag: TerminalSelectionDragState,
    _state: tempfile::TempDir,
}

impl TerminalHarness {
    fn new() -> Self {
        let mut harness = Self::with_child(MOUSE_CHILD_ARGS);
        harness.wait_for_text("READY");
        harness.wait_for_mouse_mode();
        harness
    }

    fn scrollback() -> Self {
        let mut harness = Self::with_child(SCROLLBACK_CHILD_ARGS);
        harness.wait_until_screen_shows("TAIL-MARKER");
        harness
    }

    fn with_child(args: &str) -> Self {
        let state = tempfile::tempdir().expect("private terminal state");
        let panel = Panel::spawn(
            PanelId(42),
            WorkspaceId(7),
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/bash".into()),
                args: vec!["-c".into(), args.into()],
                cwd: Some(state.path().to_path_buf()),
                transcript_root: Some(state.path().to_path_buf()),
                rows: 16,
                cols: 60,
                ..PanelOptions::default()
            },
        )
        .expect("spawn isolated test terminal");
        Self {
            ctx: Context::default(),
            panel,
            cache: TerminalGridCache::default(),
            primary_selection: PrimarySelection::new(),
            clipboard_images: crate::clipboard_image::ClipboardImages::new(),
            selection_drag: TerminalSelectionDragState::default(),
            _state: state,
        }
    }

    /// Returns after a poll that drained output and saw `text`, so the next frame sees output.
    /// `alacritty_terminal` parses PTY bytes and queues its wakeup while it holds the grid lock.
    /// So each poll retries the snapshot until the grid is free: a poll that drains a wakeup then
    /// sees its text. A poll can also find no output and then see text parsed after that drain.
    /// The wakeup for that text is still in the queue, so the poll does not count. The next poll
    /// drains the wakeup and sees the text.
    fn wait_for_text(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.panel.process_output();
            let had_output = self.panel.had_recent_output();
            let lines = loop {
                if let Some(lines) = self.panel.terminal().expect("terminal").try_last_lines_text(20) {
                    break lines;
                }
                assert!(Instant::now() < deadline, "terminal grid stayed busy");
                std::thread::sleep(Duration::from_millis(1));
            };
            if had_output && lines.contains(text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no poll drained output and saw {text}; screen was:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls until the screen shows `text`. Unlike [`Self::wait_for_text`] this
    /// does not require the poll that drained the output, which matters for a
    /// fixture that prints everything at once and then waits for input.
    fn wait_until_screen_shows(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.panel.process_output();
            if self.text().contains(text) {
                return;
            }
            assert!(Instant::now() < deadline, "the screen never showed {text}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn text(&self) -> String {
        self.panel.terminal().expect("terminal").last_lines_text(20)
    }

    /// Polls until a poll finds no output, so the next frame has no pending wakeup.
    fn drain_output(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.panel.process_output();
            if !self.panel.had_recent_output() {
                return;
            }
            assert!(Instant::now() < deadline, "terminal kept reporting output");
        }
    }

    fn wait_for_mouse_mode(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.panel.process_output();
            let enabled = self
                .panel
                .terminal()
                .expect("terminal")
                .try_with_renderable_content(|content| content.mode.intersects(TermMode::MOUSE_MODE));
            if enabled == Some(true) {
                return;
            }
            assert!(Instant::now() < deadline, "terminal did not enable mouse reporting");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// One frame with `events` offered to the focused terminal, returning the
    /// viewport commands the frame sent to the window system.
    fn frame_with_events(&mut self, events: Vec<Event>) -> Vec<egui::ViewportCommand> {
        let keyboard_events = crate::input::terminal_input_events(&events, Vec::new());
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0))),
            events,
            ..RawInput::default()
        };
        self.ctx
            .run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    TerminalView::new(&mut self.panel, Some(&mut self.cache), 0.0).show(
                        ui,
                        true,
                        true,
                        &mut self.selection_drag,
                        TerminalKeyboardContext {
                            keyboard_events: &keyboard_events,
                            primary_selection: &self.primary_selection,
                            clipboard_images: &self.clipboard_images,
                            local_ssh_reconnect_enabled: false,
                            reconnect_requested: &mut false,
                        },
                    );
                });
            })
            .discard_textures()
            .viewport_output
            .remove(&egui::ViewportId::ROOT)
            .map(|output| output.commands)
            .unwrap_or_default()
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
                    TerminalView::new(&mut self.panel, Some(&mut self.cache), 0.0).show(
                        ui,
                        true,
                        interactive,
                        &mut self.selection_drag,
                        TerminalKeyboardContext {
                            keyboard_events: &[],
                            primary_selection: &self.primary_selection,
                            clipboard_images: &self.clipboard_images,
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
    let screen = harness.text();
    // alacritty_terminal can send a Wakeup with no output: when its loop takes an input message
    // before the sender wakes the poller, that wakeup finds nothing to do and is sent as one.
    // Each frame starts after a poll with no output, so such a wakeup cannot rebuild the grid.
    for interactive in [true, false] {
        for x in [50.0, 80.0, 110.0] {
            harness.drain_output();
            let rebuilds = harness.cache.rebuilds;
            harness.frame(Pos2::new(x, 40.0), interactive);
            assert_eq!(
                harness.cache.rebuilds, rebuilds,
                "pointer motion must not rebuild unchanged terminal text (x {x}, interactive {interactive})"
            );
        }
    }
    assert_eq!(harness.text(), screen, "pointer reports must not change the screen");

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
    let field = egui::Id::unique("synthetic-pin");
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
                TerminalView::new(&mut harness.panel, Some(&mut harness.cache), 0.0).show(
                    ui,
                    true,
                    true,
                    &mut harness.selection_drag,
                    TerminalKeyboardContext {
                        keyboard_events: &[],
                        primary_selection: &harness.primary_selection,
                        clipboard_images: &harness.clipboard_images,
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

fn scrollback(harness: &TerminalHarness) -> usize {
    harness.panel.terminal().expect("terminal").scrollback()
}

fn chord(key: egui::Key, modifiers: egui::Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn terminal_clipboard_chords_ask_the_window_for_the_clipboard() {
    let mut harness = TerminalHarness::new();
    // The terminal must be reachable by the frame's focus resolution first.
    harness.frame(Pos2::new(40.0, 40.0), true);
    for modifiers in [egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Modifiers::SHIFT] {
        let key = if modifiers.ctrl {
            egui::Key::V
        } else {
            egui::Key::Insert
        };
        let commands = harness.frame_with_events(vec![chord(key, modifiers)]);
        assert!(
            commands.contains(&egui::ViewportCommand::RequestPaste),
            "{modifiers:?}+{key:?} must ask the window for the clipboard, got {commands:?}"
        );
    }
}

#[test]
fn shift_page_and_home_end_scroll_the_scrollback_without_touching_the_program() {
    let mut harness = TerminalHarness::scrollback();
    harness.frame(Pos2::new(40.0, 40.0), true);

    let history = harness.panel.terminal().expect("terminal").history_size();
    assert!(
        history > 20,
        "the fixture must fill the scrollback, got {history} lines"
    );
    assert_eq!(scrollback(&harness), 0, "the newest output is in view");

    let commands = harness.frame_with_events(vec![chord(egui::Key::PageUp, egui::Modifiers::SHIFT)]);
    assert!(
        commands.is_empty(),
        "a scroll chord must not ask the window for anything"
    );
    assert_eq!(
        scrollback(&harness),
        15,
        "one page of a 16-row panel, keeping one line of context"
    );

    harness.frame_with_events(vec![chord(egui::Key::PageUp, egui::Modifiers::SHIFT)]);
    assert_eq!(scrollback(&harness), 30, "a repeated page press keeps scrolling");

    harness.frame_with_events(vec![chord(egui::Key::PageDown, egui::Modifiers::SHIFT)]);
    assert_eq!(scrollback(&harness), 15);

    harness.frame_with_events(vec![chord(egui::Key::Home, egui::Modifiers::SHIFT)]);
    assert_eq!(scrollback(&harness), history, "one keypress reaches the oldest output");

    harness.frame_with_events(vec![chord(egui::Key::End, egui::Modifiers::SHIFT)]);
    assert_eq!(scrollback(&harness), 0, "one keypress returns to the newest output");

    harness.frame_with_events(vec![chord(
        egui::Key::ArrowUp,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    )]);
    assert_eq!(scrollback(&harness), 1, "a control arrow scrolls a single line");
}
