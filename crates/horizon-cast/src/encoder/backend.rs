use crate::{VideoFormat, session::lock};
use std::{
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderBackend {
    Software,
    Nvenc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderSelection {
    pub backend: EncoderBackend,
    pub fallback_reason: Option<String>,
}

impl EncoderBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "libx264",
            Self::Nvenc => "h264_nvenc",
        }
    }

    pub(super) fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::Software => &[
                "-c:v",
                "libx264",
                "-crf",
                "18",
                "-threads",
                "4",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-profile:v",
                "baseline",
                "-x264-params",
                "aud=1:repeat-headers=1",
            ],
            Self::Nvenc => &[
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-tune",
                "ull",
                "-rc",
                "vbr",
                "-cq",
                "18",
                "-b:v",
                "0",
                "-pix_fmt",
                "rgba",
                "-profile:v",
                "baseline",
                "-zerolatency",
                "1",
                "-rc-lookahead",
                "0",
                "-delay",
                "0",
                "-aud",
                "1",
            ],
        }
    }
}

pub(crate) fn select(format: VideoFormat, stop: &AtomicBool, process: &Arc<Mutex<Option<Child>>>) -> EncoderSelection {
    if !cfg!(all(feature = "nvenc", target_os = "linux")) {
        return software(None);
    }
    let (width, height) = format.dimensions();
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("color=black:s={width}x{height}:r=15,format=rgba"),
            "-frames:v",
            "1",
            "-an",
        ])
        .args(EncoderBackend::Nvenc.arguments())
        .args(["-g", "30", "-bf", "0", "-f", "null", "-"]);
    match probe(command, stop, process, Duration::from_secs(5)) {
        Ok(true) => EncoderSelection {
            backend: EncoderBackend::Nvenc,
            fallback_reason: None,
        },
        _ => software(Some("NVENC unavailable; using CPU encoding".into())),
    }
}

fn software(fallback_reason: Option<String>) -> EncoderSelection {
    EncoderSelection {
        backend: EncoderBackend::Software,
        fallback_reason,
    }
}

fn probe(
    mut command: Command,
    stop: &AtomicBool,
    process: &Arc<Mutex<Option<Child>>>,
    timeout: Duration,
) -> std::io::Result<bool> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    *lock(process) = Some(child);
    let deadline = Instant::now() + timeout;
    let result = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break Ok(false);
        }
        match lock(process).as_mut().map(Child::try_wait).transpose() {
            Ok(Some(Some(status))) => break Ok(status.success()),
            Ok(_) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => break Err(error),
        }
    };
    if let Some(mut child) = lock(process).take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[test]
    #[cfg(unix)]
    fn probe_is_bounded_and_reaps_a_stalled_encoder() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let child = Arc::new(Mutex::new(None));
        let started = Instant::now();
        assert!(!probe(command, &AtomicBool::new(false), &child, Duration::from_millis(50)).expect("probe"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(lock(&child).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn probe_failure_and_cancellation_are_not_hardware_success() {
        for cancelled in [false, true] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exit 1"]);
            let child = Arc::new(Mutex::new(None));
            assert!(!probe(command, &AtomicBool::new(cancelled), &child, Duration::from_secs(1)).expect("probe"));
            assert!(lock(&child).is_none());
        }
    }
}
