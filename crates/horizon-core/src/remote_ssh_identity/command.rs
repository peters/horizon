use super::RemoteSshIdentityError as Error;
use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(5);
const PUBLIC_OUTPUT_LIMIT: u16 = 1024;

pub(super) fn generate(path: &Path) -> Result<(), Error> {
    let mut command = Command::new("ssh-keygen");
    command
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
        .arg(path);
    run(command, DEADLINE).map(|_| ())
}

pub(super) fn public_key(path: &Path) -> Result<String, Error> {
    let mut command = Command::new("ssh-keygen");
    command.args(["-y", "-P", "", "-f"]).arg(path);
    let output = run(command, DEADLINE)?;
    let key = String::from_utf8(output).map_err(|_| Error::InvalidIdentity)?;
    Ok(key.trim_end_matches(['\r', '\n']).into())
}

fn run(mut command: Command, timeout: Duration) -> Result<Vec<u8>, Error> {
    let started = Instant::now();
    let child = command
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::KeyUtilityUnavailable
            } else {
                Error::KeyUtilityFailed
            }
        })?;
    let mut child = OwnedChild(child);
    let stdout = child.0.stdout.take().ok_or(Error::KeyUtilityFailed)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("remote-key-output".into())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout
                .take(u64::from(PUBLIC_OUTPUT_LIMIT) + 1)
                .read_to_end(&mut output)
                .map(|_| output);
            let _ = sender.send(result);
        })
        .map_err(|_| Error::KeyUtilityFailed)?;
    loop {
        if let Some(status) = child.0.try_wait().map_err(|_| Error::KeyUtilityFailed)? {
            if !status.success() {
                return Err(Error::KeyUtilityFailed);
            }
            break;
        }
        if started.elapsed() >= timeout {
            return Err(Error::Deadline);
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = receiver
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| Error::Deadline)?
        .map_err(|_| Error::KeyUtilityFailed)?;
    if output.len() > usize::from(PUBLIC_OUTPUT_LIMIT) {
        return Err(Error::InvalidIdentity);
    }
    Ok(output)
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_missing_and_overlong_command_output_is_redacted() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf private-test-marker >&2; exit 4"]);
        let error = run(command, Duration::from_secs(2)).expect_err("failure");
        assert_eq!(error, Error::KeyUtilityFailed);
        assert!(!format!("{error:?} {error}").contains("private-test-marker"));
        assert_eq!(
            run(Command::new("/nonexistent-horizon-key-utility"), DEADLINE),
            Err(Error::KeyUtilityUnavailable)
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '%02000d' 0"]);
        assert_eq!(run(command, DEADLINE), Err(Error::InvalidIdentity));
    }

    #[test]
    fn command_deadline_is_bounded_and_reaps_its_owned_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 5"]);
        let started = Instant::now();
        assert_eq!(run(command, Duration::from_millis(40)), Err(Error::Deadline));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
