use std::io;
use std::path::Path;

use horizon_core::agent_work::{HOOK_COMMAND, HOOK_SHELL};
use serde_json::json;

/// This plugin is attached only to opted-in panels. Its command uses a quoted
/// environment expansion so paths remain data and every hook runs the same
/// executable as its owning Horizon process.
pub(super) fn install(directory: &Path) -> io::Result<()> {
    super::sync_file_if_changed(
        &directory.join(".claude-plugin/plugin.json"),
        &json!({"name": "horizon-work-resume", "version": "0.1.0", "description": "Record opted-in panel work across restarts"}).to_string(),
    )?;
    let mut hooks = serde_json::Map::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "SessionEnd",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Elicitation",
        "ElicitationResult",
    ] {
        hooks.insert(
            event.into(),
            json!([{"hooks": [{
                "type": "command", "command": HOOK_COMMAND, "shell": HOOK_SHELL, "timeout": 3
            }]}]),
        );
    }
    super::sync_file_if_changed(
        &directory.join("hooks/hooks.json"),
        &json!({"hooks": hooks}).to_string(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedicated_plugin_preserves_hook_identity_without_embedded_host_paths() {
        let directory = tempfile::tempdir().expect("plugin");
        install(directory.path()).expect("install");
        let text = std::fs::read_to_string(directory.path().join("hooks/hooks.json")).expect("hooks");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["hooks"].as_object().expect("events").len(), 10);
        assert_eq!(value["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
        assert!(!text.contains(directory.path().to_str().expect("path")));
        install(directory.path()).expect("idempotent install");
        assert_eq!(
            text,
            std::fs::read_to_string(directory.path().join("hooks/hooks.json")).expect("hooks")
        );
    }
}
