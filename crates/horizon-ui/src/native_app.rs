use eframe::{AppCreator, EframeWinitApplication, NativeOptions, UserEvent};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

use super::input::ObservedKeyboardInputs;

pub(crate) fn run_native_with_keyboard_observer(
    app_name: &str,
    native_options: NativeOptions,
    app_creator: AppCreator<'_>,
    observed_keyboard_inputs: ObservedKeyboardInputs,
) -> eframe::Result {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let eframe_app = eframe::create_native(app_name, native_options, app_creator, &event_loop);
    let mut app = KeyboardAwareApp::new(eframe_app, observed_keyboard_inputs);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct KeyboardAwareApp<'app> {
    inner: EframeWinitApplication<'app>,
    observed_keyboard_inputs: ObservedKeyboardInputs,
    modifiers: egui::Modifiers,
    native_window_liveness: NativeWindowLiveness,
}

impl<'app> KeyboardAwareApp<'app> {
    fn new(inner: EframeWinitApplication<'app>, observed_keyboard_inputs: ObservedKeyboardInputs) -> Self {
        Self {
            inner,
            observed_keyboard_inputs,
            modifiers: egui::Modifiers::default(),
            native_window_liveness: NativeWindowLiveness::default(),
        }
    }
}

#[derive(Default)]
struct NativeWindowLiveness {
    root_window_id: Option<winit::window::WindowId>,
    root_destroyed: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum NativeWindowEventAction {
    Forward,
    RootDestroyed,
    Ignore,
}

impl NativeWindowLiveness {
    /// A destroyed native handle must never reach eframe again. Its queued
    /// redraw path queries window geometry, which panics on X11 `BadWindow`.
    fn classify(&mut self, window_id: winit::window::WindowId, event: &WindowEvent) -> NativeWindowEventAction {
        if self.root_destroyed {
            return NativeWindowEventAction::Ignore;
        }

        // `resumed` creates the root window before any viewport callback can
        // create children, so the first dispatched window event identifies it.
        let root_window_id = *self.root_window_id.get_or_insert(window_id);
        if window_id == root_window_id && matches!(event, WindowEvent::Destroyed) {
            self.root_destroyed = true;
            NativeWindowEventAction::RootDestroyed
        } else {
            NativeWindowEventAction::Forward
        }
    }

    fn is_root_destroyed(&self) -> bool {
        self.root_destroyed
    }
}

impl ApplicationHandler<UserEvent> for KeyboardAwareApp<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.resumed(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: winit::window::WindowId, event: WindowEvent) {
        match self.native_window_liveness.classify(window_id, &event) {
            NativeWindowEventAction::RootDestroyed => {
                tracing::warn!(
                    ?window_id,
                    "root native window was destroyed outside the normal close flow; exiting cleanly"
                );
                event_loop.exit();
                return;
            }
            NativeWindowEventAction::Ignore => return,
            NativeWindowEventAction::Forward => {}
        }

        match &event {
            WindowEvent::ModifiersChanged(state) => {
                let state = state.state();
                let super_ = state.super_key();
                self.modifiers = egui::Modifiers {
                    alt: state.alt_key(),
                    ctrl: state.control_key(),
                    shift: state.shift_key(),
                    mac_cmd: cfg!(target_os = "macos") && super_,
                    command: if cfg!(target_os = "macos") {
                        super_
                    } else {
                        state.control_key()
                    },
                };
            }
            WindowEvent::KeyboardInput {
                event, is_synthetic, ..
            } if !(*is_synthetic && event.state == ElementState::Pressed) => {
                self.observed_keyboard_inputs.observe(event, self.modifiers);
            }
            _ => {}
        }

        self.inner.window_event(event_loop, window_id, event);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.user_event(event_loop, event);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.memory_warning(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use winit::event::WindowEvent;

    use super::{NativeWindowEventAction, NativeWindowLiveness};

    #[test]
    fn native_window_destruction_stops_later_event_dispatch() {
        let mut liveness = NativeWindowLiveness::default();
        let root_window_id = winit::window::WindowId::from(1);
        let child_window_id = winit::window::WindowId::from(2);

        assert_eq!(
            liveness.classify(root_window_id, &WindowEvent::CloseRequested),
            NativeWindowEventAction::Forward
        );
        assert_eq!(
            liveness.classify(child_window_id, &WindowEvent::Destroyed),
            NativeWindowEventAction::Forward
        );
        assert_eq!(
            liveness.classify(root_window_id, &WindowEvent::Destroyed),
            NativeWindowEventAction::RootDestroyed
        );
        assert_eq!(
            liveness.classify(root_window_id, &WindowEvent::RedrawRequested),
            NativeWindowEventAction::Ignore
        );
    }
}
