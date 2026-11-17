use std::io::Read;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use horizon_pty_tap::frames::{Frame, Frames};

use super::{FollowFeed, FollowSpec, MAX_FOLLOW_COLS, MAX_FOLLOW_ROWS, MAX_TYPED, MAX_WAITING_OUTPUT};
use crate::panel::PanelKind;
use crate::terminal::Terminal;

/// A wait that the speed of the machine must not decide: the event loop of a
/// follower parses its output in milliseconds, and this guard only separates
/// that from output that never arrives.
const HANG_GUARD: Duration = Duration::from_secs(30);

fn spec(cols: u16, rows: u16) -> FollowSpec {
    FollowSpec {
        device: "device-1".to_owned(),
        label: "other computer".to_owned(),
        panel: "panel-1".to_owned(),
        title: "build".to_owned(),
        kind: PanelKind::Shell,
        cols,
        rows,
        typing: true,
    }
}

fn follower(cols: u16, rows: u16) -> (Terminal, FollowFeed) {
    Terminal::spawn_follow(spec(cols, rows)).expect("a follower starts")
}

/// Takes the events of `terminal` until `ready` holds.
fn wait_until(terminal: &mut Terminal, mut ready: impl FnMut(&Terminal) -> bool, what: &str) {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        terminal.process_events();
        if ready(terminal) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}: {:?}", terminal.viewport_text());
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for_screen(terminal: &mut Terminal, wanted: &[&str]) {
    wait_until(
        terminal,
        |terminal| terminal.viewport_text() == wanted,
        "the screen of the follower",
    );
}

/// Columns and rows of the grid that the VT parser draws on.
fn grid_of(terminal: &Terminal) -> (usize, usize) {
    let term = terminal.term.lock();
    (term.columns(), term.screen_lines())
}

/// The frames that the terminal wrote to its link, which is what the program of
/// a holder would read. The attach of the link writes two sizes, so a read that
/// sees them sees every later frame too.
fn frames_of_the_link(terminal: &mut Terminal, feed: &FollowFeed) -> Vec<Frame> {
    // Two passes of the event loop, so it had the time to write what it was asked.
    for mark in ["\r\nfirst pass", "\r\nsecond pass"] {
        feed.output(mark.as_bytes());
        wait_until(
            terminal,
            |terminal| terminal.viewport_text().last().is_some_and(|line| line == mark.trim()),
            "the pass of the event loop",
        );
    }
    std::thread::sleep(Duration::from_millis(20));
    let follow = terminal.follow.as_ref().expect("a follower");
    let mut link = follow.shared.state().link.try_clone().expect("the link of the feed");
    let (mut frames, mut decoder, mut chunk) = (Vec::new(), Frames::default(), [0u8; 4096]);
    while let Ok(read) = link.read(&mut chunk) {
        if read == 0 {
            break;
        }
        frames.extend(decoder.push(&chunk[..read]).expect("frames of the link"));
    }
    frames
}

fn only_the_sizes_of_the_attach(frames: &[Frame]) -> bool {
    frames.len() == 2 && frames.iter().all(|frame| matches!(frame, Frame::Resize { .. }))
}

#[test]
fn a_screen_and_its_output_draw_the_grid() {
    let (mut terminal, feed) = follower(80, 24);

    feed.screen(40, 10);
    feed.output(b"first line\r\nsecond");

    wait_for_screen(&mut terminal, &["first line", "second"]);
    assert_eq!((terminal.cols(), terminal.rows()), (40, 10));
    assert_eq!(grid_of(&terminal), (40, 10));
    assert_eq!(terminal.child_pid(), None, "a follower owns no process");
}

#[test]
fn a_second_screen_replaces_the_screen_the_history_and_the_modes() {
    let (mut terminal, feed) = follower(80, 24);
    feed.screen(80, 5);
    let old: String = (0..40).fold(String::new(), |text, line| format!("{text}old {line}\r\n"));
    feed.output(old.as_bytes());
    feed.output(b"\x1b[?1h\x1b[?2004h");
    wait_until(
        &mut terminal,
        |terminal| terminal.history_size() > 0 && terminal.mode().contains(TermMode::APP_CURSOR),
        "the first screen has a history and a mode",
    );

    // Output of the old screen that is still on its way belongs to that screen,
    // also when it ends inside a synchronized update and inside a sequence.
    feed.output(old.as_bytes());
    feed.output(b"\x1b[?2026h\x1b]0;a title that the stream cut");
    feed.screen(30, 8);
    feed.output(b"new");

    wait_for_screen(&mut terminal, &["new"]);
    assert_eq!(terminal.history_size(), 0);
    assert_eq!(grid_of(&terminal), (30, 8));
    assert_eq!((terminal.cols(), terminal.rows()), (30, 8));
    assert!(
        !terminal
            .mode()
            .intersects(TermMode::APP_CURSOR | TermMode::BRACKETED_PASTE)
    );
}

/// The order rule: the terminal has the grid of a new screen before the link
/// holds a byte of that screen. Each round marks the last cell of a large grid
/// right after a small one. A byte of the large screen that the event loop
/// parsed on the small grid would leave the mark in the last cell of the small
/// grid, where the resize keeps it.
#[test]
fn no_byte_of_a_new_screen_is_parsed_on_the_old_grid() {
    let (mut terminal, feed) = follower(20, 5);
    for round in 0..100u16 {
        let (cols, rows) = (60 + round % 7, 20 + round % 5);
        feed.screen(cols, rows);
        assert_eq!(
            grid_of(&terminal),
            (usize::from(cols), usize::from(rows)),
            "the grid is there when the call of the screen returns"
        );
        feed.output(format!("\x1b[{rows};{cols}HX").as_bytes());
        let mark = format!("{}X", " ".repeat(usize::from(cols) - 1));
        wait_until(
            &mut terminal,
            |terminal| {
                let lines = terminal.viewport_text();
                lines.len() == usize::from(rows) && lines.last() == Some(&mark)
            },
            "the mark stands in the last cell of the new grid",
        );
        // The next round starts while the event loop may still parse this.
        feed.screen(20, 5);
        feed.output(b"small screen\r\nwith two rows");
    }
}

#[test]
fn only_the_bytes_of_the_person_reach_the_feed() {
    let (mut terminal, feed) = follower(80, 24);
    // What a program asks its terminal: the cursor position, a clipboard write,
    // a clipboard read, the device attributes, a colour, the size of the text
    // area and the status protocol. Then the mode that reports the focus.
    feed.output(
        b"\x1b[6n\x1b]52;c;SGVsbG8=\x07\x1b]52;c;?\x07\x1b[c\x1b]11;?\x07\x1b[18t\x1b]7501;?\x1b\\\x1b[?1004hasked",
    );
    wait_for_screen(&mut terminal, &["asked"]);
    terminal.set_focused(true);
    terminal.set_focused(false);
    terminal.process_events();

    assert!(only_the_sizes_of_the_attach(&frames_of_the_link(&mut terminal, &feed)));
    assert!(feed.take_typed().is_empty(), "no answer of the terminal is typed text");
    assert!(terminal.take_clipboard_writes().is_empty());
    assert!(!terminal.take_answered_query());

    terminal.write_input(b"ls\r");
    terminal.write_pointer_motion(b"\x1b[<35;1;1M");
    assert_eq!(feed.take_typed(), b"ls\r\x1b[<35;1;1M");
    assert!(feed.take_typed().is_empty());
    assert!(
        frames_of_the_link(&mut terminal, &feed).is_empty(),
        "the bytes of the person go to the feed and not to the link"
    );
}

#[test]
fn a_resize_changes_nothing_of_a_follower() {
    let (mut terminal, feed) = follower(80, 24);

    terminal.resize(10, 30, 9, 18);
    terminal.resize_immediately(50, 120, 9, 18);
    terminal.process_events();

    assert_eq!((terminal.cols(), terminal.rows()), (80, 24));
    assert_eq!(grid_of(&terminal), (80, 24));
    assert!(
        only_the_sizes_of_the_attach(&frames_of_the_link(&mut terminal, &feed)),
        "no size reaches the link after the attach"
    );
}

#[test]
fn a_stream_that_ended_keeps_its_screen_until_a_new_screen() {
    let (mut terminal, feed) = follower(80, 24);
    feed.output(b"last words");
    wait_for_screen(&mut terminal, &["last words"]);
    terminal.write_input(b"before");

    feed.ended("the other computer\x07 went away");

    assert_eq!(terminal.follow_ended().as_deref(), Some("the other computer went away"));
    terminal.write_input(b"into the void");
    assert!(feed.take_typed().is_empty(), "a stream that ended takes no typed text");
    wait_for_screen(&mut terminal, &["last words"]);

    feed.screen(80, 24);
    feed.output(b"back");
    wait_for_screen(&mut terminal, &["back"]);
    assert_eq!(terminal.follow_ended(), None);
    terminal.write_input(b"x");
    assert_eq!(feed.take_typed(), b"x");
}

#[test]
fn a_follower_without_typing_drops_the_bytes_of_the_person() {
    let (terminal, feed) = Terminal::spawn_follow(FollowSpec {
        typing: false,
        ..spec(80, 24)
    })
    .expect("a follower starts");

    terminal.write_input(b"secret");
    terminal.write_pointer_motion(b"\x1b[<35;1;1M");

    assert!(feed.take_typed().is_empty());
}

/// A worker thread may feed a follower: the feed holds no lock while it waits
/// for the grid.
#[test]
fn a_feed_goes_to_another_thread() {
    fn sendable<T: Send + Sync + Clone>() {}
    sendable::<FollowFeed>();
}

#[test]
fn the_feed_knows_that_the_person_closed_the_panel() {
    let (terminal, feed) = follower(80, 24);
    assert!(!feed.closed());
    assert!(terminal.follow_feed().is_some_and(|feed| !feed.closed()));

    drop(terminal);

    assert!(feed.closed());
    // A stream that still sends reaches nothing and breaks nothing.
    feed.screen(10, 10);
    feed.output(b"late");
    feed.ended("closed");
    assert!(feed.take_typed().is_empty());
}

#[test]
fn the_bytes_of_the_person_above_the_bound_drop_the_oldest() {
    let (terminal, feed) = follower(80, 24);

    terminal.write_input(b"oldest");
    terminal.write_input(&vec![b'n'; MAX_TYPED]);

    let typed = feed.take_typed();
    assert_eq!(typed.len(), MAX_TYPED);
    assert!(typed.iter().all(|byte| *byte == b'n'));
}

#[test]
fn the_grid_and_the_texts_of_a_stream_are_bounded() {
    let (mut terminal, feed) = Terminal::spawn_follow(FollowSpec {
        title: format!("title\x1b]0;x\x07{}", "t".repeat(4096)),
        ..spec(u16::MAX, u16::MAX)
    })
    .expect("a follower starts");
    let bound = (usize::from(MAX_FOLLOW_COLS), usize::from(MAX_FOLLOW_ROWS));
    assert_eq!(grid_of(&terminal), bound);
    let title = &terminal.follow_spec().expect("a follower").title;
    assert_eq!(title.chars().count(), 200);
    assert!(title.starts_with("title]0;x") && !title.contains(char::is_control));

    feed.screen(0, 0);
    assert_eq!(grid_of(&terminal), (2, 1));
    feed.screen(u16::MAX, u16::MAX);
    assert_eq!(grid_of(&terminal), bound);
    terminal.process_events();
    assert_eq!((terminal.cols(), terminal.rows()), (MAX_FOLLOW_COLS, MAX_FOLLOW_ROWS));
}

#[test]
fn output_above_the_bound_drops_what_waits_and_the_terminal_goes_on() {
    let (mut terminal, feed) = follower(80, 24);
    // Two chunks fit the bound whatever the link takes, and three pass it by
    // more than the link and the buffer of the event loop can take.
    let chunk = b"line\r\n".repeat((MAX_WAITING_OUTPUT / 2 - 100_000) / 6);
    {
        // The event loop cannot parse while this test holds the grid, so the
        // link fills and the output waits in the feed.
        let _grid = terminal.term.lock();
        feed.output(&chunk);
        feed.output(&chunk);
        assert!(!feed.take_lost(), "two chunks fit the bound");
        feed.output(&chunk);
        assert!(feed.take_lost(), "the third chunk passes the bound");
        assert!(!feed.take_lost());
        let waiting = terminal
            .follow
            .as_ref()
            .expect("a follower")
            .shared
            .state()
            .waiting_bytes;
        assert!(waiting <= MAX_WAITING_OUTPUT, "{waiting} bytes wait");
    }
    feed.output(b"end");

    wait_until(
        &mut terminal,
        |terminal| terminal.viewport_text().last().is_some_and(|line| line == "end"),
        "the newest output reaches the screen",
    );
}

/// The pump of a window feeds a follower on a thread of its own. The feed says
/// which screen the terminal shows, and it asks for the frame that shows a
/// stream when a frame drew the terminal.
#[test]
fn the_feed_knows_the_alternate_screen_and_asks_for_the_frame_of_its_stream() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (mut terminal, feed) = follower(20, 4);
    assert!(terminal.share_source().is_none(), "a follower has no share source");
    let frames = Arc::new(AtomicUsize::new(0));
    let asked = Arc::clone(&frames);
    let wake: crate::terminal::OutputWake = Arc::new(move || {
        asked.fetch_add(1, Ordering::Relaxed);
    });
    terminal.attach_output_wake(&wake);

    // No frame drew the terminal, so its stream asks for none.
    feed.screen(20, 4);
    feed.output(b"\x1b[?1049hALT");
    wait_for_screen(&mut terminal, &["ALT"]);
    assert!(feed.on_alternate_screen(), "the stream entered the alternate screen");
    assert_eq!(
        frames.load(Ordering::Relaxed),
        0,
        "a follower that no frame draws asks for no frame"
    );

    // A frame drew it: the next bytes of the stream ask for the frame that shows them.
    terminal.note_drawn(true);
    feed.output(b"\x1b[?1049lMAIN");
    wait_until(
        &mut terminal,
        |_| !feed.on_alternate_screen(),
        "the stream left the alternate screen",
    );
    wait_until(
        &mut terminal,
        |_| frames.load(Ordering::Relaxed) > 0,
        "the frame of the stream",
    );

    drop(terminal);
    assert!(
        !feed.on_alternate_screen(),
        "a feed without its terminal shows no alternate screen"
    );
}
