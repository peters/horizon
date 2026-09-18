use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;

use eframe::UserEvent;
use winit::event::{DeviceId, TouchPhase, WindowEvent};
use winit::event_loop::EventLoop;
use winit::platform::x11::EventLoopExtX11;
use winit::window::WindowId;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, GesturePinchBeginEvent};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, thiserror::Error)]
enum PinchError {
    #[error(transparent)]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error(transparent)]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error(transparent)]
    Reply(#[from] x11rb::errors::ReplyError),
    #[error(transparent)]
    Id(#[from] x11rb::errors::ReplyOrIdError),
    #[error(transparent)]
    Thread(#[from] std::io::Error),
}

pub(super) struct NativePinch {
    connection: Arc<RustConnection>,
    wake_window: u32,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    incoming: Option<Receiver<BridgeInput>>,
    state: PinchState,
}

enum BridgeInput {
    Pinch(GesturePinchBeginEvent),
    Focus { window: u32, focused: bool },
}

#[derive(Default)]
struct PinchState {
    windows: HashMap<u32, (WindowId, bool)>,
    sequences: HashMap<(u32, u16), PinchSequence>,
}

impl NativePinch {
    pub(super) fn start(event_loop: &EventLoop<UserEvent>) -> Option<Self> {
        if !event_loop.is_x11() {
            return None;
        }
        match Self::connect(event_loop) {
            Ok(bridge) => bridge,
            Err(error) => {
                tracing::warn!(%error, "native X11 pinch input unavailable");
                None
            }
        }
    }

    fn connect(event_loop: &EventLoop<UserEvent>) -> Result<Option<Self>, PinchError> {
        let (connection, screen) = x11rb::connect(None)?;
        let version = connection.xinput_xi_query_version(2, 4)?.reply()?;
        if (version.major_version, version.minor_version) < (2, 4) {
            tracing::debug!("native pinch needs XInput 2.4");
            return Ok(None);
        }
        let wake_window = connection.generate_id()?;
        connection
            .create_window(
                0,
                wake_window,
                connection.setup().roots[screen].root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::default(),
            )?
            .check()?;
        let connection = Arc::new(connection);
        let stopped = Arc::new(AtomicBool::new(false));
        let (sender, incoming) = sync_channel(256);
        let proxy = event_loop.create_proxy();
        let reader = Arc::clone(&connection);
        let worker_stopped = Arc::clone(&stopped);
        let worker = std::thread::Builder::new().name("x11-pinch".into()).spawn(move || {
            while let Ok(event) = reader.wait_for_event() {
                if worker_stopped.load(Ordering::Acquire) {
                    break;
                }
                let input = match event {
                    Event::XinputGesturePinchBegin(event)
                    | Event::XinputGesturePinchUpdate(event)
                    | Event::XinputGesturePinchEnd(event) => BridgeInput::Pinch(event),
                    Event::XinputFocusIn(event) => BridgeInput::Focus {
                        window: event.event,
                        focused: true,
                    },
                    Event::XinputFocusOut(event) => BridgeInput::Focus {
                        window: event.event,
                        focused: false,
                    },
                    _ => continue,
                };
                // Backpressure preserves begin/end/focus ordering during UI stalls.
                if sender.send(input).is_err() {
                    break;
                }
                let _ = proxy.send_event(UserEvent::RequestRepaint {
                    viewport_id: egui::ViewportId::ROOT,
                    when: Instant::now(),
                    cumulative_pass_nr: 0,
                });
            }
        })?;
        tracing::info!("native X11 pinch input enabled");
        Ok(Some(Self {
            connection,
            wake_window,
            stopped,
            worker: Some(worker),
            incoming: Some(incoming),
            state: PinchState::default(),
        }))
    }

    pub(super) fn observe_window(&mut self, id: WindowId, event: &WindowEvent) {
        // winit's X11 WindowId is the XID; never use this conversion on Wayland.
        let Ok(window) = u32::try_from(u64::from(id)) else {
            return;
        };
        if matches!(event, WindowEvent::Destroyed) {
            self.state.windows.remove(&window);
            self.state.sequences.retain(|(target, _), _| *target != window);
            return;
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = self.state.windows.entry(window) {
            // x11rb 0.14 parses the gesture events but lacks their mask constants.
            let mask: u32 = (1 << xinput::GESTURE_PINCH_BEGIN_EVENT)
                | (1 << xinput::GESTURE_PINCH_UPDATE_EVENT)
                | (1 << xinput::GESTURE_PINCH_END_EVENT);
            let result = self
                .connection
                .xinput_xi_select_events(
                    window,
                    &[xinput::EventMask {
                        deviceid: xinput::Device::ALL_MASTER.into(),
                        mask: vec![
                            xinput::XIEventMask::from(mask)
                                | xinput::XIEventMask::FOCUS_IN
                                | xinput::XIEventMask::FOCUS_OUT,
                        ],
                    }],
                )
                .map_err(PinchError::from)
                .and_then(|cookie| cookie.check().map_err(PinchError::from));
            if let Err(error) = result {
                tracing::debug!(%error, window, "could not select window pinch events");
                return;
            }
            let focused = self
                .connection
                .get_input_focus()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.focus == window);
            entry.insert((id, focused));
        }
    }

    pub(super) fn take_events(&mut self) -> Vec<(WindowId, WindowEvent)> {
        self.incoming
            .as_ref()
            .map_or_else(Vec::new, |incoming| self.state.route(incoming.try_iter()))
    }
}

impl PinchState {
    fn route(&mut self, incoming: impl IntoIterator<Item = BridgeInput>) -> Vec<(WindowId, WindowEvent)> {
        let mut output = Vec::new();
        for input in incoming {
            let event = match input {
                BridgeInput::Pinch(event) => event,
                BridgeInput::Focus { window, focused } => {
                    if let Some((id, active)) = self.windows.get_mut(&window) {
                        *active = focused;
                        output.retain(|(target, _)| target != id);
                    }
                    self.sequences.retain(|(target, _), _| *target != window);
                    continue;
                }
            };
            let Some(&(id, true)) = self.windows.get(&event.event) else {
                continue;
            };
            let key = (event.event, event.sourceid);
            let sequence = self.sequences.entry(key).or_default();
            if let Some((delta, phase)) = sequence.update(event.event_type, event.scale) {
                tracing::debug!(delta, ?phase, "native X11 pinch");
                output.push((
                    id,
                    WindowEvent::PinchGesture {
                        device_id: DeviceId::dummy(),
                        delta,
                        phase,
                    },
                ));
            }
            if event.event_type == xinput::GESTURE_PINCH_END_EVENT {
                self.sequences.remove(&key);
            }
        }
        output
    }
}

impl Drop for NativePinch {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Release a producer blocked on a full queue before joining it.
        self.incoming.take();
        // An unmapped, connection-owned window wakes the blocking reader without
        // grabbing input, polling the desktop, or touching any application window.
        let event = ClientMessageEvent::new(32, self.wake_window, AtomEnum::WM_NAME, [0; 5]);
        let _ = self
            .connection
            .send_event(false, self.wake_window, EventMask::NO_EVENT, event);
        let _ = self.connection.flush();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = self.connection.destroy_window(self.wake_window);
        let _ = self.connection.flush();
    }
}

#[derive(Default)]
struct PinchSequence {
    scale: Option<f64>,
}

impl PinchSequence {
    fn update(&mut self, event_type: u16, fixed_scale: i32) -> Option<(f64, TouchPhase)> {
        let scale = f64::from(fixed_scale) / 65536.0;
        match event_type {
            xinput::GESTURE_PINCH_BEGIN_EVENT => {
                self.scale = Some(1.0);
                None
            }
            xinput::GESTURE_PINCH_UPDATE_EVENT if scale > 0.0 => {
                let previous = self.scale?;
                self.scale = Some(scale);
                // XI2 reports cumulative scale; egui-winit exponentiates its delta.
                Some(((scale / previous).ln(), TouchPhase::Moved))
            }
            xinput::GESTURE_PINCH_END_EVENT => {
                self.scale = None;
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture(window: u32, event_type: u16, scale: i32) -> BridgeInput {
        BridgeInput::Pinch(GesturePinchBeginEvent {
            event: window,
            sourceid: 12,
            event_type,
            scale,
            ..Default::default()
        })
    }

    #[test]
    fn ordered_focus_changes_discard_old_motion_and_require_a_new_contact() {
        let mut state = PinchState::default();
        state.windows.insert(1, (WindowId::from(1), true));
        let output = state.route([
            gesture(1, xinput::GESTURE_PINCH_BEGIN_EVENT, 65536),
            gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 131_072),
            BridgeInput::Focus {
                window: 1,
                focused: false,
            },
            gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 196_608),
            BridgeInput::Focus {
                window: 1,
                focused: true,
            },
            gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 262_144),
        ]);
        assert!(output.is_empty());
        let output = state.route([
            gesture(1, xinput::GESTURE_PINCH_BEGIN_EVENT, 65536),
            gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 81920),
        ]);
        assert_eq!(output.len(), 1);
        let WindowEvent::PinchGesture { delta, .. } = output[0].1 else {
            panic!("expected pinch")
        };
        assert!((delta.exp() - 1.25).abs() < 0.0001);
    }

    #[test]
    fn native_windows_keep_separate_gestures_and_closed_targets_are_ignored() {
        let mut state = PinchState::default();
        for id in [1, 2] {
            state.windows.insert(id, (WindowId::from(u64::from(id)), true));
        }
        let output = state.route([
            gesture(1, xinput::GESTURE_PINCH_BEGIN_EVENT, 65536),
            gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 131_072),
            gesture(2, xinput::GESTURE_PINCH_UPDATE_EVENT, 131_072),
        ]);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].0, WindowId::from(1));
        state.windows.remove(&1);
        assert!(
            state
                .route([gesture(1, xinput::GESTURE_PINCH_UPDATE_EVENT, 196_608)])
                .is_empty()
        );
    }

    #[test]
    fn cumulative_scale_becomes_incremental_zoom_and_reverses() {
        let mut sequence = PinchSequence::default();
        assert!(sequence.update(xinput::GESTURE_PINCH_BEGIN_EVENT, 65536).is_none());
        let mut zoom = 1.0;
        for scale in [81920, 98304, 49152] {
            let (delta, _) = sequence
                .update(xinput::GESTURE_PINCH_UPDATE_EVENT, scale)
                .expect("active pinch");
            zoom *= delta.exp();
            assert!((zoom - f64::from(scale) / 65536.0).abs() < 0.0001);
        }
        assert!(sequence.update(xinput::GESTURE_PINCH_END_EVENT, 49152).is_none());
    }

    #[test]
    fn updates_need_a_begin_and_invalid_scale_does_not_poison_zoom() {
        let mut sequence = PinchSequence::default();
        assert!(sequence.update(xinput::GESTURE_PINCH_UPDATE_EVENT, 65536).is_none());
        sequence.update(xinput::GESTURE_PINCH_BEGIN_EVENT, 65536);
        assert!(sequence.update(xinput::GESTURE_PINCH_UPDATE_EVENT, 0).is_none());
        assert!(sequence.update(xinput::GESTURE_PINCH_UPDATE_EVENT, -1).is_none());
        let (delta, _) = sequence
            .update(xinput::GESTURE_PINCH_UPDATE_EVENT, 131_072)
            .expect("valid pinch");
        assert!((delta.exp() - 2.0).abs() < 0.0001);
        sequence.update(xinput::GESTURE_PINCH_END_EVENT, 131_072);
        assert!(sequence.update(xinput::GESTURE_PINCH_UPDATE_EVENT, 131_072).is_none());
    }
}
