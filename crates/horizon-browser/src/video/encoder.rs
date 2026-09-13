//! Dedicated AV1 encode thread sampling `FrameSlot` RGB frames.

use std::fs::{File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use rav1e::prelude::{ChromaSampling, EncoderStatus, FrameType, PixelRange, Rational};
use rav1e::{Config, Context, EncoderConfig, Frame};

use super::convert::{encode_size, rgb_to_yuv420};
use super::muxer::{WebmMuxer, av1_codec_private, sequence_header_obu};
use crate::frames::FrameSlot;
use crate::{BrowserVideoCapture, BrowserVideoCaptureOptions, BrowserVideoState};

const STATE_IDLE: u8 = 0;
const STATE_RECORDING: u8 = 1;
const STATE_PAUSED: u8 = 2;
const STATE_STOPPED: u8 = 3;

#[derive(Debug)]
pub(super) enum EncoderCommand {
    Pause,
    Resume,
    Stop,
}

#[derive(Debug)]
pub struct VideoCaptureHandle {
    state: AtomicU8,
    width: AtomicU32,
    height: AtomicU32,
    fps: AtomicU32,
    frames_encoded: AtomicU64,
    frames_dropped: AtomicU64,
    frames_repeated: AtomicU64,
    bytes_written: AtomicU64,
    file_limit_reached: AtomicBool,
    encoder_failed: AtomicBool,
    started_at_millis: AtomicI64,
    elapsed_millis: AtomicU64,
    capture_id: std::sync::Mutex<String>,
    path: std::sync::Mutex<String>,
}

impl Default for VideoCaptureHandle {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(STATE_IDLE),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            fps: AtomicU32::new(0),
            frames_encoded: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            frames_repeated: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            file_limit_reached: AtomicBool::new(false),
            encoder_failed: AtomicBool::new(false),
            started_at_millis: AtomicI64::new(0),
            elapsed_millis: AtomicU64::new(0),
            capture_id: std::sync::Mutex::new(String::new()),
            path: std::sync::Mutex::new(String::new()),
        }
    }
}

impl VideoCaptureHandle {
    #[must_use]
    pub fn snapshot(&self) -> Option<BrowserVideoCapture> {
        let state = match self.state.load(Ordering::Relaxed) {
            STATE_RECORDING => BrowserVideoState::Recording,
            STATE_PAUSED => BrowserVideoState::Paused,
            STATE_STOPPED => BrowserVideoState::Stopped,
            _ => return None,
        };
        let capture_id = self
            .capture_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if capture_id.is_empty() {
            return None;
        }
        let path = self
            .path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Some(BrowserVideoCapture {
            capture_id,
            path,
            state,
            active: matches!(state, BrowserVideoState::Recording | BrowserVideoState::Paused),
            width: self.width.load(Ordering::Relaxed),
            height: self.height.load(Ordering::Relaxed),
            fps: self.fps.load(Ordering::Relaxed),
            frames_encoded: self.frames_encoded.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            frames_repeated: self.frames_repeated.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            file_limit_reached: self.file_limit_reached.load(Ordering::Relaxed),
            encoder_failed: self.encoder_failed.load(Ordering::Relaxed),
            started_at_millis: self.started_at_millis.load(Ordering::Relaxed),
            elapsed_millis: self.elapsed_millis.load(Ordering::Relaxed),
        })
    }

    fn reset(&self, capture_id: &str, path: &str, fps: u32, started_at_millis: i64) {
        self.state.store(STATE_RECORDING, Ordering::Relaxed);
        self.width.store(0, Ordering::Relaxed);
        self.height.store(0, Ordering::Relaxed);
        self.fps.store(fps, Ordering::Relaxed);
        self.frames_encoded.store(0, Ordering::Relaxed);
        self.frames_dropped.store(0, Ordering::Relaxed);
        self.frames_repeated.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.file_limit_reached.store(false, Ordering::Relaxed);
        self.encoder_failed.store(false, Ordering::Relaxed);
        self.started_at_millis.store(started_at_millis, Ordering::Relaxed);
        self.elapsed_millis.store(0, Ordering::Relaxed);
        *self
            .capture_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = capture_id.to_string();
        *self.path.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = path.to_string();
    }

    fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
    }

    fn clear_active(&self) {
        self.state.store(STATE_IDLE, Ordering::Relaxed);
        self.file_limit_reached.store(false, Ordering::Relaxed);
        self.encoder_failed.store(false, Ordering::Relaxed);
        *self
            .capture_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = String::new();
        *self.path.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = String::new();
    }
}

#[derive(Debug)]
pub(super) struct EncoderThread {
    pub path: PathBuf,
    sender: Option<Sender<EncoderCommand>>,
    thread: Option<JoinHandle<io::Result<()>>>,
    handle: Arc<VideoCaptureHandle>,
}

impl EncoderThread {
    pub(super) fn start(
        directory: &Path,
        capture_id: &str,
        frame_slot: Arc<FrameSlot>,
        options: BrowserVideoCaptureOptions,
        handle: Arc<VideoCaptureHandle>,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let directory = std::fs::canonicalize(directory)?;
        let path = directory.join(format!("{}.webm", safe_file_stem(capture_id)));
        let mut open = OpenOptions::new();
        open.create_new(true).read(true).write(true);
        #[cfg(unix)]
        open.mode(0o600);
        let file = open.open(&path)?;
        let started_at_millis = system_now_millis();
        handle.reset(capture_id, &path.to_string_lossy(), options.fps, started_at_millis);
        let (sender, receiver) = mpsc::channel();
        let thread_handle = Arc::clone(&handle);
        let thread_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("browser-video-encode".to_string())
            .spawn(move || encode_loop(file, frame_slot, options, receiver, thread_handle));
        match thread {
            Ok(thread) => Ok(Self {
                path: thread_path,
                sender: Some(sender),
                thread: Some(thread),
                handle,
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                handle.clear_active();
                Err(error)
            }
        }
    }

    pub(super) fn send(&self, command: EncoderCommand) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(command);
        }
    }

    pub(super) fn snapshot(&self, state: BrowserVideoState) -> BrowserVideoCapture {
        let mut capture = self.handle.snapshot().unwrap_or_else(|| BrowserVideoCapture {
            capture_id: String::new(),
            path: self.path.to_string_lossy().into_owned(),
            state,
            active: matches!(state, BrowserVideoState::Recording | BrowserVideoState::Paused),
            width: 0,
            height: 0,
            fps: 0,
            frames_encoded: 0,
            frames_dropped: 0,
            frames_repeated: 0,
            bytes_written: 0,
            file_limit_reached: false,
            encoder_failed: false,
            started_at_millis: 0,
            elapsed_millis: 0,
        });
        capture.state = state;
        capture.active = matches!(state, BrowserVideoState::Recording | BrowserVideoState::Paused);
        capture
    }

    pub(super) fn is_finished(&self) -> bool {
        self.thread.as_ref().is_some_and(std::thread::JoinHandle::is_finished)
    }

    pub(super) fn finish(mut self) -> io::Result<BrowserVideoCapture> {
        self.send(EncoderCommand::Stop);
        self.sender.take();
        let joined = self.thread.take().map(|thread| {
            thread
                .join()
                .map_err(|_| io::Error::other("browser video encoder panicked"))
        });
        self.handle.set_state(STATE_STOPPED);
        match joined {
            Some(Ok(Ok(()))) | None => Ok(self.snapshot(BrowserVideoState::Stopped)),
            Some(Ok(Err(error)) | Err(error)) => {
                self.handle.encoder_failed.store(true, Ordering::Relaxed);
                Err(error)
            }
        }
    }
}

impl Drop for EncoderThread {
    fn drop(&mut self) {
        self.send(EncoderCommand::Stop);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.handle.set_state(STATE_STOPPED);
    }
}

fn encode_loop(
    file: File,
    frame_slot: Arc<FrameSlot>,
    options: BrowserVideoCaptureOptions,
    commands: Receiver<EncoderCommand>,
    handle: Arc<VideoCaptureHandle>,
) -> io::Result<()> {
    let mut session = EncodeSession {
        file: Some(file),
        encoder: None,
        frame_slot,
        options,
        handle,
        commands,
        started: Instant::now(),
        paused_at: None,
        paused_total: Duration::ZERO,
        paused: false,
        next_tick: Instant::now(),
        tick: 0,
        last_seq: 0,
    };
    session.run()
}

struct EncodeSession {
    file: Option<File>,
    encoder: Option<ActiveEncoder>,
    frame_slot: Arc<FrameSlot>,
    options: BrowserVideoCaptureOptions,
    handle: Arc<VideoCaptureHandle>,
    commands: Receiver<EncoderCommand>,
    started: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
    paused: bool,
    next_tick: Instant,
    tick: u64,
    last_seq: u64,
}

impl EncodeSession {
    fn run(&mut self) -> io::Result<()> {
        let run_result = loop {
            if !self.drain_commands() {
                break Ok(());
            }
            self.handle.elapsed_millis.store(
                elapsed_millis(self.started, self.paused_total, self.paused_at),
                Ordering::Relaxed,
            );
            if self.paused {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if !self.wait_for_tick() {
                continue;
            }
            if let Err(error) = self.encode_tick() {
                break Err(error);
            }
            if self.handle.file_limit_reached.load(Ordering::Relaxed) {
                break Ok(());
            }
        };
        let finish_result = self.finish_file();
        self.handle.set_state(STATE_STOPPED);
        match (run_result, finish_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (Ok(()), Err(error)) => {
                self.handle.encoder_failed.store(true, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    fn drain_commands(&mut self) -> bool {
        match self.commands.try_recv() {
            Ok(EncoderCommand::Stop) | Err(mpsc::TryRecvError::Disconnected) => false,
            Ok(EncoderCommand::Pause) => {
                if !self.paused {
                    self.paused = true;
                    self.paused_at = Some(Instant::now());
                    if let Some(encoder) = self.encoder.as_mut() {
                        if let Err(error) = encoder.flush_cluster() {
                            self.handle.encoder_failed.store(true, Ordering::Relaxed);
                            tracing::warn!(target: "browser", "failed to flush video cluster on pause: {error}");
                        } else if let Some(bytes) = encoder.bytes_written() {
                            self.handle.bytes_written.store(bytes, Ordering::Relaxed);
                        }
                    }
                    self.handle.set_state(STATE_PAUSED);
                }
                true
            }
            Ok(EncoderCommand::Resume) => {
                if self.paused {
                    self.paused = false;
                    if let Some(at) = self.paused_at.take() {
                        self.paused_total += Instant::now().saturating_duration_since(at);
                    }
                    self.next_tick = Instant::now();
                    self.handle.set_state(STATE_RECORDING);
                }
                true
            }
            Err(mpsc::TryRecvError::Empty) => true,
        }
    }

    fn wait_for_tick(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_tick {
            std::thread::sleep((self.next_tick - now).min(Duration::from_millis(20)));
            return false;
        }
        let fps = self.options.fps.max(1);
        let interval = Duration::from_millis(1_000 / u64::from(fps));
        while self.next_tick + interval <= Instant::now() {
            self.handle.frames_dropped.fetch_add(1, Ordering::Relaxed);
            self.tick += 1;
            self.next_tick += interval;
        }
        self.next_tick += interval;
        true
    }

    fn encode_tick(&mut self) -> io::Result<()> {
        let fps = self.options.fps.max(1);
        let timestamp_ms = self.tick.saturating_mul(1_000) / u64::from(fps);
        self.tick += 1;
        let Some(frame) = self.frame_slot.latest() else {
            self.handle.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        if frame.seq == self.last_seq {
            self.handle.frames_repeated.fetch_add(1, Ordering::Relaxed);
        }
        self.last_seq = frame.seq;
        if self.encoder.is_none() {
            let (width, height) = encode_size(frame.width, frame.height, self.options.max_width);
            let file = self
                .file
                .take()
                .ok_or_else(|| io::Error::other("video file already taken"))?;
            let encoder = ActiveEncoder::new(file, width, height, &self.options)?;
            self.handle.width.store(width, Ordering::Relaxed);
            self.handle.height.store(height, Ordering::Relaxed);
            self.encoder = Some(encoder);
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Ok(());
        };
        match encoder.encode_rgb(&frame.rgb, frame.width, frame.height, timestamp_ms)? {
            PushOutcome::Written => {}
            PushOutcome::FileLimit => {
                self.handle.file_limit_reached.store(true, Ordering::Relaxed);
            }
        }
        self.handle.frames_encoded.store(encoder.muxed, Ordering::Relaxed);
        if let Some(bytes) = encoder.bytes_written() {
            self.handle.bytes_written.store(bytes, Ordering::Relaxed);
        }
        Ok(())
    }

    fn finish_file(&mut self) -> io::Result<()> {
        if let Some(encoder) = self.encoder.take() {
            match encoder.finish() {
                Ok(stats) => {
                    self.handle.bytes_written.store(stats.bytes, Ordering::Relaxed);
                    self.handle.frames_encoded.store(stats.muxed, Ordering::Relaxed);
                    if stats.limit_reached {
                        self.handle.file_limit_reached.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                }
                Err(error) => {
                    self.handle.encoder_failed.store(true, Ordering::Relaxed);
                    Err(error)
                }
            }
        } else if let Some(file) = self.file.take() {
            let bytes = write_empty_webm(file)?;
            self.handle.bytes_written.store(bytes, Ordering::Relaxed);
            Ok(())
        } else {
            Ok(())
        }
    }
}

enum PushOutcome {
    Written,
    FileLimit,
}

struct ActiveEncoder {
    context: Context<u8>,
    muxer: Option<WebmMuxer>,
    file: Option<File>,
    width: u32,
    height: u32,
    max_file_bytes: u64,
    fps: u32,
    muxed: u64,
    timestamps: Vec<u64>,
    pending: Vec<(u64, bool, Vec<u8>)>,
    limit_reached: bool,
}

impl ActiveEncoder {
    fn new(file: File, width: u32, height: u32, options: &BrowserVideoCaptureOptions) -> io::Result<Self> {
        let mut encoder = EncoderConfig::with_speed_preset(speed_preset(options.compression_level));
        encoder.width = usize::try_from(width).unwrap_or(16);
        encoder.height = usize::try_from(height).unwrap_or(16);
        encoder.bit_depth = 8;
        encoder.chroma_sampling = ChromaSampling::Cs420;
        encoder.pixel_range = PixelRange::Full;
        encoder.quantizer = quality_to_quantizer(options.quality);
        encoder.bitrate = 0;
        encoder.low_latency = true;
        encoder.time_base = Rational {
            num: 1,
            den: u64::from(options.fps.max(1)),
        };
        let fps = u64::from(options.fps.max(1));
        encoder.set_key_frame_interval(fps, fps.saturating_mul(2));
        encoder.tiles = 1;
        let context = Config::new()
            .with_encoder_config(encoder)
            .with_threads(1)
            .new_context()
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self {
            context,
            muxer: None,
            file: Some(file),
            width,
            height,
            max_file_bytes: options.max_file_bytes,
            fps: options.fps.max(1),
            muxed: 0,
            timestamps: Vec::new(),
            pending: Vec::new(),
            limit_reached: false,
        })
    }

    fn encode_rgb(
        &mut self,
        rgb: &[u8],
        src_width: u32,
        src_height: u32,
        timestamp_ms: u64,
    ) -> io::Result<PushOutcome> {
        let yuv = rgb_to_yuv420(rgb, src_width, src_height, self.width, self.height);
        let mut frame = self.context.new_frame();
        fill_plane(&mut frame, 0, &yuv.y, usize::try_from(self.width).unwrap_or(16));
        fill_plane(&mut frame, 1, &yuv.u, usize::try_from(self.width).unwrap_or(16) / 2);
        fill_plane(&mut frame, 2, &yuv.v, usize::try_from(self.width).unwrap_or(16) / 2);
        self.timestamps.push(timestamp_ms);
        self.context
            .send_frame(frame)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.drain_packets()
    }

    fn timestamp_for(&self, frameno: u64) -> u64 {
        usize::try_from(frameno)
            .ok()
            .and_then(|index| self.timestamps.get(index).copied())
            .or_else(|| self.timestamps.last().copied())
            .unwrap_or(0)
    }

    fn drain_packets(&mut self) -> io::Result<PushOutcome> {
        let mut outcome = PushOutcome::Written;
        loop {
            match self.context.receive_packet() {
                Ok(packet) => {
                    if self.limit_reached {
                        outcome = PushOutcome::FileLimit;
                        continue;
                    }
                    let keyframe = packet.frame_type == FrameType::KEY;
                    let timestamp_ms = self.timestamp_for(packet.input_frameno);
                    match self.push_packet(timestamp_ms, keyframe, packet.data)? {
                        PushOutcome::FileLimit => outcome = PushOutcome::FileLimit,
                        PushOutcome::Written => {}
                    }
                }
                Err(EncoderStatus::Encoded | EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => break,
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
        Ok(outcome)
    }

    fn push_packet(&mut self, timestamp_ms: u64, keyframe: bool, data: Vec<u8>) -> io::Result<PushOutcome> {
        if self.limit_reached {
            return Ok(PushOutcome::FileLimit);
        }
        let extra = u64::try_from(data.len()).unwrap_or(u64::MAX).saturating_add(16);
        if self.muxer.is_none() {
            if !keyframe {
                self.pending.push((timestamp_ms, keyframe, data));
                return Ok(PushOutcome::Written);
            }
            let file = self
                .file
                .take()
                .ok_or_else(|| io::Error::other("video muxer file already taken"))?;
            let private = av1_codec_private(&sequence_header_obu(&data));
            let mut muxer = WebmMuxer::create(file, self.width, self.height, &private)?;
            if muxer.would_exceed(extra, self.max_file_bytes)? {
                self.limit_reached = true;
                self.muxer = Some(muxer);
                return Ok(PushOutcome::FileLimit);
            }
            muxer.write_frame(timestamp_ms, true, &data)?;
            self.muxed = self.muxed.saturating_add(1);
            for (pending_ts, pending_key, pending_data) in self.pending.drain(..) {
                let pending_extra = u64::try_from(pending_data.len()).unwrap_or(u64::MAX).saturating_add(16);
                if muxer.would_exceed(pending_extra, self.max_file_bytes)? {
                    self.limit_reached = true;
                    self.muxer = Some(muxer);
                    return Ok(PushOutcome::FileLimit);
                }
                muxer.write_frame(pending_ts, pending_key, &pending_data)?;
                self.muxed = self.muxed.saturating_add(1);
            }
            self.muxer = Some(muxer);
            return Ok(PushOutcome::Written);
        }
        if let Some(muxer) = self.muxer.as_mut() {
            if muxer.would_exceed(extra, self.max_file_bytes)? {
                self.limit_reached = true;
                return Ok(PushOutcome::FileLimit);
            }
            muxer.write_frame(timestamp_ms, keyframe, &data)?;
            self.muxed = self.muxed.saturating_add(1);
        }
        Ok(PushOutcome::Written)
    }

    fn bytes_written(&mut self) -> Option<u64> {
        self.muxer.as_mut().and_then(|muxer| muxer.bytes_written().ok())
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        if let Some(muxer) = self.muxer.as_mut() {
            muxer.flush_cluster()?;
        }
        Ok(())
    }

    fn finish(mut self) -> io::Result<FinishStats> {
        self.context.flush();
        for _ in 0..1_024 {
            match self.context.receive_packet() {
                Ok(packet) => {
                    if self.limit_reached {
                        continue;
                    }
                    let keyframe = packet.frame_type == FrameType::KEY;
                    let timestamp_ms = self.timestamp_for(packet.input_frameno);
                    let _ = self.push_packet(timestamp_ms, keyframe, packet.data)?;
                }
                Err(EncoderStatus::Encoded) => {}
                Err(EncoderStatus::LimitReached | EncoderStatus::NeedMoreData) => break,
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
        let frame_ms = 1_000 / u64::from(self.fps.max(1));
        let bytes = if let Some(muxer) = self.muxer.take() {
            muxer.finish(frame_ms)?
        } else if let Some(file) = self.file.take() {
            write_empty_webm(file)?
        } else {
            0
        };
        Ok(FinishStats {
            bytes,
            muxed: self.muxed,
            limit_reached: self.limit_reached,
        })
    }
}

struct FinishStats {
    bytes: u64,
    muxed: u64,
    limit_reached: bool,
}

fn fill_plane(frame: &mut Frame<u8>, plane: usize, source: &[u8], stride: usize) {
    frame.planes[plane].copy_from_raw_u8(source, stride, 1);
}

fn write_empty_webm(file: File) -> io::Result<u64> {
    let muxer = WebmMuxer::create(file, 16, 16, &[0x81, 0x1F, 0x0C, 0x00])?;
    muxer.finish(1)
}

fn quality_to_quantizer(quality: u32) -> usize {
    let quality = quality.clamp(1, 100);
    usize::try_from(((100 - quality) * 255) / 99).unwrap_or(76)
}

fn speed_preset(compression_level: u32) -> u8 {
    u8::try_from(10_u32.saturating_sub(compression_level.min(10))).unwrap_or(6)
}

fn elapsed_millis(started: Instant, paused_total: Duration, paused_at: Option<Instant>) -> u64 {
    let running = paused_at.map_or_else(
        || Instant::now().saturating_duration_since(started),
        |paused_at| paused_at.saturating_duration_since(started),
    );
    u64::try_from(running.saturating_sub(paused_total).as_millis()).unwrap_or(u64::MAX)
}

fn system_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(0))
}

fn safe_file_stem(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len().saturating_mul(2).saturating_add(1));
    encoded.push('%');
    for byte in value.bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
