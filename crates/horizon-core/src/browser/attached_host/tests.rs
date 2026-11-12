//! A stand-in `ssh` program, a mock `DevTools` endpoint on a loopback port and
//! a captured command log, rather than a machine that runs a browser.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::{BackendKind, BrowserButton, BrowserCommand, BrowserEvent, BrowserInput, BrowserModifiers};
use horizon_browser_control::ssh_forward::Ssh;

use super::super::{BrowserConfig, BrowserDrainOutput, BrowserPanelState, BrowserStatus};
use super::{AttachedHost, AttachedHostFailureKind, attached_host_backend_refusal, connect};

const DEADLINE: Duration = Duration::from_secs(10);
/// A lost link waits three seconds before its first automatic attempt, so a
/// test that waits for that attempt needs a wider bound than a plain connect.
const RETRY_DEADLINE: Duration = Duration::from_secs(60);

/// One test at a time. A child that another thread of this process forked holds
/// the open script of this test, and Linux then refuses to execute it
/// (`ETXTBSY`), so no test writes a script while another one spawns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn quoted(path: &Path) -> String {
    script_text(&path.to_string_lossy())
}

/// One value as the remote shell must read it: one word, with no expansion.
fn script_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// One `sh` program that stands in for `ssh`: it logs the call it got and
/// answers it the way that test needs.
fn stand_in(directory: &Path, log: &Path, body: &str) -> String {
    let script = format!(
        "printf '%s\\n' \"$*\" | tr '\\n' ' ' >> {log}\nprintf '\\n' >> {log}\n{body}",
        log = quoted(log)
    );
    let path = crate::util::stand_in_program_in(directory, "ssh", &script).expect("the stand-in ssh");
    path.to_string_lossy().into_owned()
}

/// The commands that the stand-in received, oldest first.
fn calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn count_start_snippets(log: &Path) -> usize {
    calls(log).iter().filter(|call| call.contains("nohup")).count()
}

fn count_port_reads(log: &Path) -> usize {
    calls(log)
        .iter()
        .filter(|call| call.contains("DevToolsActivePort") && !call.contains("nohup"))
        .count()
}

/// A mock `/json/version` endpoint: it answers the loopback `port` that the
/// forward of a connect names, so the connect asks a real HTTP server without a
/// network.
fn serve_version(port: u16) -> bool {
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) else {
        return false;
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = answer_version(stream, port);
        }
    });
    true
}

fn answer_version(mut stream: TcpStream, port: u16) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? <= 2 {
            break;
        }
    }
    let body = format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/mock"}}"#);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

/// The ports that a probe served, in the order of the forwards of one test.
type Served = Arc<Mutex<Vec<u16>>>;

/// The `ssh` of a test: the stand-in program of `program`, a short bound for one
/// forward, and a probe that records the ports it saw.
fn test_ssh(program: &str, probe: impl Fn(u16) -> bool + Send + Sync + 'static) -> Ssh {
    Ssh::default()
        .with_program(program)
        .with_forward_deadline(DEADLINE)
        .with_probe(probe)
}

/// The endpoint file of a browser that an earlier connect left running.
const RUNNING_BROWSER: &str = "41000";
/// The endpoint file of the browser that the start snippet writes.
const STARTED_BROWSER: &str = "41001";

fn endpoint_text(port: &str) -> String {
    format!("{port}\n/devtools/browser/{port}")
}

/// A panel on `browser-vm`, whose connect asks the stand-in `ssh`.
fn panel(ssh: Ssh, host: &AttachedHost) -> BrowserPanelState {
    BrowserPanelState::start_attached_host_with(
        ssh,
        "panel-attached",
        &BrowserConfig::default(),
        Some("https://example.test/".to_string()),
        host,
    )
}

/// Drain the panel until the connect that is in flight has an outcome.
fn wait_for_connect(panel: &mut BrowserPanelState) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        panel.drain_events();
        let pending = panel.attached.as_ref().is_some_and(|slot| slot.pending.is_some());
        if !pending {
            return;
        }
        assert!(Instant::now() < deadline, "the connect did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Drain the panel until it reports an outcome or an ended session.
fn wait_until(panel: &mut BrowserPanelState, done: impl Fn(&BrowserPanelState) -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !done(panel) {
        panel.drain_events();
        assert!(Instant::now() < deadline, "the panel did not reach the expected state");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_host_needs_one_ssh_name_and_one_profile_under_the_host_rule() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    assert_eq!(host.ssh_target(), "browser-vm");
    assert_eq!(host.profile(), "mail");

    let unnamed = AttachedHost::new("browser-vm", None).expect("a host without a profile");
    assert_eq!(unnamed.profile(), super::DEFAULT_HOST_PROFILE);
    let blank = AttachedHost::new("  browser-vm  ", Some("  ")).expect("spaces around a value");
    assert_eq!(blank.ssh_target(), "browser-vm");
    assert_eq!(blank.profile(), super::DEFAULT_HOST_PROFILE);

    for target in ["-oProxyCommand=evil", "two hosts", "host\nname", ""] {
        let error = AttachedHost::new(target, None).expect_err(target);
        assert!(error.to_string().contains("one SSH name"), "{target}: {error}");
    }
    for profile in ["Mail", "../mail", "-mail", "mail_box"] {
        let error = AttachedHost::new("browser-vm", Some(profile)).expect_err(profile);
        assert!(error.to_string().contains("one profile name"), "{profile}: {error}");
    }
}

#[test]
fn a_browser_host_serves_chromium_only() {
    let _serial = serial();
    assert_eq!(attached_host_backend_refusal(BackendKind::ChromiumCdp), None);
    for backend in [BackendKind::FirefoxBidi, BackendKind::SafariWebDriver] {
        let reason = attached_host_backend_refusal(backend).expect("a refusal");
        assert!(reason.contains("Chromium only"), "{reason}");
    }
}

#[test]
fn an_endpoint_file_that_answers_reuses_the_browser_that_wrote_it() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let served: Served = Arc::default();
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "printf '%s\\n' {running}",
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

    assert_eq!(
        panel.attached_host_failure(),
        None,
        "the browser of that machine answered"
    );
    assert_eq!(
        count_port_reads(&log),
        1,
        "the connect reads the endpoint file that the browser wrote"
    );
    assert_eq!(
        count_start_snippets(&log),
        0,
        "a browser that answers is reused instead of started"
    );
    assert_eq!(lock(&served).len(), 1, "one forward served the connect");
    assert!(panel.session.is_some(), "the ordinary session drives that browser");
    assert_eq!(panel.attached_host_target(), Some("browser-vm"));
    assert_eq!(panel.attached_host_profile(), Some("mail"));
    assert!(
        panel.attach_endpoint.is_none(),
        "the session took the endpoint of the connect"
    );
    panel.stop();
}

#[test]
fn a_stale_endpoint_file_starts_the_browser_of_that_machine_once_more() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let served: Served = Arc::default();
    let attempts = Arc::new(Mutex::new(0_usize));
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "case \" $* \" in\n  *nohup*) printf '%s\\n' {started} ;;\n  *) printf '%s\\n' {running} ;;\nesac",
            running = script_text(&endpoint_text(RUNNING_BROWSER)),
            started = script_text(&endpoint_text(STARTED_BROWSER))
        ),
    );
    let probe = {
        let served = Arc::clone(&served);
        let attempts = Arc::clone(&attempts);
        move |port: u16| {
            *lock(&attempts) += 1;
            if *lock(&attempts) == 1 {
                // The forward of the stale endpoint carries nothing.
                return true;
            }
            lock(&served).push(port);
            serve_version(port)
        }
    };
    let host = AttachedHost::new("browser-vm", None).expect("one SSH name");
    let mut panel = panel(test_ssh(&program, probe), &host);

    wait_for_connect(&mut panel);

    assert_eq!(
        panel.attached_host_failure(),
        None,
        "the browser that the start snippet named answered"
    );
    assert_eq!(count_port_reads(&log), 1);
    assert_eq!(
        count_start_snippets(&log),
        1,
        "a stale endpoint starts exactly one browser"
    );
    assert_eq!(lock(&served).len(), 1, "only the fresh forward answered");
    assert!(panel.session.is_some(), "the ordinary session drives that browser");
    assert_eq!(panel.attached_host_profile(), Some(super::DEFAULT_HOST_PROFILE));
    panel.stop();
}

#[test]
fn a_machine_without_a_chromium_program_keeps_the_words_of_the_snippet() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in(
        directory.path(),
        &log,
        "case \" $* \" in\n  *nohup*) printf '%s\\n' 'this machine has no chromium program' >&2; exit 3 ;;\nesac",
    );
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel(test_ssh(&program, |_port| true), &host);

    wait_for_connect(&mut panel);

    let failure = panel.attached_host_failure().expect("a typed reason");
    assert_eq!(failure.kind(), AttachedHostFailureKind::NoBrowser);
    assert_eq!(failure.code(), "attached_host_no_browser");
    assert!(
        failure.message().contains("has no Chromium program to start"),
        "{}",
        failure.message()
    );
    assert!(
        failure.message().contains("this machine has no chromium program"),
        "the snippet names the reason: {}",
        failure.message()
    );
    assert!(panel.session.is_none(), "no session exists for a connect that failed");
    match &panel.status {
        BrowserStatus::Error { message } => assert_eq!(message, failure.message()),
        other => panic!("the panel must show the reason: {other:?}"),
    }
}

#[test]
fn an_ssh_that_refuses_the_host_key_keeps_its_own_words() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in(
        directory.path(),
        &log,
        "printf '%s\\n' 'Host key verification failed.' >&2\nexit 255",
    );
    let host = AttachedHost::new("browser-vm", None).expect("one SSH name");
    let mut panel = panel(test_ssh(&program, |_port| true), &host);

    wait_for_connect(&mut panel);

    let failure = panel.attached_host_failure().expect("a typed reason");
    assert_eq!(failure.kind(), AttachedHostFailureKind::Unreachable);
    assert_eq!(failure.code(), "attached_host_unreachable");
    assert!(
        failure.message().contains("Host key verification failed."),
        "ssh keeps its own words: {}",
        failure.message()
    );
    assert!(panel.session.is_none());
}

#[test]
fn retry_after_a_lost_link_reconnects_with_a_fresh_forward() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let served: Served = Arc::default();
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "printf '%s\\n' {running}",
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

    let started = Instant::now();
    panel.relaunch();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "Retry never waits for ssh"
    );
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

#[test]
fn a_connect_that_a_panel_stopped_delivers_no_forward() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let served: Served = Arc::default();
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "printf '%s\\n' {running}",
            running = script_text(&endpoint_text(RUNNING_BROWSER))
        ),
    );
    let host = AttachedHost::new("browser-vm", None).expect("one SSH name");
    let stop = AtomicBool::new(false);
    let probed = Arc::clone(&served);
    let ssh = test_ssh(&program, move |port| {
        lock(&probed).push(port);
        serve_version(port)
    });
    stop.store(true, std::sync::atomic::Ordering::Release);
    let outcome = connect(&ssh, &host, &stop);
    assert!(
        matches!(outcome, super::AttachOutcome::Cancelled),
        "a stopped panel starts no forward"
    );
    assert!(lock(&served).is_empty(), "nothing was forwarded");
}

#[test]
fn a_panel_that_was_created_for_a_host_keeps_that_host_after_a_stop() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in(directory.path(), &log, "exit 255");
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = panel(test_ssh(&program, |_port| true), &host);
    wait_for_connect(&mut panel);
    assert!(panel.attached_host_failure().is_some());

    panel.stop();
    assert!(panel.is_attached(), "a stop keeps the host of the panel");
    assert_eq!(panel.attached_host_target(), Some("browser-vm"));
    assert_eq!(panel.attached_host_profile(), Some("mail"));

    panel.relaunch();
    assert_eq!(panel.status, BrowserStatus::Starting);
    wait_for_connect(&mut panel);
    assert!(
        panel.attached_host_failure().is_some(),
        "the retry reached the host again and reported its refusal"
    );
    assert!(count_port_reads(&log) >= 2, "the retry reads the endpoint again");
}

/// The live lane: a real SSH target and a real Chromium of that machine. It runs
/// only when a person names that machine, because it starts a browser there:
///
/// ```sh
/// HORIZON_BROWSER_ATTACH_SSH_TARGET=<target> [HORIZON_BROWSER_ATTACH_PROFILE=<profile>] \
///   cargo test -p horizon-core attached_host_panel_drives_a_real_ssh_target -- --ignored --nocapture
/// ```
#[test]
#[ignore = "starts and drives a real Chromium on the machine named by HORIZON_BROWSER_ATTACH_SSH_TARGET"]
fn attached_host_panel_drives_a_real_ssh_target() {
    let target = std::env::var("HORIZON_BROWSER_ATTACH_SSH_TARGET")
        .expect("set HORIZON_BROWSER_ATTACH_SSH_TARGET to the SSH target of a machine that has a Chromium");
    let profile = std::env::var("HORIZON_BROWSER_ATTACH_PROFILE").ok();
    let host = AttachedHost::new(target, profile.as_deref()).expect("one SSH name");
    let mut panel = BrowserPanelState::start_attached_host(
        "panel-attached-live",
        &BrowserConfig::default(),
        Some("https://example.com/".to_string()),
        &host,
    );

    let deadline = Instant::now() + Duration::from_secs(180);
    while panel.status != BrowserStatus::Ready {
        panel.drain_events();
        if let Some(failure) = panel.attached_host_failure() {
            panic!("{} ({})", failure.message(), failure.code());
        }
        assert!(
            Instant::now() < deadline,
            "the panel did not reach Ready: {:?}",
            panel.status
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let pressed = BrowserCommand::Input(BrowserInput::MousePress {
        x: 100.0,
        y: 120.0,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 1,
        modifiers: BrowserModifiers::default(),
    });
    let released = BrowserCommand::Input(BrowserInput::MouseRelease {
        x: 100.0,
        y: 120.0,
        button: BrowserButton::Left,
        click_count: 1,
        buttons: 0,
        modifiers: BrowserModifiers::default(),
    });
    assert!(panel.try_send(pressed), "the page takes a pointer press");
    assert!(panel.try_send(released), "the page takes a pointer release");

    while panel.frame_slot.latest().is_none() {
        panel.drain_events();
        assert!(Instant::now() < deadline, "the page of that machine sent no frame");
        std::thread::sleep(Duration::from_millis(50));
    }

    panel.stop();
    let stop = AtomicBool::new(false);
    match connect(&Ssh::default(), &host, &stop) {
        super::AttachOutcome::Connected(_) => {}
        super::AttachOutcome::Failed(failure) => panic!(
            "the remote Chromium must outlive the panel: {} ({})",
            failure.message(),
            failure.code()
        ),
        super::AttachOutcome::Cancelled => panic!("the check of the running browser was cancelled"),
    }
}

#[test]
fn a_panel_of_a_browser_host_records_that_host_and_profile() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let program = stand_in(directory.path(), &log, "exit 255");
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let panel = panel(test_ssh(&program, |_port| true), &host);

    let record = crate::runtime_state::browser_profile_state(&panel, false);
    let saved = record.host.expect("the saved panel names its browser host");
    assert_eq!(saved.ssh_target, "browser-vm");
    assert_eq!(saved.profile.as_deref(), Some("mail"));
    assert_eq!(record.backend, Some(BackendKind::ChromiumCdp));
    assert!(
        record.root.is_none(),
        "the Chromium of another machine owns its profile folder there"
    );
    assert!(record.session_id.is_none(), "no local profile group holds this page");
}

#[test]
fn a_browser_that_stops_after_a_page_keeps_the_panel_at_its_last_address() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = BrowserPanelState::attached_host_for_tests(&host, "https://example.test/");

    // The events of a session that showed a page and then lost its WebSocket.
    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::UrlChanged("https://example.test/inbox".to_string()),
        &mut output,
    );
    panel.apply_event(BrowserEvent::Ready, &mut output);
    panel.apply_event(
        BrowserEvent::Warning(
            "CDP connection lost: websocket: WebSocket protocol error: Connection reset without closing handshake"
                .to_string(),
        ),
        &mut output,
    );
    panel.apply_event(BrowserEvent::Stopped { code: None }, &mut output);

    let failure = panel.attached_host_failure().expect("a typed reason");
    assert_eq!(failure.kind(), AttachedHostFailureKind::ConnectionLost);
    assert_eq!(failure.code(), "attached_host_connection_lost");
    assert_eq!(
        failure.message(),
        "the browser of browser-vm in the profile mail lost its connection, so this panel keeps its last page; \
         Retry connects again",
        "the panel says one plain sentence, and no word of the engine"
    );
    assert_eq!(
        failure.detail(),
        Some("CDP connection lost: websocket: WebSocket protocol error: Connection reset without closing handshake"),
        "the log and an agent keep the words of the lost connection"
    );
    assert!(
        failure.report().contains("Connection reset without closing handshake"),
        "an agent reads the words of the tool: {}",
        failure.report()
    );
    assert!(
        failure.report().contains("browser-vm") && failure.report().contains("mail"),
        "the reason of an agent names the host and its profile: {}",
        failure.report()
    );
    assert!(!panel.has_ended(), "the panel stays where the person put it");
    assert_eq!(
        panel.display_url(),
        "https://example.test/inbox",
        "the last address stays in the panel"
    );
    assert!(panel.can_retry(), "the panel offers the Retry of a connect");
    assert!(panel.retry_ready(), "nothing waits before that Retry");
}

#[test]
fn a_browser_that_never_showed_a_page_ends_the_panel_of_the_host() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", None).expect("one SSH name");
    let mut panel = BrowserPanelState::attached_host_for_tests(&host, "https://example.test/");

    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::Warning("CDP connect failed: the handshake timed out".to_string()),
        &mut output,
    );
    panel.apply_event(BrowserEvent::Stopped { code: None }, &mut output);

    assert_eq!(
        panel.attached_host_failure(),
        None,
        "a panel that showed no page keeps no reason of a lost page"
    );
    assert!(panel.has_ended(), "the create reports why no page opened");
}

#[test]
fn the_delays_of_a_lost_link_grow_and_a_long_connection_starts_them_again() {
    use super::next_attempt;
    assert_eq!(next_attempt(0, None).0, 1);
    assert_eq!(next_attempt(0, None).1, Duration::from_secs(3));
    assert_eq!(next_attempt(3, None), (4, Duration::from_secs(24)));
    assert_eq!(next_attempt(4, None), (5, Duration::from_secs(30)));
    assert_eq!(next_attempt(9, None), (10, Duration::from_secs(30)));
    let long_connection = Instant::now()
        .checked_sub(crate::RECONNECT_RESET_AFTER)
        .expect("the test machine has been up for ten seconds");
    assert_eq!(
        next_attempt(4, Some(long_connection)),
        (1, Duration::from_secs(3)),
        "a connection that lasted ten seconds starts the delays again"
    );
    assert_eq!(
        next_attempt(4, Some(Instant::now())),
        (5, Duration::from_secs(30)),
        "a link that drops again waits longer"
    );
}

/// The events of a session that showed a page and then lost its WebSocket.
fn lose_the_link(panel: &mut BrowserPanelState) {
    let mut output = BrowserDrainOutput::default();
    panel.apply_event(
        BrowserEvent::UrlChanged("https://example.test/inbox".to_string()),
        &mut output,
    );
    panel.apply_event(BrowserEvent::Ready, &mut output);
    panel.apply_event(
        BrowserEvent::Warning(
            "CDP connection lost: websocket: WebSocket protocol error: Connection reset without closing handshake"
                .to_string(),
        ),
        &mut output,
    );
    panel.apply_event(BrowserEvent::Stopped { code: None }, &mut output);
}

#[test]
fn a_lost_link_waits_for_the_next_attempt_and_keeps_the_address() {
    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = BrowserPanelState::attached_host_for_tests(&host, "https://example.test/");

    lose_the_link(&mut panel);

    let (attempt, retry_at) = panel.attached_retry().expect("an automatic attempt");
    assert_eq!(attempt, 1, "the first attempt after the first loss");
    assert!(
        retry_at > Instant::now() && retry_at <= Instant::now() + Duration::from_secs(3),
        "the attempt starts after the first delay of the link"
    );
    assert!(
        !panel.has_ended(),
        "a panel that waits for its own attempt has not ended"
    );
    assert!(!panel.status.is_alive(), "nothing is connected while it waits");
    assert_eq!(
        panel.display_url(),
        "https://example.test/inbox",
        "the person keeps the address of the page"
    );
    assert_eq!(
        panel.attached_host_failure().map(super::AttachedHostFailure::kind),
        Some(AttachedHostFailureKind::ConnectionLost),
        "the reason of the loss stays for the log and for an agent"
    );
    assert!(panel.can_retry(), "a person can still ask for the connect now");
}

#[test]
fn an_unreachable_attempt_continues_but_a_missing_browser_needs_a_person() {
    use horizon_browser_control::ssh_forward::SshForwardError;

    let _serial = serial();
    let host = AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name");
    let mut panel = BrowserPanelState::attached_host_for_tests(&host, "https://example.test/");
    lose_the_link(&mut panel);
    let (first, _) = panel.attached_retry().expect("an automatic attempt");

    // A cut ends the attempt of an unchanged route, so the next one follows.
    panel.attached_failed(super::AttachedHostFailure::unreachable(
        &host,
        "ssh: connect to host browser-vm port 22: Connection timed out",
    ));
    let (next, retry_at) = panel.attached_retry().expect("the next attempt");
    assert_eq!(next, first + 1);
    assert!(retry_at > Instant::now(), "the next delay starts after the attempt");

    // A machine without Chromium will not heal by itself.
    panel.attached_failed(super::AttachedHostFailure::from_ssh(
        &host,
        &SshForwardError::RemoteCommandFailed {
            target: "browser-vm".to_string(),
            code: Some(3),
            reason: "no chromium".to_string(),
        },
    ));
    assert_eq!(panel.attached_retry(), None, "a person must install that browser");
    assert!(
        matches!(panel.status, BrowserStatus::Error { .. }),
        "the panel keeps the present failure: {:?}",
        panel.status
    );
    assert!(panel.attached_host_failure().is_some());
}

#[test]
fn a_lost_link_comes_back_by_itself_and_says_that_the_page_is_new() {
    let _serial = serial();
    let directory = scratch();
    let log = directory.path().join("commands.log");
    let served: Served = Arc::default();
    let program = stand_in(
        directory.path(),
        &log,
        &format!(
            "printf '%s\\n' {running}",
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
    // The mock endpoint answers no CDP handshake, so the first session ends as
    // it would on a lost link, before a page.
    wait_until(&mut panel, BrowserPanelState::has_ended);
    assert_eq!(count_port_reads(&log), 1);

    // The person had a page of that browser before the link went away.
    panel.attached_reached_ready();
    lose_the_link(&mut panel);
    let (attempt, retry_at) = panel.attached_retry().expect("an automatic attempt");
    assert_eq!(attempt, 1);
    assert!(retry_at > Instant::now());
    let waiting = Instant::now();

    // No person presses Retry: the panel connects again by itself.
    let deadline = waiting + RETRY_DEADLINE;
    while panel.session.is_none() {
        panel.drain_events();
        assert!(Instant::now() < deadline, "the automatic attempt never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        waiting.elapsed() >= Duration::from_secs(3),
        "the attempt waited the first delay of the link"
    );
    assert_eq!(
        count_port_reads(&log),
        2,
        "the attempt read the endpoint of that browser again"
    );
    assert_eq!(lock(&served).len(), 2, "the attempt opened one fresh forward");
    assert_eq!(
        count_start_snippets(&log),
        0,
        "the browser of that machine keeps running"
    );
    assert_eq!(
        panel.display_url(),
        "https://example.test/inbox",
        "the new page opens the address of the one the person had"
    );
    assert_eq!(
        panel.navigation_error.as_deref(),
        Some(super::PAGE_STATE_LOST),
        "the panel says that the page is new and that what it held is gone"
    );
    assert_eq!(
        panel.attached_host_failure(),
        None,
        "the automatic attempt reached the browser of that machine"
    );
    panel.stop();
}
