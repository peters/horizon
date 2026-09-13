//! `WebDriver` scrollbar gestures backed by the sampled top-level page geometry.

use std::time::Instant;

use serde_json::json;

use crate::page_scroll::{VerticalScrollbarDrag, VerticalScrollbarPress};
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
    Drag(VerticalScrollbarDrag),
}

impl Driver {
    pub(super) fn handle_scrollbar_input(&mut self, input: &BrowserInput) -> Result<bool, String> {
        match input {
            BrowserInput::MousePress {
                x,
                y,
                button: BrowserButton::Left,
                ..
            } => {
                let Some(press) = self.scrollbar.page.and_then(|state| state.vertical_press(*x, *y)) else {
                    return Ok(false);
                };
                let gesture = match press {
                    VerticalScrollbarPress::Track(_) => Gesture::Track,
                    VerticalScrollbarPress::Drag(drag) => Gesture::Drag(drag),
                };
                self.scrollbar.gesture = Some(gesture);
                if let VerticalScrollbarPress::Track(target) = press {
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
                    self.scroll_page_to(drag.target_scroll_y(*y))?;
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
                    self.scroll_page_to(drag.target_scroll_y(*y))?;
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
        if let Some(page) = self.scrollbar.page {
            self.scrollbar.sample(page.with_scroll_y(target));
        }
        self.scrollbar.refresh_at = Instant::now();
        self.frames.demand();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::PageScrollState;
    use crate::page_scroll::VerticalScrollbarPress;

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
    fn webdriver_scrollbar_track_pages_and_thumb_drag_is_absolute() {
        let Some(VerticalScrollbarPress::Track(target)) = state().vertical_press(1_195.0, 100.0) else {
            panic!("track should own the press");
        };
        assert!((target - 600.0).abs() < f64::EPSILON);
        let Some(VerticalScrollbarPress::Drag(drag)) = state().vertical_press(1_195.0, 300.0) else {
            panic!("thumb should own the press");
        };
        assert!((drag.target_scroll_y(400.0) - 1_700.0).abs() < f64::EPSILON);
    }

    #[test]
    fn webdriver_scrollbar_overlay_claims_its_painted_gutter() {
        assert!(state().vertical_press(1_180.0, 300.0).is_none());
        let mut overlay = state();
        overlay.client_width = overlay.viewport_width;
        overlay.content_width = overlay.client_width;
        assert!(overlay.vertical_press(1_187.0, 300.0).is_none());
        assert!(overlay.vertical_press(1_195.0, 300.0).is_some());
        overlay.scroll_y = 0.0;
        overlay.content_height = 100_000.0;
        assert!(matches!(
            overlay.vertical_press(1_195.0, 22.0),
            Some(VerticalScrollbarPress::Drag(_))
        ));
    }
}
