//! Host bootstrap for providers that rent whole virtual machines. A typed plan
//! becomes `#cloud-config` user data that makes a fresh Ubuntu server with Docker
//! run the unchanged worker image: the container owns port 22 and serves
//! `/workspace` from the attached volume. Every value is written through YAML
//! serialization into files; scripts only read those files, so no plan value is
//! ever interpolated into a shell command.
use crate::{CloudError, Credential, valid_image};
use base64::Engine as _;
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write as _};

/// The largest user data providers accept (Hetzner's limit).
pub const USER_DATA_LIMIT: usize = 32 * 1024;
/// Where the workspace volume is mounted on the host.
pub const VOLUME_MOUNT: &str = "/mnt/horizon-volume";
/// The directory on the volume the container sees as `/workspace`. A subdirectory
/// keeps the filesystem's own `lost+found` out of the worker's view.
pub const WORKSPACE: &str = "/mnt/horizon-volume/workspace";
const ENVIRONMENT_FILE: &str = "/etc/horizon-worker/worker.env";
const IMAGE_FILE: &str = "/etc/horizon-worker/image";
const REGISTRY_FILE: &str = "/root/.docker/config.json";
const GUARD: &str = "/usr/local/sbin/horizon-worker-guard";
const START: &str = "/usr/local/sbin/horizon-worker-start";
const UNIT: &str = "/etc/systemd/system/horizon-worker.service";
const DEVICE_PREFIX: &str = "/dev/disk/by-id/";

/// A private registry the host logs in to before pulling the image.
#[derive(Debug)]
pub struct RegistryLogin {
    /// Host name with an optional port, such as `example.azurecr.io`.
    pub server: String,
    pub username: String,
    pub password: Credential,
}

#[derive(Debug)]
pub struct Plan {
    /// An immutable image reference, `name@sha256:<digest>`.
    pub image: String,
    /// The container's environment, such as `PUBLIC_KEY` and `HORIZON_CLOUD_OPERATION`.
    pub environment: BTreeMap<String, String>,
    pub registry: Option<RegistryLogin>,
    /// The attached volume's stable device path under `/dev/disk/by-id/`. The
    /// provider formats it as ext4; the host never formats a volume.
    pub workspace_device: String,
    /// Size of `/dev/shm` in GB, which Chromium needs beyond Docker's 64 MB default.
    pub shm_gb: u8,
}

#[derive(Serialize)]
struct CloudConfig<'a> {
    write_files: Vec<WriteFile<'a>>,
    mounts: Vec<[&'a str; 6]>,
    runcmd: Vec<Vec<&'a str>>,
}
#[derive(Serialize)]
struct WriteFile<'a> {
    path: &'a str,
    owner: &'a str,
    permissions: &'a str,
    content: String,
}

impl Plan {
    /// The `#cloud-config` user data for this plan.
    /// # Errors
    /// Refuses invalid values and user data above `USER_DATA_LIMIT`.
    pub fn cloud_config(&self) -> Result<zeroize::Zeroizing<String>, CloudError> {
        self.validate()?;
        let file = |path, permissions, content| WriteFile {
            path,
            owner: "root:root",
            permissions,
            content,
        };
        let mut write_files = vec![
            file(ENVIRONMENT_FILE, "0600", self.environment_file()),
            file(IMAGE_FILE, "0600", format!("{}\n", self.image)),
            file(GUARD, "0700", guard_script()),
            file(START, "0700", start_script(self.shm_gb)),
            file(UNIT, "0644", unit()),
        ];
        if let Some(registry) = &self.registry {
            write_files.push(file(REGISTRY_FILE, "0600", registry_config(registry)?));
        }
        let config = CloudConfig {
            write_files,
            mounts: vec![[
                &self.workspace_device,
                VOLUME_MOUNT,
                "ext4",
                "defaults,nofail,discard",
                "0",
                "2",
            ]],
            runcmd: vec![
                // The container owns port 22; the host keeps no SSH or password login.
                vec!["systemctl", "disable", "--now", "ssh.socket", "ssh.service"],
                vec!["passwd", "--lock", "root"],
                vec!["systemctl", "daemon-reload"],
                vec!["systemctl", "enable", "--now", "horizon-worker.service"],
            ],
        };
        let yaml = zeroize::Zeroizing::new(
            serde_yaml::to_string(&config)
                .map_err(|_| CloudError::Invalid("Host configuration could not be encoded"))?,
        );
        let document = zeroize::Zeroizing::new(format!("#cloud-config\n{}", yaml.as_str()));
        if document.len() > USER_DATA_LIMIT {
            return Err(CloudError::Invalid(
                "Host configuration exceeds the 32 KiB user-data limit",
            ));
        }
        Ok(document)
    }

    fn validate(&self) -> Result<(), CloudError> {
        if !valid_image(&self.image) || !self.image.contains("@sha256:") {
            return Err(CloudError::Invalid("Host image must be an immutable image digest"));
        }
        if !self
            .environment
            .iter()
            .all(|(name, value)| valid_variable(name) && !value.chars().any(char::is_control))
        {
            return Err(CloudError::Invalid(
                "Worker environment names use A-Z, 0-9 and underscores, and values are single-line text",
            ));
        }
        let device = self.workspace_device.strip_prefix(DEVICE_PREFIX).unwrap_or_default();
        if device.is_empty()
            || device.starts_with('.')
            || !device
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(CloudError::Invalid(
                "Workspace device must be a stable path under /dev/disk/by-id/",
            ));
        }
        if !(1..=64).contains(&self.shm_gb) {
            return Err(CloudError::Invalid("Shared memory must be between 1 and 64 GB"));
        }
        if let Some(registry) = &self.registry {
            let server = !registry.server.is_empty()
                && registry.server.len() <= 253
                && registry
                    .server
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-:".contains(&b));
            let username = !registry.username.is_empty()
                && registry.username.len() <= 256
                && !registry.username.contains(':')
                && !registry.username.chars().any(char::is_control);
            if !server || !username {
                return Err(CloudError::Invalid("Invalid registry server or username"));
            }
        }
        Ok(())
    }

    /// Docker's env-file format: one `NAME=value` per line, taken literally.
    fn environment_file(&self) -> String {
        self.environment.iter().fold(String::new(), |mut file, (name, value)| {
            let _ = writeln!(file, "{name}={value}");
            file
        })
    }
}

fn valid_variable(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && name.len() <= 128
}

fn registry_config(registry: &RegistryLogin) -> Result<String, CloudError> {
    let pair = zeroize::Zeroizing::new(format!("{}:{}", registry.username, registry.password.value()));
    let auth = zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(pair.as_bytes()));
    let config = serde_json::json!({"auths": {registry.server.as_str(): {"auth": auth.as_str()}}});
    serde_json::to_string(&config).map_err(|_| CloudError::Invalid("Registry login could not be encoded"))
}

/// Runs before every container start. The metadata service serves the user data,
/// including any registry credential, so the container must never reach it.
fn guard_script() -> String {
    format!(
        "#!/bin/sh
set -eu
command -v docker >/dev/null || {{ echo 'horizon-worker: Docker is not installed on this host' >&2; exit 1; }}
mountpoint -q {VOLUME_MOUNT} || {{ echo 'horizon-worker: the workspace volume is not mounted' >&2; exit 1; }}
mkdir -p {WORKSPACE}
iptables -C DOCKER-USER -d 169.254.169.254/32 -j DROP 2>/dev/null || iptables -I DOCKER-USER -d 169.254.169.254/32 -j DROP
"
    )
}

/// Pulls the image once, retrying, and keeps it cached so restarts need no registry.
fn start_script(shm_gb: u8) -> String {
    format!(
        "#!/bin/sh
set -eu
image=$(cat {IMAGE_FILE})
attempt=0
until docker image inspect \"$image\" >/dev/null 2>&1 || docker pull --quiet \"$image\"; do
  attempt=$((attempt + 1))
  [ \"$attempt\" -lt 10 ] || {{ echo 'horizon-worker: the image could not be pulled' >&2; exit 1; }}
  sleep 15
done
exec docker run --rm --name horizon-worker --pull never -p 22:22 --shm-size {shm_gb}g \\
  -v {WORKSPACE}:/workspace --env-file {ENVIRONMENT_FILE} \"$image\"
"
    )
}

fn unit() -> String {
    format!(
        "[Unit]
Description=Horizon worker container
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target
RequiresMountsFor={VOLUME_MOUNT}

[Service]
ExecStartPre={GUARD}
ExecStartPre=-/usr/bin/docker rm --force horizon-worker
ExecStart={START}
ExecStop=/usr/bin/docker stop horizon-worker
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
"
    )
}

#[cfg(test)]
mod tests;
