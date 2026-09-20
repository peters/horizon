//! Wayland trackpad pinch.
//!
//! winit 0.30 never binds `zwp_pointer_gestures_v1`, so pinch never reaches the
//! application on Wayland. Unlike X11 we cannot watch another client's surfaces
//! from a second connection — Wayland only delivers gestures over a client's
//! own surfaces. Instead we adopt the toolkit's `wl_display` through
//! libwayland's foreign-display entry point and drive a private event queue on
//! it, which makes this the *same* client and puts its surfaces in scope.
//!
//! The bridge owns no thread and never blocks. libwayland demultiplexes a read
//! to every queue of the connection, so the toolkit's own event loop feeds our
//! queue as a side effect of its normal polling and [`PinchBridge::poll`] only
//! drains what is already pending.
//!
//! libwayland requires the adopted `wl_display` to outlive the backend and
//! every clone of it, and that cannot be expressed in the type system here:
//! winit's `run_app` takes the event loop by value, so no borrow of it can be
//! held across the run. [`PinchBridge::start`] is therefore `unsafe`, and the
//! lifetime proof belongs to the integration layer that owns both ends.

use std::collections::HashMap;

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

/// A bridge feeding trackpad pinch gestures from the compositor.
pub struct PinchBridge {
    connection: Connection,
    queue: EventQueue<Gestures>,
    state: Gestures,
}

impl PinchBridge {
    /// Adopt `display`'s Wayland connection and start listening for pinch.
    ///
    /// Returns `None` when the handle is not Wayland, the compositor offers no
    /// pointer gestures, or the seat has no pointer.
    ///
    /// Call [`PinchBridge::poll`] from the display owner's event loop.
    ///
    /// # Safety
    ///
    /// `display`'s `wl_display` must outlive the returned bridge. The borrow
    /// only proves it is alive for this call, and libwayland requires it to
    /// outlive the adopted backend and every clone of it, so the caller must
    /// drop the bridge before the display is torn down.
    #[allow(unsafe_code)]
    pub unsafe fn start(display: &impl HasDisplayHandle) -> Option<Self> {
        let handle = display.display_handle().ok()?;
        let RawDisplayHandle::Wayland(display) = handle.as_raw() else {
            return None;
        };
        // SAFETY: `display_handle()` yields a live `wl_display`, and this
        // function's own contract makes the caller responsible for keeping it
        // alive for as long as the bridge exists. A foreign-display backend
        // never disconnects the display it adopted.
        #[allow(unsafe_code)]
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        match Self::bind(connection) {
            Ok(bridge) => {
                tracing::info!("native Wayland pinch input enabled");
                Some(bridge)
            }
            Err(error) => {
                tracing::warn!(%error, "native Wayland pinch input unavailable");
                None
            }
        }
    }

    fn bind(connection: Connection) -> Result<Self, PinchError> {
        // A private queue keeps our proxies off the toolkit's queue: libwayland
        // routes every event to the queue its proxy was created on, so neither
        // side can swallow the other's events.
        let (globals, mut queue): (_, EventQueue<Gestures>) = registry_queue_init(&connection)?;
        let handle = queue.handle();
        let mut state = Gestures {
            protocol: globals.bind(&handle, 1..=3, ())?,
            seats: HashMap::new(),
            pending: Vec::new(),
        };
        // Bind every advertised seat, not just the first: a trackpad can belong
        // to any of them. `Dispatch` then keeps pointer and gesture state keyed
        // by the seat's registry name, and the registry callback below follows
        // seats added or removed later.
        let registry = globals.registry();
        globals.contents().with_list(|globals| {
            for global in globals {
                if global.interface == wl_seat::WlSeat::interface().name {
                    state.bind_seat(registry, global.name, global.version, &handle);
                }
            }
        });
        // A second `wl_seat` binding is legal and its `wl_pointer` receives the
        // same focus and motion events as the toolkit's own. Each seat's pinch
        // object is created once that seat announces a pointer, since asking
        // for one without the capability is a protocol error.
        queue.roundtrip(&mut state)?;
        // A session can start with no pointer at all and gain one when a
        // trackpad is plugged in, so keep the bridge and let the seat callback
        // pick it up. Only a compositor without the gesture protocol (the bind
        // above) makes pinch permanently unavailable.
        state.pending.clear();
        Ok(Self {
            connection,
            queue,
            state,
        })
    }

    /// Drain the pinch steps that arrived since the last call.
    ///
    /// Never blocks: it dispatches only what the toolkit's own socket reads
    /// have already queued for us.
    pub fn poll(&mut self) -> Vec<Pinch> {
        let _ = self.connection.flush();
        if self.queue.dispatch_pending(&mut self.state).is_err() {
            return Vec::new();
        }
        std::mem::take(&mut self.state.pending)
    }
}

struct Gestures {
    protocol: ZwpPointerGesturesV1,
    /// Every bound seat, keyed by its registry name so the registry callback
    /// can drop one when its global goes away.
    seats: HashMap<u32, Seat>,
    pending: Vec<Pinch>,
}

/// One seat's pointer, gesture object and the gesture currently in flight on
/// it. Seats are independent, so their gestures cannot share state.
struct Seat {
    proxy: wl_seat::WlSeat,
    /// Kept so both objects can be released when the seat drops its pointer.
    pointer: Option<wl_pointer::WlPointer>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    sequence: PinchSequence,
}

impl Gestures {
    fn bind_seat(&mut self, registry: &wl_registry::WlRegistry, name: u32, version: u32, handle: &QueueHandle<Self>) {
        if self.seats.contains_key(&name) {
            return;
        }
        let seat: wl_seat::WlSeat = registry.bind(name, version.min(9), handle, name);
        self.seats.insert(
            name,
            Seat {
                proxy: seat,
                pointer: None,
                pinch: None,
                sequence: PinchSequence::default(),
            },
        );
    }

    fn drop_seat(&mut self, name: u32) {
        if let Some(mut seat) = self.seats.remove(&name) {
            seat.release_pointer();
            if seat.proxy.version() >= wl_seat::REQ_RELEASE_SINCE {
                seat.proxy.release();
            }
        }
    }

    fn acquire_pointer(&mut self, name: u32, handle: &QueueHandle<Self>) {
        let protocol = self.protocol.clone();
        let Some(seat) = self.seats.get_mut(&name) else {
            return;
        };
        if seat.pinch.is_some() {
            return;
        }
        let pointer = seat.proxy.get_pointer(handle, name);
        seat.pinch = Some(protocol.get_pinch_gesture(&pointer, handle, name));
        seat.pointer = Some(pointer);
    }
}

impl Seat {
    /// A seat can drop its pointer across a device or seat reconfiguration.
    /// The protocol forbids a pre-removal pointer from emitting again once a
    /// version 5+ seat regains the capability, so both objects have to go and
    /// be recreated rather than reused.
    fn release_pointer(&mut self) {
        if let Some(pinch) = self.pinch.take() {
            pinch.destroy();
        }
        if let Some(pointer) = self.pointer.take()
            && pointer.version() >= wl_pointer::REQ_RELEASE_SINCE
        {
            pointer.release();
        }
        self.sequence.end();
    }
}

/// The gesture in flight, tracked apart from the protocol objects so the scale
/// conversion stays testable on its own.
#[derive(Default)]
struct PinchSequence {
    /// Cumulative scale of the last step handed to the caller, absent between
    /// gestures.
    scale: Option<f64>,
    surface: Option<u64>,
}

impl PinchSequence {
    fn begin(&mut self, surface: u64) {
        self.scale = Some(1.0);
        self.surface = Some(surface);
    }

    fn end(&mut self) {
        *self = Self::default();
    }

    /// `zwp_pointer_gesture_pinch_v1` reports cumulative scale, while winit's
    /// pinch delta is exponentiated by its consumers, so emit the log of the
    /// ratio against the last step actually produced. Advancing the baseline
    /// only when a step is produced keeps a rejected scale coalesced into the
    /// next one instead of dropping it out of the gesture.
    fn advance(&mut self, scale: f64) -> Option<Pinch> {
        if scale <= 0.0 {
            return None;
        }
        let previous = self.scale?;
        let surface = self.surface?;
        self.scale = Some(scale);
        Some(Pinch {
            surface,
            delta: (scale / previous).ln(),
        })
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for Gestures {
    fn event(
        state: &mut Self,
        _seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        name: &u32,
        _connection: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        if capabilities.contains(wl_seat::Capability::Pointer) {
            state.acquire_pointer(*name, handle);
        } else if let Some(seat) = state.seats.get_mut(name) {
            seat.release_pointer();
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, u32> for Gestures {
    fn event(
        state: &mut Self,
        _proxy: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        name: &u32,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.get_mut(name) else {
            return;
        };
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { surface, .. } => {
                seat.sequence.begin(surface.id().as_ptr() as u64);
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                state.pending.extend(seat.sequence.advance(scale));
            }
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => seat.sequence.end(),
            _ => {}
        }
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
    wl_pointer::WlPointer: u32,
    wl_surface::WlSurface,
    wl_callback::WlCallback,
    ZwpPointerGesturesV1,
);

/// Follow seats appearing and disappearing at runtime, so a replaced seat is
/// picked up instead of leaving the bridge bound to a global that is gone.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Gestures {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == wl_seat::WlSeat::interface().name => {
                state.bind_seat(registry, name, version, handle);
            }
            wl_registry::Event::GlobalRemove { name } => state.drop_seat(name),
            _ => {}
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum PinchError {
    #[error(transparent)]
    Global(#[from] wayland_client::globals::GlobalError),
    #[error(transparent)]
    Bind(#[from] wayland_client::globals::BindError),
    #[error(transparent)]
    Dispatch(#[from] wayland_client::DispatchError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence() -> PinchSequence {
        let mut sequence = PinchSequence::default();
        sequence.begin(7);
        sequence
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
        state.end();
        assert!(state.advance(2.5).is_none());
    }

    #[test]
    fn a_rejected_scale_coalesces_into_the_next_step() {
        // An update that yields no step must not move the baseline, or the
        // gesture would silently lose that part of its scale.
        let mut state = sequence();
        assert!(state.advance(0.0).is_none());
        let pinch = state.advance(1.5).expect("valid pinch");
        assert!((pinch.delta.exp() - 1.5).abs() < 0.0001);
    }
}
