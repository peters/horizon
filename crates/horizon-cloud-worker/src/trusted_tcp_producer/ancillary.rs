//! One-byte reads keep ancillary receipt separate from application payload.
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags};
use std::{
    io::{self, IoSliceMut},
    mem::MaybeUninit,
    os::fd::OwnedFd,
};
use tokio::{io::Interest, net::UnixStream};

pub(super) async fn receipt(
    stream: &UnixStream,
    until: tokio::time::Instant,
) -> Result<super::accepted::Control, super::Error> {
    let (prefix, descriptor) = tokio::time::timeout_at(until, byte(stream, true))
        .await
        .map_err(|_| super::Error::Deadline)??;
    if prefix != Some(b'P') {
        return Err(super::Error::Encoding);
    }
    Ok(super::accepted::Control::new(
        descriptor.ok_or(super::Error::Encoding)?,
    )?)
}

pub(super) async fn byte(stream: &UnixStream, rights: bool) -> io::Result<(Option<u8>, Option<OwnedFd>)> {
    loop {
        stream.readable().await?;
        match stream.try_io(Interest::READABLE, || receive(stream, rights)) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => tokio::task::yield_now().await,
            result => return result,
        }
    }
}

fn receive(stream: &UnixStream, rights: bool) -> io::Result<(Option<u8>, Option<OwnedFd>)> {
    let mut bytes = [0];
    // Two slots distinguish an extra descriptor even when cmsghdr alignment
    // leaves room for it. Overflow is a hard refusal, never partial authority.
    let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
    let mut control = RecvAncillaryBuffer::new(&mut storage);
    let message = rustix::net::recvmsg(
        stream,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC | RecvFlags::DONTWAIT,
    )?;
    let mut descriptor = None;
    let mut messages = 0;
    let mut valid = !message.flags.intersects(ReturnFlags::CTRUNC | ReturnFlags::TRUNC);
    for item in control.drain() {
        messages += 1;
        match item {
            RecvAncillaryMessage::ScmRights(mut fds) if rights => {
                let first = fds.next();
                valid &= descriptor.is_none() && first.is_some();
                if descriptor.is_none() {
                    descriptor = first;
                }
                // OwnedFd and the remaining ancillary iterator close every
                // delivered descriptor, including all rejected extras.
                valid &= fds.next().is_none();
            }
            _ => valid = false,
        }
    }
    valid &= if rights {
        messages == 1 && descriptor.is_some()
    } else {
        messages == 0
    };
    if !valid || (rights && message.bytes != 1) {
        return Err(invalid());
    }
    Ok(((message.bytes == 1).then_some(bytes[0]), descriptor))
}

pub(super) fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid original-flow control protocol")
}

#[cfg(test)]
#[path = "ancillary/tests.rs"]
mod tests;
