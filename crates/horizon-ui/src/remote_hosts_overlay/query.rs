use horizon_core::{RemoteHost, SshConnection};

use super::{RemoteConnectMode, RemoteHostsOverlayAction, WorkspaceChoice};

/// What the filter text adds to a connection: `user@` in front picks the
/// SSH user, `:port` at the end picks the VNC port for this session only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct QueryOverrides<'a> {
    pub(super) user: Option<&'a str>,
    pub(super) vnc_port: Option<u16>,
}

impl QueryOverrides<'_> {
    /// The host's connection with the typed user applied.
    pub(super) fn connection(self, host: &RemoteHost) -> SshConnection {
        let mut connection = host.ssh_connection.clone();
        if let Some(user) = self.user {
            connection.user = Some(user.to_string());
        }
        connection
    }
}

/// Split `user@filter:port` into its overrides and the text that filters hosts.
pub(super) fn parse_query(query: &str) -> (QueryOverrides<'_>, &str) {
    let (user, filter) = parse_user_prefix(query);
    let (filter, vnc_port) = parse_port_suffix(filter);
    (QueryOverrides { user, vnc_port }, filter)
}

fn parse_user_prefix(query: &str) -> (Option<&str>, &str) {
    if let Some(at_pos) = query.find('@') {
        let user = query[..at_pos].trim();
        let filter = query[at_pos + 1..].trim();
        if user.is_empty() {
            (None, filter)
        } else {
            (Some(user), filter)
        }
    } else {
        (None, query)
    }
}

/// A trailing `:1..65535` is a VNC port, but only when it is the filter's
/// sole colon: an IPv6 address such as `fd7a::1` stays a plain filter.
fn parse_port_suffix(filter: &str) -> (&str, Option<u16>) {
    let Some((rest, port)) = filter.rsplit_once(':') else {
        return (filter, None);
    };
    if rest.contains(':') || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return (filter, None);
    }
    match port.parse::<u16>() {
        Ok(port) if port != 0 => (rest.trim_end(), Some(port)),
        _ => (filter, None),
    }
}

pub(super) fn connect_action(
    host: &RemoteHost,
    overrides: QueryOverrides<'_>,
    mode: RemoteConnectMode,
    destination: WorkspaceChoice,
) -> RemoteHostsOverlayAction {
    RemoteHostsOverlayAction::Open {
        label: host.label.clone(),
        connection: overrides.connection(host),
        mode,
        destination,
        vnc_port: overrides.vnc_port,
    }
}

pub(super) fn filtered_indices(hosts: &[RemoteHost], query: &str) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| query.is_empty() || host_matches(&query, host))
        .map(|(index, _)| index)
        .collect()
}

fn host_matches(query: &str, host: &RemoteHost) -> bool {
    contains_lowercase(host.label.as_bytes(), query.as_bytes())
        || contains_lowercase(host.ssh_connection.host.as_bytes(), query.as_bytes())
        || host
            .hostname
            .as_deref()
            .is_some_and(|hostname| contains_lowercase(hostname.as_bytes(), query.as_bytes()))
        || host
            .os
            .as_deref()
            .is_some_and(|os| contains_lowercase(os.as_bytes(), query.as_bytes()))
        || contains_lowercase(host.sources.label().as_bytes(), query.as_bytes())
        || contains_lowercase(host.status.label().as_bytes(), query.as_bytes())
        || host
            .tags
            .iter()
            .chain(host.ips.iter())
            .any(|value| contains_lowercase(value.as_bytes(), query.as_bytes()))
}

/// Case-insensitive substring search without allocation.
/// Assumes `needle` is already ASCII-lowercased.
fn contains_lowercase(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }

    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(haystack_byte, needle_byte)| haystack_byte.to_ascii_lowercase() == *needle_byte)
    })
}

#[cfg(test)]
mod tests {
    use horizon_core::{RemoteHost, RemoteHostSources, RemoteHostStatus, SshConnection};

    use super::{QueryOverrides, connect_action, filtered_indices, parse_query, parse_user_prefix};
    use crate::remote_hosts_overlay::{RemoteConnectMode, RemoteHostsOverlayAction, WorkspaceChoice};

    #[test]
    fn parse_user_prefix_extracts_user_and_filter() {
        assert_eq!(parse_user_prefix("deploy@prod"), (Some("deploy"), "prod"));
        assert_eq!(parse_user_prefix("@prod"), (None, "prod"));
        assert_eq!(parse_user_prefix("prod"), (None, "prod"));
    }

    #[test]
    fn parse_query_takes_a_port_suffix_only_when_it_is_the_sole_colon() {
        let overrides = |user, vnc_port| QueryOverrides { user, vnc_port };
        assert_eq!(
            parse_query("deploy@prod:5901"),
            (overrides(Some("deploy"), Some(5901)), "prod")
        );
        assert_eq!(parse_query("prod :5901"), (overrides(None, Some(5901)), "prod"));
        assert_eq!(
            parse_query(":5901"),
            (overrides(None, Some(5901)), ""),
            "a port alone keeps every host"
        );
        assert_eq!(
            parse_query("prod:"),
            (overrides(None, None), "prod:"),
            "no digits, no port"
        );
        assert_eq!(
            parse_query("prod:0"),
            (overrides(None, None), "prod:0"),
            "port zero is not a port"
        );
        assert_eq!(parse_query("prod:70000"), (overrides(None, None), "prod:70000"));
        assert_eq!(parse_query("prod:59a"), (overrides(None, None), "prod:59a"));
        assert_eq!(
            parse_query("fd7a:115c::1"),
            (overrides(None, None), "fd7a:115c::1"),
            "an IPv6 filter keeps its last group"
        );
        assert_eq!(parse_query("prod"), (overrides(None, None), "prod"));
    }

    #[test]
    fn filtered_indices_match_multiple_fields_case_insensitively() {
        let hosts = vec![
            remote_host(
                "Prod API",
                "prod-api",
                RemoteHostStatus::Online,
                &["app", "blue"],
                &["100.64.0.1"],
            ),
            remote_host(
                "Staging DB",
                "db-stage",
                RemoteHostStatus::Offline,
                &["database"],
                &["100.64.0.2"],
            ),
        ];

        assert_eq!(filtered_indices(&hosts, "prod"), vec![0]);
        assert_eq!(filtered_indices(&hosts, "BLUE"), vec![0]);
        assert_eq!(filtered_indices(&hosts, "offline"), vec![1]);
        assert_eq!(filtered_indices(&hosts, "100.64.0.2"), vec![1]);
    }

    #[test]
    fn connect_action_applies_user_override_without_mutating_host() {
        let host = remote_host("Prod API", "prod-api", RemoteHostStatus::Online, &["app"], &[]);

        let overrides = QueryOverrides {
            user: Some("deploy"),
            vnc_port: Some(5901),
        };
        let action = connect_action(&host, overrides, RemoteConnectMode::Ssh, WorkspaceChoice::Default);

        match action {
            RemoteHostsOverlayAction::Open {
                label,
                connection,
                mode,
                destination,
                vnc_port,
            } => {
                assert_eq!(label, "Prod API");
                assert_eq!(connection.user.as_deref(), Some("deploy"));
                assert_eq!(host.ssh_connection.user, None);
                assert_eq!(mode, RemoteConnectMode::Ssh);
                assert_eq!(destination, WorkspaceChoice::Default);
                assert_eq!(vnc_port, Some(5901), "the launch side decides whether the port matters");
            }
            RemoteHostsOverlayAction::None
            | RemoteHostsOverlayAction::Cancelled
            | RemoteHostsOverlayAction::SetDefaultWorkspace(_)
            | RemoteHostsOverlayAction::SaveShortcut { .. } => {
                panic!("expected an open action")
            }
        }
    }

    fn remote_host(label: &str, host: &str, status: RemoteHostStatus, tags: &[&str], ips: &[&str]) -> RemoteHost {
        RemoteHost {
            label: label.to_string(),
            ssh_connection: SshConnection {
                host: host.to_string(),
                ..SshConnection::default()
            },
            sources: RemoteHostSources::default(),
            status,
            last_seen_secs: None,
            os: Some("linux".to_string()),
            hostname: Some(host.to_string()),
            tags: tags.iter().map(ToString::to_string).collect(),
            ips: ips.iter().map(ToString::to_string).collect(),
        }
    }
}
