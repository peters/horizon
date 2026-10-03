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
    NvencCuda,
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
            Self::Nvenc | Self::NvencCuda => "h264_nvenc",
        }
    }

    #[must_use]
    pub const fn scaler(self) -> &'static str {
        if self.source_frames() { "cuda" } else { "cpu" }
    }

    #[must_use]
    pub const fn source_frames(self) -> bool {
        matches!(self, Self::NvencCuda)
    }

    pub(super) fn input_arguments(self, format: VideoFormat) -> Vec<String> {
        let (width, height) = format.dimensions();
        let mut arguments: Vec<String> = ["-hide_banner", "-loglevel", "error", "-f"].map(String::from).into();
        if self.source_frames() {
            arguments.extend(["image2pipe", "-vcodec", "pam"].map(String::from));
            arguments.extend(["-probesize", "32", "-analyzeduration", "0"].map(String::from));
        } else {
            arguments.extend(["rawvideo", "-pixel_format", "rgba", "-video_size"].map(String::from));
            arguments.push(format!("{width}x{height}"));
        }
        arguments.extend(["-framerate", "15", "-i", "pipe:0", "-an"].map(String::from));
        if self.source_frames() {
            arguments.push("-vf".into());
            // NV12 conversion precedes CUDA upload; CPU padding follows the scaled download.
            arguments.push(format!("format=nv12,hwupload_cuda,scale_cuda={width}:{height}:format=nv12:interp_algo=nearest:force_original_aspect_ratio=decrease:force_divisible_by=2,hwdownload,format=nv12,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:black"));
            arguments.extend(["-fps_mode", "passthrough"].map(String::from));
        }
        arguments
    }

    pub(super) fn arguments(self) -> Vec<&'static str> {
        let arguments: &[&str] = match self {
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
                "-profile:v",
                "baseline",
                "-x264-params",
                "aud=1:repeat-headers=1",
            ],
            Self::Nvenc | Self::NvencCuda => &[
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
        };
        let mut arguments = arguments.to_vec();
        arguments.extend([
            "-pix_fmt",
            match self {
                Self::Software => "yuv420p",
                Self::Nvenc => "rgba",
                Self::NvencCuda => "nv12",
            },
        ]);
        arguments
    }
}

pub(crate) fn select(format: VideoFormat, stop: &AtomicBool, process: &Arc<Mutex<Option<Child>>>) -> EncoderSelection {
    if !cfg!(all(feature = "nvenc", target_os = "linux")) {
        return software(None);
    }
    let (width, height) = format.dimensions();
    let mut accelerated = Command::new("ffmpeg");
    accelerated
        .args(EncoderBackend::NvencCuda.input_arguments(format))
        .args(EncoderBackend::NvencCuda.arguments())
        .args(["-frames:v", "1", "-g", "30", "-bf", "0", "-f", "null", "-"]);
    let source = super::Frame::new(17, 13, [0, 0, 0, 255].repeat(17 * 13)).ok();
    if matches!(
        probe(accelerated, stop, process, source, Duration::from_secs(5)),
        Ok(true)
    ) {
        return EncoderSelection {
            backend: EncoderBackend::NvencCuda,
            fallback_reason: None,
        };
    }
    if stop.load(Ordering::Relaxed) {
        return software(None);
    }
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
    match probe(command, stop, process, None, Duration::from_secs(5)) {
        Ok(true) => EncoderSelection {
            backend: EncoderBackend::Nvenc,
            fallback_reason: Some("CUDA scaling unavailable; using CPU scaling".into()),
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
    source: Option<super::Frame>,
    timeout: Duration,
) -> std::io::Result<bool> {
    let mut child = command
        .stdin(if source.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take();
    *lock(process) = Some(child);
    // The qualification crop is tiny and fits in the empty pipe; cancellation can kill the child.
    let written = match (source, input) {
        (Some(frame), Some(mut input)) => frame.write(&mut input, true),
        (None, _) => Ok(()),
        _ => Err(std::io::Error::other("probe input unavailable")),
    };
    let deadline = Instant::now() + timeout;
    let result = loop {
        if written.is_err() || stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
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
        assert!(
            !probe(
                command,
                &AtomicBool::new(false),
                &child,
                None,
                Duration::from_millis(50)
            )
            .expect("probe")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(lock(&child).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn source_probe_is_bounded_when_the_encoder_never_reads_input() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let child = Arc::new(Mutex::new(None));
        let source = super::super::Frame::new(17, 13, vec![0; 17 * 13 * 4]).expect("probe crop");
        let started = Instant::now();
        assert!(
            !probe(
                command,
                &AtomicBool::new(false),
                &child,
                Some(source),
                Duration::from_millis(50)
            )
            .expect("probe")
        );
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
            assert!(
                !probe(
                    command,
                    &AtomicBool::new(cancelled),
                    &child,
                    None,
                    Duration::from_secs(1)
                )
                .expect("probe")
            );
            assert!(lock(&child).is_none());
        }
    }
}
