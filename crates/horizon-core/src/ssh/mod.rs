mod config_parser;
mod connection_status;

use serde::{Deserialize, Serialize};

pub use config_parser::{DiscoveredSshHost, discover_ssh_hosts};
pub use connection_status::SshConnectionStatus;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SshConnection {
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub remote_command: Option<String>,
    pub extra_args: Vec<String>,
}

impl SshConnection {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.host.trim().is_empty()
    }

    #[must_use]
    pub fn display_label(&self) -> String {
        self.user
            .as_deref()
            .filter(|user| !user.trim().is_empty())
            .map_or_else(|| self.host.clone(), |user| format!("{user}@{}", self.host))
    }

    #[must_use]
    pub fn to_command_args(&self) -> Vec<String> {
        let mut args = self.base_transport_args("-p", false);
        args.push(self.transport_target());

        if let Some(remote_command) = non_empty(self.remote_command.as_deref()) {
            args.push(remote_command.to_string());
        }

        args
    }

    #[must_use]
    pub fn ssh_transport_args(&self) -> Vec<String> {
        let mut args = self.base_transport_args("-p", true);
        args.push(self.transport_target());
        args
    }

    #[must_use]
    pub fn scp_transport_args(&self) -> Vec<String> {
        self.base_transport_args("-P", true)
    }

    #[must_use]
    pub fn transport_target(&self) -> String {
        self.display_label()
    }

    fn base_transport_args(&self, port_flag: &str, batch_mode: bool) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(port) = self.port {
            args.extend([port_flag.to_string(), port.to_string()]);
        }
        if let Some(identity_file) = self.identity_file.as_deref() {
            args.extend(["-i".to_string(), expand_tilde(identity_file)]);
        }
        if let Some(proxy_jump) = non_empty(self.proxy_jump.as_deref()) {
            args.extend(["-J".to_string(), proxy_jump.to_string()]);
        }
        if batch_mode {
            args.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
        }

        args.extend(self.extra_args.iter().cloned());
        args
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest).display().to_string();
    }

    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::SshConnection;

    #[test]
    fn display_label_prefers_user_when_available() {
        let connection = SshConnection {
            host: "prod".to_string(),
            user: Some("deploy".to_string()),
            ..SshConnection::default()
        };

        assert_eq!(connection.display_label(), "deploy@prod");
    }

    #[test]
    fn to_command_args_includes_structured_ssh_options() {
        let connection = SshConnection {
            host: "prod".to_string(),
            port: Some(2222),
            user: Some("deploy".to_string()),
            identity_file: Some("/tmp/id_ed25519".to_string()),
            proxy_jump: Some("bastion".to_string()),
            remote_command: Some("tmux attach".to_string()),
            extra_args: vec!["-o".to_string(), "StrictHostKeyChecking=no".to_string()],
        };

        assert_eq!(
            connection.to_command_args(),
            vec![
                "-p".to_string(),
                "2222".to_string(),
                "-i".to_string(),
                "/tmp/id_ed25519".to_string(),
                "-J".to_string(),
                "bastion".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=no".to_string(),
                "deploy@prod".to_string(),
                "tmux attach".to_string(),
            ]
        );
    }

    #[test]
    fn ssh_transport_args_force_batch_mode_without_remote_command() {
        let connection = SshConnection {
            host: "prod".to_string(),
            user: Some("deploy".to_string()),
            remote_command: Some("tmux attach".to_string()),
            ..SshConnection::default()
        };

        assert_eq!(
            connection.ssh_transport_args(),
            vec!["-o".to_string(), "BatchMode=yes".to_string(), "deploy@prod".to_string(),]
        );
    }

    #[test]
    fn scp_transport_args_use_uppercase_port_flag() {
        let connection = SshConnection {
            host: "prod".to_string(),
            port: Some(2222),
            ..SshConnection::default()
        };

        assert_eq!(
            connection.scp_transport_args(),
            vec![
                "-P".to_string(),
                "2222".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ]
        );
    }
}
