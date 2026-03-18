#![cfg(not(windows))]

mod support;

use std::ffi::OsString;

use support::transcript_capture::{assert_shell_and_command_panels_start_without_capture, run_self};

#[test]
fn shell_and_command_panels_launch_when_script_is_missing() {
    run_self(
        "shell_and_command_panels_launch_when_script_is_missing_child",
        path_without_script_dirs(),
    );
}

#[test]
#[ignore = "helper child process for PATH-isolated transcript fallback coverage"]
fn shell_and_command_panels_launch_when_script_is_missing_child() {
    assert_shell_and_command_panels_start_without_capture();
}

fn path_without_script_dirs() -> Option<OsString> {
    let path = std::env::var_os("PATH")?;
    let filtered: Vec<_> = std::env::split_paths(&path)
        .filter(|dir| !dir.join("script").is_file())
        .collect();

    if filtered.is_empty() {
        None
    } else {
        Some(std::env::join_paths(filtered).expect("filtered PATH"))
    }
}
