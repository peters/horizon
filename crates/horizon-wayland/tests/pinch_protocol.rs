//! Drives [`PinchBridge`] end to end against an in-process compositor.
//!
//! The compositor speaks the real protocol over a Unix socket, and the client
//! side is a libwayland display the test owns the way winit owns its own. The
//! bridge adopts that display through the same `unsafe` entry point Horizon
//! uses, and pinch steps have to arrive in [`PinchBridge::poll`] purely as a
//! side effect of the "toolkit" reading its own queue, which is the property
//! the bridge's thread-free design depends on.

#![cfg(target_os = "linux")]
#![deny(unsafe_code)]

use std::ffi::c_void;
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use horizon_wayland::{Pinch, PinchBridge};
use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, RawDisplayHandle, WaylandDisplayHandle};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::pointer_gestures::zv1::server::{
    zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1,
    zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
    zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1,
    zwp_pointer_gestures_v1::{self, ZwpPointerGesturesV1},
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::{
    wl_compositor as server_compositor, wl_pointer, wl_region, wl_seat, wl_surface as server_surface,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle as ServerHandle, GlobalDispatch, New, Resource,
};

// ---------------------------------------------------------------------------
// Compositor
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Features {
    gestures: bool,
    pointer_at_start: bool,
}

enum Command {
    /// Announce a pointer on every bound seat.
    GrantPointer,
    /// One complete gesture over the client's surface, as cumulative scales.
    Pinch(Vec<f64>),
    Stop,
}

#[derive(Default)]
struct Compositor {
    pointer_granted: bool,
    seats: Vec<wl_seat::WlSeat>,
    surface: Option<server_surface::WlSurface>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    serial: u32,
}

impl Compositor {
    fn capabilities(&self) -> wl_seat::Capability {
        if self.pointer_granted {
            wl_seat::Capability::Pointer
        } else {
            wl_seat::Capability::empty()
        }
    }

    /// `true` once the gesture was sent; `false` while the client has not yet
    /// created the objects it needs.
    fn pinch(&mut self, scales: &[f64]) -> bool {
        let (Some(pinch), Some(surface)) = (&self.pinch, &self.surface) else {
            return false;
        };
        self.serial += 1;
        pinch.begin(self.serial, 0, surface, 2);
        for scale in scales {
            pinch.update(0, 0.0, 0.0, *scale, 0.0);
        }
        self.serial += 1;
        pinch.end(self.serial, 0, 0);
        true
    }
}

struct TestClient;

impl ClientData for TestClient {
    fn initialized(&self, _client: ClientId) {}
    fn disconnected(&self, _client: ClientId, _reason: DisconnectReason) {}
}

/// A compositor on its own thread, commanded over a channel so the test
/// thread can play the toolkit's blocking role.
struct Server {
    commands: Sender<Command>,
    acks: Receiver<bool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(features: Features) -> (Self, UnixStream) {
        let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
        let (commands, command_rx) = channel();
        let (ack_tx, acks) = channel();
        let thread = std::thread::spawn(move || serve(features, server_stream, &command_rx, &ack_tx));
        (
            Self {
                commands,
                acks,
                thread: Some(thread),
            },
            client_stream,
        )
    }

    fn command(&self, command: Command) -> bool {
        self.commands.send(command).expect("compositor alive");
        self.acks.recv_timeout(Duration::from_secs(5)).expect("compositor ack")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(features: Features, stream: UnixStream, commands: &Receiver<Command>, acks: &Sender<bool>) {
    let mut display: Display<Compositor> = Display::new().expect("server display");
    let mut handle = display.handle();
    handle.create_global::<Compositor, server_compositor::WlCompositor, ()>(4, ());
    handle.create_global::<Compositor, wl_seat::WlSeat, ()>(7, ());
    if features.gestures {
        handle.create_global::<Compositor, ZwpPointerGesturesV1, ()>(3, ());
    }
    handle
        .insert_client(stream, Arc::new(TestClient))
        .expect("insert client");
    let mut state = Compositor {
        pointer_granted: features.pointer_at_start,
        ..Compositor::default()
    };
    loop {
        let _ = display.dispatch_clients(&mut state);
        let _ = display.flush_clients();
        match commands.recv_timeout(Duration::from_millis(1)) {
            Ok(Command::GrantPointer) => {
                state.pointer_granted = true;
                for seat in &state.seats {
                    seat.capabilities(state.capabilities());
                }
                let _ = display.flush_clients();
                let _ = acks.send(true);
            }
            Ok(Command::Pinch(scales)) => {
                let sent = state.pinch(&scales);
                let _ = display.flush_clients();
                let _ = acks.send(sent);
            }
            Ok(Command::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

impl GlobalDispatch<server_compositor::WlCompositor, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &ServerHandle,
        _client: &Client,
        resource: New<server_compositor::WlCompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<server_compositor::WlCompositor, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &server_compositor::WlCompositor,
        request: server_compositor::Request,
        _data: &(),
        _handle: &ServerHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            server_compositor::Request::CreateSurface { id } => {
                state.surface = Some(data_init.init(id, ()));
            }
            server_compositor::Request::CreateRegion { id } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<wl_seat::WlSeat, ()> for Compositor {
    fn bind(
        state: &mut Self,
        _handle: &ServerHandle,
        _client: &Client,
        resource: New<wl_seat::WlSeat>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let seat = data_init.init(resource, ());
        seat.capabilities(state.capabilities());
        state.seats.push(seat);
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_seat::WlSeat,
        request: wl_seat::Request,
        _data: &(),
        _handle: &ServerHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            data_init.init(id, ());
        }
    }
}

impl GlobalDispatch<ZwpPointerGesturesV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &ServerHandle,
        _client: &Client,
        resource: New<ZwpPointerGesturesV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwpPointerGesturesV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwpPointerGesturesV1,
        request: zwp_pointer_gestures_v1::Request,
        _data: &(),
        _handle: &ServerHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_pointer_gestures_v1::Request::GetPinchGesture { id, .. } => {
                state.pinch = Some(data_init.init(id, ()));
            }
            zwp_pointer_gestures_v1::Request::GetSwipeGesture { id, .. } => {
                data_init.init(id, ());
            }
            zwp_pointer_gestures_v1::Request::GetHoldGesture { id, .. } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

/// Requests on these objects need no compositor behavior for the test.
macro_rules! accept_requests {
    ($($resource:ty),* $(,)?) => {$(
        impl Dispatch<$resource, ()> for Compositor {
            fn request(
                _state: &mut Self,
                _client: &Client,
                _resource: &$resource,
                _request: <$resource as Resource>::Request,
                _data: &(),
                _handle: &ServerHandle,
                _data_init: &mut DataInit<'_, Self>,
            ) {
            }
        }
    )*};
}

accept_requests!(
    server_surface::WlSurface,
    wl_region::WlRegion,
    wl_pointer::WlPointer,
    ZwpPointerGesturePinchV1,
    ZwpPointerGestureSwipeV1,
    ZwpPointerGestureHoldV1,
);

// ---------------------------------------------------------------------------
// Toolkit stand-in
// ---------------------------------------------------------------------------

/// Plays winit: it owns the libwayland display and the window surface, and
/// reads the socket through its own queue.
struct Toolkit {
    queue: EventQueue<ToolkitState>,
    state: ToolkitState,
    surface: wl_surface::WlSurface,
    // Declared last so the display it owns outlives the queue and surface.
    connection: Connection,
}

struct ToolkitState;

impl Toolkit {
    fn connect(stream: UnixStream) -> Self {
        let connection = Connection::from_socket(stream).expect("libwayland client");
        let (globals, mut queue) = registry_queue_init::<ToolkitState>(&connection).expect("registry");
        let compositor: wl_compositor::WlCompositor = globals.bind(&queue.handle(), 4..=4, ()).expect("compositor");
        let surface = compositor.create_surface(&queue.handle(), ());
        let mut state = ToolkitState;
        queue.roundtrip(&mut state).expect("toolkit roundtrip");
        Self {
            queue,
            state,
            surface,
            connection,
        }
    }

    /// One pass of the toolkit's event loop: flush every queue's requests and
    /// read everything the compositor has sent so far.
    fn pump(&mut self) {
        self.queue.roundtrip(&mut self.state).expect("toolkit roundtrip");
    }

    fn surface_key(&self) -> u64 {
        self.surface.id().as_ptr() as u64
    }

    fn display(&self) -> ForeignDisplay {
        let pointer = self.connection.backend().display_ptr().cast::<c_void>();
        ForeignDisplay(NonNull::new(pointer).expect("live wl_display"))
    }
}

impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for ToolkitState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

impl wayland_client::Dispatch<wl_compositor::WlCompositor, ()> for ToolkitState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

impl wayland_client::Dispatch<wl_surface::WlSurface, ()> for ToolkitState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

/// The raw `wl_display` the toolkit owns, handed out the way winit's event
/// loop exposes its display handle.
struct ForeignDisplay(NonNull<c_void>);

impl HasDisplayHandle for ForeignDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.0));
        // SAFETY: the pointer comes from a `Connection` that `Toolkit` owns,
        // and every test drops the bridge before the toolkit, so the display
        // is live for any borrow of this handle.
        #[allow(unsafe_code)]
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

fn adopt(toolkit: &Toolkit) -> Option<PinchBridge> {
    // SAFETY: each test keeps `toolkit`, which owns the adopted `wl_display`,
    // alive until after the bridge is dropped.
    #[allow(unsafe_code)]
    unsafe {
        PinchBridge::start(&toolkit.display())
    }
}

/// Send one gesture once the bridge's objects exist, pumping the toolkit in
/// between so the bridge's pending requests reach the compositor.
fn pinch(server: &Server, toolkit: &mut Toolkit, bridge: &mut PinchBridge, scales: &[f64]) -> Vec<Pinch> {
    let mut steps = Vec::new();
    for _ in 0..50 {
        steps.extend(bridge.poll());
        toolkit.pump();
        if server.command(Command::Pinch(scales.to_vec())) {
            toolkit.pump();
            steps.extend(bridge.poll());
            return steps;
        }
    }
    panic!("the bridge never created a pinch gesture object");
}

fn assert_gesture(steps: &[Pinch], surface: u64, scales: &[f64]) {
    assert_eq!(steps.len(), scales.len(), "one step per update: {steps:?}");
    let mut zoom = 1.0_f64;
    for (step, scale) in steps.iter().zip(scales) {
        assert_eq!(step.surface, surface, "steps name the toolkit's own surface");
        zoom *= step.delta.exp();
        assert!((zoom - scale).abs() < 1e-9, "cumulative zoom {zoom}, expected {scale}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

const SCALES: [f64; 3] = [1.25, 1.5, 0.75];

#[test]
fn pinch_arrives_through_the_toolkits_own_reads() {
    let (server, stream) = Server::start(Features {
        gestures: true,
        pointer_at_start: true,
    });
    let mut toolkit = Toolkit::connect(stream);
    let mut bridge = adopt(&toolkit).expect("compositor offers pointer gestures");

    let steps = pinch(&server, &mut toolkit, &mut bridge, &SCALES);
    assert_gesture(&steps, toolkit.surface_key(), &SCALES);

    // A second gesture starts from its own baseline, not the last scale.
    let steps = pinch(&server, &mut toolkit, &mut bridge, &[2.0]);
    assert_gesture(&steps, toolkit.surface_key(), &[2.0]);

    // Nothing is replayed once drained.
    toolkit.pump();
    assert!(bridge.poll().is_empty());
    drop(bridge);
}

#[test]
fn a_seat_that_gains_a_pointer_later_still_delivers_pinch() {
    let (server, stream) = Server::start(Features {
        gestures: true,
        pointer_at_start: false,
    });
    let mut toolkit = Toolkit::connect(stream);
    let mut bridge = adopt(&toolkit).expect("a pointerless seat keeps the bridge");
    toolkit.pump();
    assert!(
        !server.command(Command::Pinch(SCALES.to_vec())),
        "no pinch object without a pointer"
    );

    assert!(server.command(Command::GrantPointer));
    let steps = pinch(&server, &mut toolkit, &mut bridge, &SCALES);
    assert_gesture(&steps, toolkit.surface_key(), &SCALES);
    drop(bridge);
}

#[test]
fn a_compositor_without_pointer_gestures_declines_the_bridge() {
    let (_server, stream) = Server::start(Features {
        gestures: false,
        pointer_at_start: true,
    });
    let mut toolkit = Toolkit::connect(stream);
    assert!(adopt(&toolkit).is_none());
    // The toolkit's own connection is unaffected by the declined adoption.
    toolkit.pump();
}
