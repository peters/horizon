use crate::{Error, Result};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn cannot_execute(group: rustix::process::Pid) -> Result<bool> {
    let number = group.as_raw_nonzero().get();
    if let Some(bytes) = rows(number)? {
        return parse(&bytes, number);
    }
    // Empty ps output is not evidence. Require a fresh positive no-group result.
    match rustix::process::test_kill_process_group(group) {
        Err(rustix::io::Errno::SRCH) => Ok(true),
        _ => Err(Error::CleanupUncertain),
    }
}

pub(super) fn retained_leader(group: rustix::process::Pid) -> Result<bool> {
    let group = group.as_raw_nonzero().get();
    let bytes = rows(group)?.ok_or(Error::CleanupUncertain)?;
    Ok(parse_state(&bytes, group)?.leader_zombie)
}

fn rows(group: i32) -> Result<Option<Vec<u8>>> {
    let mut child = Command::new("/bin/ps")
        .args(["-g", &group.to_string(), "-o", "pid=,pgid=,stat="])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::CleanupUncertain)?;
    let output = child.stdout.take().ok_or(Error::CleanupUncertain)?;
    let read = std::thread::Builder::new()
        .name("native-group-state".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            output.take(32769).read_to_end(&mut bytes).map(|_| bytes)
        })
        .map_err(|_| {
            let _ = child.kill();
            let _ = child.wait();
            Error::CleanupUncertain
        })?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = read
        .join()
        .map_err(|_| Error::CleanupUncertain)?
        .map_err(|_| Error::CleanupUncertain)?;
    if !success || bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > 32768 {
        return Err(Error::CleanupUncertain);
    }
    Ok(Some(bytes))
}

struct State {
    stopped: bool,
    leader_zombie: bool,
}

fn parse(bytes: &[u8], group: i32) -> Result<bool> {
    Ok(parse_state(bytes, group)?.stopped)
}

fn parse_state(bytes: &[u8], group: i32) -> Result<State> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::CleanupUncertain)?;
    if text.trim().is_empty() {
        return Err(Error::CleanupUncertain);
    }
    let mut stopped = true;
    let mut leader_zombie = false;
    for line in text.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3
            || !fields[0].parse::<u32>().is_ok_and(|pid| pid > 0)
            || fields[1].parse::<i32>().ok() != Some(group)
            || !fields[2]
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic() || b"<+N".contains(&byte))
        {
            return Err(Error::CleanupUncertain);
        }
        let zombie = fields[2].starts_with('Z');
        stopped &= zombie;
        leader_zombie |= fields[0].parse::<i32>().ok() == Some(group) && zombie;
    }
    Ok(State { stopped, leader_zombie })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zombie_leader_identity_does_not_hide_live_descendants() {
        let state = parse_state(b"123 123 Z\n124 123 S\n", 123).unwrap();
        assert!(state.leader_zombie);
        assert!(!state.stopped);
        assert!(!parse_state(b"124 123 Z\n", 123).unwrap().leader_zombie);
        assert!(!parse_state(b"123 123 S\n", 123).unwrap().leader_zombie);
        assert!(parse_state(b"123 456 Z\n", 123).is_err());
    }

    #[test]
    fn only_positive_zombie_rows_can_acknowledge_the_exact_group() {
        assert_eq!(parse(b"123 123 Z\n124 123 Z+\n", 123), Ok(true));
        assert_eq!(parse(b"123 123 Z\n124 123 S\n", 123), Ok(false));
        for bytes in [
            b"".as_slice(),
            b"123 456 Z\n",
            b"123 123\n",
            b"0 123 Z\n",
            b"123 123 ?\n",
        ] {
            assert_eq!(parse(bytes, 123), Err(Error::CleanupUncertain));
        }
    }
}
