//! Real `AF_UNIX` and `SCM_RIGHTS` mechanics, never daemon or `AcceptedPeer` authority.
use super::*;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use std::{io::IoSlice, mem::MaybeUninit, os::fd::AsFd};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
    time::{Duration, timeout},
};
const WAIT: Duration = Duration::from_secs(3);

fn queued() -> (PendingStream, UnixStream, UnixStream) {
    let (payload, sender) = UnixStream::pair().unwrap();
    let (endpoint, control) = UnixStream::pair().unwrap();
    {
        let descriptors = [endpoint.as_fd()];
        let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut rights = SendAncillaryBuffer::new(&mut storage);
        assert!(rights.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            rustix::net::sendmsg(&sender, &[IoSlice::new(b"P")], &mut rights, SendFlags::NOSIGNAL).unwrap(),
            1
        );
    }
    drop(endpoint);
    let mut byte = [0];
    assert_eq!(
        control.try_read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    (fixture(payload), sender, control)
}

async fn eof(stream: &mut UnixStream) {
    let mut byte = [0];
    assert_eq!(timeout(WAIT, stream.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn weak_cancellation_releases_queued_original_rights_while_pending_owner_is_retained() {
    let (pending, mut sender, mut control) = queued();
    let retained = pending.cancellation();
    retained.cancel();
    eof(&mut control).await;
    eof(&mut sender).await;
    assert!(
        pending
            .receive_control(tokio::time::Instant::now() + WAIT)
            .await
            .is_err()
    );
    // Keep both the original owner and weak handle alive through the EOF proof.
    retained.cancel();
    drop(pending);
}

#[tokio::test]
async fn atomic_receipt_keeps_header_and_application_bytes_and_cancels_installed_control() {
    let (pending, mut sender, mut daemon) = queued();
    let retained = pending.cancellation();
    sender
        .write_all(b"ROXY TCP4 100.64.0.1 100.64.0.2 4567 20000\r\nPING")
        .await
        .unwrap();
    pending
        .receive_control(tokio::time::Instant::now() + WAIT)
        .await
        .unwrap();
    pending.control().unwrap().live().unwrap();
    let header = super::super::proxy::read(pending.payload().unwrap(), tokio::time::Instant::now() + WAIT)
        .await
        .unwrap();
    assert_eq!(header.destination.port(), 20000);
    let stream = super::super::AcceptedStream::new(pending.into_inner().unwrap());
    let mut bytes = [0; 4];
    timeout(WAIT, stream.readable()).await.unwrap().unwrap();
    assert_eq!(stream.try_read(&mut bytes).unwrap(), 4);
    assert_eq!(&bytes, b"PING");
    retained.cancel();
    eof(&mut daemon).await;
    eof(&mut sender).await;
}

#[tokio::test]
async fn pending_drop_releases_queued_rights_with_retained_weak_handles() {
    let (pending, mut sender, mut control) = queued();
    let retained = pending.cancellation();
    let other = retained.clone();
    drop(pending);
    eof(&mut control).await;
    eof(&mut sender).await;
    retained.cancel();
    other.cancel();
}

#[tokio::test]
async fn unpolled_identification_releases_queued_rights_with_retained_weak_handles() {
    let (pending, mut sender, mut control) = queued();
    let retained = pending.cancellation();
    let slots = Arc::new(Semaphore::new(1));
    let identification = super::super::begin_identification(pending, Arc::clone(&slots));
    assert_eq!(
        slots.available_permits(),
        0,
        "An unpolled owner reserves its finite slot"
    );
    drop(identification);
    assert_eq!(slots.available_permits(), 1);
    eof(&mut control).await;
    eof(&mut sender).await;
    retained.cancel();
}

#[tokio::test]
async fn busy_refusal_releases_queued_rights_even_while_completed_future_and_handles_remain() {
    let (pending, mut sender, mut control) = queued();
    let retained = pending.cancellation();
    let slots = Arc::new(Semaphore::new(0));
    let mut identification = Box::pin(super::super::begin_identification(pending, slots));
    assert!(matches!(identification.as_mut().await, Err(Error::Busy)));
    eof(&mut control).await;
    eof(&mut sender).await;
    retained.cancel();
}

#[tokio::test]
async fn original_pending_handle_cancels_after_control_receipt_and_accepted_transfer() {
    let (pending, mut sender, mut daemon) = queued();
    let retained = pending.cancellation();
    let original = pending.cancellation();
    pending
        .receive_control(tokio::time::Instant::now() + WAIT)
        .await
        .unwrap();
    let stream = Arc::new(super::super::AcceptedStream::new(pending.into_inner().unwrap()));
    assert!(stream.matches_pending(&original));
    let (foreign, _peer) = UnixStream::pair().unwrap();
    let foreign = fixture(foreign);
    assert!(!stream.matches_pending(&foreign.cancellation()));
    let finishing = tokio::spawn({
        let stream = Arc::clone(&stream);
        async move { stream.finish().await }
    });
    retained.cancel();
    eof(&mut sender).await;
    eof(&mut daemon).await;
    assert!(timeout(WAIT, finishing).await.unwrap().unwrap().is_err());
    assert!(Arc::strong_count(&stream) >= 1);
}

#[tokio::test]
async fn cancellation_before_late_control_install_refuses_and_closes_original_endpoint() {
    let (payload, mut peer) = UnixStream::pair().unwrap();
    let pending = fixture(payload);
    let handle = pending.cancellation();
    let (endpoint, mut daemon) = UnixStream::pair().unwrap();
    let control = Control::new(endpoint.into_std().unwrap().into()).unwrap();
    handle.cancel();
    assert!(matches!(pending.install_control(control), Err(Error::Custody)));
    eof(&mut daemon).await;
    eof(&mut peer).await;
}

#[tokio::test]
async fn accepted_transport_keeps_cookie_and_weak_handles_never_retain_its_control() {
    let (payload, mut peer) = UnixStream::pair().unwrap();
    let pending = fixture(payload);
    let handle = pending.cancellation();
    let (endpoint, mut daemon) = UnixStream::pair().unwrap();
    let mut accepted = super::super::pending_transport_fixture(pending, endpoint);
    assert!(accepted.matches_pending(&handle));
    accepted.write_all(b"original").await.unwrap();
    let mut bytes = [0; 8];
    peer.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"original");
    drop(accepted);
    eof(&mut peer).await;
    eof(&mut daemon).await;
    handle.cancel();
}
