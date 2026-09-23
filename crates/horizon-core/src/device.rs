mod view;
pub use view::{DeviceImageLayout, DeviceViewOptions, DeviceViewport};

use std::net::SocketAddr;

use crate::browser::manifest::device::DeviceIdentity;
use crate::ssh::SshConnection;
use crate::{Error, Result};

/// An explicit local VNC endpoint; connecting and rendering belong to the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceViewTarget {
    address: SocketAddr,
}

impl DeviceViewTarget {
    /// Parse a numeric loopback address and nonzero port, without DNS or defaults.
    ///
    /// # Errors
    /// Rejects hostnames, remote addresses, missing ports, and port zero.
    pub fn parse(value: &str) -> Result<Self> {
        let address: SocketAddr = value.parse().map_err(|_| {
            Error::Config("Device target must be a numeric loopback address and port, such as 127.0.0.1:5900".into())
        })?;
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Error::Config(
                "Device target requires a loopback address and nonzero port".into(),
            ));
        }
        Ok(Self { address })
    }

    #[must_use]
    pub const fn address(self) -> SocketAddr {
        self.address
    }
}

/// A read-only viewer target, separate from application/device input control.
#[derive(Clone, Debug)]
pub struct DevicePanelState {
    /// The VNC endpoint: on this machine, or as seen from the `ssh_tunnel` host.
    pub target: DeviceViewTarget,
    pub identity: Option<DeviceIdentity>,
    /// Restored panels require explicit reconnection because local ports can be reused.
    pub connect_on_start: bool,
    /// Reach `target` through `ssh -W` on this host instead of connecting directly.
    pub ssh_tunnel: Option<SshConnection>,
}

impl DevicePanelState {
    const MAX_LABEL_CHARS: usize = 256;

    /// Where the desktop lives, for titles and connection details.
    #[must_use]
    pub fn endpoint_label(&self) -> String {
        match &self.ssh_tunnel {
            Some(connection) => format!("{} via {}", self.target.address(), connection.display_label()),
            None => self.target.address().to_string(),
        }
    }

    /// Bound the untrusted handshake label and flatten controls for plain-text display.
    #[must_use]
    pub fn server_label(raw: &str) -> Option<String> {
        let bounded: String = raw
            .chars()
            .take(Self::MAX_LABEL_CHARS)
            .map(|character| {
                if Self::is_label_control(character) {
                    ' '
                } else {
                    character
                }
            })
            .collect();
        let trimmed = bounded.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    fn is_label_control(character: char) -> bool {
        character.is_control()
            || matches!(character,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }

    /// Choose an available human-readable label without treating it as verified identity.
    #[must_use]
    pub fn display_name<'a>(&'a self, server_name: Option<&'a str>) -> Option<&'a str> {
        self.identity
            .as_ref()
            .and_then(|identity| {
                identity
                    .machine_name
                    .as_deref()
                    .or(identity.hostname.as_deref())
                    .or(identity.tailscale_name.as_deref())
            })
            .or(server_name.filter(|name| !name.is_empty()))
    }

    /// Normalize creator labels and bound the metadata stored with a panel.
    ///
    /// # Errors
    /// Rejects control or bidi-formatting characters, labels over 256 characters and more than 16 IPs.
    pub fn normalize_identity(identity: &mut DeviceIdentity) -> Result<()> {
        for label in [
            &mut identity.machine_name,
            &mut identity.hostname,
            &mut identity.tailscale_name,
        ] {
            if let Some(value) = label {
                let trimmed = value.trim();
                if trimmed.chars().count() > Self::MAX_LABEL_CHARS || value.chars().any(Self::is_label_control) {
                    return Err(Error::Config(
                        "Device identity labels must be plain text of at most 256 characters".into(),
                    ));
                }
                *label = (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
        }
        if identity.ip_addresses.len() > 16 {
            return Err(Error::Config("Device identity supports at most 16 IP addresses".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DevicePanelState, DeviceViewTarget};
    use crate::{
        Board, CanvasViewState, Panel, PanelId, PanelKind, PanelOptions, PanelResume, PresetConfig, RuntimeState,
        WindowConfig, WorkspaceId,
    };

    #[test]
    fn device_target_accepts_only_numeric_loopback_endpoints() {
        for value in ["127.0.0.1:5900", "127.0.0.2:65535", "[::1]:5900"] {
            assert!(DeviceViewTarget::parse(value).is_ok(), "{value}");
        }
        for value in [
            "",
            "localhost:5900",
            "127.0.0.1",
            "127.0.0.1:0",
            "0.0.0.0:5900",
            "192.0.2.1:5900",
            "[::]:5900",
            "vnc://127.0.0.1:5900",
            "/bin/sh",
        ] {
            assert!(DeviceViewTarget::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn device_spawn_has_no_terminal_browser_or_text_input() -> crate::Result<()> {
        let mut panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Device,
                command: Some("127.0.0.1:5900".into()),
                ..PanelOptions::default()
            },
        )?;
        assert!(panel.terminal().is_none());
        assert!(panel.browser().is_none());
        assert!(!panel.kind.accepts_text_input());
        assert!(!panel.kind.is_agent());
        assert_eq!(panel.launch_command.as_deref(), Some("127.0.0.1:5900"));
        assert!(panel.device().is_some_and(|device| device.connect_on_start));
        panel.write_input(b"must not launch a shell");
        for _ in 0..2 {
            panel.restart()?;
            panel.request_shutdown();
            assert!(panel.wait_for_shutdown(Duration::ZERO));
            assert!(panel.shutdown_with_timeout(Duration::ZERO));
            assert!(panel.terminal().is_none());
        }
        assert!(
            Panel::spawn(
                PanelId(2),
                WorkspaceId(1),
                PanelOptions {
                    kind: PanelKind::Device,
                    ..PanelOptions::default()
                }
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn device_target_survives_persistence_but_restore_does_not_connect() -> crate::Result<()> {
        let mut board = Board::new();
        let workspace = board.create_workspace("isolated device");
        let preset = PresetConfig {
            name: "Device fixture".into(),
            alias: None,
            kind: PanelKind::Device,
            command: Some("[::1]:5901".into()),
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        };
        assert!(!preset.requires_workspace_cwd());
        board.create_panel(
            preset.to_panel_options(&crate::browser::BrowserConfig::default()),
            workspace,
        )?;
        let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let yaml = state.to_yaml()?;
        let restored: RuntimeState =
            serde_yaml::from_str(&yaml).map_err(|error| crate::Error::State(error.to_string()))?;
        let saved = &restored.workspaces[0].panels[0];
        assert_eq!(saved.kind, PanelKind::Device);
        assert_eq!(saved.command.as_deref(), Some("[::1]:5901"));
        assert!(saved.browser_profile.is_none());
        let panel = Panel::spawn(
            PanelId(2),
            workspace,
            saved.to_panel_options(&crate::browser::BrowserConfig::default()),
        )?;
        assert!(
            panel
                .device()
                .is_some_and(|device| !device.connect_on_start && device.target.address().to_string() == "[::1]:5901")
        );
        assert!(panel.terminal().is_none());
        Ok(())
    }
    #[test]
    fn tunnelled_device_keeps_its_ssh_host_across_restore() -> crate::Result<()> {
        use crate::SshConnection;
        let tunnel = SshConnection {
            host: "lab".into(),
            user: Some("deploy".into()),
            port: Some(2222),
            ..SshConnection::default()
        };
        let mut board = Board::new();
        let workspace = board.create_workspace("tunnel fixture");
        let id = board.create_panel(
            PanelOptions {
                kind: PanelKind::Device,
                command: Some("127.0.0.1:5901".into()),
                ssh_connection: Some(tunnel.clone()),
                ..PanelOptions::default()
            },
            workspace,
        )?;
        let panel = board
            .panel(id)
            .ok_or_else(|| crate::Error::State("missing panel".into()))?;
        assert_eq!(panel.ssh_connection.as_ref(), Some(&tunnel));
        assert_eq!(panel.title, "Device deploy@lab");
        let device = panel
            .device()
            .ok_or_else(|| crate::Error::State("missing device".into()))?;
        assert_eq!(device.ssh_tunnel.as_ref(), Some(&tunnel));
        assert!(device.connect_on_start);
        assert_eq!(device.endpoint_label(), "127.0.0.1:5901 via deploy@lab");

        let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let restored: RuntimeState =
            serde_yaml::from_str(&state.to_yaml()?).map_err(|error| crate::Error::State(error.to_string()))?;
        let saved = &restored.workspaces[0].panels[0];
        assert_eq!(saved.ssh_connection.as_ref(), Some(&tunnel));
        let panel = Panel::spawn(
            PanelId(2),
            workspace,
            saved.to_panel_options(&crate::browser::BrowserConfig::default()),
        )?;
        let device = panel
            .device()
            .ok_or_else(|| crate::Error::State("missing device".into()))?;
        assert_eq!(device.ssh_tunnel.as_ref(), Some(&tunnel));
        assert_eq!(device.target.address().to_string(), "127.0.0.1:5901");
        assert!(!device.connect_on_start);
        Ok(())
    }

    #[test]
    fn tunnel_without_a_host_is_rejected_before_panel_creation() {
        use crate::SshConnection;
        assert!(
            Panel::spawn(
                PanelId(1),
                WorkspaceId(1),
                PanelOptions {
                    kind: PanelKind::Device,
                    command: Some("127.0.0.1:5900".into()),
                    ssh_connection: Some(SshConnection {
                        host: "   ".into(),
                        ..SshConnection::default()
                    }),
                    ..PanelOptions::default()
                }
            )
            .is_err()
        );
        let local = DevicePanelState {
            target: DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
            identity: None,
            connect_on_start: true,
            ssh_tunnel: None,
        };
        assert_eq!(local.endpoint_label(), "127.0.0.1:5900");
    }

    #[test]
    fn supplied_identity_is_normalized_and_selected_without_endpoint_inference() -> crate::Result<()> {
        use crate::browser::manifest::device::DeviceIdentity;
        let mut identity = DeviceIdentity {
            machine_name: Some("  Lab workstation  ".into()),
            hostname: Some("lab-host".into()),
            tailscale_name: Some("lab-host.example.ts.net".into()),
            ip_addresses: vec!["192.0.2.10".parse().unwrap(), "2001:db8::10".parse().unwrap()],
        };
        super::DevicePanelState::normalize_identity(&mut identity)?;
        let mut device = super::DevicePanelState {
            target: DeviceViewTarget::parse("127.0.0.1:5900")?,
            identity: Some(identity),
            connect_on_start: false,
            ssh_tunnel: None,
        };
        assert_eq!(device.display_name(Some("Desktop")), Some("Lab workstation"));
        device.identity.as_mut().unwrap().machine_name = None;
        assert_eq!(device.display_name(Some("Desktop")), Some("lab-host"));
        device.identity.as_mut().unwrap().hostname = None;
        assert_eq!(device.display_name(Some("Desktop")), Some("lab-host.example.ts.net"));
        device.identity = None;
        assert_eq!(device.display_name(Some("Desktop")), Some("Desktop"));
        assert_eq!(device.display_name(Some("")), None);
        assert_eq!(device.display_name(None), None);
        Ok(())
    }

    #[test]
    fn invalid_identity_is_rejected_before_panel_creation() {
        use crate::browser::manifest::device::DeviceIdentity;
        for label in [
            "bad\nname".into(),
            "\nname".into(),
            "name\t".into(),
            "\n\t".into(),
            "a".repeat(257),
        ] {
            assert!(
                Panel::spawn(
                    PanelId(1),
                    WorkspaceId(1),
                    PanelOptions {
                        kind: PanelKind::Device,
                        command: Some("127.0.0.1:5900".into()),
                        device_identity: Some(DeviceIdentity {
                            machine_name: Some(label),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                )
                .is_err()
            );
        }
        let mut blank = DeviceIdentity {
            machine_name: Some("  ".into()),
            ..Default::default()
        };
        super::DevicePanelState::normalize_identity(&mut blank).unwrap();
        assert!(blank.machine_name.is_none());
        blank.ip_addresses = vec!["192.0.2.1".parse().unwrap(); 17];
        assert!(super::DevicePanelState::normalize_identity(&mut blank).is_err());
    }
    #[test]
    fn creator_identity_survives_restore_without_reconnecting() -> crate::Result<()> {
        use crate::browser::manifest::device::DeviceIdentity;
        let mut board = Board::new();
        let workspace = board.create_workspace("identity fixture");
        let identity = DeviceIdentity {
            machine_name: Some("Lab workstation".into()),
            hostname: Some("lab-host".into()),
            ip_addresses: vec!["192.0.2.10".parse().unwrap(), "2001:db8::10".parse().unwrap()],
            tailscale_name: Some("lab-host.example.ts.net".into()),
        };
        board.create_panel(
            PanelOptions {
                kind: PanelKind::Device,
                command: Some("127.0.0.1:5900".into()),
                device_identity: Some(identity.clone()),
                ..Default::default()
            },
            workspace,
        )?;
        let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let yaml = saved.to_yaml()?;
        let restored: RuntimeState = serde_yaml::from_str(&yaml).unwrap();
        let panel = Panel::spawn(
            PanelId(2),
            workspace,
            restored.workspaces[0].panels[0].to_panel_options(&crate::browser::BrowserConfig::default()),
        )?;
        let device = panel.device().unwrap();
        assert_eq!(device.identity.as_ref(), Some(&identity));
        assert!(!device.connect_on_start);
        let legacy: crate::PanelState = serde_yaml::from_str("kind: device\ncommand: '127.0.0.1:5900'\n").unwrap();
        assert!(legacy.device_identity.is_none());
        assert!(
            legacy
                .to_panel_options(&crate::browser::BrowserConfig::default())
                .device_identity
                .is_none()
        );
        Ok(())
    }
    #[test]
    fn server_labels_are_optional_bounded_unicode_plain_text() {
        use super::DevicePanelState;
        assert_eq!(DevicePanelState::server_label("\0\n "), None);
        assert_eq!(
            DevicePanelState::server_label("  Lab\nÆØÅ\t ").as_deref(),
            Some("Lab ÆØÅ")
        );
        assert_eq!(
            DevicePanelState::server_label(&"Æ".repeat(300))
                .unwrap()
                .chars()
                .count(),
            256
        );
        assert_eq!(
            DevicePanelState::server_label("left\u{202e}right").as_deref(),
            Some("left right")
        );
    }
    #[test]
    fn directional_formatting_is_rejected_for_supplied_identity_and_flattened_for_servers() {
        use super::DevicePanelState;
        use crate::browser::manifest::device::DeviceIdentity;
        for marker in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
            '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            for field in 0..3 {
                let mut identity = DeviceIdentity::default();
                let label = Some(format!("host{marker}"));
                match field {
                    0 => identity.machine_name = label,
                    1 => identity.hostname = label,
                    _ => identity.tailscale_name = label,
                }
                assert!(
                    DevicePanelState::normalize_identity(&mut identity).is_err(),
                    "{marker:?}"
                );
            }
            assert_eq!(
                DevicePanelState::server_label(&format!("left{marker}right")).as_deref(),
                Some("left right")
            );
        }
        let mut identity = DeviceIdentity {
            machine_name: Some("مختبر".into()),
            ..Default::default()
        };
        DevicePanelState::normalize_identity(&mut identity).unwrap();
        assert_eq!(identity.machine_name.as_deref(), Some("مختبر"));
        assert_eq!(DevicePanelState::server_label("مختبر").as_deref(), Some("مختبر"));
    }
}
