use crate::{DevicePanelState, DeviceViewTarget, Error, Result, editor::PanelContent};

use super::{Panel, PanelKind, StaticPanelSeed};

pub(super) fn spawn_device(
    mut seed: StaticPanelSeed,
    command: Option<&str>,
    is_restore: bool,
    mut identity: Option<crate::browser::manifest::device::DeviceIdentity>,
) -> Result<Panel> {
    if let Some(identity) = &mut identity {
        DevicePanelState::normalize_identity(identity)?;
    }
    let target =
        DeviceViewTarget::parse(command.ok_or_else(|| {
            Error::Config("Device panel requires an explicit numeric loopback address and port".into())
        })?)?;
    let address = target.address().to_string();
    let (title, has_custom_name) = seed.take_title(|| format!("Device {address}"));
    Ok(seed.into_panel(
        title,
        PanelKind::Device,
        PanelContent::Device(DevicePanelState {
            target,
            identity,
            connect_on_start: !is_restore,
        }),
        Some(address),
        None,
        has_custom_name,
    ))
}
