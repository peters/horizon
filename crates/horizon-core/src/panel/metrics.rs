//! The size of a new panel of each kind, and the scrollback that its terminal keeps.
//!
//! A default lives here and not at a call site, so one place answers what a new
//! panel of a kind looks like, and the tests of the window read the same value.

pub(crate) const DEFAULT_CELL_WIDTH: u16 = 8;
pub(crate) const DEFAULT_CELL_HEIGHT: u16 = 17;

/// The size of a new shell, SSH, command or agent panel, in points.
pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];

/// A new desktop viewer: the rows above its picture left the generic size's
/// body about 20 points high, so this keeps a 16:10 picture of 480 by 300.
pub const DEFAULT_DEVICE_PANEL_SIZE: [f32; 2] = [560.0, 540.0];

/// Lines of terminal scrollback kept for a shell, SSH, command, or agent panel.
pub const PANEL_SCROLLBACK_LIMIT: usize = 24_000;
pub(crate) const DEFAULT_PANEL_SCROLLBACK_LIMIT: usize = PANEL_SCROLLBACK_LIMIT;
pub(crate) const AGENT_PANEL_SCROLLBACK_LIMIT: usize = PANEL_SCROLLBACK_LIMIT;
