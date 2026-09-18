use super::*;
use crate::cli::dispatch::{Command, Dispatcher};

fn config() -> Value {
    json!({"id":"fixture","endpoint":{"kind":"local_x11","display":":999999"},
        "desktop_resize":{"vnc_address":"127.0.0.1:5900","policy":{"max_width":1920,"max_height":1080,"max_pixels":2_073_600}},
        "owner_note":"retained"})
}
#[test]
fn permission_changes_preserve_configuration_and_journals_without_a_device() -> Result<()> {
    let directory = tempfile::tempdir().map_err(unavailable)?;
    let path = directory.path().join("target.json");
    let original = config();
    std::fs::write(&path, original.to_string()).map_err(unavailable)?;
    for suffix in ["resize-pending", "resize-observe"] {
        std::fs::write(directory.path().join(format!("target.json.{suffix}")), b"retained").map_err(unavailable)?;
    }
    let dispatcher = Dispatcher {
        target_file: path.clone(),
        resize_factory: None,
    };
    for enabled in [true, false, false] {
        let response = dispatcher.call(Command::SetResizeEnabled(ResizePermission { enabled }));
        assert_eq!(response.value["ok"], true);
        assert_eq!(response.value["result"]["enabled"], enabled);
        assert_eq!(response.value["result"]["max_pixels"], 2_073_600);
        drop(response);
        let mut saved: Value =
            serde_json::from_slice(&std::fs::read(&path).map_err(unavailable)?).map_err(unavailable)?;
        assert_eq!(saved["desktop_resize"]["policy"]["enabled"], enabled);
        saved["desktop_resize"]["policy"]
            .as_object_mut()
            .ok_or_else(invalid_object)?
            .remove("enabled");
        assert_eq!(saved, original);
        for suffix in ["resize-pending", "resize-observe"] {
            assert_eq!(
                std::fs::read(directory.path().join(format!("target.json.{suffix}"))).map_err(unavailable)?,
                b"retained"
            );
        }
    }
    Ok(())
}
#[test]
fn failed_preparation_preserves_original_configuration() -> Result<()> {
    let directory = tempfile::tempdir().map_err(unavailable)?;
    let path = directory.path().join("target.json");
    let original = config().to_string().into_bytes();
    let target: Target = serde_json::from_slice(&original).map_err(unavailable)?;
    let permission = ResizePermission { enabled: true };
    std::fs::write(&path, b"external edit").map_err(unavailable)?;
    assert!(save(&path, &original, target, &permission).is_err());
    assert_eq!(std::fs::read(&path).map_err(unavailable)?, b"external edit");
    assert_eq!(std::fs::read_dir(directory.path()).map_err(unavailable)?.count(), 1);
    let mut oversized = config();
    oversized["owner_note"] = Value::String("x".repeat(4096));
    let bytes = serde_json::to_vec(&oversized).map_err(unavailable)?;
    std::fs::write(&path, &bytes).map_err(unavailable)?;
    assert!(
        save(
            &path,
            &bytes,
            serde_json::from_slice(&bytes).map_err(unavailable)?,
            &permission
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).map_err(unavailable)?, bytes);
    Ok(())
}
#[cfg(unix)]
#[test]
fn permission_writes_preserve_mode_and_refuse_readonly_or_symlink_targets() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().map_err(unavailable)?;
    let path = directory.path().join("target.json");
    let bytes = config().to_string().into_bytes();
    std::fs::write(&path, &bytes).map_err(unavailable)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).map_err(unavailable)?;
    let permission = ResizePermission { enabled: true };
    save(
        &path,
        &bytes,
        serde_json::from_slice(&bytes).map_err(unavailable)?,
        &permission,
    )?;
    assert_eq!(
        path.metadata().map_err(unavailable)?.permissions().mode() & 0o777,
        0o640
    );
    let saved = std::fs::read(&path).map_err(unavailable)?;
    let link = directory.path().join("link.json");
    symlink(&path, &link).map_err(unavailable)?;
    assert!(
        save(
            &link,
            &saved,
            serde_json::from_slice(&saved).map_err(unavailable)?,
            &permission
        )
        .is_err()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440)).map_err(unavailable)?;
    assert!(
        save(
            &path,
            &saved,
            serde_json::from_slice(&saved).map_err(unavailable)?,
            &permission
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).map_err(unavailable)?, saved);
    Ok(())
}
