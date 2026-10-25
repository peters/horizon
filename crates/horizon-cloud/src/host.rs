//! Host bootstrap for providers that rent whole virtual machines. A typed plan
//! becomes `#cloud-config` user data that makes a fresh Ubuntu server with Docker
//! run the unchanged worker image: the container owns port 22 and serves
//! `/workspace` from the attached volume. Every string value is written through
//! YAML serialization into files that scripts only read, so none is interpolated
//! into a shell command. The one number the start script embeds, the shared
//! memory size, is range-checked first.
//!
//! A [`Platform`] says what the host must do itself before the worker starts: a
//! Hetzner `docker-ce` server has Docker and an ext4 volume already, while a plain
//! Ubuntu VM on Azure installs Docker and formats its blank data disk once.
pub mod pull;

use crate::{CloudError, Credential, valid_image};
use base64::Engine as _;
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write as _};

/// The largest user data providers accept (Hetzner's limit).
pub const USER_DATA_LIMIT: usize = 32 * 1024;
/// Where the workspace volume is mounted on a host whose image does not mount `/mnt`.
pub const VOLUME_MOUNT: &str = "/mnt/horizon-volume";
/// Where an Azure host mounts the workspace volume. The Azure image mounts the VM's
/// temporary disk at `/mnt`, and that mount hides anything mounted beneath it, so an
/// Azure volume sits outside `/mnt` where no image mount can shadow it.
pub const AZURE_VOLUME_MOUNT: &str = "/horizon-volume";
/// The directory on the volume the container sees as `/workspace`. A subdirectory
/// keeps the filesystem's own `lost+found` out of the worker's view.
pub const WORKSPACE: &str = "/mnt/horizon-volume/workspace";
const ENVIRONMENT_FILE: &str = "/etc/horizon-worker/worker.env";
const IMAGE_FILE: &str = "/etc/horizon-worker/image";
const REGISTRY_FILE: &str = "/root/.docker/config.json";
const GUARD: &str = "/usr/local/sbin/horizon-worker-guard";
const SETUP: &str = "/usr/local/sbin/horizon-worker-setup";
const START: &str = "/usr/local/sbin/horizon-worker-start";
const INGRESS: &str = "/usr/local/sbin/horizon-worker-ingress";
const UNIT: &str = "/etc/systemd/system/horizon-worker.service";
const DEVICE_PREFIX: &str = "/dev/disk/by-id/";
/// The stable path of an Azure VM's data disk on LUN 0, made by the Azure Linux
/// agent's udev rules from the disk's LUN, so it is known before the VM exists.
pub const AZURE_DATA_DISK: &str = "/dev/disk/azure/scsi1/lun0";
/// The ext4 label the host gives a data disk it formats.
const WORKSPACE_LABEL: &str = "horizon-ws";
/// Azure's platform address (`WireServer`). It serves the VM's goal state, which
/// carries the custom data and so any registry login in it.
pub const AZURE_WIRESERVER: &str = "168.63.129.16";

/// A private registry the host logs in to before pulling the image.
#[derive(Debug)]
pub struct RegistryLogin {
    /// A lowercase DNS host name with an optional numeric port, such as
    /// `example.azurecr.io` or `registry.example:5000`. IPv6 literals are not supported.
    pub server: String,
    pub username: String,
    pub password: Credential,
}

impl RegistryLogin {
    /// Whether a server and username are usable for a host login, so settings
    /// can be checked when they are loaded rather than when a host is rendered.
    #[must_use]
    pub fn valid_identity(server: &str, username: &str) -> bool {
        valid_authority(server)
            && !username.is_empty()
            && username.len() <= 256
            && !username.contains(':')
            && !username.chars().any(char::is_control)
    }
}

/// What the host image provides before the bootstrap runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    /// Docker is installed and the volume arrives formatted as ext4, as on
    /// Hetzner's `docker-ce` image. The user data is unchanged by this type.
    #[default]
    Prepared,
    /// A plain Ubuntu VM on Azure: cloud-init installs Docker from Ubuntu, formats
    /// the data disk at [`AZURE_DATA_DISK`] as ext4 only when it holds no
    /// filesystem, and the guard also closes Azure's platform address and every
    /// IPv6 path out of the container.
    Azure,
}

#[derive(Debug)]
pub struct Plan {
    /// An immutable image reference, `name@sha256:<digest>`.
    pub image: String,
    /// The container's environment, such as `PUBLIC_KEY` and `HORIZON_CLOUD_OPERATION`.
    pub environment: BTreeMap<String, String>,
    pub registry: Option<RegistryLogin>,
    /// The attached volume's stable device path: under `/dev/disk/by-id/` for a
    /// [`Platform::Prepared`] host, whose provider formats it, or exactly
    /// [`AZURE_DATA_DISK`] for an Azure host, which formats it once itself.
    pub workspace_device: String,
    /// Size of `/dev/shm` in GB, which Chromium needs beyond Docker's 64 MB default.
    pub shm_gb: u8,
    /// Explicitly requested only when this new host is provisioned.
    pub exposure_ingress: bool,
    pub platform: Platform,
}

/// Serialized in field order; `write_files` is last so the registry login can be
/// appended to that list without passing through the YAML serializer.
#[derive(Serialize)]
struct CloudConfig<'a> {
    // Absent for a prepared host, so its user data keeps its bytes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    package_update: bool,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    packages: &'a [&'a str],
    ssh_pwauth: bool,
    disable_root: bool,
    mounts: Vec<[&'a str; 6]>,
    runcmd: Vec<Vec<&'a str>>,
    write_files: Vec<WriteFile<'a>>,
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
        let (image, setup, guard, start, unit) = (
            format!("{}\n", self.image),
            setup_script(self.platform, &self.workspace_device),
            guard_script(self.platform),
            if self.exposure_ingress {
                format!(
                    "#!/bin/sh\nset -eu\nexec /usr/bin/python3 -I {INGRESS} {} {}\n",
                    self.shm_gb,
                    workspace(self.platform)
                )
            } else {
                start_script(self.shm_gb, self.platform)
            },
            unit(self.platform),
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
            file(SETUP, "0700", &setup),
            file(GUARD, "0700", &guard),
            file(START, "0700", &start),
            file(UNIT, "0644", &unit),
        ];
        if self.exposure_ingress {
            write_files.push(file(INGRESS, "0700", include_str!("host/ingress.py")));
        }
        let azure = self.platform == Platform::Azure;
        let config = CloudConfig {
            package_update: azure,
            packages: if azure { &["docker.io"] } else { &[] },
            ssh_pwauth: false,
            disable_root: true,
            write_files,
            mounts: vec![[
                &self.workspace_device,
                volume_mount(self.platform),
                "ext4",
                "defaults,nofail,discard",
                "0",
                "2",
            ]],
            runcmd: vec![vec![SETUP]],
        };
        let yaml = zeroize::Zeroizing::new(
            serde_yaml::to_string(&config)
                .map_err(|_| CloudError::Invalid("Host configuration could not be encoded"))?,
        );
        let entry = registry.as_ref().map(|json| registry_entry(json)).transpose()?;
        let length = HEADER.len() + yaml.len() + entry.as_ref().map_or(0, |entry| entry.len());
        let mut document = zeroize::Zeroizing::new(String::with_capacity(length));
        document.push_str(HEADER);
        document.push_str(&yaml);
        if let Some(entry) = &entry {
            document.push_str(entry);
        }
        if document.len() > USER_DATA_LIMIT {
            return Err(CloudError::Invalid(
                "Host configuration exceeds the 32 KiB user-data limit",
            ));
        }
        Ok(document)
    }

    fn validate(&self) -> Result<(), CloudError> {
        if self.exposure_ingress
            && !self
                .environment
                .get("HORIZON_CLOUD_OPERATION")
                .is_some_and(|id| crate::valid_id(id))
        {
            return Err(CloudError::Invalid(
                "Protected ingress requires the allocation operation ID",
            ));
        }
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
        match self.platform {
            Platform::Prepared => {
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
            }
            // The host formats this device, so only the one disk Azure attaches is accepted.
            Platform::Azure if self.workspace_device != AZURE_DATA_DISK => {
                return Err(CloudError::Invalid(
                    "An Azure workspace device must be the data disk on LUN 0",
                ));
            }
            Platform::Azure => {}
        }
        if !(1..=64).contains(&self.shm_gb) {
            return Err(CloudError::Invalid("Shared memory must be between 1 and 64 GB"));
        }
        if let Some(registry) = &self.registry
            && !RegistryLogin::valid_identity(&registry.server, &registry.username)
        {
            return Err(CloudError::Invalid("Invalid registry server or username"));
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

const HEADER: &str = "#cloud-config\n";

/// Serializes `value` as JSON into a wiped buffer of exactly `capacity` bytes.
/// The writer is a fixed slice, so an oversized value fails with nothing
/// reallocated, and no unwiped copy of a secret it carries is left behind.
pub(crate) fn json_in_wiped(
    value: &impl Serialize,
    capacity: usize,
) -> Result<zeroize::Zeroizing<Vec<u8>>, CloudError> {
    let mut buffer = zeroize::Zeroizing::new(vec![0; capacity]);
    let mut rest = &mut buffer[..];
    serde_json::to_writer(&mut rest, value).map_err(|_| CloudError::Invalid("Request does not fit its buffer"))?;
    let written = capacity - rest.len();
    buffer.truncate(written);
    Ok(buffer)
}

/// The `write_files` item for Docker's login, written by hand so the credential
/// never passes through the YAML serializer, which copies scalars into memory it
/// does not wipe. The content is base64, which needs no YAML quoting, and every
/// buffer is wiped and sized up front. Must follow the serialized `write_files`.
fn registry_entry(json: &str) -> Result<zeroize::Zeroizing<String>, CloudError> {
    let prefix =
        format!("- path: {REGISTRY_FILE}\n  owner: root:root\n  permissions: '0600'\n  encoding: b64\n  content: ");
    let encoded = json.len().div_ceil(3) * 4;
    let mut entry = zeroize::Zeroizing::new(String::with_capacity(prefix.len() + encoded + 1));
    let allocated = entry.capacity();
    entry.push_str(&prefix);
    base64::engine::general_purpose::STANDARD.encode_string(json.as_bytes(), &mut entry);
    entry.push('\n');
    if entry.capacity() != allocated {
        return Err(CloudError::Invalid("Registry login outgrew its buffer"));
    }
    Ok(entry)
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
    // The key is a validated host name; JSON escaping at most multiplies it by six.
    let capacity = key
        .len()
        .checked_mul(6)
        .and_then(|key| key.checked_add(auth.len()))
        .and_then(|size| size.checked_add(64))
        .ok_or(CloudError::Invalid("Registry login could not be encoded"))?;
    let mut json = json_in_wiped(&config, capacity)?;
    let text = String::from_utf8(std::mem::take(&mut *json))
        .map_err(|_| CloudError::Invalid("Registry login could not be encoded"))?;
    Ok(zeroize::Zeroizing::new(text))
}

/// The mount point of the workspace volume on this platform's host image.
fn volume_mount(platform: Platform) -> &'static str {
    if platform == Platform::Azure {
        AZURE_VOLUME_MOUNT
    } else {
        VOLUME_MOUNT
    }
}

/// The directory on the volume the container sees as `/workspace`. A subdirectory
/// keeps the filesystem's own `lost+found` out of the worker's view.
fn workspace(platform: Platform) -> String {
    format!("{}/workspace", volume_mount(platform))
}

/// Runs once on first boot. The worker service is enabled last, so it never
/// starts while the host could still accept an SSH or password login.
fn setup_script(platform: Platform, device: &str) -> String {
    let mut script = "#!/bin/sh
set -eu
systemctl mask --now ssh.socket ssh.service
passwd --lock root
"
    .to_owned();
    if platform == Platform::Azure {
        let _ = write!(script, "{}", format_script(device, volume_mount(platform)));
    }
    script.push_str(
        "systemctl daemon-reload
systemctl enable --now horizon-worker.service
",
    );
    script
}

/// Formats and mounts an Azure workspace. cloud-init 26.2 runs `mkfs` on a whole device
/// that already holds a filesystem whatever `fs_setup` says — both `overwrite` and
/// `replace_fs` failed a live rebuild — and a rebuild must keep the filesystem it finds,
/// so the host asks `blkid` itself and formats only an empty device.
fn format_script(device: &str, mount: &str) -> String {
    format!(
        "if [ -z \"$(blkid -o value -s TYPE {device} 2>/dev/null)\" ]; then
  mkfs.ext4 -L {WORKSPACE_LABEL} {device}
fi
mountpoint -q {mount} || mount {mount}
"
    )
}

/// Runs before every container start. The metadata service serves the user data,
/// including any registry credential, so the container must never reach it or
/// anything else link-local. On Azure the platform address serves the same custom
/// data on ports 80 and 32526, so those two ports are closed; the address itself
/// stays reachable because it is also the region's DNS resolver, and a container
/// without it resolves no name at all. Every forwarded IPv6 packet from the
/// container's bridge is closed too: the worker has no IPv6 address to use, and a
/// later IPv6 bridge cannot open a path the IPv4 rules do not see. Each rule is
/// checked before it is added, so a restart adds none twice, and `set -eu` keeps the
/// container stopped when a rule cannot be applied.
fn guard_script(platform: Platform) -> String {
    let mount = volume_mount(platform);
    // An Azure host names the path it checked, because that mount point is what a
    // temporary-disk mount at `/mnt` once hid.
    let missing = if platform == Platform::Azure {
        format!("the workspace volume is not mounted at {mount}")
    } else {
        "the workspace volume is not mounted".to_owned()
    };
    let mut script = format!(
        "#!/bin/sh
set -eu
command -v docker >/dev/null || {{ echo 'horizon-worker: Docker is not installed on this host' >&2; exit 1; }}
mountpoint -q {mount} || {{ echo 'horizon-worker: {missing}' >&2; exit 1; }}
mkdir -p {}
iptables -C DOCKER-USER -d 169.254.0.0/16 -j DROP 2>/dev/null || iptables -I DOCKER-USER -d 169.254.0.0/16 -j DROP
",
        workspace(platform)
    );
    if platform == Platform::Azure {
        let _ = write!(
            script,
            "iptables -C DOCKER-USER -d {AZURE_WIRESERVER}/32 -p tcp -m multiport --dports 80,32526 -j DROP 2>/dev/null \
|| iptables -I DOCKER-USER -d {AZURE_WIRESERVER}/32 -p tcp -m multiport --dports 80,32526 -j DROP
ip6tables -C FORWARD -i docker0 -j DROP 2>/dev/null || ip6tables -I FORWARD -i docker0 -j DROP
"
        );
    }
    script
}

/// Pulls the image once, retrying, and keeps it cached so restarts need no registry.
fn start_script(shm_gb: u8, platform: Platform) -> String {
    let workspace = workspace(platform);
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
  -v {workspace}:/workspace --env-file {ENVIRONMENT_FILE} \"$image\"
"
    )
}

fn unit(platform: Platform) -> String {
    let mount = volume_mount(platform);
    format!(
        "[Unit]
Description=Horizon worker container
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target
RequiresMountsFor={mount}

[Service]
Environment=DOCKER_CONFIG=/root/.docker
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
