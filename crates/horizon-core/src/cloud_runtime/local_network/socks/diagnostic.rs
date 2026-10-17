//! Throwaway observation only; compiled solely into the opted-in Windows test graph.
use socket2::Socket;
use std::{
    fmt,
    os::windows::io::AsRawSocket,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_EVENTS: usize = 128;
static ENABLED: OnceLock<bool> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static EVENTS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("HORIZON_WINDOWS_RELAY_DIAGNOSTIC").is_some_and(|v| v == "1"))
}

pub(super) fn socket(socket: &Socket) -> String {
    let local = socket.local_addr().map(|a| a.as_socket()).map_err(|e| e.raw_os_error());
    let peer = socket.peer_addr().map(|a| a.as_socket()).map_err(|e| e.raw_os_error());
    format!("handle={} local={local:?} peer={peer:?}", socket.as_raw_socket())
}

pub(super) fn record(event: &str, fields: fmt::Arguments<'_>) -> Option<usize> {
    let sequence = EVENTS.fetch_add(1, Ordering::Relaxed);
    if sequence > MAX_EVENTS {
        return None;
    }
    let started = START.get_or_init(Instant::now);
    let epoch_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|v| v.as_millis());
    if sequence == MAX_EVENTS {
        eprintln!(
            "HORIZON_RELAY_DIAG pid={} seq={sequence} event=event_limit_reached",
            std::process::id()
        );
        return None;
    }
    eprintln!(
        "HORIZON_RELAY_DIAG pid={} seq={sequence} us={} epoch_ms={epoch_ms:?} thread={:?} event={event} {fields}",
        std::process::id(),
        started.elapsed().as_micros(),
        std::thread::current().name()
    );
    Some(sequence)
}
