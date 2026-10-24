use std::collections::BTreeMap;

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
    /// Per-host overrides of `vnc_port`, keyed by the label the overlay
    /// shows (an SSH config alias or Tailscale device name) or by the SSH
    /// host name. A label match wins over a host-name match.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub vnc_ports: BTreeMap<String, u16>,
}

impl RemoteHostsConfig {
    pub const DEFAULT_WORKSPACE: &'static str = "Remote Sessions";
    pub const DEFAULT_VNC_PORT: u16 = 5900;

    /// # Errors
    /// Rejects a blank workspace name, a zero `vnc_port`, and any `vnc_ports`
    /// entry with a blank key or a zero port.
    pub fn validate(&self) -> Result<()> {
        if self.default_workspace.trim().is_empty() {
            return Err(Error::Config("remote_hosts.default_workspace cannot be empty".into()));
        }
        if self.vnc_port == 0 {
            return Err(Error::Config("remote_hosts.vnc_port must be nonzero".into()));
        }
        for (host, port) in &self.vnc_ports {
            if host.trim().is_empty() {
                return Err(Error::Config("remote_hosts.vnc_ports keys cannot be empty".into()));
            }
            if *port == 0 {
                return Err(Error::Config(format!("remote_hosts.vnc_ports.{host} must be nonzero")));
            }
        }
        Ok(())
    }

    /// The configured workspace name, exactly as written: workspace names are
    /// not normalized anywhere else, so a default must match them verbatim.
    #[must_use]
    pub fn default_workspace_name(&self) -> &str {
        &self.default_workspace
    }

    /// The VNC port for a host: an explicit override (typed as `:port` in the
    /// overlay filter) beats the per-host map, which beats `vnc_port`.
    #[must_use]
    pub fn vnc_port_for(&self, label: &str, host: &str, port_override: Option<u16>) -> u16 {
        port_override
            .or_else(|| self.vnc_ports.get(label).copied())
            .or_else(|| self.vnc_ports.get(host).copied())
            .unwrap_or(self.vnc_port)
    }

    /// The VNC endpoint as seen from the remote host, in Device panel form.
    #[must_use]
    pub fn vnc_target(&self, label: &str, host: &str, port_override: Option<u16>) -> String {
        format!("127.0.0.1:{}", self.vnc_port_for(label, host, port_override))
    }
}

impl Default for RemoteHostsConfig {
    fn default() -> Self {
        Self {
            default_workspace: Self::DEFAULT_WORKSPACE.to_string(),
            vnc_port: Self::DEFAULT_VNC_PORT,
            vnc_ports: BTreeMap::new(),
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
        assert_eq!(config.vnc_target("lab", "lab.example", None), "127.0.0.1:5900");
        assert!(config.validate().is_ok());
        assert_eq!(Config::default().remote_hosts, config);
    }

    #[test]
    fn missing_section_and_partial_sections_fill_in_defaults() {
        let config = Config::from_yaml("version: 11\n").unwrap();
        assert_eq!(config.remote_hosts, RemoteHostsConfig::default());

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 5901\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Remote Sessions");
        assert_eq!(
            config.remote_hosts.vnc_target("lab", "lab.example", None),
            "127.0.0.1:5901"
        );

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  Ops  '\n").unwrap();
        assert_eq!(
            config.remote_hosts.default_workspace_name(),
            "  Ops  ",
            "names stay exact"
        );
    }

    #[test]
    fn blank_workspace_and_port_zero_are_rejected() {
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  '\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.default_workspace"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_port"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_ports:\n    lab: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_ports.lab"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_ports:\n    ' ': 5901\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_ports keys"));
    }

    #[test]
    fn per_host_ports_resolve_override_then_label_then_host_then_global() {
        let config = Config::from_yaml(
            "version: 11\nremote_hosts:\n  vnc_port: 5901\n  vnc_ports:\n    lab: 5902\n    lab.example: 5903\n    db.example: 5904\n",
        )
        .unwrap();
        let hosts = &config.remote_hosts;
        assert_eq!(
            hosts.vnc_port_for("lab", "lab.example", Some(5999)),
            5999,
            "the typed port wins"
        );
        assert_eq!(
            hosts.vnc_port_for("lab", "lab.example", None),
            5902,
            "the label beats the host name"
        );
        assert_eq!(
            hosts.vnc_port_for("db", "db.example", None),
            5904,
            "the host name is the fallback key"
        );
        assert_eq!(
            hosts.vnc_port_for("Lab", "other", None),
            5901,
            "keys are exact, then the global port"
        );
        assert_eq!(hosts.vnc_target("db", "db.example", None), "127.0.0.1:5904");
    }

    #[test]
    fn section_round_trips_through_yaml() {
        let mut config = Config::default();
        config.remote_hosts.default_workspace = "Ops".into();
        config.remote_hosts.vnc_port = 5901;
        let yaml = config.to_yaml().unwrap();
        assert!(
            yaml.contains("remote_hosts:\n  default_workspace: Ops\n  vnc_port: 5901\n"),
            "an empty per-host map is not written"
        );
        assert!(!yaml.contains("vnc_ports"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);

        config.remote_hosts.vnc_ports.insert("lab".into(), 5902);
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("  vnc_port: 5901\n  vnc_ports:\n    lab: 5902\n"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);
    }
}
