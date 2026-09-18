use crate::{DeviceError, Result, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizePermission {
    pub enabled: bool,
}

pub(super) fn save(path: &Path, original: &[u8], mut target: Target, permission: &ResizePermission) -> Result<Value> {
    target.desktop_resize.validate()?;
    let metadata = path.symlink_metadata().map_err(unavailable)?;
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(DeviceError::Invalid(
            "permission changes require a writable regular target file".into(),
        ));
    }
    let mut document: Value = serde_json::from_slice(original).map_err(unavailable)?;
    let mut policy = &mut document;
    for field in ["desktop_resize", "policy"] {
        policy = policy
            .as_object_mut()
            .ok_or_else(invalid_object)?
            .entry(field)
            .or_insert_with(|| json!({}));
    }
    policy
        .as_object_mut()
        .ok_or_else(invalid_object)?
        .insert("enabled".into(), json!(permission.enabled));
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let encoded = serde_json::to_vec(&document).map_err(unavailable)?;
    if encoded.len() + 1 > usize::from(super::MAX_TARGET_BYTES) {
        return Err(DeviceError::Invalid("updated target config too large".into()));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(unavailable)?;
    temporary.write_all(&encoded).map_err(unavailable)?;
    temporary.write_all(b"\n").map_err(unavailable)?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())
        .map_err(unavailable)?;
    temporary.as_file().sync_all().map_err(unavailable)?;
    if std::fs::read(path).map_err(unavailable)? != original {
        return Err(DeviceError::Unavailable(
            "target configuration changed during update".into(),
        ));
    }
    temporary.persist(path).map_err(unavailable)?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(unavailable)?;
    target.desktop_resize.policy.enabled = permission.enabled;
    serde_json::to_value(target.desktop_resize.policy).map_err(unavailable)
}
fn invalid_object() -> DeviceError {
    DeviceError::Invalid("target resize policy must be an object".into())
}
fn unavailable(error: impl std::fmt::Display) -> DeviceError {
    DeviceError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests;
