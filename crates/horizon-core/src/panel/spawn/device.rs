use crate::{
    DeviceInput, DevicePanelState, DeviceViewTarget, Error, FrameSourceRef, Result, SshConnection, editor::PanelContent,
};

use super::{Panel, PanelKind, StaticPanelSeed};

/// Spawn one native Device viewer: a VNC desktop, or a frame source program of this
/// computer.
///
/// A frame source viewer carries the name of one source of the settings and no
/// command, so a restored panel finds its program again through that name.
pub(super) fn spawn_device(
    mut seed: StaticPanelSeed,
    command: Option<&str>,
    is_restore: bool,
    mut identity: Option<crate::browser::manifest::device::DeviceIdentity>,
    input: DeviceInput,
    ssh_tunnel: Option<SshConnection>,
    frame_source: Option<FrameSourceRef>,
) -> Result<Panel> {
    if let Some(identity) = &mut identity {
        DevicePanelState::normalize_identity(identity)?;
    }
    if ssh_tunnel.as_ref().is_some_and(|connection| !connection.is_valid()) {
        return Err(Error::Config("Device SSH tunnel requires a host".into()));
    }
    let target = match frame_source {
        Some(reference) => DeviceViewTarget::frame_source(reference),
        None => DeviceViewTarget::parse(command.ok_or_else(|| {
            Error::Config("Device panel requires an explicit numeric loopback address and port".into())
        })?)?,
    };
    if target.is_frame_source() && ssh_tunnel.is_some() {
        return Err(Error::Config(
            "A frame source viewer is a program of this computer".into(),
        ));
    }
    let label = target.command();
    let (title, has_custom_name) = seed.take_title(|| match target.frame_source_ref() {
        Some(reference) => format!("Frame source {}", reference.name),
        None => ssh_tunnel.as_ref().map_or_else(
            || format!("Device {}", target.command()),
            |connection| format!("Device {}", connection.display_label()),
        ),
    });
    let mut panel = seed.into_panel(
        title,
        PanelKind::Device,
        PanelContent::Device(DevicePanelState {
            target,
            input,
            identity,
            connect_on_start: !is_restore,
            ssh_tunnel: ssh_tunnel.clone(),
        }),
        Some(label),
        None,
        has_custom_name,
    );
    // Persisted like an SSH panel's connection so a restart restores the route.
    panel.ssh_connection = ssh_tunnel;
    Ok(panel)
}
