use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::BrowserNetworkRecord;

const QUEUE_CAPACITY: usize = 2_048;
const QUEUE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const WRITE_BUFFER_BYTES: usize = 256 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const FLUSH_RECORDS: usize = 128;
const PRIORITY_ENQUEUE_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
struct WriterMetrics {
    enqueued: AtomicU64,
    written: AtomicU64,
    bytes: AtomicU64,
    dropped: AtomicU64,
    queued_bytes: AtomicU64,
    truncated: AtomicU64,
    file_limit_reached: AtomicBool,
    writer_failed: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WriterSnapshot {
    pub(super) enqueued: u64,
    pub(super) written: u64,
    pub(super) bytes: u64,
    pub(super) dropped: u64,
    pub(super) truncated: u64,
    pub(super) file_limit_reached: bool,
    pub(super) writer_failed: bool,
}

#[derive(Debug)]
pub(super) struct CaptureWriter {
    path: PathBuf,
    sender: Option<SyncSender<BrowserNetworkRecord>>,
    thread: Option<JoinHandle<io::Result<()>>>,
    metrics: Arc<WriterMetrics>,
}

impl CaptureWriter {
    pub(super) fn start(directory: &Path, capture_id: &str, max_file_bytes: u64) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let directory = std::fs::canonicalize(directory)?;
        let path = directory.join(format!("{}.ndjson", safe_file_stem(capture_id)));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&path)?;
        let metrics = Arc::new(WriterMetrics::default());
        let thread_metrics = Arc::clone(&metrics);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("browser-network-capture".to_string())
            .spawn(move || write_records(file, &receiver, max_file_bytes, &thread_metrics));
        match thread {
            Ok(thread) => Ok(Self {
                path,
                sender: Some(sender),
                thread: Some(thread),
                metrics,
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn try_record(&self, record: BrowserNetworkRecord) {
        self.enqueue(record, false);
    }

    pub(super) fn record_priority(&self, record: BrowserNetworkRecord) {
        self.enqueue(record, true);
    }

    fn enqueue(&self, record: BrowserNetworkRecord, priority: bool) {
        if self.metrics.file_limit_reached.load(Ordering::Relaxed) || self.metrics.writer_failed.load(Ordering::Relaxed)
        {
            self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Some(sender) = self.sender.as_ref() else {
            self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        enqueue_record(
            sender,
            &self.metrics,
            record,
            priority,
            if priority {
                PRIORITY_ENQUEUE_TIMEOUT
            } else {
                Duration::ZERO
            },
        );
    }

    pub(super) fn note_truncated(&self) {
        self.metrics.truncated.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn snapshot(&self) -> WriterSnapshot {
        WriterSnapshot {
            enqueued: self.metrics.enqueued.load(Ordering::Relaxed),
            written: self.metrics.written.load(Ordering::Relaxed),
            bytes: self.metrics.bytes.load(Ordering::Relaxed),
            dropped: self.metrics.dropped.load(Ordering::Relaxed),
            truncated: self.metrics.truncated.load(Ordering::Relaxed),
            file_limit_reached: self.metrics.file_limit_reached.load(Ordering::Relaxed),
            writer_failed: self.metrics.writer_failed.load(Ordering::Relaxed),
        }
    }

    pub(super) fn finish(&mut self) -> io::Result<()> {
        self.sender.take();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| io::Error::other("browser network capture writer panicked"))?
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn reserve_queued_bytes(
    metrics: &WriterMetrics,
    record_bytes: u64,
    priority: bool,
    deadline: std::time::Instant,
) -> bool {
    loop {
        if metrics
            .queued_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current
                    .checked_add(record_bytes)
                    .filter(|next| *next <= QUEUE_MAX_BYTES)
            })
            .is_ok()
        {
            return true;
        }
        if !priority || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn enqueue_record(
    sender: &SyncSender<BrowserNetworkRecord>,
    metrics: &WriterMetrics,
    record: BrowserNetworkRecord,
    priority: bool,
    timeout: Duration,
) {
    let record_bytes = estimated_record_bytes(&record);
    let deadline = std::time::Instant::now() + timeout;
    if !reserve_queued_bytes(metrics, record_bytes, priority, deadline) {
        metrics.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut pending = Some(record);
    while let Some(record) = pending.take() {
        match sender.try_send(record) {
            Ok(()) => {
                metrics.enqueued.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Err(TrySendError::Disconnected(_)) => {
                metrics.queued_bytes.fetch_sub(record_bytes, Ordering::Relaxed);
                metrics.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Err(TrySendError::Full(full)) => {
                if !priority || std::time::Instant::now() >= deadline {
                    metrics.queued_bytes.fetch_sub(record_bytes, Ordering::Relaxed);
                    metrics.dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                pending = Some(full);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn write_records(
    file: File,
    receiver: &mpsc::Receiver<BrowserNetworkRecord>,
    max_file_bytes: u64,
    metrics: &WriterMetrics,
) -> io::Result<()> {
    let result = write_records_inner(file, receiver, max_file_bytes, metrics);
    if result.is_err() {
        metrics.writer_failed.store(true, Ordering::Relaxed);
    }
    result
}

fn write_records_inner(
    file: File,
    receiver: &mpsc::Receiver<BrowserNetworkRecord>,
    max_file_bytes: u64,
    metrics: &WriterMetrics,
) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
    let mut unflushed = 0usize;
    loop {
        match receiver.recv_timeout(FLUSH_INTERVAL) {
            Ok(record) => {
                metrics
                    .queued_bytes
                    .fetch_sub(estimated_record_bytes(&record), Ordering::Relaxed);
                write_record(&mut writer, &record, max_file_bytes, metrics)?;
                unflushed += 1;
                if unflushed >= FLUSH_RECORDS {
                    writer.flush()?;
                    unflushed = 0;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if unflushed > 0 {
                    writer.flush()?;
                    unflushed = 0;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    writer.flush()
}

fn estimated_record_bytes(record: &BrowserNetworkRecord) -> u64 {
    let strings = [
        Some(record.capture_id.as_str()),
        record.connection_id.as_deref(),
        record.url.as_deref(),
        record.method.as_deref(),
        record.resource_type.as_deref(),
        record.payload.as_deref(),
        record.error.as_deref(),
    ];
    strings.into_iter().flatten().fold(512u64, |total, value| {
        total.saturating_add(u64::try_from(value.len()).unwrap_or(u64::MAX))
    })
}

fn write_record(
    writer: &mut BufWriter<File>,
    record: &BrowserNetworkRecord,
    max_file_bytes: u64,
    metrics: &WriterMetrics,
) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(record).map_err(io::Error::other)?;
    encoded.push(b'\n');
    let encoded_bytes = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    let current = metrics.bytes.load(Ordering::Relaxed);
    if current.saturating_add(encoded_bytes) > max_file_bytes {
        metrics.file_limit_reached.store(true, Ordering::Relaxed);
        metrics.dropped.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    writer.write_all(&encoded)?;
    metrics.bytes.fetch_add(encoded_bytes, Ordering::Relaxed);
    metrics.written.fetch_add(1, Ordering::Relaxed);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendKind, BrowserNetworkEventKind};

    #[test]
    fn writer_is_private_bounded_and_flushes_on_finish() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let mut writer = CaptureWriter::start(root.path(), "../capture", 2_048)
            .unwrap_or_else(|error| panic!("writer start failed: {error}"));
        let path = writer.path().to_path_buf();
        writer.try_record(BrowserNetworkRecord::empty(
            "capture",
            1,
            BackendKind::ChromiumCdp,
            BrowserNetworkEventKind::CaptureStarted,
        ));
        writer
            .finish()
            .unwrap_or_else(|error| panic!("writer finish failed: {error}"));

        let encoded = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("capture read failed: {error}"));
        assert!(encoded.contains("capture_started"));
        let canonical_root =
            std::fs::canonicalize(root.path()).unwrap_or_else(|error| panic!("canonicalize tempdir failed: {error}"));
        assert_eq!(path.parent(), Some(canonical_root.as_path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    fn sample_record(sequence: u64) -> BrowserNetworkRecord {
        BrowserNetworkRecord::empty(
            "capture",
            sequence,
            BackendKind::ChromiumCdp,
            BrowserNetworkEventKind::HttpResponseBody,
        )
    }

    #[test]
    fn non_priority_enqueue_drops_immediately_when_the_channel_is_full() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let metrics = WriterMetrics::default();
        sender
            .send(sample_record(1))
            .unwrap_or_else(|error| panic!("occupying send failed: {error}"));

        enqueue_record(&sender, &metrics, sample_record(2), false, Duration::ZERO);

        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.queued_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(
            receiver
                .try_recv()
                .unwrap_or_else(|error| panic!("occupying record missing: {error}"))
                .sequence,
            1
        );
    }

    #[test]
    fn priority_enqueue_retries_until_a_full_channel_has_room() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let metrics = WriterMetrics::default();
        sender
            .send(sample_record(1))
            .unwrap_or_else(|error| panic!("occupying send failed: {error}"));
        let drain = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let occupying = receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| panic!("occupying recv failed: {error}"));
            let priority = receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| panic!("priority recv failed: {error}"));
            (occupying.sequence, priority.sequence)
        });

        enqueue_record(&sender, &metrics, sample_record(2), true, Duration::from_secs(1));

        let sequences = drain
            .join()
            .unwrap_or_else(|_| panic!("priority drain thread panicked"));
        assert_eq!(sequences, (1, 2));
        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics.queued_bytes.load(Ordering::Relaxed),
            estimated_record_bytes(&sample_record(2))
        );
    }

    #[test]
    fn priority_enqueue_drops_after_timeout_when_the_channel_stays_full() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let metrics = WriterMetrics::default();
        sender
            .send(sample_record(1))
            .unwrap_or_else(|error| panic!("occupying send failed: {error}"));

        let started = std::time::Instant::now();
        enqueue_record(&sender, &metrics, sample_record(2), true, PRIORITY_ENQUEUE_TIMEOUT);
        let elapsed = started.elapsed();

        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.queued_bytes.load(Ordering::Relaxed), 0);
        assert!(elapsed >= PRIORITY_ENQUEUE_TIMEOUT);
        assert_eq!(
            receiver
                .try_recv()
                .unwrap_or_else(|error| panic!("occupying record missing: {error}"))
                .sequence,
            1
        );
    }

    #[test]
    fn enqueue_drops_when_queued_bytes_would_exceed_the_cap() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let metrics = WriterMetrics::default();
        metrics.queued_bytes.store(QUEUE_MAX_BYTES, Ordering::Relaxed);

        enqueue_record(&sender, &metrics, sample_record(2), true, Duration::ZERO);

        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.queued_bytes.load(Ordering::Relaxed), QUEUE_MAX_BYTES);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn priority_enqueue_retries_until_queued_bytes_have_room() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let metrics = std::sync::Arc::new(WriterMetrics::default());
        metrics.queued_bytes.store(QUEUE_MAX_BYTES, Ordering::Relaxed);
        let release = std::sync::Arc::clone(&metrics);
        let drain = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            release.queued_bytes.store(0, Ordering::Relaxed);
        });

        enqueue_record(&sender, &metrics, sample_record(2), true, Duration::from_secs(1));
        drain
            .join()
            .unwrap_or_else(|_| panic!("queued-bytes release thread panicked"));

        assert_eq!(metrics.enqueued.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics.queued_bytes.load(Ordering::Relaxed),
            estimated_record_bytes(&sample_record(2))
        );
        assert_eq!(
            receiver
                .try_recv()
                .unwrap_or_else(|error| panic!("priority record missing: {error}"))
                .sequence,
            2
        );
    }
}
