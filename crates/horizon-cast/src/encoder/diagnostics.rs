use std::{
    io::{ErrorKind, Read},
    sync::mpsc,
    thread,
};

const MAX_DIAGNOSTIC_BYTES: usize = 8192;

pub(super) fn drain(mut stderr: impl Read + Send + 'static) -> mpsc::Receiver<Option<&'static str>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match stderr.read(&mut buffer) {
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let remaining = MAX_DIAGNOSTIC_BYTES - captured.len();
                    captured.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
        let _ = send.send(classify(&captured));
    });
    receive
}

fn classify(stderr: &[u8]) -> Option<&'static str> {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    // Only fixed diagnostic categories leave the worker: raw stderr may contain private data.
    if message.contains("unknown encoder") {
        if message.contains("libx264") {
            Some("FFmpeg does not provide libx264; install an FFmpeg build with that encoder")
        } else if message.contains("h264_nvenc") {
            Some("FFmpeg does not provide h264_nvenc; use CPU encoding or install an NVENC-enabled build")
        } else {
            Some("FFmpeg does not provide the selected H.264 encoder")
        }
    } else if [
        "unrecognized option",
        "error setting option",
        "error parsing option",
        "option not found",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        Some("FFmpeg rejected the selected encoder configuration; check the installed FFmpeg version")
    } else if [
        "cannot load libcuda",
        "cannot load libnvidia",
        "cuda_error",
        "no capable devices",
        "no nvenc capable devices",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        Some("FFmpeg could not initialize NVIDIA encoding; check the driver or use CPU encoding")
    } else if message.contains("invalid frame size") || message.contains("width not divisible") {
        Some("FFmpeg rejected the requested frame dimensions")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, time::Duration};

    #[test]
    fn startup_failures_are_actionable_without_echoing_private_stderr() {
        let reason =
            classify(b"\x1b[31mUnknown encoder 'libx264' /private/path secret=1234\x1b[0m").expect("diagnostic");
        assert!(reason.contains("install"));
        assert!(!reason.contains("secret") && !reason.contains("private") && !reason.contains('\x1b'));
        assert!(
            classify(b"Unrecognized option 'private-option'")
                .expect("configuration")
                .contains("configuration")
        );
        assert_eq!(classify(b"private unknown failure secret=1234"), None);
    }

    #[test]
    fn stderr_is_fully_drained_with_bounded_retention() {
        let mut output = vec![b'x'; MAX_DIAGNOSTIC_BYTES];
        output.extend_from_slice(b"Unknown encoder 'libx264'");
        let result = drain(Cursor::new(output))
            .recv_timeout(Duration::from_secs(1))
            .expect("drained");
        assert_eq!(result, None);
        let result = drain(Cursor::new(b"Unknown encoder 'libx264'".to_vec()))
            .recv_timeout(Duration::from_secs(1))
            .expect("drained");
        assert!(result.expect("diagnostic").contains("libx264"));
    }
}
