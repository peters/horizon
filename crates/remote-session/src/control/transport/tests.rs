use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Wake, Waker},
    time::Duration,
};

use tokio::{io::AsyncReadExt, net::TcpListener, time::timeout};

use super::*;

#[derive(Default)]
struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn cancellation_wakes_both_io_directions_without_peer_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let mut peer = listener.accept().await.unwrap().0;
    let control = Cancellation::default();
    // Isolate the watch wake-up: no installed shutdown handle can make the OS
    // socket readable. The command-level regression also checks physical EOF.
    let mut transport = Transport::new(socket, &control);
    let read_woken = Arc::new(Woken::default());
    let write_woken = Arc::new(Woken::default());
    let read_waker = Waker::from(Arc::clone(&read_woken));
    let write_waker = Waker::from(Arc::clone(&write_woken));
    let mut read_cx = Context::from_waker(&read_waker);
    let mut write_cx = Context::from_waker(&write_waker);
    let mut bytes = [0; 1];
    let mut buf = ReadBuf::new(&mut bytes);
    assert!(Pin::new(&mut transport).poll_read(&mut read_cx, &mut buf).is_pending());
    assert!(Pin::new(&mut transport).poll_flush(&mut write_cx).is_ready());

    // No peer data or peer shutdown can supply either notification.
    control.cancel();
    assert!(read_woken.0.load(Ordering::Acquire));
    assert!(write_woken.0.load(Ordering::Acquire));
    for _ in 0..2 {
        assert!(matches!(
            Pin::new(&mut transport).poll_read(&mut read_cx, &mut buf),
            Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::ConnectionAborted
        ));
        assert!(matches!(
            Pin::new(&mut transport).poll_write(&mut write_cx, b"blocked"),
            Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::ConnectionAborted
        ));
    }
    drop(transport);
    let mut received = Vec::new();
    timeout(Duration::from_secs(1), peer.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert!(received.is_empty());
}
