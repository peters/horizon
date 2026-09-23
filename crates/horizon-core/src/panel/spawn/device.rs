use crate::{DevicePanelState, DeviceViewTarget, Error, Result, SshConnection, editor::PanelContent};

use super::{Panel, PanelKind, StaticPanelSeed};

pub(super) fn spawn_device(
    mut seed: StaticPanelSeed,
    command: Option<&str>,
    is_restore: bool,
    mut identity: Option<crate::browser::manifest::device::DeviceIdentity>,
    ssh_tunnel: Option<SshConnection>,
) -> Result<Panel> {
    if let Some(identity) = &mut identity {
        DevicePanelState::normalize_identity(identity)?;
    }
    if ssh_tunnel.as_ref().is_some_and(|connection| !connection.is_valid()) {
        return Err(Error::Config("Device SSH tunnel requires a host".into()));
    }
    let target =
        DeviceViewTarget::parse(command.ok_or_else(|| {
            Error::Config("Device panel requires an explicit numeric loopback address and port".into())
        })?)?;
    let address = target.address().to_string();
    let (title, has_custom_name) = seed.take_title(|| {
        ssh_tunnel.as_ref().map_or_else(
            || format!("Device {address}"),
            |connection| format!("Device {}", connection.display_label()),
        )
    });
    let mut panel = seed.into_panel(
        title,
        PanelKind::Device,
        PanelContent::Device(DevicePanelState {
            target,
            identity,
            connect_on_start: !is_restore,
            ssh_tunnel: ssh_tunnel.clone(),
        }),
        Some(address),
        None,
        has_custom_name,
    );
    // Persisted like an SSH panel's connection so a restart restores the route.
    panel.ssh_connection = ssh_tunnel;
    Ok(panel)
}
