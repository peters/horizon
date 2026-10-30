#[cfg(unix)]
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{RuntimeState, StartupDecision};
use tempfile::TempDir;

use crate::app::HorizonApp;
use crate::app::test_support;

/// How long a read that the UI thread performs instead of the worker may hold
/// a test before the release lets it finish and the timing assertion fails.
#[cfg(unix)]
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the coordination worker may take to open the dropped file it reads.
#[cfg(unix)]
const READER_TIMEOUT: Duration = Duration::from_secs(30);
/// A frame that read the dropped file itself waits for `RELEASE_TIMEOUT`.
const FRAME_LIMIT: Duration = Duration::from_secs(2);

/// Five lines of 60 columns: wider than the fallback size and taller than the
/// smallest panel, so a computed size differs from both.
const HELD_SIZE: [f32; 2] = [560.0, 280.0];
/// One line of 10 columns, which clamps to the smallest width and height.
const SMALL_SIZE: [f32; 2] = [400.0, 280.0];

fn held_content() -> String {
    format!("{}\n", "0123456789".repeat(6)).repeat(5)
}

fn test_app() -> (TempDir, Context, HorizonApp) {
    test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    })
}

fn drop_files(app: &mut HorizonApp, ctx: &Context, paths: &[PathBuf]) {
    let dropped = paths
        .iter()
        .map(|path| test_support::dropped_file(path.clone()))
        .collect();
    ctx.input_mut(|input| input.raw.dropped_files = dropped);
    // The position the person let go at, which the panels of a drop are placed
    // at. It also keeps the panels out of a workspace layout that would give
    // every panel of the workspace the size of its first one.
    app.file_hover_positions
        .insert(ctx.viewport_id(), egui::pos2(400.0, 300.0));
    app.handle_root_file_drop(ctx);
}

/// The title and size of each editor panel, in board order.
fn editor_panels(app: &HorizonApp) -> Vec<(String, [f32; 2])> {
    app.board
        .panels
        .iter()
        .filter(|panel| panel.kind == horizon_core::PanelKind::Editor)
        .map(|panel| (panel.title.clone(), panel.layout.size))
        .collect()
}

#[test]
fn a_dropped_file_that_is_gone_keeps_the_fallback_size() {
    let (temp, ctx, mut app) = test_app();
    let gone = temp.path().join("gone.md");

    drop_files(&mut app, &ctx, &[gone]);
    app.settle_browser_host_io();

    assert_eq!(
        editor_panels(&app),
        [("gone.md".to_string(), [520.0, 400.0])],
        "a file Horizon cannot read opens with the size it uses for such a file"
    );
}

#[test]
#[cfg(unix)]
fn a_held_read_never_stalls_the_drop_frame() {
    let (temp, ctx, mut app) = test_app();
    let held = HeldRead::start(temp.path().join("held.md"));
    let started = Instant::now();
    drop_files(&mut app, &ctx, std::slice::from_ref(&held.path));
    let elapsed = started.elapsed();
    let queued = app.pending_editor_drops.queued();
    let opened = editor_panels(&app);
    held.release();

    assert!(elapsed < FRAME_LIMIT, "the drop frame waited for the read: {elapsed:?}");
    assert_eq!(queued, 1, "the dropped file waits for its size");
    assert!(opened.is_empty(), "the panel opens on the frame that applies its size");

    app.settle_browser_host_io();
    assert_eq!(editor_panels(&app), [("held.md".to_string(), HELD_SIZE)]);
    assert_eq!(app.pending_editor_drops.queued(), 0);
}

#[test]
#[cfg(unix)]
fn two_dropped_files_open_one_panel_each_in_drop_order() {
    let (temp, ctx, mut app) = test_app();
    let held = HeldRead::start(temp.path().join("held.md"));
    let small = temp.path().join("small.md");
    std::fs::write(&small, "small drop\n").expect("fixture");

    drop_files(&mut app, &ctx, &[held.path.clone(), small]);
    let opened = editor_panels(&app);
    held.release();

    assert!(opened.is_empty(), "the second read waits for the first one");

    app.settle_browser_host_io();
    assert_eq!(
        editor_panels(&app),
        [("held.md".to_string(), HELD_SIZE), ("small.md".to_string(), SMALL_SIZE),],
        "each file opens one panel, in the order the person dropped them"
    );
}

#[test]
#[cfg(unix)]
fn a_read_that_applies_after_a_board_replacement_opens_no_panel() {
    let (temp, ctx, mut app) = test_app();
    let held = HeldRead::start(temp.path().join("held.md"));

    drop_files(&mut app, &ctx, std::slice::from_ref(&held.path));
    // A session switch or a cloud prototype restore replaces the board while
    // the read runs, so its outcome may not touch the board that stands now.
    app.browser_create_host.io.replace_board();
    held.release();
    app.settle_browser_host_io();

    assert!(
        editor_panels(&app).is_empty(),
        "the replaced board's drop opens nothing"
    );
    assert_eq!(app.pending_editor_drops.queued(), 0, "the outcome settles its entry");
}

/// A dropped file whose read waits on a named pipe until the test releases it,
/// so a frame that read the file itself would stall.
#[cfg(unix)]
struct HeldRead {
    path: PathBuf,
    permit: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<bool>,
}

#[cfg(unix)]
impl HeldRead {
    fn start(path: PathBuf) -> Self {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .is_ok_and(|status| status.success()),
            "the fixture needs mkfifo"
        );
        let (permit, permitted) = std::sync::mpsc::channel();
        let released = path.clone();
        let thread = std::thread::spawn(move || {
            let mut writer = open_writer(&released);
            // The timeout releases a read that the UI thread performed, so a
            // regression fails on its elapsed time instead of hanging.
            let explicit = permitted.recv_timeout(RELEASE_TIMEOUT).is_ok();
            let content = held_content();
            writer.write_all(content.as_bytes()).expect("written");
            drop(writer);
            // The panel opens its editor from the same path on a later frame; a
            // named pipe would wait there for a writer that never comes.
            std::fs::remove_file(&released).expect("pipe removed");
            std::fs::write(&released, content).expect("regular file");
            explicit
        });
        Self { path, permit, thread }
    }

    fn release(self) {
        self.permit.send(()).expect("release sent");
        assert!(
            self.thread.join().expect("release thread"),
            "the read waited on the pipe instead of the coordination worker"
        );
    }
}

/// Opens the writing end of the pipe once the read holds its own open, which
/// is how the test observes that the worker started the read.
#[cfg(unix)]
fn open_writer(path: &std::path::Path) -> std::fs::File {
    use std::os::unix::fs::OpenOptionsExt as _;

    let deadline = Instant::now() + READER_TIMEOUT;
    loop {
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path);
        if let Ok(writer) = opened {
            return writer;
        }
        assert!(Instant::now() < deadline, "the dropped file was never read");
        std::thread::sleep(Duration::from_millis(1));
    }
}
