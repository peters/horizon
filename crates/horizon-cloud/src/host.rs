//! Host bootstrap for providers that rent whole virtual machines. A typed plan
//! becomes `#cloud-config` user data that makes a fresh Ubuntu server with Docker
//! run the unchanged worker image: the container owns port 22 and serves
//! `/workspace` from the attached volume. Every string value is written through
//! YAML serialization into files that scripts only read, so none is interpolated
//! into a shell command. The one number the start script embeds, the shared
//! memory size, is range-checked first.
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
    /// A lowercase DNS host name with an optional numeric port, such as
    /// `example.azurecr.io` or `registry.example:5000`. IPv6 literals are not supported.
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
    content: &'a str,
}

impl Plan {
    /// The `#cloud-config` user data for this plan.
    /// # Errors
    /// Refuses invalid values and user data above `USER_DATA_LIMIT`.
    pub fn cloud_config(&self) -> Result<zeroize::Zeroizing<String>, CloudError> {
        self.validate()?;
        // Contents that may carry secrets stay in zeroizing storage until encoded.
        let environment = zeroize::Zeroizing::new(self.environment_file());
        let registry = self.registry.as_ref().map(registry_config).transpose()?;
        let (image, guard, start, unit) = (
            format!("{}\n", self.image),
            guard_script(),
            start_script(self.shm_gb),
            unit(),
        );
        let file = |path, permissions, content| WriteFile {
            path,
            owner: "root:root",
            permissions,
            content,
        };
        let mut write_files = vec![
            file(ENVIRONMENT_FILE, "0600", environment.as_str()),
            file(IMAGE_FILE, "0600", &image),
            file(GUARD, "0700", &guard),
            file(START, "0700", &start),
            file(UNIT, "0644", &unit),
        ];
        if let Some(registry) = &registry {
            write_files.push(file(REGISTRY_FILE, "0600", registry.as_str()));
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
        if !valid_image(&self.image) || !valid_digest_reference(&self.image) {
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
            let server = valid_authority(&registry.server);
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

/// `[host[:port]/]path[:tag]@sha256:<64 hex>`, as Docker parses it, so a plan never
/// renders a reference every boot would fail to pull.
fn valid_digest_reference(image: &str) -> bool {
    let Some((name, digest)) = image.split_once('@') else {
        return false;
    };
    let digest_valid = digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let mut components: Vec<&str> = name.split('/').collect();
    let Some(last) = components.pop() else {
        return false;
    };
    let (last, tag) = match last.split_once(':') {
        Some((last, tag)) => (last, Some(tag)),
        None => (last, None),
    };
    let tag_valid = tag.is_none_or(|tag| {
        (1..=128).contains(&tag.len())
            && !tag.starts_with(['.', '-'])
            && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    });
    let host = components
        .first()
        .filter(|first| first.contains(['.', ':']) || **first == "localhost")
        .copied();
    let paths_valid = components
        .iter()
        .skip(usize::from(host.is_some()))
        .chain(std::iter::once(&last))
        .all(|component| valid_path_component(component));
    // Docker limits the repository path, without registry, tag or digest, to 255 characters.
    let path_length = components[usize::from(host.is_some())..]
        .iter()
        .map(|component| component.len() + 1)
        .sum::<usize>()
        + last.len();
    digest_valid && tag_valid && paths_valid && path_length <= 255 && host.is_none_or(valid_authority)
}

/// Lowercase alphanumeric runs joined by Docker's separators: one `.`, one or two
/// `_`, or any run of `-`.
fn valid_path_component(component: &str) -> bool {
    let mut rest = component;
    loop {
        let run = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
            .unwrap_or(rest.len());
        if run == 0 {
            return false;
        }
        rest = &rest[run..];
        if rest.is_empty() {
            return true;
        }
        let separator = rest
            .find(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(rest.len());
        if !matches!(&rest[..separator], "." | "_" | "__") && !rest[..separator].bytes().all(|b| b == b'-') {
            return false;
        }
        rest = &rest[separator..];
    }
}

/// `host[:port]`: dot-separated DNS labels and an optional port from 1 to 65535.
fn valid_authority(authority: &str) -> bool {
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    host.len() <= 253
        && host.split('.').all(label)
        && port.is_none_or(|port| {
            port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|port| port > 0)
        })
}

/// Docker keeps Docker Hub credentials under its legacy index URL, not the host name.
fn auth_key(server: &str) -> &str {
    match server {
        "docker.io" | "index.docker.io" | "registry-1.docker.io" => "https://index.docker.io/v1/",
        _ => server,
    }
}

/// Docker's `config.json` with one login. Every buffer holding the credential is
/// zeroizing and sized up front, so no copy is left behind by reallocation.
fn registry_config(registry: &RegistryLogin) -> Result<zeroize::Zeroizing<String>, CloudError> {
    #[derive(Serialize)]
    struct Config<'a> {
        auths: BTreeMap<&'a str, Auth<'a>>,
    }
    #[derive(Serialize)]
    struct Auth<'a> {
        auth: &'a str,
    }
    let password = registry.password.value();
    let mut pair = zeroize::Zeroizing::new(String::with_capacity(registry.username.len() + 1 + password.len()));
    pair.push_str(&registry.username);
    pair.push(':');
    pair.push_str(password);
    let mut auth = zeroize::Zeroizing::new(String::with_capacity(pair.len().div_ceil(3) * 4));
    base64::engine::general_purpose::STANDARD.encode_string(pair.as_bytes(), &mut auth);
    let key = auth_key(&registry.server);
    let config = Config {
        auths: BTreeMap::from([(key, Auth { auth: &auth })]),
    };
    let mut json = zeroize::Zeroizing::new(Vec::with_capacity(auth.len() + key.len() + 64));
    serde_json::to_writer(&mut *json, &config)
        .map_err(|_| CloudError::Invalid("Registry login could not be encoded"))?;
    let text = String::from_utf8(std::mem::take(&mut *json))
        .map_err(|_| CloudError::Invalid("Registry login could not be encoded"))?;
    Ok(zeroize::Zeroizing::new(text))
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
  --log-driver local --log-opt max-size=10m --log-opt max-file=5 \\
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
