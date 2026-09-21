mod view;
pub use view::{DeviceImageLayout, DeviceViewOptions, DeviceViewport};

use std::net::SocketAddr;

use crate::browser::manifest::device::DeviceIdentity;
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
    pub target: DeviceViewTarget,
    pub identity: Option<DeviceIdentity>,
    /// Restored panels require explicit reconnection because local ports can be reused.
    pub connect_on_start: bool,
}

impl DevicePanelState {
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
    /// Rejects control characters, labels longer than 256 characters and more than 16 IPs.
    pub fn normalize_identity(identity: &mut DeviceIdentity) -> Result<()> {
        for label in [
            &mut identity.machine_name,
            &mut identity.hostname,
            &mut identity.tailscale_name,
        ] {
            if let Some(value) = label {
                let trimmed = value.trim();
                if trimmed.chars().count() > 256 || trimmed.chars().any(char::is_control) {
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

    use super::DeviceViewTarget;
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
        for label in ["bad\nname".into(), "a".repeat(257)] {
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
}
