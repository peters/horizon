//! Runs a test again in a child process that cannot start Docker: its `PATH` holds every
//! program of this process except `docker`, and `DOCKER_HOST` names no daemon. Any Docker
//! call on the path under test then fails, so passing proves that the path needs none.
use std::process::Command;

const CHILD: &str = "HORIZON_TEST_WITHOUT_DOCKER";

/// In the parent, runs `test`, its full path as `--exact` takes it, in the child and
/// asserts that it passed, then returns `false`. In the child, makes sure that `docker`
/// cannot start and returns `true`, so the caller runs the test body there.
pub(in crate::cloud_runtime) fn child(test: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        let docker = Command::new("docker").arg("--version").output();
        assert!(
            matches!(&docker, Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "docker is still on PATH: {docker:?}"
        );
        return true;
    }
    let programs = tempfile::tempdir().unwrap();
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let link = programs.path().join(&name);
            // The first directory on PATH wins, as it does for a lookup.
            if name.to_string_lossy().starts_with("docker") || link.symlink_metadata().is_ok() {
                continue;
            }
            std::os::unix::fs::symlink(entry.path(), link).unwrap();
        }
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("PATH", programs.path())
        .env("DOCKER_HOST", "unix:///nonexistent/horizon-test-docker.sock")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("1 passed"), "{stdout}");
    false
}
