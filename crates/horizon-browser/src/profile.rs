use std::path::PathBuf;

use crate::{BrowserConfig, paths};

pub(crate) fn resolved_for_launch(config: &BrowserConfig) -> std::io::Result<BrowserConfig> {
    let mut resolved = config.clone();
    resolved.quality = resolved.quality.clamp(1, 100);
    let Some(configured_root) = &config.profile_root else {
        return Ok(resolved);
    };
    let expanded_root = paths::expand_tilde(&configured_root.to_string_lossy());
    resolved.profile_root = Some(if expanded_root.is_absolute() {
        expanded_root
    } else {
        std::env::current_dir()?.join(expanded_root)
    });
    Ok(resolved)
}

#[must_use]
pub(crate) fn profile_dir(config: &BrowserConfig, panel_local_id: &str) -> PathBuf {
    match &config.profile_root {
        Some(root) => paths::expand_tilde(&root.to_string_lossy()).join(paths::safe_local_id(panel_local_id)),
        None => paths::browser_profile_dir(&paths::default_root(), panel_local_id),
    }
}

pub(crate) fn schedule_removal(profile_dir: PathBuf) -> std::sync::mpsc::Receiver<std::io::Result<()>> {
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();
    let worker_tx = completion_tx.clone();
    let spawn = std::thread::Builder::new()
        .name("browser-profile-cleanup".into())
        .spawn(move || {
            let result = match std::fs::remove_dir_all(&profile_dir) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            let _ = worker_tx.send(result);
        });
    if let Err(error) = spawn {
        tracing::warn!("failed to start browser profile cleanup: {error}");
        let _ = completion_tx.send(Err(error));
    }
    completion_rx
}
