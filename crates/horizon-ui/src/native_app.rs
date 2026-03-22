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
}

impl<'app> KeyboardAwareApp<'app> {
    fn new(inner: EframeWinitApplication<'app>, observed_keyboard_inputs: ObservedKeyboardInputs) -> Self {
        Self {
            inner,
            observed_keyboard_inputs,
            modifiers: egui::Modifiers::default(),
        }
    }
}

impl ApplicationHandler<UserEvent> for KeyboardAwareApp<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.resumed(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: winit::window::WindowId, event: WindowEvent) {
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
        self.inner.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.inner.user_event(event_loop, event);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        self.inner.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.inner.memory_warning(event_loop);
    }
}
