//! Native file input through the toolkit's existing clipboard data device.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

use sctk::data_device_manager::ReadPipe;
use sctk::data_device_manager::data_offer::DragOffer;
use sctk::reexports::calloop::channel::Sender;
use sctk::reexports::client::Proxy;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

type Wake = Arc<dyn Fn() + Send + Sync>;
type Registry = HashMap<usize, Vec<Weak<Hub>>>;
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(Mutex::default)
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// File input; surface positions are Wayland logical coordinates.
#[derive(Debug)]
pub enum Event {
    /// Discard pending native transfers after worker loss or queue backpressure.
    Reset,
    PasteCancelled {
        recipient: u64,
    },
    Motion {
        surface: u64,
        position: [f64; 2],
        entered: bool,
    },
    Leave {
        surface: u64,
    },
    Drop {
        surface: u64,
        position: [f64; 2],
        mime: String,
        bytes: Vec<u8>,
    },
    Paste {
        recipient: u64,
        mime: String,
        bytes: Vec<u8>,
        text: Option<String>,
        fallback: Option<TextFallback>,
    },
}

pub(crate) struct Hub {
    alive: AtomicBool,
    generation: AtomicU64,
    commands: Sender<crate::worker::Command>,
    events: Mutex<VecDeque<Event>>,
    available: Mutex<HashMap<u64, bool>>,
    wake: Mutex<Option<Wake>>,
}
impl Hub {
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    pub(crate) fn stop(&self) {
        self.alive.store(false, Ordering::Release);
        let wake = lock(&self.wake).clone();
        if let Some(wake) = wake {
            wake();
        }
    }
    pub(crate) fn register(display: usize, commands: Sender<crate::worker::Command>) -> Arc<Self> {
        let hub = Arc::new(Self {
            alive: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            commands,
            events: Mutex::default(),
            available: Mutex::default(),
            wake: Mutex::default(),
        });
        let mut entries = lock(registry());
        entries.retain(|_, hubs| {
            hubs.retain(|hub| hub.strong_count() > 0);
            !hubs.is_empty()
        });
        let hubs = entries.entry(display).or_default();
        hubs.retain(|hub| hub.strong_count() > 0);
        hubs.push(Arc::downgrade(&hub));
        hub
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    fn reset(&self) {
        {
            let mut events = lock(&self.events);
            self.generation.fetch_add(1, Ordering::AcqRel);
            events.clear();
        }
        let _ = self.commands.send(crate::worker::Command::ResetNative);
    }
    #[cfg(test)]
    pub(crate) fn emit(&self, event: Event) -> bool {
        self.emit_at(self.generation(), event)
    }
    pub(crate) fn emit_at(&self, generation: u64, event: Event) -> bool {
        let mut events = lock(&self.events);
        if generation != self.generation() {
            return false;
        }
        if matches!((&event, events.back()), (Event::Motion { surface, entered: false, .. }, Some(Event::Motion { surface: old_surface, entered: false, .. })) if surface == old_surface)
        {
            events.pop_back();
        }
        let bytes = |event: &Event| match event {
            Event::Drop { bytes, .. } => bytes.len(),
            Event::Paste {
                bytes, text, fallback, ..
            } => {
                bytes.len()
                    + text.as_ref().map_or(0, String::len)
                    + fallback.as_ref().map_or(0, |pending| pending.bytes.len())
            }
            _ => 0,
        };
        let accepted =
            events.len() < 256 && events.iter().map(bytes).sum::<usize>() + bytes(&event) <= 32 * 1024 * 1024;
        if accepted {
            events.push_back(event);
        } else {
            // A bounded queue must explicitly cancel its batch rather than
            // leave a claimed paste or drag waiting for an event we discarded.
            events.clear();
            self.generation.fetch_add(1, Ordering::AcqRel);
            events.push_back(Event::Reset);
        }
        drop(events);
        if !accepted {
            let _ = self.commands.send(crate::worker::Command::ResetNative);
        }
        let wake = lock(&self.wake).clone();
        if let Some(wake) = wake {
            wake();
        }
        accepted
    }
    pub(crate) fn selection(&self, surface: u64, available: bool) {
        let mut selections = lock(&self.available);
        if available {
            selections.insert(surface, true);
        } else {
            selections.remove(&surface);
        }
    }
    pub(crate) fn replace_selections(&self, surfaces: impl IntoIterator<Item = u64>) {
        *lock(&self.available) = surfaces.into_iter().map(|surface| (surface, true)).collect();
    }
}

pub(crate) struct WorkerGuard(pub Arc<Hub>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// Subscribe without opening another connection or binding another data device.
pub struct Subscription {
    display: usize,
    hubs: Vec<Arc<Hub>>,
    wake: Wake,
    reset_pending: bool,
}
impl Subscription {
    /// `display` is an identity key only and is never dereferenced.
    pub fn new(display: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            display,
            hubs: Vec::new(),
            wake: Arc::new(wake),
            reset_pending: false,
        }
    }
    pub fn refresh(&mut self) {
        self.hubs.retain(|hub| {
            let alive = hub.is_alive();
            self.reset_pending |= !alive;
            alive
        });
        let mut entries = lock(registry());
        let Some(hubs) = entries.get_mut(&self.display) else {
            return;
        };
        hubs.retain(|hub| hub.strong_count() > 0);
        for hub in hubs
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|hub| hub.alive.load(Ordering::Acquire))
        {
            if !self.hubs.iter().any(|known| Arc::ptr_eq(known, &hub)) {
                *lock(&hub.wake) = Some(Arc::clone(&self.wake));
                self.hubs.push(hub);
            }
        }
    }
    pub fn poll(&mut self) -> Vec<Event> {
        self.refresh();
        let mut events = Vec::new();
        if std::mem::take(&mut self.reset_pending) {
            events.push(Event::Reset);
        }
        events.extend(
            self.hubs
                .iter()
                .flat_map(|hub| lock(&hub.events).drain(..).collect::<Vec<_>>()),
        );
        events
    }
    /// Cancel native transfers at a session boundary, preserving text clipboard state.
    pub fn resetter(&self) -> impl Fn() + Send + Sync + 'static {
        let display = self.display;
        move || {
            let hubs: Vec<_> = lock(registry())
                .get(&display)
                .into_iter()
                .flatten()
                .filter_map(Weak::upgrade)
                .collect();
            for hub in hubs {
                hub.reset();
            }
        }
    }
    #[must_use]
    pub fn request_paste(&self, surface: u64, recipient: u64) -> bool {
        self.hubs
            .iter()
            .find(|hub| lock(&hub.available).get(&surface) == Some(&true))
            .is_some_and(|hub| {
                hub.commands
                    .send(crate::worker::Command::NativeRead {
                        surface,
                        recipient,
                        generation: hub.generation(),
                    })
                    .is_ok()
            })
    }
}

pub(crate) fn preferred(mimes: &[String]) -> Option<String> {
    ["text/uri-list", "image/png", "image/jpeg"]
        .into_iter()
        .find(|mime| mimes.iter().any(|offered| offered == mime))
        .map(str::to_owned)
}

pub(crate) fn preferred_clipboard(mimes: &[String]) -> Option<String> {
    ["image/png", "image/jpeg"]
        .into_iter()
        .find(|mime| mimes.iter().any(|offered| offered == mime))
        .map(str::to_owned)
        .or_else(|| preferred(mimes))
}
pub(crate) fn preferred_text(mimes: &[String]) -> Option<String> {
    crate::mime::MimeType::find_allowed(mimes).map(|mime| mime.to_string())
}
#[must_use]
pub fn is_text_mime(mime: &str) -> bool {
    crate::mime::ALLOWED_MIME_TYPES.contains(&mime)
}
/// Decode text with the toolkit's lossy UTF-8 and MIME-specific newline rules.
#[must_use]
pub fn decode_clipboard_text(mime: &str, bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if mime == "UTF8_STRING" {
        text.into_owned()
    } else {
        crate::mime::normalize_to_lf(&text)
    }
}
/// Same-offer text pipe handed to the file worker, used only after native decoding fails.
#[derive(Debug)]
pub struct TextFallback {
    reader: File,
    bytes: Vec<u8>,
    mime: String,
    deadline: Instant,
    limit: usize,
}
impl TextFallback {
    /// Read the bounded fallback off the UI thread, until EOF or its original deadline.
    #[must_use]
    pub fn read_text(mut self) -> Option<String> {
        while Instant::now() < self.deadline {
            match read_bounded(&mut self.reader, &mut self.bytes, self.limit) {
                Ok(true) => return (!self.bytes.is_empty()).then(|| decode_clipboard_text(&self.mime, &self.bytes)),
                Ok(false) => std::thread::sleep(Duration::from_millis(16)),
                Err(_) => return None,
            }
        }
        None
    }
}

pub(crate) enum Destination {
    Drop(DragOffer),
    Paste(u64),
}
struct Pending {
    reader: File,
    mime: String,
    destination: Destination,
    bytes: Vec<u8>,
    deadline: Instant,
    generation: u64,
    complete: bool,
    fallback: Option<TextRead>,
}
struct TextRead {
    mime: String,
    reader: File,
    bytes: Vec<u8>,
    complete: bool,
}

fn read_bounded(reader: &mut File, bytes: &mut Vec<u8>, limit: usize) -> std::io::Result<bool> {
    let mut buffer = [0; 8192];
    for _ in 0..16 {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) if bytes.len() + count <= limit => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
            _ => return Err(std::io::ErrorKind::InvalidData.into()),
        }
    }
    Ok(false)
}
pub(crate) struct Transfers {
    pub hub: Arc<Hub>,
    pub drags: HashMap<u32, (u64, u64)>,
    reads: Vec<Pending>,
}
impl Transfers {
    pub fn new(hub: Arc<Hub>) -> Self {
        Self {
            hub,
            reads: Vec::new(),
            drags: HashMap::new(),
        }
    }
    pub fn reset(&mut self) {
        for read in std::mem::take(&mut self.reads) {
            self.cancel_at(read.destination, read.generation);
        }
        self.drags.clear();
    }
    pub fn cancel_at(&self, destination: Destination, generation: u64) {
        match destination {
            Destination::Drop(offer) => {
                self.hub.emit_at(
                    generation,
                    Event::Leave {
                        surface: offer.surface.id().as_ptr() as u64,
                    },
                );
                offer.destroy();
            }
            Destination::Paste(recipient) => {
                self.hub.emit_at(generation, Event::PasteCancelled { recipient });
            }
        }
    }
    pub fn receive(&mut self, pipe: ReadPipe, mime: String, destination: Destination, generation: u64) -> bool {
        if self.reads.len() >= 4 {
            self.cancel_at(destination, generation);
            return false;
        }
        let fd: OwnedFd = pipe.into();
        if rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK).is_err() {
            self.cancel_at(destination, generation);
            return false;
        }
        self.reads.push(Pending {
            reader: fd.into(),
            mime,
            destination,
            bytes: Vec::new(),
            deadline: Instant::now() + Duration::from_secs(10),
            generation,
            complete: false,
            fallback: None,
        });
        true
    }
    pub fn receive_paste(
        &mut self,
        pipe: ReadPipe,
        mime: String,
        recipient: u64,
        generation: u64,
        text: Option<(ReadPipe, String)>,
    ) {
        if self.receive(pipe, mime, Destination::Paste(recipient), generation)
            && let Some((pipe, text_mime)) = text
        {
            let fd: OwnedFd = pipe.into();
            if rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK).is_ok()
                && let Some(read) = self.reads.last_mut()
            {
                read.fallback = Some(TextRead {
                    mime: text_mime,
                    reader: fd.into(),
                    bytes: Vec::new(),
                    complete: false,
                });
            }
        }
    }
    pub fn pending(&self) -> bool {
        !self.reads.is_empty()
    }
    pub fn action(&mut self, offer: &DragOffer) {
        for read in &mut self.reads {
            if let Destination::Drop(pending) = &mut read.destination
                && pending.inner() == offer.inner()
            {
                pending.selected_action = offer.selected_action;
            }
        }
    }
    pub fn poll(&mut self) {
        for mut read in std::mem::take(&mut self.reads) {
            let expired = Instant::now() >= read.deadline;
            if !expired && !read.complete {
                let limit = 32 * 1024 * 1024 - read.fallback.as_ref().map_or(0, |text| text.bytes.len());
                if let Ok(complete) = read_bounded(&mut read.reader, &mut read.bytes, limit) {
                    read.complete = complete;
                } else {
                    read.bytes.clear();
                    read.complete = true;
                }
            }
            if !expired
                && let Some(text) = &mut read.fallback
                && !text.complete
            {
                if let Ok(complete) =
                    read_bounded(&mut text.reader, &mut text.bytes, 32 * 1024 * 1024 - read.bytes.len())
                {
                    text.complete = complete;
                } else {
                    text.bytes.clear();
                    text.complete = true;
                }
            }
            if expired
                && !read.complete
                && read
                    .fallback
                    .as_ref()
                    .is_some_and(|text| text.complete && !text.bytes.is_empty())
            {
                read.bytes.clear();
                read.complete = true;
            }
            if expired && !read.complete {
                self.cancel_at(read.destination, read.generation);
            } else if !read.complete {
                self.reads.push(read);
            } else {
                match read.destination {
                    Destination::Drop(offer) => {
                        if (offer.inner().version() < 3
                            || offer.selected_action
                                == sctk::reexports::client::protocol::wl_data_device_manager::DndAction::Copy)
                            && !read.bytes.is_empty()
                        {
                            let accepted = self.hub.emit_at(
                                read.generation,
                                Event::Drop {
                                    surface: offer.surface.id().as_ptr() as u64,
                                    position: [offer.x, offer.y],
                                    mime: read.mime,
                                    bytes: read.bytes,
                                },
                            );
                            if accepted {
                                offer.finish();
                            }
                            offer.destroy();
                        } else {
                            self.cancel_at(Destination::Drop(offer), read.generation);
                        }
                    }
                    Destination::Paste(recipient) => {
                        let (text, fallback) = match read.fallback {
                            Some(text) if text.complete => (
                                (!text.bytes.is_empty()).then(|| decode_clipboard_text(&text.mime, &text.bytes)),
                                None,
                            ),
                            Some(text) if !expired => (
                                None,
                                Some(TextFallback {
                                    reader: text.reader,
                                    bytes: text.bytes,
                                    mime: text.mime,
                                    deadline: read.deadline,
                                    limit: 32 * 1024 * 1024 - read.bytes.len(),
                                }),
                            ),
                            _ => (None, None),
                        };
                        if read.bytes.is_empty() && text.is_none() && fallback.is_none() {
                            self.cancel_at(Destination::Paste(recipient), read.generation);
                        } else {
                            self.hub.emit_at(
                                read.generation,
                                Event::Paste {
                                    recipient,
                                    mime: read.mime,
                                    bytes: read.bytes,
                                    text,
                                    fallback,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Transfers {
    fn drop(&mut self) {
        for read in std::mem::take(&mut self.reads) {
            self.cancel_at(read.destination, read.generation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    fn hub(display: usize) -> Arc<Hub> {
        let (sender, _receiver) = sctk::reexports::calloop::channel::channel();
        Hub::register(display, sender)
    }

    #[test]
    fn clipboard_images_precede_uris_and_text_uri_offers_keep_text_paste() {
        let mimes = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            preferred_clipboard(&mimes(&["text/uri-list", "image/png", "text/plain"])).as_deref(),
            Some("image/png")
        );
        assert_eq!(
            preferred_clipboard(&mimes(&["text/uri-list", "image/jpeg"])).as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(
            preferred_clipboard(&mimes(&["text/uri-list", "text/plain;charset=utf-8"])).as_deref(),
            Some("text/uri-list")
        );
        assert!(preferred_clipboard(&mimes(&["text/plain"])).is_none());
        assert_eq!(
            preferred_clipboard(&mimes(&["text/uri-list"])).as_deref(),
            Some("text/uri-list")
        );
        assert_eq!(
            preferred(&mimes(&["text/uri-list", "image/png"])).as_deref(),
            Some("text/uri-list")
        );
    }

    #[test]
    fn fallback_text_matches_toolkit_lossy_utf8_and_mime_newline_rules() {
        let bytes = b"a\r\nb\rc\n\xff";
        assert_eq!(
            decode_clipboard_text("text/plain;charset=utf-8", bytes),
            "a\nb\nc\n\u{fffd}"
        );
        assert_eq!(decode_clipboard_text("UTF8_STRING", bytes), "a\r\nb\rc\n\u{fffd}");
        assert!(preferred_text(&["STRING".into(), "TEXT".into()]).is_none());
    }

    #[test]
    fn availability_snapshot_removes_old_focus_without_losing_another_seat() {
        let hub = hub(113);
        hub.selection(10, true);
        hub.replace_selections([20, 20, 30]);
        assert!(!lock(&hub.available).contains_key(&10));
        assert_eq!(lock(&hub.available).len(), 2);
        hub.replace_selections([]);
        assert!(lock(&hub.available).is_empty());
    }

    #[test]
    fn completed_native_payload_hands_its_original_text_pipe_to_the_file_worker() -> std::io::Result<()> {
        let hub = hub(114);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, mut writer) = UnixStream::pair()?;
        let (text_reader, mut text_writer) = UnixStream::pair()?;
        transfers.receive_paste(
            ReadPipe::from(OwnedFd::from(reader)),
            "text/uri-list".into(),
            42,
            0,
            Some((
                ReadPipe::from(OwnedFd::from(text_reader)),
                "text/plain;charset=utf-8".into(),
            )),
        );
        writer.write_all(b"https://example.invalid/image")?;
        drop(writer);
        transfers.poll();
        let Some(Event::Paste {
            recipient: 42,
            fallback: Some(fallback),
            ..
        }) = lock(&hub.events).pop_front()
        else {
            panic!("completed native payload was delayed")
        };
        text_writer.write_all(b"synthetic text fallback")?;
        drop(text_writer);
        assert_eq!(fallback.read_text().as_deref(), Some("synthetic text fallback"));
        assert!(!transfers.pending());
        Ok(())
    }

    #[test]
    fn complete_image_never_waits_for_a_stalled_text_representation() -> std::io::Result<()> {
        let hub = hub(115);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, mut writer) = UnixStream::pair()?;
        let (text_reader, _text_writer) = UnixStream::pair()?;
        transfers.receive_paste(
            ReadPipe::from(OwnedFd::from(reader)),
            "image/png".into(),
            42,
            0,
            Some((
                ReadPipe::from(OwnedFd::from(text_reader)),
                "text/plain;charset=utf-8".into(),
            )),
        );
        writer.write_all(b"\x89PNG\r\n\x1a\nfixture")?;
        drop(writer);
        transfers.poll();
        assert!(matches!(
            lock(&hub.events).pop_front(),
            Some(Event::Paste {
                recipient: 42,
                text: None,
                fallback: Some(_),
                ..
            })
        ));
        assert!(!transfers.pending());
        Ok(())
    }

    #[test]
    fn timed_out_native_pipe_delivers_completed_text_without_partial_image() -> std::io::Result<()> {
        let hub = hub(116);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, mut writer) = UnixStream::pair()?;
        let (text_reader, mut text_writer) = UnixStream::pair()?;
        transfers.receive_paste(
            ReadPipe::from(OwnedFd::from(reader)),
            "image/png".into(),
            42,
            0,
            Some((
                ReadPipe::from(OwnedFd::from(text_reader)),
                "text/plain;charset=utf-8".into(),
            )),
        );
        writer.write_all(b"partial image")?;
        text_writer.write_all(b"fallback")?;
        drop(text_writer);
        transfers.poll();
        assert!(lock(&hub.events).is_empty());
        transfers.reads[0].deadline = Instant::now();
        transfers.poll();
        assert!(
            matches!(lock(&hub.events).pop_front(), Some(Event::Paste { recipient: 42, bytes, text: Some(text), .. }) if bytes.is_empty() && text == "fallback")
        );
        Ok(())
    }

    #[test]
    fn delayed_transfer_waits_for_eof_and_delivers_once_to_original_recipient() -> std::io::Result<()> {
        let hub = hub(101);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, mut writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        transfers.reads.push(Pending {
            reader: File::from(OwnedFd::from(reader)),
            mime: "image/png".into(),
            destination: Destination::Paste(42),
            bytes: Vec::new(),
            generation: 0,
            deadline: Instant::now() + Duration::from_secs(10),
            complete: false,
            fallback: None,
        });
        writer.write_all(b"partial")?;
        transfers.poll();
        assert!(transfers.pending());
        assert!(lock(&hub.events).is_empty());
        writer.write_all(b" image")?;
        drop(writer);
        transfers.poll();
        assert!(!transfers.pending());
        assert!(
            matches!(lock(&hub.events).pop_front(), Some(Event::Paste {recipient: 42, bytes, ..}) if bytes == b"partial image")
        );
        transfers.poll();
        assert!(lock(&hub.events).is_empty());
        Ok(())
    }

    #[test]
    fn expired_transfer_never_delivers_partial_data() -> std::io::Result<()> {
        let hub = hub(102);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, _writer) = UnixStream::pair()?;
        transfers.reads.push(Pending {
            reader: File::from(OwnedFd::from(reader)),
            mime: "image/png".into(),
            destination: Destination::Paste(7),
            bytes: b"partial".to_vec(),
            generation: 0,
            deadline: Instant::now(),
            complete: false,
            fallback: None,
        });
        transfers.poll();
        assert!(!transfers.pending());
        assert!(matches!(
            lock(&hub.events).pop_front(),
            Some(Event::PasteCancelled { recipient: 7 })
        ));
        assert!(lock(&hub.events).is_empty());
        Ok(())
    }

    #[test]
    fn session_boundary_discards_queued_and_delayed_transfer_events() -> std::io::Result<()> {
        let hub = hub(112);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, mut writer) = UnixStream::pair()?;
        transfers.receive(
            ReadPipe::from(OwnedFd::from(reader)),
            "image/png".into(),
            Destination::Paste(42),
            0,
        );
        writer.write_all(b"partial")?;
        transfers.poll();
        assert!(hub.emit(Event::Motion {
            surface: 1,
            position: [1.0, 2.0],
            entered: true
        }));
        let subscription = Subscription::new(112, || {});
        subscription.resetter()();
        writer.write_all(b" late")?;
        drop(writer);
        transfers.poll();
        assert!(lock(&hub.events).is_empty());
        assert!(!hub.emit_at(
            0,
            Event::Drop {
                surface: 1,
                position: [1.0, 2.0],
                mime: "image/png".into(),
                bytes: vec![1],
            }
        ));
        assert!(hub.emit_at(
            hub.generation(),
            Event::Paste {
                recipient: 43,
                mime: "image/png".into(),
                bytes: vec![2],
                text: None,
                fallback: None
            }
        ));
        assert!(matches!(
            lock(&hub.events).pop_front(),
            Some(Event::Paste { recipient: 43, .. })
        ));
        Ok(())
    }

    #[test]
    fn empty_paste_completes_as_cancellation() -> std::io::Result<()> {
        let hub = hub(108);
        let mut transfers = Transfers::new(hub.clone());
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        drop(writer);
        transfers.reads.push(Pending {
            reader: File::from(OwnedFd::from(reader)),
            mime: "image/png".into(),
            destination: Destination::Paste(8),
            bytes: Vec::new(),
            generation: 0,
            deadline: Instant::now() + Duration::from_secs(10),
            complete: false,
            fallback: None,
        });
        transfers.poll();
        assert!(!transfers.pending());
        assert!(matches!(
            lock(&hub.events).pop_front(),
            Some(Event::PasteCancelled { recipient: 8 })
        ));
        Ok(())
    }

    #[test]
    fn admission_capacity_cancels_rejected_and_dropped_reads() -> std::io::Result<()> {
        let hub = hub(110);
        let mut transfers = Transfers::new(hub.clone());
        let mut writers = Vec::new();
        for recipient in 0..5 {
            let (reader, writer) = UnixStream::pair()?;
            writers.push(writer);
            transfers.receive(
                ReadPipe::from(OwnedFd::from(reader)),
                "image/png".into(),
                Destination::Paste(recipient),
                0,
            );
        }
        assert_eq!(transfers.reads.len(), 4);
        assert!(matches!(
            lock(&hub.events).pop_front(),
            Some(Event::PasteCancelled { recipient: 4 })
        ));
        drop(transfers);
        let events = lock(&hub.events);
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| matches!(event, Event::PasteCancelled { .. })));
        Ok(())
    }

    #[test]
    fn full_event_queue_cancels_the_batch_instead_of_losing_completion() {
        let hub = hub(109);
        for surface in 0..256 {
            assert!(hub.emit(Event::Leave { surface }));
        }
        assert!(!hub.emit(Event::Paste {
            recipient: 9,
            mime: "image/png".into(),
            bytes: vec![1],
            text: None,
            fallback: None
        }));
        let mut events = lock(&hub.events);
        assert!(matches!(events.pop_front(), Some(Event::Reset)));
        assert!(events.is_empty());
        drop(events);
        assert_eq!(hub.generation(), 1);
        assert!(!hub.emit_at(0, Event::Leave { surface: 9 }));
        assert!(lock(&hub.events).is_empty());
        assert!(hub.emit_at(1, Event::Leave { surface: 10 }));
    }

    #[test]
    fn worker_stop_releases_the_wake_lock_before_invoking_callback() {
        let hub = hub(111);
        let callback_hub = hub.clone();
        *lock(&hub.wake) = Some(Arc::new(move || {
            assert!(callback_hub.wake.try_lock().is_ok());
        }));
        hub.stop();
        *lock(&hub.wake) = None;
    }

    #[test]
    fn subscription_is_display_scoped_and_drops_stopped_workers() {
        let first = hub(103);
        let second = hub(104);
        first.emit(Event::Leave { surface: 1 });
        second.emit(Event::Leave { surface: 2 });
        let mut subscription = Subscription::new(103, || {});
        assert!(matches!(subscription.poll().as_slice(), [Event::Leave { surface: 1 }]));
        assert_eq!(lock(&second.events).len(), 1);
        drop(WorkerGuard(first));
        subscription.refresh();
        assert!(subscription.hubs.is_empty());
        assert!(matches!(subscription.poll().as_slice(), [Event::Reset]));
        assert!(subscription.poll().is_empty());
    }

    #[test]
    fn motion_coalesces_only_on_the_same_surface_and_preserves_drop_order() {
        let hub = hub(105);
        for (surface, entered) in [(1, true), (1, false), (1, false), (2, false)] {
            hub.emit(Event::Motion {
                surface,
                position: [2.0, 3.0],
                entered,
            });
        }
        hub.emit(Event::Leave { surface: 2 });
        assert_eq!(lock(&hub.events).len(), 4);
    }

    #[test]
    fn only_file_or_image_offers_claim_native_paste() {
        assert_eq!(preferred(&["text/plain".into()]), None);
        assert_eq!(
            preferred(&["image/jpeg".into(), "image/png".into()]),
            Some("image/png".into())
        );
        assert_eq!(
            preferred(&["image/png".into(), "text/uri-list".into()]),
            Some("text/uri-list".into())
        );
    }
}
