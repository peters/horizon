//! The pass of a window that draws no frame: what it does for a terminal, when
//! it does its work, and which terminal it leaves alone. After its setup a
//! test runs the pass as eframe does and never a frame, unless it says so.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::{Context, RawInput};
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision, Wake};

use crate::app::HorizonApp;
use crate::app::test_support::{run_app_frame, test_app_with_startup};
use crate::test_egui::DiscardTextures;

/// The longest wait for the program of a test. It is a hang guard and not a rule
/// of the product: a loaded computer schedules the program late.
const PATIENCE: Duration = Duration::from_secs(120);

/// A place that no frame draws, so no echo of a key asks for a frame.
const OFFSCREEN: [f32; 2] = [100_000.0, 100_000.0];

/// A program that asks its terminal for the position of the cursor after its
/// first line, shows the answer, and then shows each byte that it reads. The
/// cursor stands behind `READY`, so the answer is `ESC [ 1 ; 6 R`.
const ASKS: &str = r#"stty -echo -icanon min 1 time 0; printf 'READY'; read -r line; printf '\033[6n'
answer=$(dd bs=1 count=6 2>/dev/null | tr '\033' E); printf ' ANS:%s ' "$answer"; cat -v"#;

/// A program that prints what it reads and sends no event for the window.
const PRINTS: &str = r#"stty -echo; printf 'READY'; while read -r line; do printf ' GOT:%s' "$line"; done"#;

/// One pass in which the window draws nothing, as eframe runs it for a window
/// that gets no frame. The window is not in front.
pub(in crate::app) fn run_logic_pass(ctx: &Context, app: &mut HorizonApp) {
    let _frame = horizon_core::ui_thread::FrameScope::enter();
    let mut frame = eframe::Frame::_new_kittest();
    let input = RawInput {
        focused: false,
        ..RawInput::default()
    };
    let _ = ctx.run_logic(&input, |ctx| eframe::App::logic(app, ctx, &mut frame));
}

/// Whether the last pass did the work of a window that draws nothing: it
/// told the terminals so, and a frame takes that back.
fn worked(app: &HorizonApp) -> bool {
    app.undrawn.asks.load(Ordering::Acquire)
}

/// One frame as eframe runs it for a window that it shows: the pass, and then
/// the ui. `discard` asks for a second pass of the same frame. Answers the
/// count of the passes, and whether a pass before the ui did the work.
///
/// eframe gives each frame the time of its start, and egui gives that same
/// time to each pass of the frame.
fn run_shown_frame(ctx: &Context, app: &mut HorizonApp, discard: bool) -> (usize, bool) {
    let _frame = horizon_core::ui_thread::FrameScope::enter();
    let mut frame = eframe::Frame::_new_kittest();
    let (mut passes, mut work) = (0, false);
    let input = RawInput {
        time: Some(ctx.input(|input| input.time) + 1.0),
        ..RawInput::default()
    };
    let _ = ctx
        .run_ui(input, |ui| {
            passes += 1;
            if discard && passes == 1 {
                ui.ctx().request_discard("the test asks for a second pass");
            }
            let before = worked(app);
            eframe::App::logic(app, ui.ctx(), &mut frame);
            work |= !before && worked(app);
            eframe::App::ui(app, ui, &mut frame);
        })
        .discard_textures();
    (passes, work)
}

fn window() -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let workspace = app.board.create_workspace("Work");
    app.board.focus_workspace(workspace);
    (temp, ctx, app)
}

/// A terminal panel that runs `program`, at a place that no frame draws.
fn terminal(app: &mut HorizonApp, program: &str) -> PanelId {
    let workspace = app.board.active_workspace.expect("the workspace of the test");
    let options = PanelOptions {
        kind: PanelKind::Command,
        name: Some("build".to_owned()),
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), program.to_owned()],
        position: Some(OFFSCREEN),
        size: Some([600.0, 300.0]),
        ..PanelOptions::default()
    };
    app.board.create_panel(options, workspace).expect("the terminal")
}

fn text(app: &HorizonApp, panel: PanelId) -> String {
    let terminal = app.board.panel(panel).and_then(Panel::terminal);
    terminal
        .map(|terminal| terminal.last_lines_text(60))
        .unwrap_or_default()
}

fn write(app: &mut HorizonApp, panel: PanelId, bytes: &[u8]) {
    let written = app.board.panel_mut(panel).expect("the panel").write_input(bytes);
    assert!(written, "the panel takes the bytes");
}

/// Runs frames until `done` holds: the setup of a test. At least one frame
/// runs, because the pass does its work only after a frame.
fn frames_until(ctx: &Context, app: &mut HorizonApp, what: &str, done: impl Fn(&HorizonApp) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        run_app_frame(ctx, app);
        if done(app) {
            break;
        }
        assert!(Instant::now() < deadline, "{what}: it did not happen");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The programs of a test wait for input now. One last frame reads the
    // events that they sent before, so none of them arrives after the setup.
    std::thread::sleep(Duration::from_millis(100));
    run_app_frame(ctx, app);
}

/// Waits until `done` holds, and runs nothing of the window.
fn waits(done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

/// Runs passes that draw nothing until `done` holds.
pub(in crate::app) fn passes_until(ctx: &Context, app: &mut HorizonApp, done: impl Fn(&HorizonApp) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while !done(app) && Instant::now() < deadline {
        run_logic_pass(ctx, app);
        std::thread::sleep(Duration::from_millis(5));
    }
    done(app)
}

/// Counts the requests for a repaint at once that the window gets from now
/// on. egui tells its observer of each request, also of one that joins a
/// repaint that waits already.
fn repaint_requests(ctx: &Context) -> Arc<AtomicUsize> {
    let asked = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&asked);
    ctx.set_repaint_observer(move |info| {
        count.fetch_add(usize::from(info.delay.is_zero()), Ordering::Relaxed);
    });
    asked
}

fn unread(app: &HorizonApp, panel: PanelId) -> bool {
    let terminal = app.board.panel(panel).and_then(Panel::terminal);
    terminal.is_some_and(horizon_core::Terminal::has_unread_event)
}

#[test]
fn a_program_that_asks_for_the_cursor_position_gets_the_answer_with_no_frame() {
    let (_temp, ctx, mut app) = window();
    let panel = terminal(&mut app, ASKS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });

    // From here no frame runs. The first pass that draws nothing tells the
    // terminals that no poll of a frame reads their events.
    run_logic_pass(&ctx, &mut app);
    let asked = repaint_requests(&ctx);
    write(&mut app, panel, b"go\n");
    assert!(waits(|| unread(&app, panel)), "the question waits for the window");
    assert!(
        waits(|| asked.load(Ordering::Relaxed) > 0),
        "the question asks for a pass"
    );
    assert!(!text(&app, panel).contains("ANS:"), "no pass ran, so nothing answered");

    run_logic_pass(&ctx, &mut app);
    assert!(
        waits(|| text(&app, panel).contains("ANS:E[1;6R")),
        "the program reads the position of the cursor: {:?}",
        text(&app, panel)
    );
}

#[test]
fn a_window_that_draws_frames_answers_a_question_one_time() {
    let (_temp, ctx, mut app) = window();
    let panel = terminal(&mut app, ASKS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });

    // A frame as eframe runs it, with the pass before the ui, and a pass that
    // draws nothing after each frame.
    write(&mut app, panel, b"go\n");
    let deadline = Instant::now() + PATIENCE;
    while !text(&app, panel).contains("ANS:") {
        let _ = run_shown_frame(&ctx, &mut app, false);
        run_logic_pass(&ctx, &mut app);
        assert!(Instant::now() < deadline, "the program gets its answer");
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..5 {
        let _ = run_shown_frame(&ctx, &mut app, false);
        run_logic_pass(&ctx, &mut app);
    }

    // The program now shows each byte that it reads. A second answer would
    // stand before the word that the test types last.
    write(&mut app, panel, b"end");
    assert!(
        waits(|| text(&app, panel).contains("end")),
        "the program shows what it reads"
    );
    let shown = text(&app, panel);
    assert_eq!(shown.matches("ANS:E[1;6R").count(), 1, "one answer: {shown:?}");
    assert!(!shown.contains("^["), "no second answer reached the program: {shown:?}");
}

#[test]
fn the_pass_does_its_work_only_in_place_of_a_frame() {
    let (_temp, ctx, mut app) = window();
    let panel = terminal(&mut app, PRINTS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    assert!(!worked(&app), "a frame is no pass");

    // eframe runs the pass before each frame of a window that it shows.
    for _ in 0..3 {
        let frame = run_shown_frame(&ctx, &mut app, false);
        assert_eq!(
            frame,
            (1, false),
            "the pass before a frame leaves the work to the frame"
        );
    }
    let frame = run_shown_frame(&ctx, &mut app, true);
    assert_eq!(
        frame,
        (2, false),
        "the second pass of one frame is not in place of a frame"
    );

    // While frames run, an event of a program asks for no pass: the poll of a
    // frame reads it. The read of the directory of a panel is a job of a worker,
    // and the answer of that job asks for a frame: run frames until no panel
    // waits for the answer of its read, so no job of the setup is left.
    let deadline = Instant::now() + PATIENCE;
    while app.cwd_reads.in_flight_count() > 0 {
        assert!(Instant::now() < deadline, "the panel reads its directory");
        run_app_frame(&ctx, &mut app);
    }
    let asked = repaint_requests(&ctx);
    let count = || asked.load(Ordering::Relaxed);
    let wake = Arc::clone(&app.output_wake);
    wake(Wake::Event);
    assert_eq!(count(), 0, "an event asks for nothing while frames run");
    wake(Wake::Echo);
    assert_eq!(count(), 1, "the echo of a key asks for its frame");

    run_logic_pass(&ctx, &mut app);
    assert!(worked(&app), "a pass that draws nothing does the work");
    let before = count();
    wake(Wake::Event);
    assert_eq!(count(), before + 1, "an event asks for a pass while no frame runs");
    run_logic_pass(&ctx, &mut app);
    assert!(worked(&app), "the next pass that draws nothing does the work too");

    // A frame draws again, and an event asks for nothing.
    let _ = run_shown_frame(&ctx, &mut app, false);
    assert!(!worked(&app), "the frame took the events over again");
    let before = count();
    wake(Wake::Event);
    assert_eq!(count(), before, "an event asks for nothing while frames run again");
}

#[test]
fn the_pass_reads_no_terminal_whose_program_sent_no_event() {
    let (_temp, ctx, mut app) = window();
    let quiet = terminal(&mut app, PRINTS);
    let asker = terminal(&mut app, ASKS);
    let recent = |app: &HorizonApp, panel: PanelId| app.board.panel(panel).is_some_and(Panel::had_recent_output);
    frames_until(&ctx, &mut app, "the two programs are ready", |app| {
        text(app, quiet).contains("READY") && text(app, asker).contains("READY")
    });

    // The last frame took output of the quiet terminal, and the panel says so
    // until the next read of that terminal. Then its program sends nothing.
    write(&mut app, quiet, b"one\n");
    assert!(waits(|| unread(&app, quiet) && text(&app, quiet).contains("GOT:one")));
    std::thread::sleep(Duration::from_millis(100));
    run_app_frame(&ctx, &mut app);
    assert!(
        recent(&app, quiet) && !unread(&app, quiet),
        "the frame read the quiet terminal"
    );

    // From here no frame runs. The pass reads the terminal that asked, and it
    // does not read the quiet terminal, which would say that it had no output.
    write(&mut app, asker, b"go\n");
    assert!(waits(|| unread(&app, asker)), "the question waits for the window");
    run_logic_pass(&ctx, &mut app);
    assert!(
        waits(|| text(&app, asker).contains("ANS:")),
        "the pass answered the question"
    );
    assert!(recent(&app, quiet), "the pass read no terminal that sent no event");

    // The answer of the program is output, which the next passes read. Then
    // nothing waits, and a pass that nothing woke reads no terminal at all.
    assert!(passes_until(&ctx, &mut app, |app| !unread(app, asker)));
    std::thread::sleep(Duration::from_millis(100));
    run_logic_pass(&ctx, &mut app);
    assert!(!unread(&app, asker) && !unread(&app, quiet));
    assert!(recent(&app, quiet), "a pass that nothing woke reads nothing");
}

#[test]
fn a_panel_that_opens_with_no_frame_starts_its_program() {
    let (_temp, ctx, mut app) = window();
    let first = terminal(&mut app, PRINTS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, first).contains("READY")
    });

    // A request of an agent opens a panel while the window draws nothing. The
    // program of such a panel waits for the preparation of a worker.
    let workspace = app.board.active_workspace.expect("the workspace of the test");
    let options = PanelOptions {
        kind: PanelKind::Command,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), PRINTS.to_owned()],
        position: Some(OFFSCREEN),
        ..PanelOptions::default()
    };
    let second = app.board.create_panel_waiting(options, workspace).expect("the panel");
    assert!(
        !text(&app, second).contains("READY"),
        "the program waits for its preparation"
    );
    assert!(
        passes_until(&ctx, &mut app, |app| text(app, second).contains("READY")),
        "the program of the new panel starts with no frame"
    );
}

#[test]
fn a_copy_that_a_program_asks_for_reaches_no_clipboard_in_the_pass() {
    // The program copies `hello` with OSC 52 after its first line.
    let copies = r"stty -echo; printf 'READY'; read -r line; printf '\033]52;c;aGVsbG8=\007 COPIED'; sleep 60";
    let (_temp, ctx, mut app) = window();
    let panel = terminal(&mut app, copies);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });

    write(&mut app, panel, b"go\n");
    assert!(
        waits(|| text(&app, panel).contains("COPIED")),
        "the program asked for its copy"
    );
    assert!(
        passes_until(&ctx, &mut app, |app| !unread(app, panel)),
        "the pass read the request"
    );
    assert!(
        app.board.take_terminal_clipboard_writes().is_empty(),
        "the pass hands no copy of a program to the clipboard of the person"
    );
    // The terminal keeps the copy for the frame that hands it over.
    let terminal = app.board.panel_mut(panel).and_then(Panel::terminal_mut);
    let kept = terminal.expect("the terminal").take_clipboard_writes();
    assert_eq!(kept.len(), 1, "the terminal keeps the copy until a frame");
    assert_eq!(kept[0].text, "hello");
}

#[test]
fn a_pass_that_took_output_and_asked_for_a_read_asks_for_the_two_passes_after_it() {
    let (_temp, ctx, mut app) = window();
    // An agent panel, so the pass reads the states of the agents.
    let workspace = app.board.active_workspace.expect("the workspace of the test");
    let options = PanelOptions {
        kind: PanelKind::Codex,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), PRINTS.to_owned()],
        position: Some(OFFSCREEN),
        ..PanelOptions::default()
    };
    let panel = app.board.create_panel(options, workspace).expect("the agent panel");
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    // The passes read what the last frame left, and the last of them asks for
    // nothing. Then the last read of the agent states gets old enough for a new one.
    assert!(passes_until(&ctx, &mut app, |app| !unread(app, panel)));
    std::thread::sleep(Duration::from_millis(100));
    run_logic_pass(&ctx, &mut app);
    run_logic_pass(&ctx, &mut app);
    std::thread::sleep(Duration::from_millis(600));

    let delays = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&delays);
    ctx.set_repaint_observer(move |info| seen.lock().expect("the delays").push(info.delay));
    let asked = |delay: Duration| {
        let delays = delays.lock().expect("the delays");
        delays.iter().filter(|seen| **seen == delay).count()
    };
    write(&mut app, panel, b"one\n");
    assert!(waits(|| unread(&app, panel)), "the output waits for the window");
    run_logic_pass(&ctx, &mut app);
    assert_eq!(asked(super::HOOK_TAKES), 1, "a pass after the last output of a turn");
    assert_eq!(
        asked(super::READ_TAKES),
        1,
        "a pass that takes the read of the agent states"
    );

    // The worker answered the read. The pass that takes it took no output and
    // asks for no read, so it asks for no pass after it.
    std::thread::sleep(Duration::from_millis(200));
    run_logic_pass(&ctx, &mut app);
    assert!(!app.attention.reads(), "the pass took the read of the agent states");
    run_logic_pass(&ctx, &mut app);
    assert_eq!(asked(super::HOOK_TAKES), 1, "a pass with no output asks for no pass");
    assert_eq!(asked(super::READ_TAKES), 1, "one read brings one pass");
}

#[test]
fn the_pass_reaches_the_application_through_the_bridge_of_the_window() {
    let (temp, ctx, mut app) = window();
    let panel = terminal(&mut app, ASKS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    // The source of the terminal reads its screen and types into its program
    // from this thread, while the bridge owns the application.
    let terminal = app.board.panel(panel).and_then(Panel::terminal);
    let source = terminal
        .and_then(horizon_core::Terminal::share_source)
        .expect("the source");
    let shows = |words: &str| {
        let screen = source.screen(true);
        screen.is_some_and(|screen| String::from_utf8_lossy(&screen.bytes).contains(words))
    };

    // eframe owns the bridge of the requests, which owns the application.
    let bridge = crate::app::DeviceRequestBridge::with_root(temp.path().join("requests"));
    bridge.install(app, ctx.clone());
    let mut window = crate::app::BridgeApp::new(Arc::clone(&bridge));
    source.write_input(b"go\n");
    let mut frame = eframe::Frame::_new_kittest();
    let input = RawInput {
        focused: false,
        ..RawInput::default()
    };
    let deadline = Instant::now() + PATIENCE;
    while !shows("ANS:E[1;6R") {
        let _ = ctx.run_logic(&input, |ctx| eframe::App::logic(&mut window, ctx, &mut frame));
        assert!(Instant::now() < deadline, "the pass of eframe answers the program");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A program that asks its terminal what it is for each line that it reads,
/// takes the answer of five bytes, and prints the line.
const ASKS_AGAIN: &str = r#"stty -echo -icanon min 1 time 0; printf 'READY'; while read -r line; do
printf '\033[c'; dd bs=1 count=5 >/dev/null 2>&1; printf ' A%s' "$line"; done"#;

/// The stand-in of the proxy of the event loop: it counts the calls that ask
/// the UI thread for the pass at once.
fn proxy_stand_in(app: &HorizonApp) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    app.pass_at_once(Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    calls
}

/// Does what the UI thread does with the calls of the stand-in, until `done`
/// holds: one pass for the calls that arrived, as the window runs it.
fn serve_until(
    ctx: &Context,
    app: &mut HorizonApp,
    calls: &AtomicUsize,
    served: &mut usize,
    done: impl Fn(&HorizonApp) -> bool,
) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let asked = calls.load(Ordering::Relaxed);
        if asked != *served {
            *served = asked;
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.pass_now(ctx);
        }
        if done(app) || Instant::now() >= deadline {
            return done(app);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn an_event_of_a_terminal_asks_the_ui_thread_for_the_pass_at_once() {
    let (_temp, ctx, mut app) = window();
    let calls = proxy_stand_in(&app);
    // The wake of the window, with a count of the events that reach it.
    let events = Arc::new(AtomicUsize::new(0));
    let (count, wake) = (Arc::clone(&events), Arc::clone(&app.output_wake));
    app.output_wake = Arc::new(move |cause| {
        count.fetch_add(usize::from(cause == Wake::Event), Ordering::Relaxed);
        wake(cause);
    });
    let panel = terminal(&mut app, ASKS_AGAIN);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    // From here no frame runs, and the window knows that it draws nothing.
    run_logic_pass(&ctx, &mut app);
    let events_before = events.load(Ordering::Relaxed);
    let mut served = calls.load(Ordering::Relaxed);
    let mut fastest = Duration::MAX;
    let started = Instant::now();
    let mut round = 0;
    // A question and its answer must take less than the wait of eframe. A loaded
    // computer delays the pass of the window, and the fastest round is the least
    // that the machine adds to the product's time, so the test takes rounds until
    // one is fast: three rounds gave 345 ms beside 24 busy loops.
    while fastest >= Duration::from_millis(250) {
        assert!(
            started.elapsed() < PATIENCE,
            "a question and its answer take less than the wait of eframe: {fastest:?}"
        );
        round += 1;
        // The calls of the round before are free again.
        std::thread::sleep(Duration::from_millis(200));
        let before = calls.load(Ordering::Relaxed);
        let asked = Instant::now();
        write(&mut app, panel, format!("{round}\n").as_bytes());
        assert!(
            waits(|| calls.load(Ordering::Relaxed) > before),
            "the question asks the UI thread for the pass at once"
        );
        let shown = format!(" A{round}");
        let answered = serve_until(&ctx, &mut app, &calls, &mut served, |app| {
            text(app, panel).contains(&shown)
        });
        assert!(answered, "the pass of that call answers the program");
        fastest = fastest.min(asked.elapsed());
        // The line of the program is an event too, and its pass reads it.
        assert!(serve_until(&ctx, &mut app, &calls, &mut served, |app| !unread(
            app, panel
        )));
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        events.load(Ordering::Relaxed) - events_before,
        "each event asked the UI thread at once, and none waited for eframe"
    );
    eprintln!("the fastest question and answer took {fastest:?}");
    assert!(
        fastest < Duration::from_millis(250),
        "a question and its answer take less than the wait of eframe: {fastest:?}"
    );
}

#[test]
fn a_window_that_draws_frames_gets_no_call_from_its_terminals() {
    let (_temp, ctx, mut app) = window();
    let calls = proxy_stand_in(&app);
    let panel = terminal(&mut app, ASKS_AGAIN);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    write(&mut app, panel, b"1\n");
    frames_until(&ctx, &mut app, "a frame answers the program", |app| {
        text(app, panel).contains(" A1")
    });
    assert_eq!(calls.load(Ordering::Relaxed), 0, "the poll of a frame reads the events");

    // A pass drew nothing, and then a frame draws again and takes the events back.
    run_logic_pass(&ctx, &mut app);
    run_app_frame(&ctx, &mut app);
    write(&mut app, panel, b"2\n");
    frames_until(&ctx, &mut app, "a frame answers the program again", |app| {
        text(app, panel).contains(" A2")
    });
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "a window that draws frames gets no call"
    );

    // A call that was on its way when the frame ran finds nothing to do.
    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        app.pass_now(&ctx);
    }
    assert!(!worked(&app), "the pass of a late call does no work after a frame");
}

#[test]
fn a_program_that_prints_ten_thousand_lines_gets_a_bounded_number_of_passes() {
    let prints = r#"stty -echo; printf 'READY'; read -r line; i=0
while [ $i -lt 10000 ]; do echo "line $i"; i=$((i+1)); done; echo DONE; sleep 60"#;
    let (_temp, ctx, mut app) = window();
    let calls = proxy_stand_in(&app);
    let panel = terminal(&mut app, prints);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    run_logic_pass(&ctx, &mut app);
    // Eight calls go at once, and one more each 16 ms.
    let bound = |since: Instant| 8 + usize::try_from(since.elapsed().as_millis() / 16).expect("a short test") + 2;

    // The test is the UI thread: a pass for the calls that arrived, and each
    // 50 ms the pass of eframe for the events that got no call.
    let (before, started) = (calls.load(Ordering::Relaxed), Instant::now());
    let mut served = before;
    write(&mut app, panel, b"go\n");
    let deadline = Instant::now() + PATIENCE;
    while !text(&app, panel).contains("DONE") || unread(&app, panel) {
        let _ = serve_until(&ctx, &mut app, &calls, &mut served, |_| true);
        if started.elapsed().as_millis() % 50 == 0 {
            run_logic_pass(&ctx, &mut app);
        }
        assert!(Instant::now() < deadline, "the program prints its lines");
        std::thread::sleep(Duration::from_micros(200));
    }
    let passes = calls.load(Ordering::Relaxed) - before;
    eprintln!(
        "ten thousand lines asked for {passes} passes in {:?}",
        started.elapsed()
    );
    assert!(passes >= 1, "the output asked for a pass at once");
    assert!(
        passes <= bound(started),
        "the calls of a fast program are bounded: {passes}"
    );

    // Ten thousand events with no time between them, as the reads of a
    // program that prints with no end: the calls stay in the same bound.
    std::thread::sleep(Duration::from_millis(200));
    let wake = Arc::clone(&app.output_wake);
    let (before, started) = (calls.load(Ordering::Relaxed), Instant::now());
    for _ in 0..10_000 {
        wake(Wake::Event);
    }
    let asked = calls.load(Ordering::Relaxed) - before;
    assert!(
        (1..=bound(started)).contains(&asked),
        "ten thousand events made {asked} calls"
    );
}

#[test]
fn the_call_of_a_terminal_reaches_the_pass_through_the_bridge_of_the_window() {
    let (temp, ctx, mut app) = window();
    let panel = terminal(&mut app, ASKS);
    frames_until(&ctx, &mut app, "the program is ready", |app| {
        text(app, panel).contains("READY")
    });
    run_logic_pass(&ctx, &mut app);
    let terminal = app.board.panel(panel).and_then(Panel::terminal);
    let source = terminal
        .and_then(horizon_core::Terminal::share_source)
        .expect("the source");
    let shows = |words: &str| {
        let screen = source.screen(true);
        screen.is_some_and(|screen| String::from_utf8_lossy(&screen.bytes).contains(words))
    };

    // The window gives the wake of a terminal to this call of the bridge.
    let bridge = crate::app::DeviceRequestBridge::with_root(temp.path().join("requests"));
    bridge.install(app, ctx.clone());
    source.write_input(b"go\n");
    let deadline = Instant::now() + PATIENCE;
    while !shows("ANS:E[1;6R") {
        bridge.pass_on_ui_thread();
        assert!(Instant::now() < deadline, "the pass of the call answers the program");
        std::thread::sleep(Duration::from_millis(5));
    }
}
