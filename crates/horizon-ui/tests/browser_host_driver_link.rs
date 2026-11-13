//! The panel of a Firefox browser host, driven as a person's panel drives it.
//!
//! The two ends of the link are real: the panel is [`BrowserPanelState`], and the
//! program on the other side of `ssh` is this build of Horizon in its
//! `--browser-host-driver` mode. Only two things stand where a browser host would
//! stand: the `ssh` program is a script of this test, and the session of the host
//! runs in this process on a page of a test, so the test needs no browser and no
//! second machine.
//!
//! The scenario is the promise of the design: a panel attaches, it shows a
//! picture, the link dies, and the panel comes back to the same page by itself.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use horizon_browser::FrameSlot;
use horizon_browser_control::ssh_forward::Ssh;
use horizon_core::BrowserHostEngine;
use horizon_core::browser::host_driver::{self as host_driver, HostDriverEngine};
use horizon_core::browser::{AttachedHost, BrowserConfig, BrowserPanelState, BrowserStatus};

const PROFILE: &str = "linktest";
const WAIT: Duration = Duration::from_secs(20);

/// A folder of this test: the home of the host, the script that stands in for
/// `ssh`, and the pid of the link that the script starts.
struct Stand {
    folder: tempfile::TempDir,
    home: PathBuf,
    socket: PathBuf,
    pid_file: PathBuf,
}

impl Stand {
    fn new() -> Self {
        let folder = tempfile::tempdir().expect("a folder of a test");
        let home = folder.path().join("home");
        std::fs::create_dir_all(&home).expect("the home of the host");
        let socket = host_driver::session_socket_in(&home, PROFILE, HostDriverEngine::Firefox)
            .expect("the socket of the session of the host");
        std::fs::create_dir_all(socket.parent().expect("the folder of a socket"))
            .expect("the folder of the socket of the host");
        let pid_file = folder.path().join("link.pid");
        Self {
            folder,
            home,
            socket,
            pid_file,
        }
    }

    /// The script that stands in for `ssh`: it starts this build of Horizon in
    /// the mode of the driver of a host, in the home of the host, and it writes
    /// the process id of that child, so the test kills the link of the panel.
    fn stand_in_ssh(&self) -> String {
        let body = format!(
            "echo $$ > '{}'\nHOME='{}'\nexport HOME\nexec '{}' --browser-host-driver --profile {PROFILE} --engine firefox\n",
            self.pid_file.display(),
            self.home.display(),
            env!("CARGO_BIN_EXE_horizon"),
        );
        horizon_core::stand_in_program_in(self.folder.path(), "ssh", &body)
            .expect("the stand-in ssh of the panel")
            .to_string_lossy()
            .into_owned()
    }

    fn link_pid(&self) -> Option<i32> {
        let text = std::fs::read_to_string(&self.pid_file).ok()?;
        text.trim().parse().ok()
    }
}

/// Start the session of the host on the socket of the same profile, and answer
/// the slot that its page draws into.
fn serve_session(stand: &Stand) -> (Arc<FrameSlot>, std::thread::JoinHandle<i32>) {
    let listener = std::os::unix::net::UnixListener::bind(&stand.socket).expect("the socket of the session");
    let slot = Arc::new(FrameSlot::new());
    let handle =
        host_driver::serve_test_session(listener, stand.socket.clone(), PROFILE.to_string(), Arc::clone(&slot));
    (slot, handle)
}

/// A panel of the host, as a person creates one.
fn panel(ssh: Ssh) -> BrowserPanelState {
    let host = AttachedHost::with_engine("browser-host", Some(PROFILE), BrowserHostEngine::Firefox, None)
        .expect("a host of a test");
    BrowserPanelState::start_host_driver_with(
        ssh,
        "panel-1",
        &BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        &host,
    )
}

/// Poll the panel until `want` holds, and answer whether it did inside the bound.
fn wait_for(panel: &mut BrowserPanelState, want: impl Fn(&BrowserPanelState) -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        panel.drain_events();
        if want(panel) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panel.drain_events();
    want(panel)
}

fn store(slot: &FrameSlot, value: u8) {
    slot.store_rgb(4, 2, vec![value; 4 * 2 * 3])
        .expect("a picture of the page");
}

/// Kill one process, as a lost network kills the link of a panel.
fn kill(pid: i32) {
    // A test kills the child of its own stand-in `ssh` with SIGKILL.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[test]
fn the_panel_of_a_firefox_host_attaches_shows_a_picture_and_comes_back_after_a_lost_link() {
    let stand = Stand::new();
    let (_session_slot, session) = serve_session(&stand);
    let ssh = Ssh::default().with_program(stand.stand_in_ssh());
    let mut panel = panel(ssh);

    // The link comes up, and the picture of the session reaches the frame slot of
    // the panel: this is what a person sees, and what a Chromium panel gives.
    assert!(
        wait_for(&mut panel, |panel| panel.status == BrowserStatus::Ready),
        "the link of the panel greets it: {:?}",
        panel.status
    );
    store(&panel.frame_slot, 10);
    assert!(
        wait_for(&mut panel, |panel| panel.frame_slot.latest().is_some()),
        "the picture of the page reaches the frame slot of the panel: {:?}",
        panel.status
    );
    let picture = panel.frame_slot.latest().expect("a picture of the page");
    assert_eq!((picture.width, picture.height), (4, 2));
    assert_eq!(
        picture.rgb,
        vec![10; 4 * 2 * 3],
        "the picture of the panel is the picture of the page of the host"
    );

    // The link dies without a goodbye, as a laptop that leaves a network does.
    let pid = stand.link_pid().expect("the process id of the link of the panel");
    kill(pid);
    assert!(
        wait_for(&mut panel, |panel| matches!(
            panel.status,
            BrowserStatus::Reconnecting { .. }
        )),
        "a lost link puts the panel into its next attempt: {:?}",
        panel.status
    );

    // The panel attaches again by itself, to the same page of the same session.
    assert!(
        wait_for(&mut panel, |panel| panel.status == BrowserStatus::Ready),
        "the panel attaches again without a person: {:?}",
        panel.status
    );
    assert!(
        panel.page_resumed,
        "the page of the link before is the page of this link"
    );
    assert_eq!(
        panel.navigation_error, None,
        "the page came back whole, so the panel does not say that its state is gone"
    );
    store(&panel.frame_slot, 20);
    assert!(
        wait_for(&mut panel, |panel| panel
            .frame_slot
            .latest()
            .is_some_and(|frame| frame.rgb.first() == Some(&20))),
        "the page of the link that came back draws its newest picture: {:?}",
        panel.status
    );

    // The end of the panel ends its link, so the session has no link left, and a
    // link of this test closes the session as a person's last panel does. A
    // session hands the page to a new link after a second of silence, so this
    // link waits for the link of the panel to be gone.
    drop(panel);
    std::thread::sleep(Duration::from_millis(1500));
    let mut closing = std::os::unix::net::UnixStream::connect(&stand.socket).expect("a link of the test");
    use std::io::Write as _;
    let hello = horizon_browser::host_driver::encode_line(&horizon_browser::host_driver::HostDriverRequest::Hello {
        version: horizon_browser::host_driver::HOST_DRIVER_VERSION,
        horizon_version: env!("CARGO_PKG_VERSION").to_string(),
        system: horizon_browser::host_driver::HostSystem::here(),
        quality: 60,
    })
    .expect("one line");
    closing.write_all(&hello).expect("the greeting of the closing link");
    let close = horizon_browser::host_driver::encode_line(&horizon_browser::host_driver::HostDriverRequest::Close)
        .expect("one line");
    closing.write_all(&close).expect("the close of the session");
    closing.flush().expect("the close of the session");
    let code = session.join().expect("the session of the host ends");
    assert_eq!(code, 0, "a closed session stops its browser and removes its socket");
    assert!(
        !Path::new(&stand.socket).exists(),
        "a stopped session removes its socket"
    );
}
