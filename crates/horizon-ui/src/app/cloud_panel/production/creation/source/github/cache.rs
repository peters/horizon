//! The connected account's repository names, kept from the last time GitHub listed them, so
//! the dialog searches them the moment it opens while a fresh list is asked for.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn path(root: &Path) -> PathBuf {
    root.join("github-repositories.json")
}

/// The names last listed for the app `app_id`; none for another app or a damaged file.
pub(super) fn load(root: &Path, app_id: u64) -> Option<Vec<String>> {
    let cached: Value = serde_json::from_slice(&std::fs::read(path(root)).ok()?).ok()?;
    if cached.get("app_id").and_then(Value::as_u64) != Some(app_id) {
        return None;
    }
    cached
        .get("repositories")?
        .as_array()?
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect()
}

/// Keeps `repositories` for the next dialog. A failure only costs the head start.
pub(super) fn save(root: &Path, app_id: u64, repositories: &[String]) {
    let cached = json!({"app_id": app_id, "repositories": repositories});
    let Ok(bytes) = serde_json::to_vec(&cached) else {
        return;
    };
    let target = path(root);
    let temporary = target.with_extension("json.tmp");
    // A file left by an earlier try keeps its own mode; a new one is private.
    let _ = std::fs::remove_file(&temporary);
    let written = write_private(&temporary, &bytes).and_then(|()| std::fs::rename(&temporary, &target));
    if let Err(error) = written {
        tracing::debug!(%error, "could not keep the GitHub repository list");
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::{load, save};

    #[test]
    fn the_list_is_kept_for_its_own_app_only() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(load(root.path(), 7), None);
        save(root.path(), 7, &["acme/web".into()]);
        assert_eq!(load(root.path(), 7), Some(vec!["acme/web".to_owned()]));
        assert_eq!(load(root.path(), 8), None, "another app's names are not this one's");
    }
}
