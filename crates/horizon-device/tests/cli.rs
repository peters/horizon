#![cfg(feature = "cli")]
use serde_json::{Value, json};
use std::process::Command;

#[test]
fn malformed_actions_and_unavailable_targets_fail_structurally() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target.json");
    std::fs::write(
        &target,
        json!({"id":"absent", "endpoint":{"kind":"local_x11","display":":999999"}}).to_string(),
    )?;
    for command in [vec!["doctor"], vec!["act", "{}"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
            .arg("--target")
            .arg(&target)
            .args(command)
            .output()?;
        assert!(!result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(value["ok"], false);
        assert!(value["error"]["code"].is_string());
    }
    Ok(())
}

#[test]
fn cooperating_process_lock_prevents_backend_access() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target.json");
    std::fs::write(&target, "not even a valid config")?;
    let lock = std::fs::File::create(target.with_extension("lock"))?;
    lock.lock()?;
    let result = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
        .arg("--target")
        .arg(&target)
        .arg("doctor")
        .output()?;
    let value: Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(value["ok"], false);
    assert!(value["error"]["message"].as_str().is_some_and(|m| m.contains("busy")));
    Ok(())
}
