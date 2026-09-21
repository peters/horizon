//! Shared decoded-frame resource budget for browser and native Device textures.

pub(crate) const FRAME_BUDGET_SIZE: [u16; 2] = [3840, 2160];
pub(crate) const MAX_FRAME_PIXELS: u32 = FRAME_BUDGET_SIZE[0] as u32 * FRAME_BUDGET_SIZE[1] as u32;
