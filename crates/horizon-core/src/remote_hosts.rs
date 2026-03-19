use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::ssh::{DiscoveredSshHost, SshConnection, discover_ssh_hosts};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteHostStatus {
    Online,
    Offline,
    #[default]
    Unknown,
}

impl RemoteHostStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Offline => "Offline",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHostSources {
    pub ssh_config: bool,
    pub tailscale: bool,
}

impl RemoteHostSources {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match (self.ssh_config, self.tailscale) {
            (true, true) => "SSH+TS",
            (true, false) => "SSH",
            (false, true) => "Tailscale",
            (false, false) => "Manual",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub label: String,
    pub ssh_connection: SshConnection,
    pub sources: RemoteHostSources,
    pub status: RemoteHostStatus,
    pub last_seen_secs: Option<i64>,
    pub os: Option<String>,
    pub hostname: Option<String>,
    pub tags: Vec<String>,
    pub ips: Vec<String>,
}

impl RemoteHost {
    #[must_use]
    pub fn target(&self) -> &str {
        &self.ssh_connection.host
    }

    #[must_use]
    pub fn display_target(&self) -> String {
        self.ssh_connection.display_label()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHostCatalog {
    pub hosts: Vec<RemoteHost>,
    pub refreshed_at: Option<SystemTime>,
}

/// Discover remote hosts from supported local sources.
///
/// Merges concrete aliases from `~/.ssh/config` with peer metadata from
/// `tailscale status --json`.
///
/// # Errors
///
/// Returns an error if SSH config discovery fails or if the `tailscale`
/// command returns an unexpected error or invalid JSON.
pub fn discover_remote_hosts(home_dir: Option<&Path>) -> Result<RemoteHostCatalog> {
    let ssh_hosts = discover_ssh_hosts(home_dir)?;
    let tailscale_nodes = discover_tailscale_nodes()?;
    Ok(build_remote_host_catalog(ssh_hosts, tailscale_nodes))
}

fn build_remote_host_catalog(
    ssh_hosts: Vec<DiscoveredSshHost>,
    tailscale_nodes: Vec<TailscaleNode>,
) -> RemoteHostCatalog {
    let mut hosts = Vec::new();
    let mut indices_by_target: HashMap<String, Vec<usize>> = HashMap::new();

    for discovered in ssh_hosts {
        let index = hosts.len();
        let target_key = normalized_host_key(&discovered.connection.host);
        indices_by_target.entry(target_key).or_default().push(index);
        hosts.push(RemoteHost {
            label: discovered.alias,
            ssh_connection: discovered.connection,
            sources: RemoteHostSources {
                ssh_config: true,
                tailscale: false,
            },
            status: RemoteHostStatus::Unknown,
            last_seen_secs: None,
            os: None,
            hostname: None,
            tags: Vec::new(),
            ips: Vec::new(),
        });
    }

    for node in tailscale_nodes {
        let target_key = normalized_host_key(&node.target_host);
        if let Some(indices) = indices_by_target.get(&target_key) {
            for index in indices {
                let host = &mut hosts[*index];
                host.sources.tailscale = true;
                host.status = node.status;
                host.last_seen_secs = node.last_seen_secs;
                if host.os.is_none() {
                    host.os.clone_from(&node.os);
                }
                if host.hostname.is_none() {
                    host.hostname.clone_from(&node.hostname);
                }
                host.tags = merge_unique_strings(&host.tags, &node.tags);
                host.ips = merge_unique_strings(&host.ips, &node.ips);
            }
            continue;
        }

        let index = hosts.len();
        indices_by_target.entry(target_key).or_default().push(index);
        hosts.push(RemoteHost {
            label: node.label,
            ssh_connection: SshConnection {
                host: node.target_host,
                ..SshConnection::default()
            },
            sources: RemoteHostSources {
                ssh_config: false,
                tailscale: true,
            },
            status: node.status,
            last_seen_secs: node.last_seen_secs,
            os: node.os,
            hostname: node.hostname,
            tags: node.tags,
            ips: node.ips,
        });
    }

    hosts.sort_by(|left, right| {
        remote_host_sort_rank(left)
            .cmp(&remote_host_sort_rank(right))
            .then_with(|| left.label.to_ascii_lowercase().cmp(&right.label.to_ascii_lowercase()))
            .then_with(|| {
                left.ssh_connection
                    .display_label()
                    .to_ascii_lowercase()
                    .cmp(&right.ssh_connection.display_label().to_ascii_lowercase())
            })
    });

    RemoteHostCatalog {
        hosts,
        refreshed_at: Some(SystemTime::now()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct TailscaleStatus {
    #[serde(default, rename = "Peer")]
    peers: HashMap<String, TailscalePeer>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TailscalePeer {
    #[serde(rename = "DNSName")]
    dns_name: String,
    #[serde(rename = "HostName")]
    host_name: String,
    #[serde(rename = "Online")]
    online: bool,
    #[serde(rename = "LastSeen")]
    last_seen: String,
    #[serde(rename = "OS")]
    os: Option<String>,
    #[serde(rename = "Tags")]
    tags: Vec<String>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct TailscaleNode {
    label: String,
    target_host: String,
    status: RemoteHostStatus,
    last_seen_secs: Option<i64>,
    os: Option<String>,
    hostname: Option<String>,
    tags: Vec<String>,
    ips: Vec<String>,
}

fn discover_tailscale_nodes() -> Result<Vec<TailscaleNode>> {
    let output = match Command::new("tailscale").args(["status", "--json"]).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            "tailscale status --json failed".to_string()
        } else {
            format!("tailscale status --json failed: {stderr}")
        };
        return Err(Error::State(message));
    }

    parse_tailscale_status(&String::from_utf8_lossy(&output.stdout))
}

fn parse_tailscale_status(json: &str) -> Result<Vec<TailscaleNode>> {
    let status: TailscaleStatus =
        serde_json::from_str(json).map_err(|error| Error::State(format!("invalid tailscale status json: {error}")))?;
    let mut nodes: Vec<_> = status
        .peers
        .into_values()
        .filter_map(TailscalePeer::into_node)
        .collect();
    nodes.sort_by(|left, right| {
        tailscale_node_sort_rank(left)
            .cmp(&tailscale_node_sort_rank(right))
            .then_with(|| left.label.to_ascii_lowercase().cmp(&right.label.to_ascii_lowercase()))
            .then_with(|| {
                left.target_host
                    .to_ascii_lowercase()
                    .cmp(&right.target_host.to_ascii_lowercase())
            })
    });
    Ok(nodes)
}

impl TailscalePeer {
    fn into_node(self) -> Option<TailscaleNode> {
        let dns_name = trim_dns_name(&self.dns_name);
        let target_host = dns_name
            .clone()
            .or_else(|| self.tailscale_ips.first().cloned())
            .or_else(|| sanitized_host_name(&self.host_name))?;
        let label = dns_name
            .as_deref()
            .map(short_dns_label)
            .or_else(|| non_empty_string(&self.host_name))
            .unwrap_or_else(|| target_host.clone());

        Some(TailscaleNode {
            label,
            target_host,
            status: if self.online {
                RemoteHostStatus::Online
            } else {
                RemoteHostStatus::Offline
            },
            last_seen_secs: (!self.online)
                .then(|| parse_iso8601_epoch_secs(&self.last_seen))
                .flatten(),
            os: self.os.and_then(|value| non_empty_string(&value)),
            hostname: sanitized_host_name(&self.host_name),
            tags: self.tags.into_iter().filter_map(|tag| non_empty_string(&tag)).collect(),
            ips: self
                .tailscale_ips
                .into_iter()
                .filter_map(|ip| non_empty_string(&ip))
                .collect(),
        })
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn trim_dns_name(value: &str) -> Option<String> {
    non_empty_string(value).map(|dns_name| dns_name.trim_end_matches('.').to_string())
}

fn sanitized_host_name(value: &str) -> Option<String> {
    let host_name = non_empty_string(value)?;
    (!host_name.contains(char::is_whitespace)).then_some(host_name)
}

fn short_dns_label(value: &str) -> String {
    value.split('.').next().unwrap_or(value).to_string()
}

/// Parse an ISO 8601 timestamp like "2025-09-26T11:54:48Z" to Unix epoch seconds.
fn parse_iso8601_epoch_secs(value: &str) -> Option<i64> {
    let s = value.trim().trim_end_matches('Z');
    if s.len() < 16 || s.starts_with("0001-01-01") {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19).and_then(|v| v.parse().ok()).unwrap_or(0);

    let years = year - 1970;
    let leap_years = (year - 1969) / 4 - (year - 1901) / 100 + (year - 1601) / 400;
    let month_days: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let month_offset = *month_days.get(month.checked_sub(1)? as usize)?;
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let leap_adj = i64::from(is_leap && month > 2);
    let total_days = years * 365 + leap_years + month_offset + leap_adj + day - 1;

    Some(total_days * 86400 + hour * 3600 + min * 60 + sec)
}

fn normalized_host_key(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn merge_unique_strings(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    for value in incoming {
        if merged.iter().any(|existing| existing.eq_ignore_ascii_case(value)) {
            continue;
        }
        merged.push(value.clone());
    }
    merged
}

fn remote_host_sort_rank(host: &RemoteHost) -> (u8, u8) {
    (
        match host.status {
            RemoteHostStatus::Online => 0,
            RemoteHostStatus::Unknown => 1,
            RemoteHostStatus::Offline => 2,
        },
        u8::from(!host.sources.ssh_config),
    )
}

fn tailscale_node_sort_rank(node: &TailscaleNode) -> u8 {
    match node.status {
        RemoteHostStatus::Online => 0,
        RemoteHostStatus::Offline => 1,
        RemoteHostStatus::Unknown => 2,
    }
}

#[cfg(test)]
mod tests {
    use crate::ssh::{DiscoveredSshHost, SshConnection};

    use super::{RemoteHostStatus, TailscaleNode, build_remote_host_catalog, parse_tailscale_status};

    #[test]
    fn parse_tailscale_status_discovers_online_and_offline_nodes() {
        let nodes = parse_tailscale_status(
            r#"
{
  "Peer": {
    "node-1": {
      "DNSName": "militaerveien-master.tailnet-f382.ts.net.",
      "HostName": "YP-D79ACC7ED0",
      "Online": true,
      "OS": "linux",
      "Tags": ["cuda", "node", "x86-64"],
      "TailscaleIPs": ["100.106.71.89"]
    },
    "node-2": {
      "DNSName": "gml-islandhovreslia-master.tailnet-f382.ts.net.",
      "HostName": "YP-CCB6696051",
      "Online": false,
      "LastSeen": "2025-09-26T11:54:48Z",
      "OS": "linux",
      "Tags": ["cuda", "node"],
      "TailscaleIPs": ["100.73.193.60"]
    }
  }
}
"#,
        )
        .expect("tailscale nodes");

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].label, "militaerveien-master");
        assert_eq!(nodes[0].target_host, "militaerveien-master.tailnet-f382.ts.net");
        assert_eq!(nodes[0].status, RemoteHostStatus::Online);
        assert_eq!(nodes[0].hostname.as_deref(), Some("YP-D79ACC7ED0"));
        assert_eq!(nodes[1].status, RemoteHostStatus::Offline);
        // 2025-09-26T11:54:48Z => Unix epoch seconds
        assert_eq!(nodes[1].last_seen_secs, Some(1_758_887_688));
        assert!(nodes[0].tags.iter().any(|tag| tag == "cuda"));
    }

    #[test]
    fn build_remote_host_catalog_merges_ssh_and_tailscale_targets() {
        let catalog = build_remote_host_catalog(
            vec![DiscoveredSshHost {
                alias: "mil".to_string(),
                connection: SshConnection {
                    host: "militaerveien-master.tailnet-f382.ts.net".to_string(),
                    user: Some("peter".to_string()),
                    ..SshConnection::default()
                },
            }],
            vec![TailscaleNode {
                label: "militaerveien-master".to_string(),
                target_host: "militaerveien-master.tailnet-f382.ts.net".to_string(),
                status: RemoteHostStatus::Online,
                last_seen_secs: None,
                os: Some("linux".to_string()),
                hostname: None,
                tags: vec!["cuda".to_string()],
                ips: vec!["100.106.71.89".to_string()],
            }],
        );

        let mil = catalog
            .hosts
            .iter()
            .find(|host| host.label == "mil")
            .expect("merged ssh host");
        assert_eq!(mil.target(), "militaerveien-master.tailnet-f382.ts.net");
        assert!(mil.sources.ssh_config);
        assert!(mil.sources.tailscale);
        assert_eq!(mil.status, RemoteHostStatus::Online);
        assert_eq!(mil.os.as_deref(), Some("linux"));
        assert!(mil.tags.iter().any(|tag| tag == "cuda"));
    }
}
