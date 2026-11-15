//! The reader of one watch: one `ssh` child, the lines of its report, and the rule of
//! a lost link.
//!
//! The reader runs on its own worker thread. It starts the `ssh` child with the
//! command line of [`super::WatchTarget`], reads its standard output line by line with
//! the bound of one line, and hands each whole report to [`Shared::apply_report`]. The
//! words of `ssh` on its standard error are read to their bound for one classification
//! and then dropped.
//!
//! An end of the child is a lost link: the reader waits the delay of
//! [`crate::place::after_connection_loss`] and starts the next attempt. A stop of the
//! supervisor kills the child, so the reader ends within its bound.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::place::{ConnectionLoss, after_connection_loss};

use super::report::parse_report;
use super::snapshot::{Shared, WatchLink, WatchRefusal, unix_millis};
use super::{MAX_LINE_BYTES, MAX_REFUSAL_BYTES, MAX_REPORT_LINES, WatchTarget};

/// The heartbeat line of the text of the watch: the machine answered and its
/// report did not change.
const HEARTBEAT: &[u8] = b"B";

/// The terminator line of one report.
const TERMINATOR: &[u8] = b"Z";

/// The bytes of one chunk that the reader drops of a line above its bound.
const READ_CHUNK: usize = 512;

/// Runs the watch of one machine until its supervisor stops it.
///
/// Every start of the `ssh` child is one attempt. A link that ends waits the delay
/// of the reconnect rule of a machine terminal, and a stop ends the loop.
pub(super) fn run(
    target: &WatchTarget,
    program: &str,
    shared: &Shared,
    stop: &Receiver<()>,
    slot: &Arc<Mutex<Option<Child>>>,
) {
    let machine = target.machine.as_str();
    let mut attempts = 0u32;
    loop {
        if stopped(stop) {
            return;
        }
        shared.set_link(machine, WatchLink::Connecting);
        let started = Instant::now();
        let mut child = match Command::new(program)
            .args(&target.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(machine, %error, "the watch of a machine cannot start ssh");
                shared.set_link(
                    machine,
                    WatchLink::Refused {
                        reason: WatchRefusal::Other,
                        since_millis: unix_millis(),
                    },
                );
                return;
            }
        };
        let stdout = child.stdout.take();
        let refusal = child.stderr.take().map(read_stderr);
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
        if let Some(stdout) = stdout {
            match read_reports(&mut BufReader::new(stdout), machine, shared) {
                Ok(()) => {}
                Err(error) => tracing::warn!(machine, %error, "the report of a machine ended with an error"),
            }
        }
        let Some(mut child) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() else {
            return;
        };
        let status = child.wait();
        let refused = refusal
            .and_then(|words| words.join().ok())
            .map_or(WatchRefusal::Other, |words| WatchRefusal::of_ssh(&words));
        if stopped(stop) {
            return;
        }
        let loss = ConnectionLoss {
            exit_code: status.ok().and_then(|status| status.code()),
            connected_for: Some(started.elapsed()),
            // A watch changes nothing on its machine, so a second attempt loses no
            // work of the person.
            machine_tied: true,
            closed_by_the_person: false,
            attempts,
        };
        match after_connection_loss(loss) {
            crate::place::AfterConnectionLoss::Reconnect { attempt, after } => {
                attempts = attempt;
                shared.set_link(machine, lost_link(refused, attempt));
                match stop.recv_timeout(after) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            crate::place::AfterConnectionLoss::Stay => {
                shared.set_link(machine, lost_link(refused, attempts));
                return;
            }
        }
    }
}

/// The link of a machine whose `ssh` child ended.
fn lost_link(refused: WatchRefusal, attempt: u32) -> WatchLink {
    let since_millis = unix_millis();
    match refused {
        WatchRefusal::HostKey | WatchRefusal::Key => WatchLink::Refused {
            reason: refused,
            since_millis,
        },
        WatchRefusal::Record | WatchRefusal::Other => WatchLink::Lost {
            since_millis,
            attempt,
            agents: 0,
        },
    }
}

/// Whether the supervisor stopped this watch.
fn stopped(stop: &Receiver<()>) -> bool {
    match stop.try_recv() {
        Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
        Err(std::sync::mpsc::TryRecvError::Empty) => false,
    }
}

/// Reads the reports of one `ssh` child until its standard output ends.
fn read_reports(reader: &mut impl BufRead, machine: &str, shared: &Shared) -> std::io::Result<()> {
    let mut line = Vec::new();
    let mut held: Vec<Vec<u8>> = Vec::new();
    loop {
        match read_line_bounded(reader, &mut line, MAX_LINE_BYTES)? {
            None => return Ok(()),
            Some(false) => {
                held.clear();
                tracing::warn!(
                    machine,
                    "a line of a watch report is above its bound and the report is dropped"
                );
            }
            Some(true) => {
                if held.is_empty() && line.as_slice() == HEARTBEAT {
                    shared.answered(machine);
                    continue;
                }
                let terminator = line.as_slice() == TERMINATOR;
                held.push(line.clone());
                if !terminator {
                    if held.len() > MAX_REPORT_LINES {
                        held.clear();
                        tracing::warn!(machine, "a report of a machine is above its bound and is dropped");
                    }
                    continue;
                }
                match parse_report(&held) {
                    Ok(report) => shared.apply_report(machine, &report),
                    Err(error) => tracing::warn!(machine, %error, "a report of a machine is dropped"),
                }
                held.clear();
            }
        }
    }
}

/// One line of `reader` without its newline, with the bound of `limit` bytes:
/// `Some(true)` is a whole line, `Some(false)` is a line above the limit whose rest is
/// read and dropped, and `None` is the end of the stream.
fn read_line_bounded(reader: &mut impl BufRead, line: &mut Vec<u8>, limit: usize) -> std::io::Result<Option<bool>> {
    line.clear();
    let read = {
        let mut limited = reader.take(limit as u64 + 1);
        limited.read_until(b'\n', line)?
    };
    if read == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        return Ok(Some(line.len() <= limit));
    }
    // The line is longer than its bound, or it is the last line of the stream without a
    // newline. The rest is dropped in fixed chunks, so no report grows a buffer here.
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(Some(line.len() <= limit));
        }
        if chunk[..read].contains(&b'\n') {
            return Ok(Some(false));
        }
    }
}

/// Reads the standard error of one `ssh` child to the bound of a refusal, on its own
/// thread, so a child that writes much does not block on a full pipe.
fn read_stderr(mut pipe: ChildStderr) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut tail = String::new();
        let mut reader = BufReader::new(&mut pipe);
        let mut line = Vec::new();
        loop {
            let read = match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if read == 0 {
                break;
            }
            tail.push_str(&String::from_utf8_lossy(&line));
            line.clear();
            if tail.len() > MAX_REFUSAL_BYTES {
                let cut = tail.len() - MAX_REFUSAL_BYTES;
                let mut cut = cut;
                while !tail.is_char_boundary(cut) {
                    cut += 1;
                }
                tail.drain(..cut);
            }
        }
        tail
    })
}
