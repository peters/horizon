use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::BrowserInput;
use crate::session::{BrowserEvent, BrowserEventSender};

use super::super::actions::ActionState;
use super::Driver;

pub(super) const WINDOW_CHROME_SCRIPT: &str = "return { width: Math.max(0, window.outerWidth - window.innerWidth), height: Math.max(0, window.outerHeight - window.innerHeight) };";
const WINDOW_FOCUS_SETTLE: Duration = Duration::from_millis(20);
// Keep one native click-and-type burst ahead of Safari's temporary focus steal.
const INPUT_SETTLE: Duration = Duration::from_millis(200);

pub(super) struct InputState {
    pub(super) window_handle: String,
    actions: ActionState,
    pending: Vec<BrowserInput>,
    buttons: u32,
    keys_down: HashSet<crate::BrowserKey>,
    ready_at: Option<Instant>,
}

pub(super) enum InputDispatch {
    Pending,
    Background(Value),
    Foreground(Vec<Value>),
}

impl InputState {
    pub(super) fn from_window_response(response: &Value) -> Result<Self, String> {
        let window_handle = response
            .get("value")
            .unwrap_or(response)
            .as_str()
            .ok_or_else(|| "Safari returned no window handle".to_string())?;
        Ok(Self {
            window_handle: window_handle.to_string(),
            actions: ActionState::default(),
            pending: Vec::new(),
            buttons: 0,
            keys_down: HashSet::new(),
            ready_at: None,
        })
    }

    pub(super) fn dispatch(&mut self, input: BrowserInput) -> InputDispatch {
        if self.pending.is_empty() && matches!(&input, BrowserInput::MouseMove { buttons: 0, .. }) {
            return InputDispatch::Background(self.actions.payload(input));
        }

        let completes_gesture = match &input {
            BrowserInput::MousePress { buttons, .. } | BrowserInput::MouseMove { buttons, .. } => {
                self.buttons = *buttons;
                false
            }
            BrowserInput::MouseRelease { buttons, .. } => {
                self.buttons = *buttons;
                true
            }
            BrowserInput::KeyDown {
                physical_key,
                key,
                repeat,
                ..
            } => {
                if !repeat {
                    self.keys_down.insert(physical_key.unwrap_or(*key));
                }
                false
            }
            BrowserInput::KeyUp { physical_key, key, .. } => {
                self.keys_down.remove(&physical_key.unwrap_or(*key));
                true
            }
            BrowserInput::Wheel { .. } | BrowserInput::InsertText { .. } => true,
        };
        if matches!(&input, BrowserInput::MousePress { .. } | BrowserInput::KeyDown { .. }) {
            self.ready_at = None;
        }
        if matches!(&input, BrowserInput::MouseMove { .. })
            && let Some(last @ BrowserInput::MouseMove { .. }) = self.pending.last_mut()
        {
            *last = input;
        } else {
            self.pending.push(input);
        }
        if completes_gesture && self.buttons == 0 && self.keys_down.is_empty() {
            self.ready_at = Some(Instant::now() + INPUT_SETTLE);
        }
        InputDispatch::Pending
    }

    fn take_ready(&mut self, now: Instant) -> Option<InputDispatch> {
        if self.ready_at.is_none_or(|ready_at| now < ready_at) {
            return None;
        }
        self.ready_at = None;
        let payloads = std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|input| match input {
                BrowserInput::KeyDown { text: Some(text), .. } => {
                    Some(self.actions.payload(BrowserInput::InsertText { text }))
                }
                BrowserInput::KeyUp { text: Some(_), .. } => None,
                input => Some(self.actions.payload(input)),
            })
            .collect::<Vec<_>>();
        Some(InputDispatch::Foreground(payloads))
    }

    pub(super) fn reset_actions(&mut self) {
        *self = Self {
            window_handle: self.window_handle.clone(),
            actions: ActionState::default(),
            pending: Vec::new(),
            buttons: 0,
            keys_down: HashSet::new(),
            ready_at: None,
        };
    }
}

impl Driver {
    pub(super) fn queue_safari_input(&mut self, input: BrowserInput) -> Result<(), String> {
        let Some(state) = &mut self.safari else {
            return Err("Safari input state unavailable".to_string());
        };
        let dispatch = state.dispatch(input);
        let result = execute(dispatch, "", |suffix, payload| self.classic_post(suffix, payload));
        if let Err(error) = &result {
            tracing::warn!("WebDriver input failed: {error}");
            self.reset_safari_input();
        }
        result
    }

    pub(super) fn tick_safari_input(&mut self, event_tx: &BrowserEventSender) {
        let Some((dispatch, handle)) = self.safari.as_mut().and_then(|state| {
            state
                .take_ready(Instant::now())
                .map(|dispatch| (dispatch, state.window_handle.clone()))
        }) else {
            return;
        };
        let result = execute(dispatch, &handle, |suffix, payload| self.classic_post(suffix, payload));
        let _ = event_tx.send(BrowserEvent::HostFocusRequested);
        if let Err(error) = result {
            tracing::warn!("WebDriver input failed: {error}");
            self.reset_safari_input();
        }
        if !self.retain_frame_during_navigation {
            self.scroll_state_refresh_at = Instant::now();
            self.frames.demand();
        }
    }

    fn reset_safari_input(&mut self) {
        let _ = self.classic_delete("actions");
        if let Some(state) = &mut self.safari {
            state.reset_actions();
        }
    }
}

pub(super) fn execute(
    dispatch: InputDispatch,
    window_handle: &str,
    mut post: impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Result<(), String> {
    match dispatch {
        InputDispatch::Pending => Ok(()),
        InputDispatch::Background(payload) => post("actions", &payload).map(|_| ()),
        InputDispatch::Foreground(payloads) => {
            post("window", &json!({ "handle": window_handle }))
                .map_err(|error| format!("Safari window focus failed: {error}"))?;
            std::thread::sleep(WINDOW_FOCUS_SETTLE);
            for payload in payloads {
                post("actions", &payload).map_err(|error| format!("Safari focused input failed: {error}"))?;
            }
            Ok(())
        }
    }
}

pub(super) fn window_rect(response: &Value, width: u32, height: u32) -> (u32, u32) {
    let chrome = response.get("value").unwrap_or(response);
    let dimension = |name| {
        chrome
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default()
    };
    (
        width.saturating_add(dimension("width")),
        height.saturating_add(dimension("height")),
    )
}

#[cfg(test)]
mod tests {
    use super::{InputDispatch, InputState, window_rect};
    use crate::{BrowserButton, BrowserInput, BrowserKey, BrowserModifiers};

    #[test]
    fn pointer_gesture_is_replayed_only_after_release() {
        let mut state = InputState::from_window_response(&serde_json::json!({ "value": "window" }))
            .unwrap_or_else(|error| panic!("test window handle failed: {error}"));
        let modifiers = BrowserModifiers::none();
        assert!(matches!(
            state.dispatch(BrowserInput::MousePress {
                x: 10.0,
                y: 20.0,
                button: BrowserButton::Left,
                click_count: 1,
                buttons: 1,
                modifiers,
            }),
            InputDispatch::Pending
        ));
        assert!(matches!(
            state.dispatch(BrowserInput::MouseMove {
                x: 30.0,
                y: 40.0,
                buttons: 1,
                modifiers,
            }),
            InputDispatch::Pending
        ));
        assert!(matches!(
            state.dispatch(BrowserInput::MouseRelease {
                x: 50.0,
                y: 60.0,
                button: BrowserButton::Left,
                click_count: 1,
                buttons: 0,
                modifiers,
            }),
            InputDispatch::Pending
        ));
        assert!(matches!(
            state.dispatch(BrowserInput::KeyDown {
                physical_key: Some(BrowserKey::Char('a')),
                key: BrowserKey::Char('a'),
                text: Some("a".to_string()),
                modifiers,
                repeat: false,
                edit_command: None,
            }),
            InputDispatch::Pending
        ));
        assert!(matches!(
            state.dispatch(BrowserInput::KeyUp {
                physical_key: Some(BrowserKey::Char('a')),
                key: BrowserKey::Char('a'),
                text: Some("a".to_string()),
                modifiers,
            }),
            InputDispatch::Pending
        ));
        assert!(state.take_ready(std::time::Instant::now()).is_none());
        let dispatch = state
            .take_ready(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .unwrap_or_else(|| panic!("released gesture should become ready"));
        assert!(matches!(dispatch, InputDispatch::Foreground(payloads) if payloads.len() == 4));
    }

    #[test]
    fn window_rect_includes_browser_chrome() {
        let response = serde_json::json!({ "value": { "width": 0, "height": 52 } });
        assert_eq!(window_rect(&response, 1104, 668), (1104, 720));
        assert_eq!(window_rect(&serde_json::Value::Null, 1104, 668), (1104, 668));
    }
}
