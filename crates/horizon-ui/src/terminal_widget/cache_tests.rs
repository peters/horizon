use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use egui::{Context, Event, Pos2, RawInput, Rect, Vec2};
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::{PanelNoticeAction, TerminalGridCache, TerminalKeyboardContext, TerminalSelectionDragState, TerminalView};
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

// The fixture echoes every byte that a person types, as a shell of this
// computer does, and prints nothing else after its ready line.
const ECHO_CHILD_ARGS: &str = "printf 'READY'; exec cat";

/// The longest wait for a step of a fixture. It is a hang guard and not a rule of
/// the product: a loaded computer starts the program of a panel late. The program
/// printed nothing within the earlier bound of 5 s in 11 of 20 runs of the suite
/// beside 24 busy loops on a computer at load 60.
const PATIENCE: Duration = Duration::from_secs(60);

// A program that prints without end, which a frame must not follow.
const CHATTY_CHILD_ARGS: &str = "i=0\nwhile :; do printf 'row-%s\\n' \"$i\"; i=$((i + 1)); sleep 0.005; done";

/// A wake of the window that counts its requests instead of drawing a frame.
fn counting_wake() -> (Arc<AtomicUsize>, horizon_core::OutputWake) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    // The frames that an echo asks for. An event of the program asks for a pass
    // of a window that draws nothing, which is not what these tests count.
    let wake: horizon_core::OutputWake = Arc::new(move |wake| {
        counter.fetch_add(usize::from(wake == horizon_core::Wake::Echo), Ordering::Relaxed);
    });
    (wakes, wake)
}

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
        harness.wait_for_text("TAIL-MARKER");
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

    /// Returns when the screen shows `text` and the last poll of the panel found output, so the
    /// frame that draws the panel rebuilds its grid. `alacritty_terminal` parses PTY bytes and
    /// queues its wakeup while it holds the grid lock, so the snapshot retries until the grid is
    /// free. The output can reach the grid before this call runs, and the poll that found it is
    /// then one that already ran: a wait for a poll that both drains a wakeup and sees the text
    /// would wait for a wakeup that a loaded computer never sends again.
    fn wait_for_text(&mut self, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let lines = loop {
                if let Some(lines) = self.panel.terminal().expect("terminal").try_last_lines_text(20) {
                    break lines;
                }
                assert!(Instant::now() < deadline, "terminal grid stayed busy");
                std::thread::sleep(Duration::from_millis(1));
            };
            if lines.contains(text) && self.panel.had_recent_output() {
                return;
            }
            self.panel.process_output();
            assert!(
                Instant::now() < deadline,
                "the screen never showed {text} after a poll that found output; screen was:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn text(&self) -> String {
        self.panel.terminal().expect("terminal").last_lines_text(20)
    }

    /// Polls until a poll finds no output, so the next frame has no pending wakeup.
    fn drain_output(&mut self) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.panel.process_output();
            if !self.panel.had_recent_output() {
                return;
            }
            assert!(Instant::now() < deadline, "terminal kept reporting output");
        }
    }

    fn wait_for_mouse_mode(&mut self) {
        let deadline = Instant::now() + PATIENCE;
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
                            notice_action: &mut PanelNoticeAction::default(),
                            take_keyboard_focus: false,
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
                            notice_action: &mut PanelNoticeAction::default(),
                            take_keyboard_focus: false,
                        },
                    );
                });
            })
            .discard_textures();
    }
}

impl Drop for TerminalHarness {
    fn drop(&mut self) {
        // A panel of a test has no board that closes it, and a dropped terminal
        // only detaches from its program. Without this the recorder of the
        // session (`script`) takes the hangup and waits for the program under
        // it, so both outlive the test process by hours.
        self.panel.end_program();
    }
}

/// One poll can read the echo of both lines of a burst: the text of the second
/// line is on the screen, and the poll that found it already ran. The wait takes
/// that text, where a wait for a poll that both drains a wakeup and sees the text
/// would wait for a wakeup that a loaded computer never sends again.
#[test]
fn a_wait_takes_the_text_that_the_last_poll_left_on_the_screen() {
    let mut harness = TerminalHarness::with_child(ECHO_CHILD_ARGS);
    harness.wait_for_text("READY");
    harness.panel.terminal().expect("terminal").write_input(b"one\ntwo\n");
    let deadline = Instant::now() + PATIENCE;
    let quiet_with =
        |harness: &TerminalHarness, text: &str| text_of(harness).contains(text) && harness.panel.had_recent_output();
    while !quiet_with(&harness, "two") {
        harness.panel.process_output();
        assert!(Instant::now() < deadline, "the program echoes both lines");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.wait_for_text("two");
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
                        notice_action: &mut PanelNoticeAction::default(),
                        take_keyboard_focus: false,
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

#[test]
fn the_echo_of_a_key_press_asks_for_its_frame_and_paints_it() {
    let mut harness = TerminalHarness::with_child(ECHO_CHILD_ARGS);
    harness.wait_for_text("READY");
    let (wakes, wake) = counting_wake();
    harness.panel.attach_output_wake(&wake);
    // The frame that takes the key needs the focus of the terminal first.
    harness.frame_with_events(Vec::new());

    harness.frame_with_events(vec![Event::Text("a".to_owned())]);

    let deadline = Instant::now() + PATIENCE;
    while wakes.load(Ordering::Relaxed) == 0 {
        assert!(
            Instant::now() < deadline,
            "the echo of a key press must ask for its own frame"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(wakes.load(Ordering::Relaxed), 1, "one key press asks for one frame");

    // The frame that the wake asked for drains the echo and paints it.
    harness.panel.process_output();
    harness.frame_with_events(Vec::new());
    assert!(
        harness.text().contains("READYa"),
        "the echo of the key press is painted:\n{}",
        harness.text()
    );
}

#[test]
fn a_terminal_that_prints_without_end_asks_for_no_frame_of_its_own() {
    // The bound of a terminal that nobody types into is the output poll of the
    // window: a frame every 16 ms while output arrives, and 100, 250, 500 and
    // 1 000 ms while the panel is quiet. A wake of the panel would break it.
    let mut harness = TerminalHarness::with_child(CHATTY_CHILD_ARGS);
    let (wakes, wake) = counting_wake();
    harness.panel.attach_output_wake(&wake);

    let deadline = Instant::now() + PATIENCE;
    let mut printed = 0_usize;
    while printed < 10 {
        harness.panel.process_output();
        if harness.panel.had_recent_output() {
            printed += 1;
        }
        harness.frame_with_events(Vec::new());
        assert!(
            Instant::now() < deadline,
            "the fixture must print through the window: {printed} polls, screen:\n{}",
            harness.text()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        0,
        "output that no key press of a person waits for asks for no frame"
    );
}

/// The clip rectangles of the drawn texts that contain `text`.
fn clips_of_text(output: &egui::FullOutput, text: &str) -> Vec<Rect> {
    output
        .shapes
        .iter()
        .filter(|shape| matches!(&shape.shape, egui::Shape::Text(drawn) if drawn.galley.text().contains(text)))
        .map(|shape| shape.clip_rect)
        .collect()
}

impl TerminalHarness {
    /// One frame that offers `events` to the terminal, and its shapes.
    fn frame_shapes(&mut self, events: Vec<Event>) -> egui::FullOutput {
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
                            notice_action: &mut PanelNoticeAction::default(),
                            take_keyboard_focus: false,
                        },
                    );
                });
            })
            .discard_textures()
    }
}

// The fixture reports the cell of every mouse report that it reads, without the
// bytes that would move the cursor of its own screen.
const REPORT_CHILD_ARGS: &str = r#"stty -echo -icanon min 1 time 0 || exit 1
printf '\033[?1003h\033[?1006hREADY\n'
while IFS= read -r -n 1 byte; do
  case "$byte" in
    ';'|'M'|'m'|[0-9]) printf '%s' "$byte";;
  esac
done
"#;

/// A panel whose program a follower of another computer holds the grid of draws
/// that grid from the top left of a larger body, asks for no resize, and reports
/// the cell of that grid under the pointer. The press of this computer then takes
/// the size back, because a key or a pointer button of the owner ends the hold.
/// Mutation: draw the grid of the body instead of `shown_grid` (the report names
/// a cell beyond the held grid), or call `resize` for a held grid (the body's
/// grid arrives at the program and `rows` changes).
#[test]
fn a_panel_that_holds_a_foreign_grid_draws_it_and_keeps_its_pointer_in_it() {
    let mut harness = TerminalHarness::with_child(REPORT_CHILD_ARGS);
    harness.wait_for_text("READY");
    harness.wait_for_mouse_mode();
    let source = harness
        .panel
        .terminal()
        .and_then(horizon_core::Terminal::share_source)
        .expect("a panel of this computer shares its terminal");
    assert!(source.hold_grid(40, 8), "the grid changes");
    let grid = harness.panel.terminal().expect("terminal").shown_grid();
    assert_eq!(grid, (8, 40), "the terminal takes the grid of the follower");

    let output = harness.frame_shapes(Vec::new());
    let clips = clips_of_text(&output, "READY");
    assert!(!clips.is_empty(), "the screen of the program is drawn");
    let origin = clips[0].min;
    for clip in clips {
        assert_eq!(
            clip.min, origin,
            "each row of the grid starts at the left edge of the body"
        );
    }
    assert_eq!(
        harness.panel.terminal().expect("terminal").rows(),
        16,
        "the widget asks no resize of a panel that holds a foreign grid"
    );

    // A press far outside the grid of the follower reports the last cell of that
    // grid, and not a cell of the larger body.
    let metrics = super::grid_metrics(&harness.ctx);
    let beyond = origin + Vec2::new(50.5 * metrics.char_width, 12.5 * metrics.line_height);
    harness.frame_shapes(vec![Event::PointerMoved(beyond)]);
    harness.frame_shapes(vec![press(beyond, true)]);
    harness.frame_shapes(vec![press(beyond, false)]);
    harness.wait_for_text("0;40;8M");
    assert!(
        !text_of(&harness).contains("0;50;12"),
        "no cell of the body reaches the program: {:?}",
        text_of(&harness)
    );
    assert!(
        !harness.panel.terminal().expect("terminal").shows_foreign_grid(),
        "a button of the person of this computer ends the hold"
    );
}

/// The longest wait for the processes of a dropped harness to go. It is a hang
/// guard and not a rule of the product: a loaded computer ends a program late.
const AFTER_DROP: Duration = Duration::from_secs(10);

/// A harness whose program a test drops without a board ends both: the recorder
/// of its session (`script`) takes the hangup of a closed terminal and runs on,
/// and the program of the panel runs under it until the test process ends.
#[test]
fn a_harness_that_is_dropped_ends_its_program_and_its_recorder() {
    // The word names this run of the test alone, and it reaches the command line
    // of the recorder and of the program under it.
    let word = format!("hz-dropped-{}", std::process::id());
    // A program that waits for input of its terminal for ever: the recorder of
    // the session holds the other end of that terminal, so it reads no end of
    // file and runs on after the harness is gone.
    let child = format!("printf 'READY'\nwhile IFS= read -r line; do :; done\n# {word}");
    {
        let mut harness = TerminalHarness::with_child(&child);
        harness.wait_for_text("READY");
        let running = processes_with(&word);
        assert!(
            !running.is_empty(),
            "the fixture must run under its recorder before the drop: {running:?}"
        );
    }

    let deadline = Instant::now() + AFTER_DROP;
    loop {
        let running = processes_with(&word);
        if running.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "a dropped harness left these processes running: {running:#?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The command lines of the processes of this computer that hold `word`.
fn processes_with(word: &str) -> Vec<String> {
    command_lines().into_iter().filter(|line| line.contains(word)).collect()
}

/// The command lines of every process of this computer.
#[cfg(target_os = "linux")]
fn command_lines() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        })
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .collect()
}

/// The command lines of every process of this computer.
#[cfg(not(target_os = "linux"))]
fn command_lines() -> Vec<String> {
    let answer = std::process::Command::new("ps").args(["-A", "-o", "command="]).output();
    match answer {
        Ok(answer) => String::from_utf8_lossy(&answer.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The text of the screen of a harness.
fn text_of(harness: &TerminalHarness) -> String {
    harness.panel.terminal().expect("terminal").last_lines_text(20)
}
