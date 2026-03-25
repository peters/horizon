use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use serde::Deserialize;

use crate::board::Board;
use crate::error::{Error, Result};
use crate::panel::PanelKind;
use crate::ssh::{DiscoveredSshHost, SshConnection, SshConnectionStatus, discover_ssh_hosts};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHostConnectionHistoryEntry {
    pub panel_title: String,
    pub workspace_name: String,
    pub connection_label: String,
    pub status: SshConnectionStatus,
    pub launched_at_millis: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHostConnectionSummary {
    pub connected_count: usize,
    pub connecting_count: usize,
    pub disconnected_count: usize,
    pub history: Vec<RemoteHostConnectionHistoryEntry>,
}

impl RemoteHostConnectionSummary {
    #[must_use]
    pub const fn total_sessions(&self) -> usize {
        self.connected_count + self.connecting_count + self.disconnected_count
    }

    #[must_use]
    pub const fn live_sessions(&self) -> usize {
        self.connected_count + self.connecting_count
    }

    #[must_use]
    pub const fn current_status(&self) -> Option<SshConnectionStatus> {
        if self.connected_count > 0 {
            Some(SshConnectionStatus::Connected)
        } else if self.connecting_count > 0 {
            Some(SshConnectionStatus::Connecting)
        } else if self.disconnected_count > 0 {
            Some(SshConnectionStatus::Disconnected)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteHostPanelSession {
    connection: SshConnection,
    status: SshConnectionStatus,
    panel_title: String,
    workspace_name: String,
    launched_at_millis: i64,
}

#[must_use]
pub fn summarize_remote_host_connections(
    board: &Board,
    catalog: &RemoteHostCatalog,
) -> Vec<RemoteHostConnectionSummary> {
    let workspace_names: HashMap<_, _> = board
        .workspaces
        .iter()
        .map(|workspace| (workspace.id, workspace.name.clone()))
        .collect();

    let sessions = board.panels.iter().filter_map(|panel| {
        if panel.kind != PanelKind::Ssh {
            return None;
        }

        let connection = panel.ssh_connection.clone()?;
        Some(RemoteHostPanelSession {
            connection,
            status: panel.ssh_status().unwrap_or(SshConnectionStatus::Disconnected),
            panel_title: panel.display_title().into_owned(),
            workspace_name: workspace_names
                .get(&panel.workspace_id)
                .cloned()
                .unwrap_or_else(|| "Workspace".to_string()),
            launched_at_millis: panel.launched_at_millis,
        })
    });

    build_remote_host_connection_summaries(&catalog.hosts, sessions)
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

fn remote_host_activity_key(connection: &SshConnection) -> String {
    let extra_args = connection
        .extra_args
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\x1f");

    format!(
        "{}|{}|{}|{}|{}|{}",
        normalized_host_key(&connection.host),
        connection.port.map_or_else(String::new, |port| port.to_string()),
        normalized_connection_field(connection.identity_file.as_deref()),
        normalized_connection_field(connection.proxy_jump.as_deref()),
        normalized_connection_field(connection.remote_command.as_deref()),
        extra_args,
    )
}

fn normalized_connection_field(value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| value.trim().to_ascii_lowercase())
}

fn build_remote_host_connection_summaries<I>(hosts: &[RemoteHost], sessions: I) -> Vec<RemoteHostConnectionSummary>
where
    I: IntoIterator<Item = RemoteHostPanelSession>,
{
    let mut host_indices_by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, host) in hosts.iter().enumerate() {
        host_indices_by_key
            .entry(remote_host_activity_key(&host.ssh_connection))
            .or_default()
            .push(index);
    }

    let mut summaries = vec![RemoteHostConnectionSummary::default(); hosts.len()];

    for session in sessions {
        let Some(indices) = host_indices_by_key.get(&remote_host_activity_key(&session.connection)) else {
            continue;
        };

        for index in indices {
            let summary = &mut summaries[*index];
            match session.status {
                SshConnectionStatus::Connected => summary.connected_count += 1,
                SshConnectionStatus::Connecting => summary.connecting_count += 1,
                SshConnectionStatus::Disconnected => summary.disconnected_count += 1,
            }
            summary.history.push(RemoteHostConnectionHistoryEntry {
                panel_title: session.panel_title.clone(),
                workspace_name: session.workspace_name.clone(),
                connection_label: session.connection.display_label(),
                status: session.status,
                launched_at_millis: session.launched_at_millis,
            });
        }
    }

    for summary in &mut summaries {
        summary.history.sort_by(|left, right| {
            right
                .launched_at_millis
                .cmp(&left.launched_at_millis)
                .then_with(|| left.panel_title.cmp(&right.panel_title))
        });
    }

    summaries
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
    use crate::ssh::{DiscoveredSshHost, SshConnection, SshConnectionStatus};

    use super::{
        RemoteHost, RemoteHostConnectionSummary, RemoteHostPanelSession, RemoteHostSources, RemoteHostStatus,
        TailscaleNode, build_remote_host_catalog, build_remote_host_connection_summaries, parse_tailscale_status,
    };

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

    #[test]
    fn connection_summary_matches_user_override_sessions_to_same_host() {
        let hosts = vec![remote_host(
            "prod",
            ssh_connection("prod.example.com", Some("deploy"), Some(22)),
        )];
        let sessions = vec![panel_session(
            ssh_connection("prod.example.com", Some("root"), Some(22)),
            SshConnectionStatus::Connected,
            2_000,
            "Prod API",
            "Remote Sessions",
        )];

        let summaries = build_remote_host_connection_summaries(&hosts, sessions);

        assert_eq!(
            summaries,
            vec![RemoteHostConnectionSummary {
                connected_count: 1,
                connecting_count: 0,
                disconnected_count: 0,
                history: vec![super::RemoteHostConnectionHistoryEntry {
                    panel_title: "Prod API".to_string(),
                    workspace_name: "Remote Sessions".to_string(),
                    connection_label: "root@prod.example.com".to_string(),
                    status: SshConnectionStatus::Connected,
                    launched_at_millis: 2_000,
                }],
            }]
        );
    }

    #[test]
    fn connection_summary_keeps_distinct_transport_options_separate() {
        let hosts = vec![
            remote_host(
                "prod-direct",
                ssh_connection("prod.example.com", Some("deploy"), Some(22)),
            ),
            remote_host("prod-jump", ssh_connection_with_jump("prod.example.com", "bastion")),
        ];
        let sessions = vec![
            panel_session(
                ssh_connection("prod.example.com", Some("ops"), Some(22)),
                SshConnectionStatus::Connected,
                3_000,
                "Direct session",
                "Remote Sessions",
            ),
            panel_session(
                ssh_connection_with_jump("prod.example.com", "bastion"),
                SshConnectionStatus::Disconnected,
                1_000,
                "Jump session",
                "Archive",
            ),
        ];

        let summaries = build_remote_host_connection_summaries(&hosts, sessions);

        assert_eq!(summaries[0].connected_count, 1);
        assert_eq!(summaries[0].disconnected_count, 0);
        assert_eq!(summaries[1].connected_count, 0);
        assert_eq!(summaries[1].disconnected_count, 1);
    }

    #[test]
    fn connection_summary_sorts_history_newest_first() {
        let hosts = vec![remote_host("prod", ssh_connection("prod.example.com", None, None))];
        let sessions = vec![
            panel_session(
                ssh_connection("prod.example.com", None, None),
                SshConnectionStatus::Disconnected,
                1_000,
                "Older session",
                "Remote Sessions",
            ),
            panel_session(
                ssh_connection("prod.example.com", None, None),
                SshConnectionStatus::Connecting,
                5_000,
                "Newest session",
                "Remote Sessions",
            ),
        ];

        let summaries = build_remote_host_connection_summaries(&hosts, sessions);

        assert_eq!(summaries[0].history.len(), 2);
        assert_eq!(summaries[0].history[0].panel_title, "Newest session");
        assert_eq!(summaries[0].history[1].panel_title, "Older session");
        assert_eq!(summaries[0].current_status(), Some(SshConnectionStatus::Connecting));
    }

    fn remote_host(label: &str, ssh_connection: SshConnection) -> RemoteHost {
        RemoteHost {
            label: label.to_string(),
            ssh_connection,
            sources: RemoteHostSources::default(),
            status: RemoteHostStatus::Unknown,
            last_seen_secs: None,
            os: None,
            hostname: None,
            tags: Vec::new(),
            ips: Vec::new(),
        }
    }

    fn ssh_connection(host: &str, user: Option<&str>, port: Option<u16>) -> SshConnection {
        SshConnection {
            host: host.to_string(),
            user: user.map(ToString::to_string),
            port,
            ..SshConnection::default()
        }
    }

    fn ssh_connection_with_jump(host: &str, proxy_jump: &str) -> SshConnection {
        SshConnection {
            host: host.to_string(),
            proxy_jump: Some(proxy_jump.to_string()),
            ..SshConnection::default()
        }
    }

    fn panel_session(
        connection: SshConnection,
        status: SshConnectionStatus,
        launched_at_millis: i64,
        panel_title: &str,
        workspace_name: &str,
    ) -> RemoteHostPanelSession {
        RemoteHostPanelSession {
            connection,
            status,
            panel_title: panel_title.to_string(),
            workspace_name: workspace_name.to_string(),
            launched_at_millis,
        }
    }
}
