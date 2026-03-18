#![cfg(not(windows))]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use support::transcript_capture::{assert_shell_and_command_panels_start_without_capture, run_self, temp_root};

#[test]
fn shell_and_command_panels_launch_when_script_is_incompatible() {
    let fake_script_dir = make_incompatible_script_dir();
    run_self(
        "shell_and_command_panels_launch_when_script_is_incompatible_child",
        Some(fake_script_dir.as_os_str().to_os_string()),
    );
    fs::remove_dir_all(fake_script_dir).ok();
}

#[test]
#[ignore = "helper child process for PATH-isolated transcript fallback coverage"]
fn shell_and_command_panels_launch_when_script_is_incompatible_child() {
    assert_shell_and_command_panels_start_without_capture();
}

fn make_incompatible_script_dir() -> PathBuf {
    let root = temp_root("bad-script");
    let script_path = root.join("script");
    let temp_path = script_path.with_extension("tmp");

    fs::write(&temp_path, "#!/bin/sh\necho unsupported flags >&2\nexit 64\n").expect("script body");
    let mut permissions = fs::metadata(&temp_path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&temp_path, permissions).expect("script permissions");
    fs::rename(&temp_path, &script_path).expect("script rename");

    root
}
