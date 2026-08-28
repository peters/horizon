//! Safari scrollbar gestures backed by the sampled top-level page geometry.

use std::time::Instant;

use serde_json::json;

use crate::{BrowserButton, BrowserInput, PageScrollState};

use super::Driver;

pub(super) struct State {
    pub(super) refresh_at: Instant,
    page: Option<PageScrollState>,
    gesture: Option<Gesture>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            refresh_at: Instant::now(),
            page: None,
            gesture: None,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(super) fn sample(&mut self, state: PageScrollState) {
        self.page = Some(state);
    }
}

#[derive(Clone, Copy, Debug)]
enum Gesture {
    Track,
    Drag(Drag),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    pointer_y: f64,
    scroll_y: f64,
    max_scroll: f64,
    scroll_per_pointer_pixel: f64,
}

impl Drag {
    fn target(self, pointer_y: f64) -> f64 {
        (self.scroll_y + ((pointer_y - self.pointer_y) * self.scroll_per_pointer_pixel)).clamp(0.0, self.max_scroll)
    }
}

enum Press {
    Track(f64),
    Drag(Drag),
}

impl Driver {
    pub(super) fn handle_safari_scrollbar_input(&mut self, input: &BrowserInput) -> Result<bool, String> {
        match input {
            BrowserInput::MousePress {
                x,
                y,
                button: BrowserButton::Left,
                ..
            } => {
                let Some(press) = self.scrollbar.page.and_then(|state| press(state, *x, *y)) else {
                    return Ok(false);
                };
                let gesture = match press {
                    Press::Track(_) => Gesture::Track,
                    Press::Drag(drag) => Gesture::Drag(drag),
                };
                self.scrollbar.gesture = Some(gesture);
                if let Press::Track(target) = press {
                    self.scroll_page_to(target)?;
                }
                Ok(true)
            }
            BrowserInput::MouseMove { y, buttons, .. } => {
                let Some(gesture) = self.scrollbar.gesture else {
                    return Ok(false);
                };
                if let Gesture::Drag(drag) = gesture
                    && buttons & 1 != 0
                {
                    self.scroll_page_to(drag.target(*y))?;
                }
                Ok(true)
            }
            BrowserInput::MouseRelease {
                y,
                button: BrowserButton::Left,
                ..
            } => {
                let Some(gesture) = self.scrollbar.gesture.take() else {
                    return Ok(false);
                };
                if let Gesture::Drag(drag) = gesture {
                    self.scroll_page_to(drag.target(*y))?;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn scroll_page_to(&mut self, target: f64) -> Result<(), String> {
        self.classic_post(
            "execute/sync",
            &json!({
                "script": "window.scrollTo(window.scrollX, arguments[0]);",
                "args": [target],
            }),
        )?;
        self.scrollbar.refresh_at = Instant::now();
        self.frames.demand();
        Ok(())
    }
}

fn press(state: PageScrollState, x: f64, y: f64) -> Option<Press> {
    let client_width = f64::from(state.client_width);
    let viewport_width = f64::from(state.viewport_width);
    let client_height = f64::from(state.client_height);
    let content_height = f64::from(state.content_height);
    let overlay = viewport_width <= client_width;
    let scrollbar_left = if overlay {
        (viewport_width - 8.0).max(0.0)
    } else {
        client_width
    };
    if !state.is_valid()
        || content_height <= client_height
        || x < scrollbar_left
        || x > viewport_width
        || y < 0.0
        || y > client_height
    {
        return None;
    }

    let max_scroll = content_height - client_height;
    let thumb_height = (client_height * client_height / content_height).clamp(24.0, client_height);
    let thumb_travel = client_height - thumb_height;
    let scroll_y = f64::from(state.scroll_y).clamp(0.0, max_scroll);
    let thumb_top = scroll_y / max_scroll * thumb_travel;
    if y >= thumb_top - 2.0 && y <= thumb_top + thumb_height + 2.0 {
        return Some(Press::Drag(Drag {
            pointer_y: y,
            scroll_y,
            max_scroll,
            scroll_per_pointer_pixel: max_scroll / thumb_travel.max(1.0),
        }));
    }
    let direction = if y < thumb_top { -1.0 } else { 1.0 };
    Some(Press::Track(
        (scroll_y + (direction * client_height)).clamp(0.0, max_scroll),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Press, press};
    use crate::PageScrollState;

    fn state() -> PageScrollState {
        PageScrollState {
            scroll_x: 0.0,
            scroll_y: 1_200.0,
            viewport_width: 1_200.0,
            viewport_height: 600.0,
            client_width: 1_188.0,
            client_height: 600.0,
            content_width: 1_188.0,
            content_height: 3_000.0,
        }
    }

    #[test]
    fn safari_scrollbar_track_pages_and_thumb_drag_is_absolute() {
        let Some(Press::Track(target)) = press(state(), 1_195.0, 100.0) else {
            panic!("track should own the press");
        };
        assert!((target - 600.0).abs() < f64::EPSILON);
        let Some(Press::Drag(drag)) = press(state(), 1_195.0, 300.0) else {
            panic!("thumb should own the press");
        };
        assert!((drag.target(400.0) - 1_700.0).abs() < f64::EPSILON);
    }

    #[test]
    fn safari_scrollbar_overlay_claims_only_its_painted_gutter() {
        assert!(press(state(), 1_180.0, 300.0).is_none());
        let mut overlay = state();
        overlay.client_width = overlay.viewport_width;
        overlay.content_width = overlay.client_width;
        assert!(press(overlay, 1_190.0, 300.0).is_none());
        assert!(press(overlay, 1_195.0, 300.0).is_some());
        overlay.scroll_y = 0.0;
        overlay.content_height = 100_000.0;
        assert!(matches!(press(overlay, 1_195.0, 22.0), Some(Press::Drag(_))));
    }
}
