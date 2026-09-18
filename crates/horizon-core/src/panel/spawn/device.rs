use crate::{DevicePanelState, DeviceViewTarget, Error, Result, editor::PanelContent};

use super::{Panel, PanelKind, StaticPanelSeed};

pub(super) fn spawn_device(mut seed: StaticPanelSeed, command: Option<&str>, is_restore: bool) -> Result<Panel> {
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
            connect_on_start: !is_restore,
        }),
        Some(address),
        None,
        has_custom_name,
    ))
}
