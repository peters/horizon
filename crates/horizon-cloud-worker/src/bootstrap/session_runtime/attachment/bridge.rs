//! Bounded nonblocking relay. Rights belong to exactly one queued byte chunk.
use super::invalid;
use rustix::{
    event::{PollFd, PollFlags},
    net::{
        RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer, SendAncillaryMessage,
        SendFlags,
    },
};
use std::{
    io::{self, IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};
const BYTES: usize = 64 * 1024;
const RIGHTS: usize = 16;

pub(super) struct Direction<'a> {
    source: BorrowedFd<'a>,
    destination: BorrowedFd<'a>,
    ancillary: bool,
    bytes: Vec<u8>,
    start: usize,
    end: usize,
    rights: Vec<OwnedFd>,
}
impl<'a> Direction<'a> {
    pub fn new(source: BorrowedFd<'a>, destination: BorrowedFd<'a>, ancillary: bool) -> Self {
        Self {
            source,
            destination,
            ancillary,
            bytes: vec![0; BYTES],
            start: 0,
            end: 0,
            rights: Vec::new(),
        }
    }
    pub fn interest(&self, enabled: bool) -> PollFd<'a> {
        if self.start < self.end {
            PollFd::from_borrowed_fd(self.destination, PollFlags::OUT)
        } else {
            PollFd::from_borrowed_fd(self.source, if enabled { PollFlags::IN } else { PollFlags::empty() })
        }
    }
    pub fn step(&mut self, events: PollFlags, enabled: bool) -> io::Result<bool> {
        if !enabled || events.is_empty() {
            return Ok(true);
        }
        if events.contains(PollFlags::NVAL) {
            return Err(invalid());
        }
        let result = if self.start < self.end {
            self.send()
        } else {
            self.receive()
        };
        match result {
            Ok(0) => Ok(false),
            Ok(_) => Ok(true),
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(true),
            Err(error) => Err(error),
        }
    }
    fn receive(&mut self) -> io::Result<usize> {
        let count = if self.ancillary {
            let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(RIGHTS))];
            let mut control = RecvAncillaryBuffer::new(&mut storage);
            let message = rustix::net::recvmsg(
                self.source,
                &mut [IoSliceMut::new(&mut self.bytes)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC | RecvFlags::DONTWAIT,
            )?;
            if message.flags.intersects(ReturnFlags::CTRUNC | ReturnFlags::TRUNC) {
                return Err(invalid());
            }
            let mut rights = Vec::new();
            for item in control.drain() {
                match item {
                    RecvAncillaryMessage::ScmRights(fds) => rights.extend(fds),
                    _ => return Err(invalid()),
                }
            }
            if rights.len() > RIGHTS || (message.bytes == 0 && !rights.is_empty()) {
                return Err(invalid());
            }
            self.rights = rights;
            message.bytes
        } else {
            rustix::io::read(self.source, &mut self.bytes)?
        };
        self.start = 0;
        self.end = count;
        Ok(count)
    }
    fn send(&mut self) -> io::Result<usize> {
        let count = if self.ancillary {
            let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(RIGHTS))];
            let borrowed: Vec<_> = self.rights.iter().map(AsFd::as_fd).collect();
            let mut control = SendAncillaryBuffer::new(&mut storage);
            if !borrowed.is_empty() && !control.push(SendAncillaryMessage::ScmRights(&borrowed)) {
                return Err(invalid());
            }
            rustix::net::sendmsg(
                self.destination,
                &[IoSlice::new(&self.bytes[self.start..self.end])],
                &mut control,
                SendFlags::NOSIGNAL | SendFlags::DONTWAIT,
            )?
        } else {
            rustix::io::write(self.destination, &self.bytes[self.start..self.end])?
        };
        if count > 0 {
            // A positive partial send transfers all ancillary rights. Retrying them
            // with the remaining bytes would duplicate descriptors in the peer.
            self.rights.clear();
            self.start += count;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, os::unix::net::UnixStream};
    #[test]
    fn closed_gate_ignores_buffered_input_and_hangup_even_with_queued_output() {
        use std::io::Write;
        let (source, mut sender) = UnixStream::pair().unwrap();
        let (destination, receiver) = UnixStream::pair().unwrap();
        source.set_nonblocking(true).unwrap();
        receiver.set_nonblocking(true).unwrap();
        sender.write_all(b"must remain private").unwrap();
        sender.shutdown(std::net::Shutdown::Write).unwrap();
        let mut direction = Direction::new(source.as_fd(), destination.as_fd(), false);
        direction.step(PollFlags::IN | PollFlags::HUP, false).unwrap();
        assert_eq!(direction.end, 0);
        direction.bytes[0] = 42;
        direction.end = 1;
        direction.step(PollFlags::OUT, false).unwrap();
        assert_eq!(direction.start, 0);
        assert_eq!(
            rustix::io::read(&receiver, &mut [0; 64]).unwrap_err(),
            rustix::io::Errno::AGAIN
        );
    }
    #[test]
    fn truncated_ancillary_input_is_rejected_without_queueing_user_bytes() {
        let (source, sender) = UnixStream::pair().unwrap();
        let (destination, _) = UnixStream::pair().unwrap();
        let files: Vec<_> = (0..=RIGHTS).map(|_| File::open("/dev/null").unwrap()).collect();
        let borrowed: Vec<_> = files.iter().map(AsFd::as_fd).collect();
        let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(17))];
        let mut control = SendAncillaryBuffer::new(&mut storage);
        assert!(control.push(SendAncillaryMessage::ScmRights(&borrowed)));
        rustix::net::sendmsg(
            &sender,
            &[IoSlice::new(b"not forwarded")],
            &mut control,
            SendFlags::NOSIGNAL,
        )
        .unwrap();
        let mut direction = Direction::new(source.as_fd(), destination.as_fd(), true);
        assert!(direction.receive().is_err());
        assert_eq!(direction.end, 0);
        assert!(direction.rights.is_empty());
    }
    #[test]
    fn partial_send_transfers_rights_once_and_preserves_backpressure() {
        let (source, _) = UnixStream::pair().unwrap();
        let (destination, receiver) = UnixStream::pair().unwrap();
        rustix::net::sockopt::set_socket_send_buffer_size(&destination, 1024).unwrap();
        destination.set_nonblocking(true).unwrap();
        receiver.set_nonblocking(true).unwrap();
        let mut direction = Direction::new(source.as_fd(), destination.as_fd(), true);
        direction.end = BYTES;
        direction.bytes.fill(42);
        direction.rights.push(File::open("/dev/null").unwrap().into());
        let first = direction.send().unwrap();
        assert!(first > 0 && first < BYTES);
        assert!(direction.rights.is_empty());
        assert_eq!(direction.send().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        let mut total = 0;
        let mut descriptors = 0;
        while total < BYTES {
            let mut bytes = vec![0; BYTES];
            let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(RIGHTS))];
            let mut control = RecvAncillaryBuffer::new(&mut storage);
            let message = rustix::net::recvmsg(
                &receiver,
                &mut [IoSliceMut::new(&mut bytes)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC,
            )
            .unwrap();
            total += message.bytes;
            assert!(bytes[..message.bytes].iter().all(|b| *b == 42));
            for item in control.drain() {
                if let RecvAncillaryMessage::ScmRights(fds) = item {
                    for fd in fds {
                        assert!(
                            rustix::io::fcntl_getfd(fd)
                                .unwrap()
                                .contains(rustix::io::FdFlags::CLOEXEC)
                        );
                        descriptors += 1;
                    }
                }
            }
            if direction.start < direction.end {
                direction.send().unwrap();
            }
        }
        assert_eq!(descriptors, 1);
    }
}
