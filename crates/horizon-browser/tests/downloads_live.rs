//! Live checks that a panel saves its page's downloads into the panel folder,
//! for the second and later page-initiated download too.
//!
//! A page that starts several downloads by itself is the defect of
//! <https://github.com/peters/horizon/issues/1994>: Chromium saved the first
//! file and dropped the rest without an event or an error.
//! Ignored in CI; run with
//! `cargo test -p horizon-browser --test downloads_live -- --ignored`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BackendKind, BrowserCommand, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, FrameSlot,
    start_session,
};

const PDF: &[u8] = b"%PDF-1.4 synthetic download fixture\n%%EOF\n";

/// Three downloads that no click started: the page starts them itself, two
/// seconds apart, which is the sequence a person reported in the issue.
const FIXTURE: &str = r#"<!doctype html><title>downloads live</title><body>
<a id="one" href="/files/one.pdf" download>one</a>
<a id="two" href="/files/two.pdf" download>two</a>
<a id="three" href="/files/three.pdf">three</a>
<script>
for (const id of ['one', 'two', 'three']) {
    setTimeout(() => document.getElementById(id).click(), 500 + 800 * ['one', 'two', 'three'].indexOf(id));
}
</script></body>"#;

#[test]
#[ignore = "requires a local Chromium browser"]
fn chromium_saves_every_download_a_page_starts() {
    assert_page_downloads_land_in_the_panel_folder(BackendKind::ChromiumCdp);
}

#[test]
#[ignore = "requires a local Firefox browser and geckodriver"]
fn firefox_saves_every_download_a_page_starts() {
    assert_page_downloads_land_in_the_panel_folder(BackendKind::FirefoxBidi);
}

fn assert_page_downloads_land_in_the_panel_folder(backend: BackendKind) {
    let (session, _frame_slot, temp) = start(backend);
    wait_until_loaded(&session);
    let downloads = wait_for_downloads(&temp, 3);
    let mut names: Vec<String> = downloads
        .iter()
        .filter_map(|path| path.file_name().map(|name| name.to_string_lossy().to_string()))
        .collect();
    names.sort();
    assert_eq!(names, ["one.pdf", "three.pdf", "two.pdf"], "{downloads:?}");
    for path in &downloads {
        assert_eq!(std::fs::read(path).unwrap_or_default(), PDF, "{}", path.display());
    }
    assert!(session.send(BrowserCommand::Stop));
}

fn wait_for_downloads(temp: &tempfile::TempDir, count: usize) -> Vec<PathBuf> {
    let folder = download_folder(temp);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        // The browser writes a partial file beside the final name.
        let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension != "crdownload" && extension != "part")
                    })
                    .collect()
            })
            .unwrap_or_default();
        if files.len() >= count {
            files.sort();
            return files;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("the page's downloads did not reach {}", folder.display());
}

/// The panel folder is the one the engine selects for this panel, so the check
/// reads it the way a person would: the only `downloads` folder under the
/// profile root of this session.
fn download_folder(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("profiles");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(folder) = find_downloads_folder(&root) {
            return folder;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("no panel download folder under {}", root.display());
}

fn find_downloads_folder(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join("downloads");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn start(backend: BackendKind) -> (BrowserSession, Arc<FrameSlot>, tempfile::TempDir) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || serve_fixture(&listener));
    let temp = session_tempdir();
    let frame_slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend,
            profile_root: Some(temp.path().join("profiles")),
            ..BrowserConfig::default()
        },
        panel_local_id: "downloads-live".into(),
        initial_url: Some(format!("http://{addr}/")),
        width: 800,
        height: 600,
        frame_slot: Arc::clone(&frame_slot),
        coordination: None,
        capture_directory: None,
        video: Arc::new(horizon_browser::VideoCaptureHandle::default()),
        remote: None,
    })
    .expect("start");
    (session, frame_slot, temp)
}

/// A temporary profile root that a confined browser can read. Snap Chromium
/// has a private `/tmp`, so a profile beneath the user's home is the only
/// writable place it can reach.
fn session_tempdir() -> tempfile::TempDir {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Horizon"))
        .filter(|base| std::fs::create_dir_all(base).is_ok())
        .unwrap_or_else(std::env::temp_dir);
    tempfile::tempdir_in(base).expect("temp")
}

fn serve_fixture(listener: &TcpListener) {
    listener.set_nonblocking(false).ok();
    while let Ok((mut stream, _)) = listener.accept() {
        let mut buf = [0_u8; 2048];
        let _ = stream.read(&mut buf);
        let request = String::from_utf8_lossy(&buf);
        let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
        if path.starts_with("/files/") {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                PDF.len()
            );
            let _ = stream.write_all(PDF);
        } else {
            let body = FIXTURE.as_bytes();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
    }
}

/// Wait for the fixture document itself, so the page's own script starts the
/// downloads after the panel is ready.
fn wait_until_loaded(session: &BrowserSession) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        while let Ok(event) = session.event_rx.try_recv() {
            if let BrowserEvent::Title(title) = &event
                && title.contains("downloads live")
            {
                return;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture page did not load");
}
