use std::path::{Path, PathBuf};

use serde::Deserialize;

const SURGE_RUNTIME_MANIFEST: &str = ".surge/runtime.yml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedInstall {
    pub app_id: String,
    pub version: String,
    pub channel: String,
    pub install_directory: String,
    pub supervisor_id: String,
    pub provider: String,
    pub bucket: String,
    pub region: String,
    pub endpoint: String,
    pub active_app_dir: PathBuf,
    pub install_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ManagedInstallFile {
    #[serde(rename = "id")]
    app_id: String,
    version: String,
    channel: String,
    #[serde(rename = "installDirectory", default)]
    install_directory: String,
    #[serde(rename = "supervisorId", default)]
    supervisor_id: String,
    provider: String,
    bucket: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    endpoint: String,
}

impl ManagedInstall {
    #[must_use]
    pub fn discover(current_exe: &Path) -> Option<Self> {
        let active_app_dir = current_exe.parent()?.to_path_buf();
        let install_root = active_app_dir.parent().unwrap_or(&active_app_dir).to_path_buf();
        let runtime_manifest_path = active_app_dir.join(SURGE_RUNTIME_MANIFEST);

        let Ok(raw) = std::fs::read_to_string(&runtime_manifest_path) else {
            return None;
        };

        let manifest: ManagedInstallFile = match serde_yaml::from_str(&raw) {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(
                    path = %runtime_manifest_path.display(),
                    %error,
                    "failed to parse Surge runtime manifest"
                );
                return None;
            }
        };

        if manifest.app_id.trim().is_empty()
            || manifest.version.trim().is_empty()
            || manifest.channel.trim().is_empty()
            || manifest.provider.trim().is_empty()
            || manifest.bucket.trim().is_empty()
        {
            tracing::warn!(
                path = %runtime_manifest_path.display(),
                "ignoring incomplete Surge runtime manifest"
            );
            return None;
        }

        Some(Self {
            app_id: manifest.app_id,
            version: manifest.version,
            channel: manifest.channel,
            install_directory: manifest.install_directory,
            supervisor_id: manifest.supervisor_id,
            provider: manifest.provider,
            bucket: manifest.bucket,
            region: manifest.region,
            endpoint: manifest.endpoint,
            active_app_dir,
            install_root,
        })
    }

    #[must_use]
    pub fn uses_stable_channel(&self) -> bool {
        self.channel.eq_ignore_ascii_case("stable")
    }

    #[must_use]
    pub fn uses_github_releases(&self) -> bool {
        self.provider.eq_ignore_ascii_case("github_releases")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::ManagedInstall;

    #[test]
    fn discover_returns_none_without_runtime_manifest() {
        let temp = tempdir().expect("temp dir");
        let exe = temp.path().join("app").join("horizon");
        fs::create_dir_all(exe.parent().expect("exe parent")).expect("create app dir");
        fs::write(&exe, b"binary").expect("write exe");

        assert!(ManagedInstall::discover(&exe).is_none());
    }

    #[test]
    fn discover_reads_runtime_manifest_next_to_executable() {
        let temp = tempdir().expect("temp dir");
        let app_dir = temp.path().join("install-root").join("app");
        fs::create_dir_all(app_dir.join(".surge")).expect("create surge dir");

        let exe = app_dir.join("horizon");
        fs::write(&exe, b"binary").expect("write exe");
        fs::write(
            app_dir.join(".surge/runtime.yml"),
            concat!(
                "id: horizon-linux-x64\n",
                "version: 0.2.0\n",
                "channel: stable\n",
                "installDirectory: horizon\n",
                "provider: github_releases\n",
                "bucket: peters/horizon-updates\n",
                "region: surge\n",
            ),
        )
        .expect("write runtime manifest");

        let managed = ManagedInstall::discover(&exe).expect("managed install");
        assert_eq!(managed.app_id, "horizon-linux-x64");
        assert_eq!(managed.version, "0.2.0");
        assert_eq!(managed.install_root, temp.path().join("install-root"));
        assert!(managed.uses_stable_channel());
        assert!(managed.uses_github_releases());
    }

    #[test]
    fn discover_rejects_incomplete_runtime_manifest() {
        let temp = tempdir().expect("temp dir");
        let app_dir = temp.path().join("install-root").join("app");
        fs::create_dir_all(app_dir.join(".surge")).expect("create surge dir");

        let exe = app_dir.join("horizon");
        fs::write(&exe, b"binary").expect("write exe");
        fs::write(
            app_dir.join(".surge/runtime.yml"),
            "id: horizon-linux-x64\nversion: 0.2.0\nchannel: stable\nprovider: github_releases\n",
        )
        .expect("write runtime manifest");

        assert!(ManagedInstall::discover(&exe).is_none());
    }
}
