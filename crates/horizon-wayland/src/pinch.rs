//! Wayland trackpad pinch.
//!
//! winit 0.30 never binds `zwp_pointer_gestures_v1`, so pinch never reaches the
//! application on Wayland. Unlike X11 we cannot watch another client's surfaces
//! from a second connection — Wayland only delivers gestures over a client's
//! own surfaces. Instead we adopt the toolkit's `wl_display` through
//! libwayland's foreign-display entry point and drive a private event queue on
//! it, which makes this the *same* client and puts its surfaces in scope.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};

/// One incremental pinch step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pinch {
    /// The `wl_surface` proxy pointer the gesture is over. winit derives its
    /// Wayland `WindowId` from exactly this value, so callers can compare the
    /// two directly instead of keeping a surface map.
    pub surface: u64,
    /// Natural log of the scale ratio since the previous step, which is the
    /// form `WindowEvent::PinchGesture` carries.
    pub delta: f64,
}

/// A background bridge feeding pinch gestures from the compositor.
pub struct PinchBridge {
    connection: Connection,
    handle: QueueHandle<Gestures>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    incoming: Option<Receiver<Pinch>>,
}

impl PinchBridge {
    /// Adopt `display`'s Wayland connection and start listening for pinch.
    ///
    /// Returns `None` when the handle is not Wayland, the compositor offers no
    /// pointer gestures, or the seat has no pointer. `wake` is called from the
    /// bridge thread whenever gestures are ready, so the caller can nudge its
    /// event loop into draining [`PinchBridge::take`].
    ///
    /// The display must outlive the returned bridge.
    pub fn start(display: &impl HasDisplayHandle, wake: impl Fn() + Send + 'static) -> Option<Self> {
        let handle = display.display_handle().ok()?;
        let RawDisplayHandle::Wayland(display) = handle.as_raw() else {
            return None;
        };
        // SAFETY: the caller guarantees the display outlives this bridge, and a
        // foreign-display backend never disconnects the display it adopted.
        #[allow(unsafe_code)]
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        match Self::spawn(connection, Box::new(wake)) {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                tracing::warn!(%error, "native Wayland pinch input unavailable");
                None
            }
        }
    }

    fn spawn(connection: Connection, wake: Box<dyn Fn() + Send>) -> Result<Self, PinchError> {
        // A private queue keeps our proxies off the toolkit's queue: libwayland
        // routes every event to the queue its proxy was created on, so neither
        // side can swallow the other's events.
        let (globals, mut queue): (_, EventQueue<Gestures>) = registry_queue_init(&connection)?;
        let handle = queue.handle();
        let (sender, incoming) = sync_channel(256);
        let mut state = Gestures {
            protocol: globals.bind(&handle, 1..=3, ())?,
            pinch: None,
            sender,
            sequence: PinchSequence::default(),
        };
        // A second `wl_seat` binding is legal and its `wl_pointer` receives the
        // same focus and motion events as the toolkit's own. The pinch object
        // is created once the seat announces a pointer, since asking for one
        // without that capability is a protocol error.
        let seat: wl_seat::WlSeat = globals.bind(&handle, 1..=9, ())?;
        queue.roundtrip(&mut state)?;
        if state.pinch.is_none() {
            return Err(PinchError::NoPointer);
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = std::thread::Builder::new()
            .name("wayland-pinch".into())
            .spawn(move || {
                let _keep_alive = seat;
                while !worker_stopped.load(Ordering::Acquire) {
                    if queue.blocking_dispatch(&mut state).is_err() {
                        break;
                    }
                    wake();
                }
            })?;
        tracing::info!("native Wayland pinch input enabled");
        Ok(Self {
            connection,
            handle,
            stopped,
            worker: Some(worker),
            incoming: Some(incoming),
        })
    }

    /// Drain the pinch steps that arrived since the last call.
    pub fn take(&mut self) -> Vec<Pinch> {
        self.incoming
            .as_ref()
            .map_or_else(Vec::new, |incoming| incoming.try_iter().collect())
    }
}

impl Drop for PinchBridge {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Release a producer blocked on a full queue before joining it.
        self.incoming.take();
        // The worker blocks in `wl_display_dispatch_queue`. A sync on our own
        // queue makes the compositor answer with an event it wakes up for,
        // after which it observes the stop flag.
        self.connection.display().sync(&self.handle, ());
        let _ = self.connection.flush();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Gestures {
    protocol: ZwpPointerGesturesV1,
    pinch: Option<ZwpPointerGesturePinchV1>,
    sender: SyncSender<Pinch>,
    sequence: PinchSequence,
}

/// The gesture in flight, tracked apart from the protocol objects so the scale
/// conversion stays testable on its own.
#[derive(Default)]
struct PinchSequence {
    /// Cumulative scale since `begin`, absent between gestures.
    scale: Option<f64>,
    surface: Option<u64>,
}

impl Dispatch<wl_seat::WlSeat, ()> for Gestures {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _connection: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        if capabilities.contains(wl_seat::Capability::Pointer) && state.pinch.is_none() {
            let pointer = seat.get_pointer(handle, ());
            state.pinch = Some(state.protocol.get_pinch_gesture(&pointer, handle, ()));
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for Gestures {
    fn event(
        state: &mut Self,
        _proxy: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { surface, .. } => {
                state.sequence = PinchSequence {
                    scale: Some(1.0),
                    surface: Some(surface.id().as_ptr() as u64),
                };
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                if let Some(pinch) = state.sequence.advance(scale) {
                    // A full queue means the UI is stalled; dropping the step
                    // beats blocking the protocol thread.
                    let _ = state.sender.try_send(pinch);
                }
            }
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => {
                state.sequence = PinchSequence::default();
            }
            _ => {}
        }
    }
}

impl PinchSequence {
    /// `zwp_pointer_gesture_pinch_v1` reports cumulative scale, while winit's
    /// pinch delta is exponentiated by its consumers, so emit the log ratio.
    fn advance(&mut self, scale: f64) -> Option<Pinch> {
        if scale <= 0.0 {
            return None;
        }
        let previous = self.scale?;
        self.scale = Some(scale);
        Some(Pinch {
            surface: self.surface?,
            delta: (scale / previous).ln(),
        })
    }
}

/// Every other event on these objects belongs to the toolkit's own copies.
macro_rules! ignore_events {
    ($($proxy:ty $(: $data:ty)?),* $(,)?) => {$(
        impl Dispatch<$proxy, ignore_events!(@data $($data)?)> for Gestures {
            fn event(
                _state: &mut Self,
                _proxy: &$proxy,
                _event: <$proxy as Proxy>::Event,
                _data: &ignore_events!(@data $($data)?),
                _connection: &Connection,
                _handle: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
    (@data) => { () };
    (@data $data:ty) => { $data };
}

ignore_events!(
    wl_pointer::WlPointer,
    wl_surface::WlSurface,
    wl_callback::WlCallback,
    ZwpPointerGesturesV1,
    wl_registry::WlRegistry: GlobalListContents,
);

#[derive(Debug, thiserror::Error)]
enum PinchError {
    #[error(transparent)]
    Global(#[from] wayland_client::globals::GlobalError),
    #[error(transparent)]
    Bind(#[from] wayland_client::globals::BindError),
    #[error(transparent)]
    Dispatch(#[from] wayland_client::DispatchError),
    #[error(transparent)]
    Thread(#[from] std::io::Error),
    #[error("the seat announced no pointer")]
    NoPointer,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence() -> PinchSequence {
        PinchSequence {
            scale: Some(1.0),
            surface: Some(7),
        }
    }

    #[test]
    fn cumulative_scale_becomes_incremental_zoom_and_reverses() {
        let mut state = sequence();
        let mut zoom: f64 = 1.0;
        for scale in [1.25, 1.5, 0.75] {
            zoom *= state.advance(scale).expect("active pinch").delta.exp();
            assert!((zoom - scale).abs() < 0.0001);
        }
    }

    #[test]
    fn updates_need_a_begin_and_invalid_scale_does_not_poison_zoom() {
        let mut state = PinchSequence::default();
        assert!(state.advance(1.25).is_none());
        let mut state = sequence();
        assert!(state.advance(0.0).is_none());
        assert!(state.advance(-1.0).is_none());
        let pinch = state.advance(2.0).expect("valid pinch");
        assert!((pinch.delta.exp() - 2.0).abs() < 0.0001);
        assert_eq!(pinch.surface, 7);
    }
}
