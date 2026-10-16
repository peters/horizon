//! Socket mechanics only. These fixtures never construct an `AcceptedPeer`.
use super::*;
use std::os::linux::net::SocketAddrExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{Duration, timeout},
};

const WAIT: Duration = Duration::from_secs(3);

fn fixture() -> (AcceptedStream, UnixStream, UnixStream) {
    let (payload, peer) = UnixStream::pair().unwrap();
    let (endpoint, daemon) = UnixStream::pair().unwrap();
    (super::super::transport_fixture(payload, endpoint), peer, daemon)
}

#[tokio::test]
async fn payload_write_eof_keeps_private_cancel_alive_and_reverse_data_readable() {
    let (mut stream, mut peer, mut control) = fixture();
    stream.shutdown().await.unwrap();
    let mut byte = [0];
    assert_eq!(timeout(WAIT, peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(
        control.try_read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    peer.write_all(b"r").await.unwrap();
    timeout(WAIT, stream.read_exact(&mut byte)).await.unwrap().unwrap();
    assert_eq!(&byte, b"r");
    drop(stream);
    assert_eq!(timeout(WAIT, control.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn explicit_cancel_interrupts_finish_despite_private_and_outer_retained_arcs() {
    let (stream, mut peer, mut control) = fixture();
    let stream = Arc::new(stream);
    let retained = Arc::clone(&stream);
    let mut finishing = Box::pin(stream.finish());
    std::future::poll_fn(|cx| {
        assert!(finishing.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    retained.cancel();
    retained.cancel();
    let mut byte = [0];
    assert_eq!(timeout(WAIT, control.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(timeout(WAIT, peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert!(timeout(WAIT, finishing).await.unwrap().is_err());
    assert!(Arc::strong_count(&stream) > 1);
}

#[tokio::test]
async fn finish_requires_exact_drained_then_eof_and_preserves_interrupted_progress() {
    let (stream, _peer, mut control) = fixture();
    let stream = Arc::new(stream);
    let finishing = tokio::spawn({
        let stream = Arc::clone(&stream);
        async move { stream.finish().await }
    });
    control.write_all(&[DRAINED]).await.unwrap();
    timeout(WAIT, async {
        loop {
            let drained = stream.control.state.lock().unwrap().acknowledged == Ack::Drained;
            if drained {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!finishing.is_finished(), "DRAINED alone is not a completed handshake");
    finishing.abort();
    assert!(finishing.await.unwrap_err().is_cancelled());
    drop(control);
    timeout(WAIT, stream.finish()).await.unwrap().unwrap();
    timeout(WAIT, stream.finish()).await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_wrong_and_extra_ack_bytes_latch_failure() {
    for bytes in [&[][..], &[0x02][..], &[DRAINED, DRAINED][..]] {
        let (stream, mut peer, mut control) = fixture();
        let stream = Arc::new(stream);
        let retained = Arc::clone(&stream);
        control.write_all(bytes).await.unwrap();
        drop(control);
        assert!(timeout(WAIT, stream.finish()).await.unwrap().is_err());
        let mut byte = [0];
        assert_eq!(timeout(WAIT, peer.read(&mut byte)).await.unwrap().unwrap(), 0);
        assert!(timeout(WAIT, retained.finish()).await.unwrap().is_err());
        assert_eq!(Arc::strong_count(&stream), 2);
    }
}

#[tokio::test]
async fn last_owner_drop_cancels_both_original_channels() {
    let (stream, mut peer, mut control) = fixture();
    let retained = Arc::new(stream);
    let first = Arc::clone(&retained);
    drop(first);
    let mut byte = [0];
    assert_eq!(
        control.try_read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(retained);
    assert_eq!(timeout(WAIT, peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(timeout(WAIT, control.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn control_shape_refuses_files_datagrams_named_sockets_and_wrong_flags() {
    assert!(Control::new(std::fs::File::open("/dev/null").unwrap().into()).is_err());
    let (datagram, _peer) = std::os::unix::net::UnixDatagram::pair().unwrap();
    datagram.set_nonblocking(true).unwrap();
    assert!(Control::new(datagram.into()).is_err());
    let (blocking, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    assert!(Control::new(blocking.into()).is_err());
    let (socket, _peer) = UnixStream::pair().unwrap();
    let fd = OwnedFd::from(socket.into_std().unwrap());
    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::empty()).unwrap();
    assert!(Control::new(fd).is_err());
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::UnixListener::bind(directory.path().join("named")).unwrap();
    let client = UnixStream::connect(directory.path().join("named")).await.unwrap();
    let accepted = listener.accept().await.unwrap().0;
    assert!(Control::new(accepted.into_std().unwrap().into()).is_err());
    assert!(Control::new(client.into_std().unwrap().into()).is_err());
    assert!(Control::new(listener.into_std().unwrap().into()).is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let accepted = listener.accept().await.unwrap().0;
    assert!(Control::new(accepted.into_std().unwrap().into()).is_err());
    drop(client);
    let abstract_empty = std::os::unix::net::SocketAddr::from_abstract_name([]).unwrap();
    let listener = std::os::unix::net::UnixListener::bind_addr(&abstract_empty).unwrap();
    let client = std::os::unix::net::UnixStream::connect_addr(&abstract_empty).unwrap();
    let accepted = listener.accept().unwrap().0;
    client.set_nonblocking(true).unwrap();
    accepted.set_nonblocking(true).unwrap();
    // Both valid STREAM/CLOEXEC/NONBLOCK endpoints must still refuse the
    // actual abstract-empty address; a true unnamed pair passes fixture().
    assert!(Control::new(client.into()).is_err());
    assert!(Control::new(accepted.into()).is_err());
}

#[tokio::test]
async fn control_live_refuses_closed_or_premature_ack_without_making_identity() {
    let (stream, _peer, mut control) = fixture();
    stream.control.live().unwrap();
    control.write_all(&[DRAINED]).await.unwrap();
    timeout(WAIT, async {
        while stream.control.live().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(control);
    assert!(stream.control.live().is_err());
    let (stream, _peer, control) = fixture();
    drop(control);
    assert!(stream.control.live().is_err());
}
