use std::fmt::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use horizon_core::ManagedInstall;
use surge_core::context::{Context as SurgeContext, StorageProvider};
use surge_core::update::manager::UpdateManager;

use super::HorizonApp;

const UPDATE_CHANNEL: &str = "stable";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AvailableUpdate {
    pub(super) latest_version: String,
    pub(super) installer_url: String,
    pub(super) error_message: Option<String>,
}

#[derive(Debug)]
pub(super) enum UpdateCheckMessage {
    Available(AvailableUpdate),
    Unavailable,
    Error(String),
}

impl HorizonApp {
    pub(super) fn maybe_start_update_check(&mut self) {
        let Some(managed_install) = self.managed_install.clone() else {
            return;
        };
        let Some(next_due) = self.next_surge_update_check_at else {
            return;
        };

        if !managed_install.uses_stable_channel() || !managed_install.uses_github_releases() {
            return;
        }

        if self.surge_update_check_rx.is_some() || self.surge_available_update.is_some() {
            return;
        }

        if !update_check_is_due(Instant::now(), next_due) {
            return;
        }

        self.next_surge_update_check_at = Some(next_update_check_deadline(Instant::now()));
        self.surge_update_check_rx = Some(spawn_update_check(managed_install));
    }

    pub(super) fn poll_update_check(&mut self) {
        let Some(rx) = self.surge_update_check_rx.as_ref() else {
            return;
        };

        match rx.try_recv() {
            Ok(UpdateCheckMessage::Available(update)) => {
                self.surge_available_update = Some(update);
                self.surge_update_check_rx = None;
            }
            Ok(UpdateCheckMessage::Unavailable) | Err(TryRecvError::Disconnected) => {
                self.surge_update_check_rx = None;
            }
            Ok(UpdateCheckMessage::Error(error)) => {
                tracing::warn!(%error, "Surge update check failed");
                self.surge_update_check_rx = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    pub(super) fn has_available_update(&self) -> bool {
        self.surge_available_update.is_some()
    }

    pub(super) fn available_update_hover_text(&self) -> Option<String> {
        let update = self.surge_available_update.as_ref()?;
        let mut text = format!(
            "Horizon {} is available.\nDownload the latest stable installer.",
            update.latest_version
        );
        if let Some(error_message) = &update.error_message {
            let _ = write!(text, "\n\nLast download attempt failed: {error_message}");
        }
        Some(text)
    }

    pub(super) fn open_available_update(&mut self) {
        let Some(update) = self.surge_available_update.as_mut() else {
            return;
        };

        match open_external_url(&update.installer_url) {
            Ok(()) => update.error_message = None,
            Err(error) => update.error_message = Some(error),
        }
    }
}

fn spawn_update_check(managed_install: ManagedInstall) -> mpsc::Receiver<UpdateCheckMessage> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let message = match check_for_update(&managed_install) {
            Ok(Some(prompt)) => UpdateCheckMessage::Available(prompt),
            Ok(None) => UpdateCheckMessage::Unavailable,
            Err(error) => UpdateCheckMessage::Error(error),
        };
        let _ = tx.send(message);
    });
    rx
}

fn check_for_update(managed_install: &ManagedInstall) -> Result<Option<AvailableUpdate>, String> {
    let Some(rid) = current_release_rid() else {
        return Ok(None);
    };
    let Some(installer_asset) = installer_asset_name(rid) else {
        return Ok(None);
    };
    let provider = parse_storage_provider(&managed_install.provider)?;
    let ctx = Arc::new(SurgeContext::new());
    let install_dir = managed_install.install_root.to_string_lossy().into_owned();

    ctx.set_storage(
        provider,
        &managed_install.bucket,
        &managed_install.region,
        "",
        "",
        &managed_install.endpoint,
    );

    let mut manager = UpdateManager::new(
        Arc::clone(&ctx),
        &managed_install.app_id,
        &managed_install.version,
        UPDATE_CHANNEL,
        &install_dir,
    )
    .map_err(|error| error.to_string())?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start Surge runtime: {error}"))?;

    let update_info = runtime
        .block_on(manager.check_for_updates())
        .map_err(|error| error.to_string())?;

    let Some(update_info) = update_info else {
        return Ok(None);
    };

    Ok(Some(AvailableUpdate {
        latest_version: update_info.latest_version.clone(),
        installer_url: installer_download_url(&managed_install.bucket, &update_info.latest_version, installer_asset)?,
        error_message: None,
    }))
}

fn parse_storage_provider(raw: &str) -> Result<StorageProvider, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "github" | "github_releases" | "githubreleases" => Ok(StorageProvider::GitHubReleases),
        other => Err(format!("unsupported Surge storage provider for updates: {other}")),
    }
}

fn installer_download_url(bucket: &str, version: &str, installer_asset: &str) -> Result<String, String> {
    let (owner, repo) = parse_github_repository(bucket)?;
    Ok(format!(
        "https://github.com/{owner}/{repo}/releases/download/v{version}/{installer_asset}"
    ))
}

fn parse_github_repository(bucket: &str) -> Result<(&str, &str), String> {
    let trimmed = bucket.trim().trim_matches('/');
    let mut parts = trimmed.split('/');
    let owner = parts.next().unwrap_or_default().trim();
    let repo = parts.next().unwrap_or_default().trim();
    let extra = parts.next();

    if owner.is_empty() || repo.is_empty() || extra.is_some() {
        return Err(format!(
            "invalid GitHub Releases repository bucket '{bucket}', expected 'owner/repo'"
        ));
    }

    Ok((owner, repo))
}

fn update_check_is_due(now: Instant, next_due: Instant) -> bool {
    now >= next_due
}

fn next_update_check_deadline(now: Instant) -> Instant {
    now + UPDATE_CHECK_INTERVAL
}

fn installer_asset_name(rid: &str) -> Option<&'static str> {
    match rid {
        "linux-x64" => Some("horizon-installer-linux-x64.bin"),
        "osx-arm64" => Some("horizon-installer-osx-arm64.bin"),
        "osx-x64" => Some("horizon-installer-osx-x64.bin"),
        "win-x64" => Some("horizon-installer-win-x64.exe"),
        _ => None,
    }
}

fn current_release_rid() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("osx-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("osx-x64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("win-x64")
    } else {
        None
    }
}

fn open_external_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open installer download: {error}"))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        installer_asset_name, installer_download_url, next_update_check_deadline, parse_github_repository,
        update_check_is_due,
    };

    #[test]
    fn installer_asset_name_matches_release_assets() {
        assert_eq!(
            installer_asset_name("linux-x64"),
            Some("horizon-installer-linux-x64.bin")
        );
        assert_eq!(installer_asset_name("win-x64"), Some("horizon-installer-win-x64.exe"));
        assert_eq!(installer_asset_name("linux-arm64"), None);
    }

    #[test]
    fn installer_download_url_uses_versioned_release_assets() {
        assert_eq!(
            installer_download_url("peters/horizon-surge-smoke", "0.2.0", "horizon-installer-win-x64.exe"),
            Ok(
                "https://github.com/peters/horizon-surge-smoke/releases/download/v0.2.0/horizon-installer-win-x64.exe"
                    .to_string()
            )
        );
    }

    #[test]
    fn parse_github_repository_rejects_invalid_bucket() {
        assert!(parse_github_repository("peters").is_err());
        assert!(parse_github_repository("peters/horizon/extra").is_err());
    }

    #[test]
    fn update_check_is_due_when_deadline_has_passed() {
        let now = Instant::now();

        assert!(update_check_is_due(now + Duration::from_secs(1), now));
        assert!(!update_check_is_due(now, now + Duration::from_secs(1)));
    }

    #[test]
    fn next_update_check_deadline_is_twenty_four_hours_out() {
        let now = Instant::now();
        let deadline = next_update_check_deadline(now);

        assert_eq!(deadline.duration_since(now), Duration::from_secs(24 * 60 * 60));
    }
}
