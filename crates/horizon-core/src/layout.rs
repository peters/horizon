use crate::panel::DEFAULT_PANEL_SIZE;

pub(crate) const TILE_GAP: f32 = 20.0;
pub(crate) const WS_INNER_PAD: f32 = 20.0;
pub(crate) const WORKSPACE_GAP: f32 = 80.0;

/// Padding around workspace panel bounds for the visual frame (left, right, bottom).
pub(crate) const WS_FRAME_PAD: f32 = 16.0;
/// Extra top padding for the workspace title bar area.
pub(crate) const WS_FRAME_TOP_EXTRA: f32 = 38.0;
/// Size of an empty workspace placeholder.
pub(crate) const WS_EMPTY_FRAME_SIZE: [f32; 2] = [304.0, 154.0];
/// Minimum gap maintained between workspace frames during collision resolution.
pub(crate) const WS_COLLISION_GAP: f32 = 10.0;

pub(crate) fn workspace_slot_width() -> f32 {
    let columns = 3.0;
    let content = columns * DEFAULT_PANEL_SIZE[0] + (columns - 1.0) * TILE_GAP;
    content + 2.0 * WS_INNER_PAD + WORKSPACE_GAP
}

pub(crate) fn tiled_panel_position(origin: [f32; 2], index: usize) -> [f32; 2] {
    let column = usize_to_f32(index % 3);
    let row = usize_to_f32(index / 3);
    [
        origin[0] + WS_INNER_PAD + column * (DEFAULT_PANEL_SIZE[0] + TILE_GAP),
        origin[1] + WS_INNER_PAD + row * (DEFAULT_PANEL_SIZE[1] + TILE_GAP),
    ]
}

pub(crate) fn ceil_sqrt_usize(value: usize) -> usize {
    if value <= 1 {
        return value;
    }

    let mut root = 1usize;
    while root.saturating_mul(root) < value {
        root += 1;
    }
    root
}

pub(crate) fn usize_to_f32(value: usize) -> f32 {
    let clamped = u16::try_from(value).unwrap_or(u16::MAX);
    f32::from(clamped)
}
