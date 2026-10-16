//! Safe `SCM_RIGHTS` mechanics; no original daemon identity is supplied here.
use super::*;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use std::{io::IoSlice, os::fd::AsFd, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{Duration, Instant, timeout},
};

const WAIT: Duration = Duration::from_secs(3);

fn send(stream: &UnixStream, byte: u8, descriptors: &[std::os::fd::BorrowedFd<'_>]) {
    let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(8))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    if !descriptors.is_empty() {
        assert!(control.push(SendAncillaryMessage::ScmRights(descriptors)));
    }
    assert_eq!(
        rustix::net::sendmsg(stream, &[IoSlice::new(&[byte])], &mut control, SendFlags::NOSIGNAL).unwrap(),
        1
    );
}

#[tokio::test]
async fn exact_one_fd_first_byte_leaves_header_and_application_bytes_untouched() {
    let (stream, mut sender) = UnixStream::pair().unwrap();
    let (endpoint, _daemon) = UnixStream::pair().unwrap();
    send(&sender, b'P', &[endpoint.as_fd()]);
    sender
        .write_all(b"ROXY TCP4 100.64.0.1 100.64.0.2 4567 20000\r\nPING")
        .await
        .unwrap();
    let (prefix, descriptor) = timeout(WAIT, byte(&stream, true)).await.unwrap().unwrap();
    assert_eq!(prefix, Some(b'P'));
    let descriptor = descriptor.unwrap();
    assert!(
        rustix::io::fcntl_getfd(&descriptor)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    let header = super::super::proxy::read(&stream, Instant::now() + WAIT).await.unwrap();
    assert_eq!(header.destination.port(), 20000);
    let mut stream = stream;
    let mut application = [0; 4];
    stream.read_exact(&mut application).await.unwrap();
    assert_eq!(&application, b"PING");
}

#[tokio::test]
async fn missing_extra_and_truncated_rights_close_every_delivered_endpoint() {
    for count in [0, 2, 8] {
        let (stream, sender) = UnixStream::pair().unwrap();
        let mut endpoints = Vec::new();
        let mut peers = Vec::new();
        for _ in 0..count {
            let (endpoint, peer) = UnixStream::pair().unwrap();
            endpoints.push(endpoint);
            peers.push(peer);
        }
        let descriptors: Vec<_> = endpoints.iter().map(AsFd::as_fd).collect();
        send(&sender, b'P', &descriptors);
        drop(descriptors);
        drop(endpoints);
        assert!(timeout(WAIT, byte(&stream, true)).await.unwrap().is_err());
        for mut peer in peers {
            let mut byte = [0];
            assert_eq!(timeout(WAIT, peer.read(&mut byte)).await.unwrap().unwrap(), 0);
        }
    }
}

#[tokio::test]
async fn late_header_rights_are_refused_and_closed_without_reading_application() {
    let (stream, sender) = UnixStream::pair().unwrap();
    let (endpoint, mut daemon) = UnixStream::pair().unwrap();
    send(&sender, b'R', &[endpoint.as_fd()]);
    drop(endpoint);
    assert!(super::super::proxy::read(&stream, Instant::now() + WAIT).await.is_err());
    let mut byte = [0];
    assert_eq!(timeout(WAIT, daemon.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn timeout_and_unpolled_drop_release_original_payload_and_received_control() {
    let (stream, mut sender) = UnixStream::pair().unwrap();
    let pending = Arc::new(tokio::sync::Semaphore::new(1));
    let future = super::super::begin_identification(stream, pending);
    drop(future);
    let mut byte = [0];
    assert_eq!(timeout(WAIT, sender.read(&mut byte)).await.unwrap().unwrap(), 0);

    let (stream, sender) = UnixStream::pair().unwrap();
    let (endpoint, mut daemon) = UnixStream::pair().unwrap();
    send(&sender, b'P', &[endpoint.as_fd()]);
    drop(endpoint);
    let (_, fd) = timeout(WAIT, super::byte(&stream, true)).await.unwrap().unwrap();
    let closing = super::super::Closing {
        payload: Some(stream),
        control: Some(super::super::accepted::Control::new(fd.unwrap()).unwrap()),
    };
    assert!(matches!(
        super::super::proxy::read(closing.payload.as_ref().unwrap(), Instant::now()).await,
        Err(super::super::Error::Deadline)
    ));

    drop(closing);
    assert_eq!(timeout(WAIT, daemon.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn production_receipt_refuses_wrong_prefix_and_closes_the_received_control() {
    let (stream, sender) = UnixStream::pair().unwrap();
    let (endpoint, mut daemon) = UnixStream::pair().unwrap();
    send(&sender, b'Q', &[endpoint.as_fd()]);
    drop(endpoint);
    assert!(matches!(
        super::receipt(&stream, Instant::now() + WAIT).await,
        Err(super::super::Error::Encoding)
    ));
    let mut byte = [0];
    assert_eq!(timeout(WAIT, daemon.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn production_receipt_deadline_and_eof_never_supply_a_descriptor() {
    let (stream, sender) = UnixStream::pair().unwrap();
    assert!(matches!(
        super::receipt(&stream, Instant::now()).await,
        Err(super::super::Error::Deadline)
    ));
    drop(sender);
    assert!(super::receipt(&stream, Instant::now() + WAIT).await.is_err());
}
