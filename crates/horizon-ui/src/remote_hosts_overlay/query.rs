use horizon_core::RemoteHost;

use super::RemoteHostsOverlayAction;

pub(super) fn parse_user_prefix(query: &str) -> (Option<&str>, &str) {
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

pub(super) fn connect_action(host: &RemoteHost, user_override: Option<&str>) -> RemoteHostsOverlayAction {
    let mut connection = host.ssh_connection.clone();
    if let Some(user) = user_override {
        connection.user = Some(user.to_string());
    }
    RemoteHostsOverlayAction::OpenSsh {
        label: host.label.clone(),
        connection,
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

    use super::{connect_action, filtered_indices, parse_user_prefix};
    use crate::remote_hosts_overlay::RemoteHostsOverlayAction;

    #[test]
    fn parse_user_prefix_extracts_user_and_filter() {
        assert_eq!(parse_user_prefix("deploy@prod"), (Some("deploy"), "prod"));
        assert_eq!(parse_user_prefix("@prod"), (None, "prod"));
        assert_eq!(parse_user_prefix("prod"), (None, "prod"));
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

        let action = connect_action(&host, Some("deploy"));

        match action {
            RemoteHostsOverlayAction::OpenSsh { label, connection } => {
                assert_eq!(label, "Prod API");
                assert_eq!(connection.user.as_deref(), Some("deploy"));
                assert_eq!(host.ssh_connection.user, None);
            }
            RemoteHostsOverlayAction::None | RemoteHostsOverlayAction::Cancelled => {
                panic!("expected ssh action")
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
