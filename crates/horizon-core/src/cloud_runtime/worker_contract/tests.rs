use super::*;

#[test]
fn legacy_markers_accept_only_the_original_effective_capabilities() {
    let legacy = "horizon-worker-contract=1\nhorizon-source-contract=1\n";
    assert!(validate(legacy, &Capabilities::default(), false).is_ok());
    let minimal = serde_json::from_str("{}").unwrap();
    assert!(validate(legacy, &minimal, false).is_err());
    assert!(validate(legacy, &Capabilities::default(), true).is_err());
    let remote = serde_json::from_str(r#"{"browserstack":{}}"#).unwrap();
    assert!(validate(&format!("{legacy}{CAPABILITIES_MARKER}\n"), &remote, false).is_err());
}

#[cfg(unix)]
#[test]
fn readiness_preserves_strict_legacy_arguments_and_runs_modern_service_checks() {
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let root = tempfile::tempdir().unwrap();
    let checker = root.path().join("horizon-worker-check");
    let observed = root.path().join("capabilities");
    let ready = root.path().join("ready");
    // No system utilities: marker recognition must use only shell built-ins.
    let path = root.path();
    let legacy = "#!/bin/sh\n[ $# -eq 0 ] || exit 77\nprintf '%s' \"$HORIZON_WORKER_CAPABILITIES\" > \"$OBSERVED\"\nprintf 'horizon-worker-contract=1\\nhorizon-source-contract=1\\n'\n";
    let modern = "#!/bin/sh\nprintf '%s' \"$HORIZON_WORKER_CAPABILITIES\" > \"$OBSERVED\"\nif [ $# -gt 0 ]; then\n [ \"$1\" = --ready ] || exit 78\n printf ready > \"$READY\"\n [ \"$READY_FAIL\" != 1 ] || exit 79\nfi\nprintf 'horizon-worker-contract=1\\nhorizon-source-contract=1\\nhorizon-capabilities-contract=1\\nhorizon-extra-contract=1\\n'\n";
    for script in [legacy, modern] {
        std::fs::write(&checker, script).unwrap();
        std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o700)).unwrap();
        for capabilities in [Capabilities::default(), serde_json::from_str("{}").unwrap()] {
            let output = Command::new("/bin/sh")
                .args(["-c", &readiness_command(&capabilities).unwrap()])
                .env("PATH", path)
                .env("OBSERVED", &observed)
                .env("READY", &ready)
                .env_remove("READY_FAIL")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                serde_json::from_slice::<Capabilities>(&std::fs::read(&observed).unwrap()).unwrap(),
                capabilities
            );
            assert_eq!(ready.exists(), script == modern);
            assert_eq!(
                validate(&String::from_utf8(output.stdout).unwrap(), &capabilities, false).is_ok(),
                script == modern || capabilities == Capabilities::default()
            );
        }
    }
    let failed = Command::new("/bin/sh")
        .args(["-c", &readiness_command(&Capabilities::default()).unwrap()])
        .env("PATH", path)
        .env("OBSERVED", observed)
        .env("READY", ready)
        .env("READY_FAIL", "1")
        .status()
        .unwrap();
    assert!(!failed.success(), "modern service failures must reach the coordinator");
}

#[cfg(unix)]
#[test]
fn incidental_capability_marker_does_not_enable_modern_readiness() {
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let root = tempfile::tempdir().unwrap();
    let checker = root.path().join("horizon-worker-check");
    for marker in [
        "prefix-horizon-capabilities-contract=1",
        "horizon-capabilities-contract=1-suffix",
    ] {
        std::fs::write(
            &checker,
            format!("#!/bin/sh\n[ $# -eq 0 ] || exit 99\nprintf '%s\\n' '{marker}'\n"),
        )
        .unwrap();
        std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = Command::new("/bin/sh")
            .args(["-c", &readiness_command(&Capabilities::default()).unwrap()])
            .env("PATH", root.path())
            .status()
            .unwrap();
        assert!(result.success());
    }
}
