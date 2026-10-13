//! A Horizon with `HORIZON_HOME` and `HORIZON_AGENT_INTEGRATIONS=off` writes nothing in the
//! user's home directory outside its Horizon home. The binary starts without a display, so it
//! does all of its startup work and then stops where it would open the window.
#![cfg(target_os = "linux")]
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXIT_GUARD: Duration = Duration::from_mins(2);

/// Runs the binary with only the given environment and returns every path that it left in
/// `home` outside `home/private`, the Horizon home.
fn paths_outside_the_horizon_home(integrations: &str) -> Vec<PathBuf> {
    let root = tempfile::tempdir().expect("temporary directory");
    let home = root.path().join("home");
    std::fs::create_dir(&home).expect("home directory");
    let horizon_home = home.join("private");
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("HORIZON_HOME", &horizon_home)
        .env("HORIZON_AGENT_INTEGRATIONS", integrations)
        .current_dir(root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start horizon");
    let started = Instant::now();
    while child.try_wait().expect("poll horizon").is_none() {
        if started.elapsed() > EXIT_GUARD {
            let _ = child.kill();
            let _ = child.wait();
            panic!("horizon without a display did not stop within {EXIT_GUARD:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // The session store opens after every integration install, so its folder proves that the
    // run got that far.
    assert!(
        horizon_home.join("sessions").is_dir(),
        "horizon stopped before it opened its session store"
    );
    let mut outside = Vec::new();
    collect_outside(&home, &horizon_home, &mut outside);
    outside
}

fn collect_outside(directory: &Path, horizon_home: &Path, outside: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read directory") {
        let path = entry.expect("directory entry").path();
        if path == horizon_home {
            continue;
        }
        if path.symlink_metadata().expect("metadata").is_dir() {
            collect_outside(&path, horizon_home, outside);
        }
        outside.push(path);
    }
}

#[test]
fn integrations_off_writes_nothing_outside_the_horizon_home() {
    let outside = paths_outside_the_horizon_home("off");
    assert!(outside.is_empty(), "horizon wrote in the home directory: {outside:#?}");
}

#[test]
fn integrations_on_write_into_the_home_directory() {
    // Proves that the check above sees what the integrations write.
    let outside = paths_outside_the_horizon_home("on");
    assert!(
        !outside.is_empty(),
        "the integrations wrote nothing in the home directory"
    );
}
