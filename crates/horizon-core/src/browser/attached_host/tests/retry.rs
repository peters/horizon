//! The retry of a browser host after a lost link, and the other paths that must
//! not hold the caller that asks for them.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::*;

/// The longest wait of a test before it reports a stall, so a wait of the product
/// that a test holds fails the test instead of hanging it.
const HANG_GUARD: Duration = Duration::from_secs(10);

/// How often the stand-in answered the endpoint that a connect reads.
fn count_answers(answers: &Path) -> usize {
    std::fs::read_to_string(answers).unwrap_or_default().lines().count()
}

/// A thread that releases the gate of a stand-in program if the test stalls, so a
/// wait of the product fails the test instead of hanging it.
fn release_on_hang(gate: std::path::PathBuf, done: mpsc::Receiver<()>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if done.recv_timeout(HANG_GUARD).is_err() {
            let _ = std::fs::write(&gate, b"");
        }
    })
}

#[test]
fn retry_after_a_lost_link_reconnects_with_a_fresh_forward() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let close = directory.path().join("hold-the-retry");
    let release = directory.path().join("release-the-retry");
    let answers = directory.path().join("answers.log");
    let served: Served = Arc::default();
    // The stand-in answers at once while the gate is open, so the first connect
    // runs as every other connect of these tests. The test closes the gate after
    // that connect, so the endpoint that the retry reads cannot answer until the
    // test answers for it: the retry returns while that read waits here.
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "case \"$*\" in\n  *DevToolsActivePort*)\n    if [ -e {close} ]; then while [ ! -e {release} ]; do sleep 1; done; fi\n    printf 'read answered\\n' >> {answers}\n    ;;\nesac\nprintf '%s\\n' {running}",
            close = quoted(&close),
            release = quoted(&release),
            answers = quoted(&answers),
            running = script_text(&endpoint_text(RUNNING_BROWSER))
        ),
    );
    let probe = {
        let served = Arc::clone(&served);
        move |port: u16| {
            lock(&served).push(port);
            serve_version(port)
        }
    };
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel(test_ssh(&program, probe), &host);

    wait_for_connect(&mut panel);
    assert_eq!(panel.attached_host_failure(), None);
    // The mock endpoint answers no CDP handshake, so the session of the panel
    // ends as it would on a lost link.
    wait_until(&mut panel, BrowserPanelState::has_ended);
    assert_eq!(
        panel.attached_host_failure(),
        None,
        "the connect of the first session succeeded"
    );
    let first = lock(&served).first().copied().expect("the first forward");

    assert_eq!(count_answers(&answers), 1, "the first connect read the endpoint once");
    std::fs::write(&close, b"").expect("the test closes the gate of the retry");
    let (done, waiting) = mpsc::channel();
    let watchdog = release_on_hang(release.clone(), waiting);
    panel.relaunch();
    assert_eq!(
        count_answers(&answers),
        1,
        "the retry returns while the endpoint that it reads still waits at the gate"
    );
    std::fs::write(&release, b"").expect("the test releases the gate of the retry");
    let _ = done.send(());
    watchdog.join().expect("the watchdog of the gate ends");
    assert_eq!(panel.status, BrowserStatus::Starting);
    assert_eq!(panel.attached_host_target(), Some("browser-vm"));
    assert_eq!(
        panel.attached_host_failure(),
        None,
        "the new connect starts with no reason of the old one"
    );

    wait_for_connect(&mut panel);
    assert_eq!(panel.attached_host_failure(), None);
    let ports = lock(&served).clone();
    assert_eq!(ports.len(), 2, "the retry opened one fresh forward: {ports:?}");
    assert_eq!(ports[0], first, "the first forward of this panel answered");
    assert_eq!(count_port_reads(&log), 2, "the retry reads the endpoint again");
    assert_eq!(count_answers(&answers), 2, "the read of the retry answered once");
    assert_eq!(count_start_snippets(&log), 0, "the browser of the host is reused");
    assert!(panel.session.is_some(), "the retry starts the ordinary session again");
    panel.stop();
}

#[test]
fn a_host_that_does_not_answer_never_holds_the_caller() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in(directory.path(), &log, "sleep 1\nexit 255");
    let host = AttachedHost::new("browser-vm", None).expect("one SSH name");

    let started = Instant::now();
    let mut panel = panel(test_ssh(&program, |_port| true), &host);
    let opened = started.elapsed();
    assert!(
        opened < Duration::from_millis(500),
        "a panel of a browser host opens without waiting for ssh: {opened:?}"
    );
    assert!(panel.is_attached());
    assert_eq!(panel.status, BrowserStatus::Starting);
    assert!(panel.attached_host_failure().is_none(), "the connect is in flight");

    panel.stop();
    // The connect ends at its next step and takes no forward with it.
    std::thread::sleep(Duration::from_millis(1500));
    panel.drain_events();
    assert!(panel.session.is_none());
    assert_eq!(panel.status, BrowserStatus::Stopped { code: None });
    assert_eq!(panel.attached_host_target(), Some("browser-vm"));
}
