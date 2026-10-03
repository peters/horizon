use std::sync::Arc;

use eframe::{AppCreator, EframeWinitApplication, NativeOptions, UserEvent};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

use super::app::DeviceRequestBridge;
use super::input::ObservedKeyboardInputs;

#[cfg(target_os = "linux")]
mod pinch;
#[cfg(target_os = "linux")]
mod transfer;

pub(crate) fn run_native_with_keyboard_observer(
    app_name: &str,
    native_options: NativeOptions,
    app_creator: AppCreator<'_>,
    observed_keyboard_inputs: ObservedKeyboardInputs,
    device_requests: Arc<DeviceRequestBridge>,
) -> eframe::Result {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    device_requests.start_watcher(event_loop.create_proxy());

    let eframe_app = eframe::create_native(app_name, native_options, app_creator, &event_loop);
    let mut app = KeyboardAwareApp::new(eframe_app, observed_keyboard_inputs, device_requests);
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::EventLoopExtWayland as _;
        use winit::raw_window_handle::HasDisplayHandle as _;
        app.observed_keyboard_inputs
            .set_wayland_backend(event_loop.is_wayland());
        app.pinch = pinch::NativePinch::start(&event_loop);
        if let Ok(handle) = event_loop.display_handle()
            && let winit::raw_window_handle::RawDisplayHandle::Wayland(display) = handle.as_raw()
        {
            let observed = app.observed_keyboard_inputs.clone();
            let clipboard =
                smithay_clipboard::native::Subscription::new(display.display.as_ptr() as usize, move || {
                    observed.wake_native_input();
                });
            app.observed_keyboard_inputs
                .native_worker_resetter(clipboard.resetter());
            app.clipboard = Some(clipboard);
            match transfer::TransferWorker::start(app.observed_keyboard_inputs.clone()) {
                Ok(worker) => app.transfer_worker = Some(worker),
                Err(error) => tracing::warn!(%error, "native image persistence worker unavailable"),
            }
        }
        // Keeps the platform display alive independently of the event loop,
        // so the bridge above still outlives it when a callback unwinds.
        app.display_handle = Some(event_loop.owned_display_handle());
    }
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct KeyboardAwareApp<'app> {
    inner: EframeWinitApplication<'app>,
    observed_keyboard_inputs: ObservedKeyboardInputs,
    device_requests: Arc<DeviceRequestBridge>,
    modifiers: egui::Modifiers,
    #[cfg(target_os = "linux")]
    image_paste_keys: std::collections::HashSet<(winit::window::WindowId, winit::keyboard::PhysicalKey)>,
    #[cfg(target_os = "linux")]
    clipboard: Option<smithay_clipboard::native::Subscription>,

    #[cfg(target_os = "linux")]
    transfer_worker: Option<transfer::TransferWorker>,

    native_window_liveness: NativeWindowLiveness,
    // `pinch` borrows the platform display through raw FFI, so it must be
    // declared before `display_handle`: fields drop in declaration order, and
    // that ordering is what keeps the display alive on the unwind path too,
    // where `exiting` never runs.
    #[cfg(target_os = "linux")]
    pinch: Option<pinch::NativePinch>,
    #[cfg(target_os = "linux")]
    display_handle: Option<winit::event_loop::OwnedDisplayHandle>,
}

impl<'app> KeyboardAwareApp<'app> {
    fn new(
        inner: EframeWinitApplication<'app>,
        observed_keyboard_inputs: ObservedKeyboardInputs,
        device_requests: Arc<DeviceRequestBridge>,
    ) -> Self {
        Self {
            inner,
            observed_keyboard_inputs,
            device_requests,
            modifiers: egui::Modifiers::default(),
            #[cfg(target_os = "linux")]
            image_paste_keys: std::collections::HashSet::new(),
            #[cfg(target_os = "linux")]
            clipboard: None,

            #[cfg(target_os = "linux")]
            transfer_worker: None,
            native_window_liveness: NativeWindowLiveness::default(),
            #[cfg(target_os = "linux")]
            pinch: None,
            #[cfg(target_os = "linux")]
            display_handle: None,
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

impl KeyboardAwareApp<'_> {
    /// Feed any pending native pinch into the normal window-event path. The
    /// Wayland bridge owns no thread, so this must run every loop iteration
    /// rather than only when a bridge thread wakes the loop.
    #[cfg(target_os = "linux")]
    fn drain_pinch(&mut self, event_loop: &ActiveEventLoop) {
        let transfers = self
            .clipboard
            .as_mut()
            .map_or_else(Vec::new, smithay_clipboard::native::Subscription::poll);
        for transfer in transfers {
            use smithay_clipboard::native::Event as TransferEvent;
            match transfer {
                TransferEvent::Reset => {
                    for surface in self.observed_keyboard_inputs.reset_native_transfers() {
                        self.inner.window_event(
                            event_loop,
                            winit::window::WindowId::from(surface),
                            WindowEvent::HoveredFileCancelled,
                        );
                    }
                    tracing::debug!("cancelled native file transfers after worker loss or backpressure");
                }
                TransferEvent::PasteCancelled { recipient } => {
                    self.observed_keyboard_inputs.cancel_native_paste_request(recipient);
                    tracing::debug!("native image paste transfer cancelled");
                }
                event @ (TransferEvent::Paste { .. } | TransferEvent::Drop { .. }) => {
                    self.queue_native_transfer(event_loop, event);
                }
                TransferEvent::Leave { surface } => {
                    if self.observed_keyboard_inputs.native_window(surface).is_some() {
                        self.inner.window_event(
                            event_loop,
                            winit::window::WindowId::from(surface),
                            WindowEvent::HoveredFileCancelled,
                        );
                    }
                }
                TransferEvent::Motion {
                    surface,
                    position,
                    entered,
                } => {
                    if self.observed_keyboard_inputs.native_window(surface).is_none() {
                        continue;
                    }
                    if entered {
                        self.inner.window_event(
                            event_loop,
                            winit::window::WindowId::from(surface),
                            WindowEvent::HoveredFile(std::path::PathBuf::new()),
                        );
                    }
                    self.forward_drop_position(event_loop, surface, position);
                }
            }
        }
        self.drain_native_completions(event_loop);
        let Some(pinch) = &mut self.pinch else {
            return;
        };
        for (window_id, event) in pinch.take_events() {
            self.inner.window_event(event_loop, window_id, event);
        }
    }
}

#[cfg(target_os = "linux")]
impl KeyboardAwareApp<'_> {
    fn forward_drop_position(&mut self, event_loop: &ActiveEventLoop, surface: u64, position: [f64; 2]) {
        if let Some((scale, _)) = self.observed_keyboard_inputs.native_window(surface) {
            self.inner.window_event(
                event_loop,
                winit::window::WindowId::from(surface),
                WindowEvent::CursorMoved {
                    device_id: winit::event::DeviceId::dummy(),
                    position: winit::dpi::PhysicalPosition::new(position[0] * scale, position[1] * scale),
                },
            );
        }
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
        #[cfg(target_os = "linux")]
        if let Some(pinch) = &mut self.pinch {
            pinch.observe_window(window_id, &event);
        }
        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::Destroyed) {
            self.observed_keyboard_inputs.forget_native_window(u64::from(window_id));
            self.image_paste_keys.retain(|(window, _)| *window != window_id);
        }
        #[cfg(target_os = "linux")]
        if !matches!(event, WindowEvent::Destroyed) {
            self.observed_keyboard_inputs.native_window_seen(u64::from(window_id));
        }
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

        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::KeyboardInput { .. }) {
            self.drain_pinch(event_loop);
        }

        #[cfg(target_os = "linux")]
        {
            let surface = u64::from(window_id);
            if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = &event {
                self.observed_keyboard_inputs
                    .native_window_scale(surface, *scale_factor);
            }
            if let WindowEvent::Focused(focused) = &event {
                self.observed_keyboard_inputs.native_focus(surface, *focused);
            }
            if matches!(event, WindowEvent::Focused(false)) {
                self.image_paste_keys.retain(|(window, _)| *window != window_id);
            }
            if let WindowEvent::KeyboardInput {
                event: key,
                is_synthetic: false,
                ..
            } = &event
            {
                let key_id = (window_id, key.physical_key);
                if self.image_paste_keys.contains(&key_id) {
                    if key.state == ElementState::Released {
                        self.image_paste_keys.remove(&key_id);
                    }
                    return;
                }
                let paste = key.state == ElementState::Pressed
                    && !key.repeat
                    && !self.modifiers.alt
                    && ((matches!(&key.logical_key, winit::keyboard::Key::Character(text) if text.eq_ignore_ascii_case("v"))
                        && self.modifiers.ctrl)
                        || (key.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Insert)
                            && self.modifiers.shift
                            && !self.modifiers.ctrl));
                if paste && let Some(recipient) = self.observed_keyboard_inputs.native_paste_request(surface) {
                    if self
                        .clipboard
                        .as_mut()
                        .is_some_and(|bridge| bridge.request_paste(surface, recipient))
                    {
                        self.image_paste_keys.insert(key_id);
                        return;
                    }
                    self.observed_keyboard_inputs.cancel_native_paste_request(recipient);
                }
            }
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
        #[cfg(target_os = "linux")]
        self.drain_pinch(event_loop);
        // Not a repaint. Claiming here runs while Wayland is withholding
        // `RedrawRequested` for an unpresented surface.
        if crate::app::is_device_queue_wake(&event) {
            self.device_requests.poll_on_ui_thread();
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
        #[cfg(target_os = "linux")]
        self.drain_pinch(event_loop);
        self.inner.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        if self.native_window_liveness.is_root_destroyed() {
            return;
        }
        self.inner.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        // Release the pinch bridge while the display it adopted is still
        // alive; winit tears that down once this call returns.
        #[cfg(target_os = "linux")]
        {
            self.pinch = None;
            self.transfer_worker = None;
        }
        // `exiting` is the one callback that remains safe and necessary after
        // the native root handle is gone: eframe uses it to run `App::on_exit`
        // and destroy application state. Skipping it lets browser-driver
        // threads outlive a macOS Cmd-Q long enough to orphan Chrome.
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
