#![cfg(unix)]

use super::*;
use crate::app::test_support::test_app;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_panel::{CloudGroup, Placement};
use std::time::{Duration, Instant};

fn owner(root: &std::path::Path) -> Owner {
    Owner {
        root: Some(root.into()),
        session: None,
        issue: 101,
        workspace: "fixture".into(),
        generation: 1,
        launch: CloudLaunch {
            publishing_selection: None,
            deployment_started: true,
            id: "fixture".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: deployment().profile,
            placement: Placement::default(),
        },
    }
}

fn deployment() -> cloud_runtime::state::Deployment {
    serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a".repeat(40),
        "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8},
        "stage": "Deleted", "operation": {"state": "terminated", "worker_id": "worker"}, "sessions": []
    }))
    .unwrap()
}

/// Only stops a test that would otherwise hang; each wait ends on an event.
const HANG_GUARD: Duration = Duration::from_secs(30);

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + HANG_GUARD;
    while !done() {
        assert!(Instant::now() < deadline, "worker did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(unix)]
fn fifo(root: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let path = root.join("deployment.json");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    path
}

/// A record read blocked on the FIFO, held open for writing until [`Self::release`].
///
/// A writer opened without blocking succeeds only once a reader has the FIFO open, so this
/// observes the read without taking the record's lock. A test that polled `Store::lock`
/// instead could hold the lock at the moment the worker tried it: the worker then saw
/// `Busy`, never opened the FIFO, and the test waited for a read that never came.
#[cfg(unix)]
struct BlockedRead {
    path: std::path::PathBuf,
    writer: std::fs::File,
}

#[cfg(unix)]
impl BlockedRead {
    fn wait(path: &std::path::Path) -> Self {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut writer = None;
        until(|| {
            writer = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
                .open(path)
                .ok();
            writer.is_some()
        });
        Self {
            path: path.into(),
            writer: writer.unwrap(),
        }
    }

    fn release(mut self) {
        use std::io::Write as _;
        let bytes = serde_json::to_vec(&deployment()).unwrap();
        self.writer.write_all(&bytes).unwrap();
        drop(self.writer);
        std::fs::remove_file(&self.path).unwrap();
        std::fs::write(&self.path, bytes).unwrap();
    }
}

struct Releaser {
    permit: mpsc::Sender<()>,
    blocked: mpsc::Receiver<()>,
    notice: mpsc::Receiver<bool>,
    thread: std::thread::JoinHandle<bool>,
}

impl Releaser {
    fn start(fifo: std::path::PathBuf) -> Self {
        let (permit, permitted) = mpsc::channel();
        let (blocked_sender, blocked) = mpsc::channel();
        let (notice, released) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let read = BlockedRead::wait(&fifo);
            let _ = blocked_sender.send(());
            let explicit = permitted.recv_timeout(HANG_GUARD).is_ok();
            let _ = notice.send(explicit);
            read.release();
            explicit
        });
        Self {
            permit,
            blocked,
            notice: released,
            thread,
        }
    }

    /// Waits until the record read is blocked on the FIFO, under the record's lock.
    fn wait_for_blocked_read(&self) {
        self.blocked
            .recv_timeout(HANG_GUARD)
            .expect("the record read must start");
    }

    fn release(self) {
        self.permit.send(()).unwrap();
        assert!(self.thread.join().unwrap(), "the fallback timeout was not needed");
    }
}

#[test]
#[cfg(unix)]
fn cancelled_display_retains_its_slot_until_io_finishes_then_final_check_holds_custody() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture");
    let fifo = fifo(&path);
    let ctx = egui::Context::default();
    let first = owner(temp.path());
    let mut jobs = Jobs::default();
    jobs.request(first.clone(), Purpose::Display, &ctx);
    let read = BlockedRead::wait(&fifo);
    assert!(
        matches!(Store::lock(&path), Err(cloud_runtime::Error::Busy)),
        "the deployment read holds the record's lock"
    );
    assert!(jobs.poll(&ctx).is_empty(), "the actual deployment read is blocked");
    jobs.cancel(first.issue);
    jobs.reset(); // Session changes must retain the original OS read as well.
    let mut next = first.clone();
    for generation in 2..20 {
        next.generation = generation;
        jobs.request(next.clone(), Purpose::Remove, &ctx);
        assert!(jobs.poll(&ctx).is_empty());
        assert_eq!(jobs.running.len(), 1);
        assert_eq!(jobs.running.values().next().unwrap().request.owner, first);
    }
    read.release();
    let mut completion = None;
    until(|| {
        completion = jobs.poll(&ctx).pop();
        completion.is_some()
    });
    let completion = completion.unwrap();
    assert_eq!(
        completion.owner, next,
        "no cancelled generation can authorize the removal"
    );
    let Outcome::Removal(Ok(custody)) = completion.outcome else {
        panic!("expected fresh removal custody");
    };
    assert!(matches!(Store::lock(&path), Err(cloud_runtime::Error::Busy)));
    jobs.reset();
    assert!(jobs.poll(&ctx).is_empty());
    assert!(
        matches!(Store::lock(&path), Err(cloud_runtime::Error::Busy)),
        "cancelling cannot release applied custody"
    );
    let (_other, mut app) = test_app();
    add_cloud(&mut app, temp.path());
    app.discard_cloud(0, &ctx);
    assert!(
        matches!(Store::lock(&path), Err(cloud_runtime::Error::Busy)),
        "discard runs under the worker lock"
    );
    drop(custody);
    until(|| Store::lock(&path).is_ok());
    until(|| {
        jobs.poll(&ctx);
        jobs.idle()
    });
}

fn add_cloud(app: &mut super::super::HorizonApp, root: &std::path::Path) {
    let workspace = app.board.create_workspace("Fixture");
    let mut group = CloudGroup::new(
        101,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        root.into(),
        [0.0, 0.0],
    );
    group.remote = Some(owner(root).launch);
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(root.into());
    app.cloud_prototype.production.runtimes.insert(
        101,
        super::super::Runtime {
            stage: Some(cloud_runtime::Stage::Deleted),
            state: Some(deployment()),
            ..Default::default()
        },
    );
}

#[test]
#[cfg(unix)]
fn blocked_record_does_not_block_ui_and_stale_context_cannot_remove() {
    for change in ["launch", "root", "workspace", "reopen", "session-reset"] {
        let (temp, mut app) = test_app();
        let path = temp.path().join("fixture");
        let fifo = fifo(&path);
        add_cloud(&mut app, temp.path());
        let ctx = egui::Context::default();
        // A synchronous regression releases after the timeout and fails, rather than hanging CI.
        let releaser = Releaser::start(fifo);
        app.request_cloud_close(101);
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                app.render_cloud_close_confirmation(ui.ctx());
            })
            .discard_textures();
        releaser.wait_for_blocked_read();
        assert!(
            matches!(Store::lock(&path), Err(cloud_runtime::Error::Busy)),
            "the record read holds the record's lock: {change}"
        );
        app.poll_cloud_removals(&ctx);
        let workspace = app.board.create_workspace("Another action");
        assert!(app.board.workspace(workspace).is_some());
        assert!(app.cloud_close_confirmation_open());
        assert!(
            matches!(releaser.notice.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "the UI action must finish before the blocked read is released: {change}"
        );
        app.remove_deleted_cloud(101, &ctx);
        match change {
            "launch" => app.cloud_prototype.groups.0[0].remote.as_mut().unwrap().revision = "b".repeat(40),
            "root" => app.cloud_prototype.root = Some(temp.path().join("other-root")),
            "workspace" => {
                app.cloud_prototype.groups.0[0].workspace = app.board.workspace(workspace).unwrap().local_id.clone();
            }
            "reopen" => app.request_cloud_close(101),
            "session-reset" => {
                app.cloud_prototype.production.close.reset();
                app.request_cloud_close(101);
            }
            _ => unreachable!(),
        }
        releaser.release();
        app.settle_cloud_removals();
        assert_eq!(
            app.cloud_prototype.groups.0.len(),
            1,
            "the old decision cannot remove its replacement: {change}"
        );
        assert_eq!(
            app.cloud_close_confirmation_open(),
            matches!(change, "reopen" | "session-reset"),
            "{change}"
        );
        assert!(path.join("deployment.json").exists(), "the original record stays");
    }
}

#[test]
#[cfg(unix)]
fn after_delete_pending_keeps_cloud_and_has_no_failure_until_the_fresh_check_completes() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let releaser = Releaser::start(fifo(&path));
    add_cloud(&mut app, temp.path());
    app.cloud_prototype.production.close.start_closing(101);
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(app.cloud_prototype.production.closing(101));
    assert!(!app.cloud_close_confirmation_open(), "Pending is not a failed removal");
    app.finish_closing_clouds(&ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert!(
        matches!(releaser.notice.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "Pending must return before the record read finishes"
    );
    releaser.release();
    app.settle_cloud_removals();
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(path.join("deployment.json").exists());
    until(|| Store::lock(&path).is_ok());
}
