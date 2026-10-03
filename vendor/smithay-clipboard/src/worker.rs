use std::io::{Error, Result};
use std::sync::mpsc::Sender;

use sctk::reexports::calloop::channel::Channel;
use sctk::reexports::calloop::{EventLoop, channel};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::Connection;
use sctk::reexports::client::globals::registry_queue_init;

use crate::state::{SelectionTarget, State};

/// Spawn a clipboard worker, which dispatches its own `EventQueue` and handles
/// clipboard requests.
pub fn spawn(
    name: String,
    display: Connection,
    rx_chan: Channel<Command>,
    worker_replier: Sender<Result<String>>,
    native: std::sync::Arc<crate::native::Hub>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            worker_impl(display, rx_chan, worker_replier, native);
        })
        .ok()
}

/// Clipboard worker thread command.
pub enum Command {
    NativeRead {
        surface: u64,
        recipient: u64,
        generation: u64,
    },
    ResetNative,
    /// Store data to a clipboard.
    Store(String),
    /// Store data to a primary selection.
    StorePrimary(String),
    /// Load data from a clipboard.
    Load,
    /// Load primary selection.
    LoadPrimary,
    /// Shutdown the worker.
    Exit,
}

/// Handle clipboard requests.
fn worker_impl(
    connection: Connection,
    rx_chan: Channel<Command>,
    reply_tx: Sender<Result<String>>,
    native: std::sync::Arc<crate::native::Hub>,
) {
    let _guard = crate::native::WorkerGuard(native.clone());
    let Ok((globals, event_queue)) = registry_queue_init(&connection) else {
        return;
    };

    let mut event_loop = EventLoop::<State>::try_new().unwrap();
    let loop_handle = event_loop.handle();

    let Some(mut state) = State::new(&globals, &event_queue.handle(), loop_handle.clone(), reply_tx, native) else {
        return;
    };

    loop_handle
        .insert_source(rx_chan, |event, (), state| {
            if let channel::Event::Msg(event) = event {
                match event {
                    Command::NativeRead {
                        surface,
                        recipient,
                        generation,
                    } => state.read_native(surface, recipient, generation),
                    Command::ResetNative => state.native.reset(),
                    Command::StorePrimary(contents) => {
                        state.store_selection(SelectionTarget::Primary, contents);
                    }
                    Command::Store(contents) => {
                        state.store_selection(SelectionTarget::Clipboard, contents);
                    }
                    Command::Load if state.data_device_manager_state.is_some() => {
                        if let Err(err) = state.load_selection(SelectionTarget::Clipboard) {
                            let _ = state.reply_tx.send(Err(err));
                        }
                    }
                    Command::LoadPrimary if state.data_device_manager_state.is_some() => {
                        if let Err(err) = state.load_selection(SelectionTarget::Primary) {
                            let _ = state.reply_tx.send(Err(err));
                        }
                    }
                    Command::Load | Command::LoadPrimary => {
                        let _ = state
                            .reply_tx
                            .send(Err(Error::other("requested selection is not supported")));
                    }
                    Command::Exit => state.exit = true,
                }
            }
        })
        .unwrap();

    WaylandSource::new(connection, event_queue).insert(loop_handle).unwrap();

    loop {
        let timeout = state.native.pending().then_some(std::time::Duration::from_millis(16));
        if event_loop.dispatch(timeout, &mut state).is_err() || state.exit {
            break;
        }
        // SCTK omits the selection callback when an offer becomes null.
        state.publish_native_selection();
        state.native.poll();
    }
}
