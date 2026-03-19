use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::Result;

use super::SshConnection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSshHost {
    pub alias: String,
    pub connection: SshConnection,
}

/// Discover concrete SSH host aliases from `~/.ssh/config`.
///
/// # Errors
///
/// Returns an error if the SSH config file exists but cannot be read.
pub fn discover_ssh_hosts(home_dir: Option<&Path>) -> Result<Vec<DiscoveredSshHost>> {
    let Some(config_path) = ssh_config_path(home_dir) else {
        return Ok(Vec::new());
    };
    if !config_path.is_file() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(&config_path)?;
    Ok(parse_ssh_config(&contents))
}

fn ssh_config_path(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .map(|home| home.join(".ssh").join("config"))
}

fn parse_ssh_config(contents: &str) -> Vec<DiscoveredSshHost> {
    let mut hosts = Vec::new();
    let mut active_indices = Vec::new();
    let mut seen_aliases = HashSet::new();

    for raw_line in contents.lines() {
        let line = trim_comments(raw_line);
        if line.is_empty() {
            continue;
        }

        let Some((directive, value)) = split_directive(line) else {
            continue;
        };

        match directive.as_str() {
            "host" => {
                active_indices.clear();
                for pattern in value.split_whitespace() {
                    let alias = unquote(pattern);
                    if !is_supported_host_alias(&alias) || !seen_aliases.insert(alias.to_ascii_lowercase()) {
                        continue;
                    }

                    hosts.push(DiscoveredSshHost {
                        alias: alias.clone(),
                        connection: SshConnection {
                            host: alias,
                            ..SshConnection::default()
                        },
                    });
                    active_indices.push(hosts.len() - 1);
                }
            }
            "match" => active_indices.clear(),
            _ => {
                for index in &active_indices {
                    apply_directive(&mut hosts[*index].connection, &directive, value);
                }
            }
        }
    }

    hosts.retain(|host| host.connection.is_valid());
    hosts
}

fn trim_comments(line: &str) -> &str {
    line.split('#').next().unwrap_or_default().trim()
}

fn split_directive(line: &str) -> Option<(String, &str)> {
    if let Some((directive, value)) = line.split_once(char::is_whitespace) {
        return Some((directive.trim().to_ascii_lowercase(), value.trim()));
    }

    let (directive, value) = line.split_once('=')?;
    Some((directive.trim().to_ascii_lowercase(), value.trim()))
}

fn is_supported_host_alias(alias: &str) -> bool {
    !alias.is_empty() && !alias.starts_with('!') && !alias.contains('*') && !alias.contains('?')
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|trimmed| trimmed.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|trimmed| trimmed.strip_suffix('\'')))
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn apply_directive(connection: &mut SshConnection, directive: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }

    match directive {
        "hostname" => connection.host = unquote(value),
        "user" => connection.user = Some(unquote(value)),
        "port" => connection.port = value.parse::<u16>().ok(),
        "identityfile" => connection.identity_file = Some(unquote(value)),
        "proxyjump" => connection.proxy_jump = Some(unquote(value)),
        "remotecommand" => connection.remote_command = Some(unquote(value)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{discover_ssh_hosts, parse_ssh_config};

    #[test]
    fn parse_ssh_config_discovers_supported_hosts() {
        let hosts = parse_ssh_config(
            r"
Host prod-api
  HostName api.prod.example.com
  User deploy
  Port 2222
  IdentityFile ~/.ssh/prod

Host *
  User ignored

Host staging bastion
  ProxyJump jumpbox

Match host something
  User skipped
",
        );

        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "prod-api");
        assert_eq!(hosts[0].connection.host, "api.prod.example.com");
        assert_eq!(hosts[0].connection.user.as_deref(), Some("deploy"));
        assert_eq!(hosts[0].connection.port, Some(2222));
        assert_eq!(hosts[0].connection.identity_file.as_deref(), Some("~/.ssh/prod"));
        assert_eq!(hosts[1].alias, "staging");
        assert_eq!(hosts[1].connection.proxy_jump.as_deref(), Some("jumpbox"));
        assert_eq!(hosts[2].alias, "bastion");
    }

    #[test]
    fn parse_ssh_config_skips_wildcards_and_negated_hosts() {
        let hosts = parse_ssh_config(
            r"
Host *.example.com !prod
  User deploy

Host valid
  User ops
",
        );

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "valid");
        assert_eq!(hosts[0].connection.user.as_deref(), Some("ops"));
    }

    #[test]
    fn discover_ssh_hosts_reads_from_home_directory() {
        let temp_dir = TempDir::new().expect("temporary home");
        let ssh_dir = temp_dir.path().join(".ssh");
        fs::create_dir_all(&ssh_dir).expect("ssh dir");
        fs::write(
            ssh_dir.join("config"),
            r"
Host demo
  User me
",
        )
        .expect("ssh config");

        let hosts = discover_ssh_hosts(Some(temp_dir.path())).expect("ssh hosts");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "demo");
    }
}
