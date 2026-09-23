use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Where the Remote Hosts overlay puts new sessions and how it reaches a
/// host's VNC server through SSH.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct RemoteHostsConfig {
    /// Workspace that receives new SSH and VNC panels unless the overlay
    /// picks another one; created on first use.
    pub default_workspace: String,
    /// Port of the VNC server on the remote host's loopback interface,
    /// reached with `ssh -W` rather than exposed on the network.
    pub vnc_port: u16,
}

impl RemoteHostsConfig {
    pub const DEFAULT_WORKSPACE: &'static str = "Remote Sessions";
    pub const DEFAULT_VNC_PORT: u16 = 5900;

    /// # Errors
    /// Rejects a blank workspace name and port zero.
    pub fn validate(&self) -> Result<()> {
        if self.default_workspace.trim().is_empty() {
            return Err(Error::Config("remote_hosts.default_workspace cannot be empty".into()));
        }
        if self.vnc_port == 0 {
            return Err(Error::Config("remote_hosts.vnc_port must be nonzero".into()));
        }
        Ok(())
    }

    /// The configured workspace name without surrounding whitespace.
    #[must_use]
    pub fn default_workspace_name(&self) -> &str {
        self.default_workspace.trim()
    }

    /// The VNC endpoint as seen from the remote host, in Device panel form.
    #[must_use]
    pub fn vnc_target(&self) -> String {
        format!("127.0.0.1:{}", self.vnc_port)
    }
}

impl Default for RemoteHostsConfig {
    fn default() -> Self {
        Self {
            default_workspace: Self::DEFAULT_WORKSPACE.to_string(),
            vnc_port: Self::DEFAULT_VNC_PORT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteHostsConfig;
    use crate::Config;

    #[test]
    fn defaults_name_the_remote_sessions_workspace_and_the_standard_vnc_port() {
        let config = RemoteHostsConfig::default();
        assert_eq!(config.default_workspace_name(), "Remote Sessions");
        assert_eq!(config.vnc_target(), "127.0.0.1:5900");
        assert!(config.validate().is_ok());
        assert_eq!(Config::default().remote_hosts, config);
    }

    #[test]
    fn missing_section_and_partial_sections_fill_in_defaults() {
        let config = Config::from_yaml("version: 11\n").unwrap();
        assert_eq!(config.remote_hosts, RemoteHostsConfig::default());

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 5901\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Remote Sessions");
        assert_eq!(config.remote_hosts.vnc_target(), "127.0.0.1:5901");

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  Ops  '\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Ops");
    }

    #[test]
    fn blank_workspace_and_port_zero_are_rejected() {
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  '\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.default_workspace"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_port"));
    }

    #[test]
    fn section_round_trips_through_yaml() {
        let mut config = Config::default();
        config.remote_hosts.default_workspace = "Ops".into();
        config.remote_hosts.vnc_port = 5901;
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("remote_hosts:\n  default_workspace: Ops\n  vnc_port: 5901\n"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);
    }
}
