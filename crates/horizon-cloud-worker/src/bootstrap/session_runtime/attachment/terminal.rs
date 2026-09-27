//! The attachment child receives only a private terminal, never the SSH terminal.
use super::{Identity, bridge::Direction, invalid};
use rustix::{
    event::{PollFlags, Timespec},
    fs::OFlags,
    pty::OpenptFlags,
    termios::{LocalModes, OptionalActions, Termios},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Permit {
    parent: Identity,
    child: u32,
    proxy: String,
    target: String,
}

pub(super) struct Terminal {
    pub child: Child,
    master: OwnedFd,
    slave: Option<OwnedFd>,
}
impl Terminal {
    pub fn spawn(proxy: &Path, target: &str) -> io::Result<Self> {
        let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
        rustix::pty::unlockpt(&master)?;
        let slave =
            rustix::pty::ioctl_tiocgptpeer(&master, OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
        rustix::termios::tcsetwinsize(&master, rustix::termios::tcgetwinsize(io::stdin())?)?;
        let (mut parent, child) = UnixStream::pair()?;
        parent.set_write_timeout(Some(Duration::from_secs(2)))?;
        let child = Command::new(std::env::current_exe()?)
            .arg("attach-project-terminal")
            .env_clear()
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(OwnedFd::from(child)))
            .spawn()?;
        let result = Self {
            child,
            master,
            slave: Some(slave),
        };
        serde_json::to_writer(
            &mut parent,
            &Permit {
                parent: Identity::current()?,
                child: result.child.id(),
                proxy: proxy.to_str().ok_or_else(invalid)?.into(),
                target: target.into(),
            },
        )?;
        parent.shutdown(std::net::Shutdown::Write)?;
        Ok(result)
    }
    pub fn relay(&mut self, client: &UnixStream, upstream: &UnixStream) -> io::Result<()> {
        let input = io::stdin();
        let output = io::stdout();
        let _mode = Mode::enter(&input, &output)?;
        rustix::fs::fcntl_setfl(&self.master, rustix::fs::fcntl_getfl(&self.master)? | OFlags::NONBLOCK)?;
        let mut directions = [
            Direction::new(client.as_fd(), upstream.as_fd(), true),
            Direction::new(upstream.as_fd(), client.as_fd(), true),
            Direction::new(input.as_fd(), self.master.as_fd(), false),
            Direction::new(self.master.as_fd(), output.as_fd(), false),
        ];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        'relay: loop {
            if self.child.try_wait()?.is_some() {
                break;
            }
            if !ready {
                ready = !rustix::termios::tcgetattr(self.slave.as_ref().ok_or_else(invalid)?)?
                    .local_modes
                    .intersects(LocalModes::ICANON | LocalModes::ECHO);
                if !ready && Instant::now() >= deadline {
                    return Err(invalid());
                }
            }
            rustix::termios::tcsetwinsize(&self.master, rustix::termios::tcgetwinsize(&input)?)?;
            let mut polls: Vec<_> = directions
                .iter()
                .enumerate()
                .map(|(i, d)| d.interest(i != 2 || ready))
                .collect();
            match rustix::event::poll(
                &mut polls,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 250_000_000,
                }),
            ) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
            let events: Vec<_> = polls.iter().map(rustix::event::PollFd::revents).collect();
            drop(polls);
            for (index, (direction, events)) in directions.iter_mut().zip(events).enumerate() {
                if index == 2 && !ready {
                    if events.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
                        return Ok(());
                    }
                    continue;
                }
                if events.intersects(PollFlags::IN | PollFlags::OUT | PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)
                {
                    match direction.step(events, index != 2 || ready) {
                        Ok(true) => {}
                        Ok(false) if index == 2 => return Ok(()),
                        Ok(false) => break 'relay,
                        Err(error) if index == 3 && error.raw_os_error() == Some(5) => break 'relay,
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        let _ = client.shutdown(std::net::Shutdown::Both);
        let _ = upstream.shutdown(std::net::Shutdown::Both);
        drop(self.slave.take());
        drain(&mut directions[3], &mut self.child)
    }
}
fn drain(output: &mut Direction<'_>, child: &mut Child) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ended = false;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Terminal output drain timed out",
            ));
        }
        if ended {
            if let Some(status) = child.try_wait()? {
                return if status.success() { Ok(()) } else { Err(invalid()) };
            }
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        let mut polls = [output.interest(true)];
        match rustix::event::poll(
            &mut polls,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            }),
        ) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
        match output.step(polls[0].revents(), true) {
            Ok(alive) => ended = !alive,
            Err(error) if error.raw_os_error() == Some(5) => ended = true,
            Err(error) => return Err(error),
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if self.child.try_wait().is_ok_and(|s| s.is_some()) {
            return;
        }
        // The unreaped child cannot be replaced by PID reuse. Never signal a
        // saved server/pane identity on a terminal disconnect.
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().is_ok_and(|s| s.is_some()) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(super) fn child() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let stream = UnixStream::from(io::stderr().as_fd().try_clone_to_owned()?);
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let peer = rustix::net::sockopt::socket_peercred(&stream)?;
    let mut bytes = Vec::new();
    (&stream).take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    let permit: Permit = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if permit.child != std::process::id()
        || !permit.parent.alive()
        || peer.pid.as_raw_nonzero().get() != permit.parent.pid
        || peer.uid != rustix::process::getuid()
        || rustix::process::getppid().map(|p| p.as_raw_nonzero().get()) != Some(permit.parent.pid)
        || !permit.proxy.starts_with(&format!("/proc/{}/fd/", permit.parent.pid))
        || !permit.proxy.ends_with("/s")
        || !super::valid_pane(&permit.target)
    {
        return Err(invalid());
    }
    rustix::process::setsid()?;
    rustix::process::ioctl_tiocsctty(io::stdin())?;
    let terminal = File::from(io::stdin().as_fd().try_clone_to_owned()?);
    let error = Command::new(super::super::policy::TMUX)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("TERM", "xterm-256color")
        .args(["-N", "-S", &permit.proxy, "attach-session", "-t", &permit.target])
        .stdin(Stdio::from(terminal.try_clone()?))
        .stdout(Stdio::from(terminal.try_clone()?))
        .stderr(Stdio::from(terminal))
        .exec();
    Err(error)
}

struct Mode<'a> {
    input: &'a io::Stdin,
    output: &'a io::Stdout,
    termios: Termios,
    input_flags: OFlags,
    output_flags: OFlags,
}
impl<'a> Mode<'a> {
    fn enter(input: &'a io::Stdin, output: &'a io::Stdout) -> io::Result<Self> {
        let mode = Self {
            input,
            output,
            termios: rustix::termios::tcgetattr(input)?,
            input_flags: rustix::fs::fcntl_getfl(input)?,
            output_flags: rustix::fs::fcntl_getfl(output)?,
        };
        let mut raw = mode.termios.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(input, OptionalActions::Now, &raw)?;
        rustix::fs::fcntl_setfl(input, mode.input_flags | OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(output, mode.output_flags | OFlags::NONBLOCK)?;
        Ok(mode)
    }
}
impl Drop for Mode<'_> {
    fn drop(&mut self) {
        let _ = rustix::fs::fcntl_setfl(self.input, self.input_flags);
        let _ = rustix::fs::fcntl_setfl(self.output, self.output_flags);
        let _ = rustix::termios::tcsetattr(self.input, OptionalActions::Now, &self.termios);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exited_child_drains_queued_output_under_backpressure_and_reports_failure() {
        for status in [0, 17] {
            let (source, writer) = UnixStream::pair().unwrap();
            let (destination, receiver) = UnixStream::pair().unwrap();
            source.set_nonblocking(true).unwrap();
            destination.set_nonblocking(true).unwrap();
            rustix::net::sockopt::set_socket_send_buffer_size(&destination, 1024).unwrap();
            while rustix::io::write(&destination, &[42; 4096]).is_ok() {}
            let mut child = Command::new("/bin/sh")
                .args(["-c", &format!("printf FINAL-MARKER; exit {status}")])
                .stdout(Stdio::from(OwnedFd::from(writer)))
                .spawn()
                .unwrap();
            child.wait().unwrap();
            let mut output = Direction::new(source.as_fd(), destination.as_fd(), false);
            output.step(PollFlags::IN, true).unwrap();
            let reader = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                receiver.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut bytes = Vec::new();
                let mut receiver = receiver;
                receiver.read_to_end(&mut bytes).unwrap();
                bytes
            });
            assert_eq!(drain(&mut output, &mut child).is_ok(), status == 0);
            drop(output);
            drop(destination);
            assert!(reader.join().unwrap().ends_with(b"FINAL-MARKER"));
        }
    }
}
