//! Read-only loopback RFB adapter; the controller remains the sole owner of every native session.
use crate::{Error, Result, actor::Actor};
use horizon_core::browser::manifest::device::{NativeRecipeResult, NativeSessionMetadata};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
const WIDTH: u16 = 768;
const HEIGHT: u16 = 1536;

#[derive(Clone, Serialize)]
pub struct Handle {
    pub endpoint: String,
    pub width: u16,
    pub height: u16,
    pub read_only: bool,
    pub metadata: NativeSessionMetadata,
}
struct RecipeHistory {
    updated: Instant,
    results: Vec<NativeRecipeResult>,
}

pub struct Registry {
    actor: Weak<Actor>,
    views: Mutex<BTreeMap<Uuid, View>>,
    history: Mutex<BTreeMap<(Uuid, usize), RecipeHistory>>,
}
pub(crate) struct Observer<'a> {
    registry: &'a Registry,
    run: Mutex<Option<Uuid>>,
}
impl Observer<'_> {
    pub(crate) fn observe(&self, progress: &mut crate::runner::Progress) -> Result<()> {
        *self.run.lock().map_err(|_| Error::Unavailable)? = Some(progress.run);
        self.registry.observe(progress)
    }
}
impl Drop for Observer<'_> {
    fn drop(&mut self) {
        let run = *self.run.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(run) = run else { return };
        let views = self
            .registry
            .views
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let run_id = run.to_string();
        let mut completed = Vec::new();
        for view in views.values() {
            let metadata = view.metadata.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if metadata.run_id.as_deref() == Some(run_id.as_str()) {
                view.pending.store(false, Ordering::Release);
                if view.closed.load(Ordering::Acquire) {
                    completed.push(Arc::clone(&view.finished));
                }
            }
        }
        drop(views);
        // The CLI can exit immediately after its report. Detached transports must finish first.
        let deadline = Instant::now() + Duration::from_secs(2);
        while completed.iter().any(|finished| !finished.load(Ordering::Acquire)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Registry {
    #[must_use]
    pub fn new(actor: &Arc<Actor>) -> Self {
        Self {
            actor: Arc::downgrade(actor),
            views: Mutex::new(BTreeMap::new()),
            history: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) fn observer(&self) -> Observer<'_> {
        Observer {
            registry: self,
            run: Mutex::new(None),
        }
    }
    /// Attach the same identity and progress for CLI and MCP runs.
    /// # Errors
    /// Refuses an unavailable view on session creation.
    pub(crate) fn observe(&self, progress: &mut crate::runner::Progress) -> Result<()> {
        let Some(session) = progress.session else {
            return Ok(());
        };
        let run_id = progress.run.to_string();
        let recipes = if let Some(index) = progress.matrix_index {
            let mut history = self.history.lock().map_err(|_| Error::Unavailable)?;
            // Runs have a maximum lifetime of 30 minutes; history outlives closed viewers.
            history.retain(|_, previous| previous.updated.elapsed() <= Duration::from_mins(30));
            let previous = history.entry((progress.run, index)).or_insert_with(|| RecipeHistory {
                updated: Instant::now(),
                results: Vec::new(),
            });
            previous.updated = Instant::now();
            if matches!(progress.phase, "recipe_passed" | "recipe_failed" | "recipe_blocked")
                && let Some(recipe) = &progress.recipe
            {
                previous.results.push(NativeRecipeResult {
                    recipe: recipe.clone(),
                    passed: progress.phase == "recipe_passed",
                    blocked: progress.phase == "recipe_blocked",
                });
            }
            Some(previous.results.clone())
        } else {
            None
        };
        if progress.phase == "session_created" {
            progress.view = Some(self.open(session)?);
        }
        let views = self.views.lock().map_err(|_| Error::Unavailable)?;
        let Some(view) = views.get(&session) else {
            return Ok(());
        };
        let mut metadata = view.metadata.lock().map_err(|_| Error::Unavailable)?;
        if let Some(recipes) = recipes {
            metadata.recipes = recipes;
        }
        metadata.run_id = Some(run_id.clone());
        if progress.phase != "lane_complete" {
            metadata.recipe.clone_from(&progress.recipe);
            metadata.step.clone_from(&progress.step);
        }
        if let Some(handle) = &mut progress.view {
            handle.metadata = metadata.clone();
        }
        // The transport thread publishes updates. A stalled viewer cannot block the runner.
        view.dirty.store(true, Ordering::Release);
        if progress.phase == "session_created" {
            view.pending.store(true, Ordering::Release);
            let lane = metadata.lane;
            drop(metadata);
            // Only retire the predecessor after the replacement has its identity and results.
            for (_, previous) in views.iter().filter(|(id, _)| **id != session) {
                let previous_metadata = previous.metadata.lock().map_err(|_| Error::Unavailable)?;
                if previous_metadata.run_id.as_deref() == Some(run_id.as_str()) && previous_metadata.lane == lane {
                    previous.pending.store(false, Ordering::Release);
                }
            }
        } else if progress.phase == "lane_complete" {
            // Native reset cleanup can precede results from this and later recipes.
            view.pending.store(false, Ordering::Release);
        }
        Ok(())
    }

    /// # Errors
    /// Reuse the exact session's viewer; at most two active native sessions have viewers.
    /// Closed run viewers stay available until their final results arrive.
    pub fn open(&self, session: Uuid) -> Result<Handle> {
        let mut views = self.views.lock().map_err(|_| Error::Unavailable)?;
        // A closed viewer remains discoverable until its transport has sent the final result.
        views.retain(|_, view| !view.finished.load(Ordering::Acquire));
        if let Some(view) = views.get(&session) {
            return Ok(view.handle());
        }
        if views
            .values()
            .filter(|view| !view.closed.load(Ordering::Acquire))
            .count()
            >= 2
        {
            return Err(Error::Unavailable);
        }
        drop(views);
        let actor = self.actor.upgrade().ok_or(Error::SessionUnknown)?;
        let closed = actor.view_lifetime(session)?;
        let first = actor.screenshot(session)?;
        let mut views = self.views.lock().map_err(|_| Error::Unavailable)?;
        // A closed viewer remains discoverable until its transport has sent the final result.
        views.retain(|_, view| !view.finished.load(Ordering::Acquire));
        if let Some(view) = views.get(&session) {
            return Ok(view.handle());
        }
        if views
            .values()
            .filter(|view| !view.closed.load(Ordering::Acquire))
            .count()
            >= 2
        {
            return Err(Error::Unavailable);
        }
        if closed.load(Ordering::Acquire) {
            return Err(Error::SessionUnknown);
        }
        let metadata = actor.view_metadata(session)?;
        let view = View::new(self.actor.clone(), session, &first, closed, metadata)?;
        let handle = view.handle();
        views.insert(session, view);
        Ok(handle)
    }
}
struct View {
    endpoint: String,
    stop: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    client: Arc<Mutex<Option<TcpStream>>>,
    metadata: Arc<Mutex<NativeSessionMetadata>>,
    dirty: Arc<AtomicBool>,
    pending: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}
struct TransportCompletion(Arc<AtomicBool>);
impl Drop for TransportCompletion {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl View {
    fn new(
        actor: Weak<Actor>,
        session: Uuid,
        first: &[u8],
        closed: Arc<AtomicBool>,
        metadata: NativeSessionMetadata,
    ) -> Result<Self> {
        let frame = Arc::new(Mutex::new(pixels(first)?));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| Error::Unavailable)?;
        listener.set_nonblocking(true).map_err(|_| Error::Unavailable)?;
        let endpoint = listener.local_addr().map_err(|_| Error::Unavailable)?.to_string();
        let view = Self {
            endpoint,
            stop: Arc::new(AtomicBool::new(false)),
            closed,
            client: Arc::new(Mutex::new(None)),
            metadata: Arc::new(Mutex::new(metadata)),
            dirty: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
        };
        let retained_stop = Arc::clone(&view.stop);
        let retained_frame = Arc::clone(&frame);
        let client = Arc::clone(&view.client);
        let metadata = Arc::clone(&view.metadata);
        let dirty = Arc::clone(&view.dirty);
        let transport_closed = Arc::clone(&view.closed);
        let pending = Arc::clone(&view.pending);
        let finished = Arc::clone(&view.finished);
        std::thread::Builder::new()
            .name("native-live-view".into())
            .spawn(move || {
                let _completion = TransportCompletion(finished);
                while !retained_stop.load(Ordering::Acquire) {
                    if transport_closed.load(Ordering::Acquire) && !pending.load(Ordering::Acquire) {
                        stop_view(&retained_stop, &client);
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let Ok(copy) = stream.try_clone() else {
                                continue;
                            };
                            let Ok(mut held) = client.lock() else {
                                break;
                            };
                            *held = Some(copy);
                            drop(held);
                            if retained_stop.load(Ordering::Acquire) {
                                stop_view(&retained_stop, &client);
                                break;
                            }
                            let _ = connection(
                                &mut stream,
                                &retained_frame,
                                &retained_stop,
                                &metadata,
                                &dirty,
                                &transport_closed,
                                &pending,
                            );
                            let _ = stream.shutdown(Shutdown::Both);
                            if let Ok(mut held) = client.lock() {
                                held.take();
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        Err(_) => {
                            stop_view(&retained_stop, &client);
                            break;
                        }
                    }
                }
            })
            .map_err(|_| Error::Unavailable)?;
        view.spawn_capture(actor, session, frame)?;
        Ok(view)
    }
    fn spawn_capture(&self, actor: Weak<Actor>, session: Uuid, frame: Arc<Mutex<Vec<u8>>>) -> Result<()> {
        let stop = Arc::clone(&self.stop);
        let closed = Arc::clone(&self.closed);
        let capture_client = Arc::clone(&self.client);
        let pending = Arc::clone(&self.pending);
        std::thread::Builder::new()
            .name("native-live-capture".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    for _ in 0..20 {
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        if closed.load(Ordering::Acquire) {
                            close_after_flush(&stop, &capture_client, &pending);
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let captured = (|| {
                        let actor = actor.upgrade().ok_or(Error::SessionUnknown)?;
                        pixels(&actor.screenshot(session)?)
                    })();
                    match captured {
                        Ok(pixels) => {
                            if let Ok(mut latest) = frame.lock() {
                                *latest = pixels;
                            } else {
                                stop_view(&stop, &capture_client);
                            }
                        }
                        Err(_) if closed.load(Ordering::Acquire) => {
                            close_after_flush(&stop, &capture_client, &pending);
                            return;
                        }
                        Err(_) => {
                            stop_view(&stop, &capture_client);
                        }
                    }
                }
            })
            .map(|_| ())
            .map_err(|_| Error::Unavailable)
    }
    fn handle(&self) -> Handle {
        Handle {
            endpoint: self.endpoint.clone(),
            width: WIDTH,
            height: HEIGHT,
            read_only: true,
            metadata: self
                .metadata
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        // Closed-session transports flush pending final metadata before their bounded shutdown.
        if !self.closed.load(Ordering::Acquire) {
            stop_view(&self.stop, &self.client);
        }
    }
}
fn stop_view(stop: &AtomicBool, client: &Mutex<Option<TcpStream>>) {
    stop.store(true, Ordering::Release);
    if let Ok(client) = client.lock()
        && let Some(client) = client.as_ref()
    {
        let _ = client.shutdown(Shutdown::Both);
    }
}

fn close_after_flush(stop: &AtomicBool, client: &Mutex<Option<TcpStream>>, pending: &AtomicBool) {
    // The run observer releases pending results at lane completion or on cancellation/unwind.
    while pending.load(Ordering::Acquire) {
        if stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // A separate wall-clock grace bounds closure even during partial/dripped I/O.
    for _ in 0..20 {
        if stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop_view(stop, client);
}

fn pixels(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits {
            bytes: 64 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| Error::Unavailable)?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= 64 * 1024 * 1024)
        .ok_or(Error::Unavailable)?;
    let mut samples = vec![0; size];
    let info = reader.next_frame(&mut samples).map_err(|_| Error::Unavailable)?;
    if info.width == 0 || info.height == 0 || u64::from(info.width) * u64::from(info.height) > 16_000_000 {
        return Err(Error::Unavailable);
    }
    reader.finish().map_err(|_| Error::Unavailable)?;
    let source_width = usize::try_from(info.width).map_err(|_| Error::Unavailable)?;
    let source_height = usize::try_from(info.height).map_err(|_| Error::Unavailable)?;
    let (width, height) = if source_width * usize::from(HEIGHT) > source_height * usize::from(WIDTH) {
        (
            usize::from(WIDTH),
            (source_height * usize::from(WIDTH) / source_width).max(1),
        )
    } else {
        (
            (source_width * usize::from(HEIGHT) / source_height).max(1),
            usize::from(HEIGHT),
        )
    };
    let left = (usize::from(WIDTH) - width) / 2;
    let top = (usize::from(HEIGHT) - height) / 2;
    let mut output = vec![0; usize::from(WIDTH) * usize::from(HEIGHT) * 4];
    let channels = info.color_type.samples();
    for y in 0..height {
        for x in 0..width {
            let source = ((y * source_height / height) * source_width + x * source_width / width) * channels;
            let rgb = match info.color_type {
                png::ColorType::Rgb | png::ColorType::Rgba => {
                    [samples[source], samples[source + 1], samples[source + 2]]
                }
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => [samples[source]; 3],
                png::ColorType::Indexed => return Err(Error::Unavailable),
            };
            let alpha = match info.color_type {
                png::ColorType::Rgba => samples[source + 3],
                png::ColorType::GrayscaleAlpha => samples[source + 1],
                _ => 255,
            };
            let rgb = rgb.map(|value| u8::try_from(u16::from(value) * u16::from(alpha) / 255).unwrap_or(0));
            let target = ((y + top) * usize::from(WIDTH) + x + left) * 4;
            output[target..target + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
        }
    }
    Ok(output)
}
fn read(stream: &mut TcpStream, count: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = vec![0; count];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn send_metadata(stream: &mut TcpStream, metadata: &NativeSessionMetadata) -> std::io::Result<()> {
    if let Some(text) = metadata.wire_text() {
        let length = u32::try_from(text.len()).map_err(|_| std::io::ErrorKind::InvalidData)?;
        stream.write_all(&[3, 0, 0, 0])?;
        stream.write_all(&length.to_be_bytes())?;
        stream.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn protocol_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "native_view_protocol_invalid")
}
fn initialize_stream(stream: &mut TcpStream) -> std::io::Result<()> {
    // BSD accepts can inherit the listener's nonblocking flag.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"RFB 003.008\n")?;
    if read(stream, 12)? != b"RFB 003.008\n" {
        return Err(protocol_error());
    }
    stream.write_all(&[1, 1])?;
    if read(stream, 1)? != [1] {
        return Err(protocol_error());
    }
    stream.write_all(&[0; 4])?;
    read(stream, 1)?;
    let name = b"Native app (read-only)";
    let mut initial = Vec::new();
    initial.extend(WIDTH.to_be_bytes());
    initial.extend(HEIGHT.to_be_bytes());
    initial.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    initial.extend(u32::try_from(name.len()).map_err(|_| protocol_error())?.to_be_bytes());
    initial.extend(name);
    stream.write_all(&initial)?;
    Ok(())
}

fn connection(
    stream: &mut TcpStream,
    frame: &Mutex<Vec<u8>>,
    stop: &AtomicBool,
    metadata: &Mutex<NativeSessionMetadata>,
    dirty: &AtomicBool,
    closed: &AtomicBool,
    pending: &AtomicBool,
) -> std::io::Result<()> {
    initialize_stream(stream)?;
    {
        dirty.store(false, Ordering::Release);
        let snapshot = metadata.lock().map_err(|_| protocol_error())?.clone();
        send_metadata(stream, &snapshot)?;
    }
    let mut swapped = false;
    while !stop.load(Ordering::Acquire) {
        let closing = closed.load(Ordering::Acquire) && !pending.load(Ordering::Acquire);
        if dirty.swap(false, Ordering::AcqRel) || closing {
            let snapshot = metadata.lock().map_err(|_| protocol_error())?.clone();
            send_metadata(stream, &snapshot)?;
        }
        if closing {
            return Ok(());
        }
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        let kind = match read(stream, 1) {
            Ok(bytes) => bytes[0],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        match kind {
            0 => {
                read(stream, 3)?;
                let format = read(stream, 16)?;
                if format == [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0] {
                    swapped = false;
                } else if format == [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0] {
                    swapped = true;
                } else {
                    return Err(protocol_error());
                }
            }
            2 => {
                read(stream, 1)?;
                let count = read(stream, 2)?;
                let count = usize::from(u16::from_be_bytes([count[0], count[1]]));
                if count > 64 {
                    return Err(protocol_error());
                }
                let encodings = read(stream, count * 4)?;
                if !encodings.as_chunks::<4>().0.iter().any(|bytes| bytes == &[0, 0, 0, 0]) {
                    return Err(protocol_error());
                }
            }
            3 => {
                read(stream, 9)?;
                let mut image = frame.lock().map_err(|_| protocol_error())?.clone();
                if swapped {
                    for pixel in image.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                }
                let mut header = vec![0, 0, 0, 1, 0, 0, 0, 0];
                header.extend(WIDTH.to_be_bytes());
                header.extend(HEIGHT.to_be_bytes());
                header.extend([0; 4]);
                stream.write_all(&header)?;
                stream.write_all(&image)?;
                for _ in 0..20 {
                    if stop.load(Ordering::Acquire)
                        || (closed.load(Ordering::Acquire) && !pending.load(Ordering::Acquire))
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            4 => {
                read(stream, 7)?;
            } // Intentionally discard keyboard and pointer input.
            5 => {
                read(stream, 5)?;
            }
            6 => {
                read(stream, 3)?;
                let bytes = read(stream, 4)?;
                let count = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                if count > 4096 {
                    return Err(protocol_error());
                }
                read(stream, usize::try_from(count).map_err(|_| protocol_error())?)?;
            }
            _ => return Err(protocol_error()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata() -> NativeSessionMetadata {
        NativeSessionMetadata {
            session_id: "session-1".into(),
            run_id: None,
            model: "iPhone synthetic".into(),
            os: "iOS 18.6".into(),
            form: "phone".into(),
            provider: "BrowserStack".into(),
            lane: 1,
            lanes: 2,
            app: "example.demo".into(),
            build_sha256: "a".repeat(64),
            recipe: None,
            step: None,
            recipes: Vec::new(),
        }
    }

    #[cfg(unix)]
    fn rfb_handshake(client: &mut TcpStream) {
        assert_eq!(read(client, 12).unwrap(), b"RFB 003.008\n");
        finish_handshake(client);
    }
    fn finish_handshake(client: &mut TcpStream) {
        client.write_all(b"RFB 003.008\n").unwrap();
        assert_eq!(read(client, 2).unwrap(), [1, 1]);
        client.write_all(&[1]).unwrap();
        assert_eq!(read(client, 4).unwrap(), [0; 4]);
        client.write_all(&[1]).unwrap();
        let init = read(client, 24).unwrap();
        assert_eq!(&init[..2], WIDTH.to_be_bytes());
        let length = u32::from_be_bytes([init[20], init[21], init[22], init[23]]);
        assert_eq!(
            read(client, usize::try_from(length).unwrap()).unwrap(),
            b"Native app (read-only)"
        );
    }
    fn read_metadata(client: &mut TcpStream) -> NativeSessionMetadata {
        assert_eq!(read(client, 4).unwrap(), [3, 0, 0, 0]);
        let length = usize::try_from(u32::from_be_bytes(read(client, 4).unwrap().try_into().unwrap())).unwrap();
        let text = String::from_utf8(read(client, length).unwrap()).unwrap();
        NativeSessionMetadata::from_wire_text(&text).unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn final_result_arrives_before_disconnect_during_frame_delay() {
        const CHILD: &str = "ISSUE1372_VIEW_EXIT";
        if let Ok(address) = std::env::var(CHILD) {
            let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
            let actor = Arc::new(actor);
            let app = actor
                .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
                .unwrap();
            let session = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
            let registry = Registry::new(&actor);
            let observer = registry.observer();
            let mut event = crate::runner::Progress {
                run: Uuid::new_v4(),
                matrix_index: Some(0),
                phase: "session_created",
                recipe: None,
                step: None,
                session: Some(session.id),
                view: None,
                error: None,
            };
            observer.observe(&mut event).unwrap();
            let endpoint = event.view.take().unwrap().endpoint;
            // Model an already-closed failed reset, with no native cleanup left to delay CLI exit.
            actor.close(session.id).unwrap();
            actor.release_upload(app.id).unwrap();
            drop(actor);
            drop(fixture);
            let mut control = TcpStream::connect(address).unwrap();
            control.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            control.write_all(&[u8::try_from(endpoint.len()).unwrap()]).unwrap();
            control.write_all(endpoint.as_bytes()).unwrap();
            assert_eq!(read(&mut control, 1).unwrap(), [1]);
            event.phase = "recipe_failed";
            event.recipe = Some("reset".into());
            observer.observe(&mut event).unwrap();
            event.phase = "lane_complete";
            observer.observe(&mut event).unwrap();
            // A concurrent lane can attempt retirement between completion and CLI exit.
            let _ = registry.open(session.id);
            // The production CLI drops this same guard immediately before saving its report.
            drop(observer);
            std::process::exit(0);
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "view::tests::final_result_arrives_before_disconnect_during_frame_delay",
                "--nocapture",
            ])
            .env(CHILD, listener.local_addr().unwrap().to_string())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut control = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "viewer subprocess exited before readiness"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("viewer subprocess readiness: {error}"),
            }
        };
        control.set_nonblocking(false).unwrap();
        control.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let length = usize::from(read(&mut control, 1).unwrap()[0]);
        let endpoint = String::from_utf8(read(&mut control, length).unwrap()).unwrap();
        let mut client = TcpStream::connect(endpoint).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        rfb_handshake(&mut client);
        read_metadata(&mut client);
        client.write_all(&[3, 0, 0, 0, 0, 0, 3, 0, 6, 0]).unwrap();
        read(&mut client, 16).unwrap();
        read(&mut client, usize::from(WIDTH) * usize::from(HEIGHT) * 4).unwrap();
        control.write_all(&[1]).unwrap();
        let final_metadata = read_metadata(&mut client);
        assert_eq!(
            final_metadata.recipes,
            vec![NativeRecipeResult {
                recipe: "reset".into(),
                passed: false,
                blocked: false,
            }]
        );
        let mut kind = [0];
        while client.read(&mut kind).unwrap() != 0 {
            assert_eq!(kind, [3]);
            read(&mut client, 3).unwrap();
            let length =
                usize::try_from(u32::from_be_bytes(read(&mut client, 4).unwrap().try_into().unwrap())).unwrap();
            let text = String::from_utf8(read(&mut client, length).unwrap()).unwrap();
            assert_eq!(
                NativeSessionMetadata::from_wire_text(&text),
                Some(final_metadata.clone())
            );
        }
        assert!(child.wait().unwrap().success());
    }
    #[test]
    #[cfg(unix)]
    fn delayed_failed_reset_retains_all_recipe_results_until_lane_completion() {
        let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
            .unwrap();
        let session = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let registry = Registry::new(&actor);
        let observer = registry.observer();
        let mut event = crate::runner::Progress {
            run: Uuid::new_v4(),
            matrix_index: Some(0),
            phase: "session_created",
            recipe: None,
            step: None,
            session: Some(session.id),
            view: None,
            error: None,
        };
        observer.observe(&mut event).unwrap();
        let mut client = TcpStream::connect(&event.view.take().unwrap().endpoint).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        rfb_handshake(&mut client);
        read_metadata(&mut client);
        let receiver = std::thread::spawn(move || {
            let mut latest = None;
            let mut kind = [0];
            while client.read(&mut kind).unwrap() != 0 {
                assert_eq!(kind, [3]);
                read(&mut client, 3).unwrap();
                let length =
                    usize::try_from(u32::from_be_bytes(read(&mut client, 4).unwrap().try_into().unwrap())).unwrap();
                let text = String::from_utf8(read(&mut client, length).unwrap()).unwrap();
                latest = NativeSessionMetadata::from_wire_text(&text);
            }
            latest.unwrap()
        });
        event.phase = "step";
        event.recipe = Some("reset".into());
        event.step = Some("replace".into());
        observer.observe(&mut event).unwrap();
        fixture.fake.reject_verification.store(true, Ordering::SeqCst);
        *fixture.fake.transport.after_create.lock().unwrap() = Some(Box::new(|| {
            std::thread::sleep(Duration::from_millis(1500));
        }));
        let (replacement, cleanup_confirmed) = actor.reset_for_run_outcome(session.id);
        assert!(replacement.is_err());
        assert!(cleanup_confirmed);
        assert!(fixture.fake.transport.active.lock().unwrap().is_empty());
        for recipe in ["reset", "later"] {
            if recipe == "later" {
                event.phase = "step";
                event.recipe = Some(recipe.into());
                event.step = Some("continue".into());
                observer.observe(&mut event).unwrap();
                std::thread::sleep(Duration::from_millis(1100));
            }
            event.phase = if recipe == "later" {
                "recipe_blocked"
            } else {
                "recipe_failed"
            };
            event.recipe = Some(recipe.into());
            event.step = None;
            observer.observe(&mut event).unwrap();
        }
        event.phase = "lane_complete";
        observer.observe(&mut event).unwrap();
        let final_metadata = receiver.join().unwrap();
        assert_eq!(final_metadata.recipe.as_deref(), Some("later"));
        assert_eq!(
            final_metadata.recipes,
            ["reset", "later"].map(|recipe| NativeRecipeResult {
                recipe: recipe.into(),
                passed: false,
                blocked: recipe == "later",
            })
        );
        drop(observer);
        actor.release_upload(app.id).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn observer_unwind_releases_only_its_closed_run_viewer() {
        let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
            .unwrap();
        let session = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let registry = Registry::new(&actor);
        let mut event = crate::runner::Progress {
            run: Uuid::new_v4(),
            matrix_index: Some(0),
            phase: "session_created",
            recipe: None,
            step: None,
            session: Some(session.id),
            view: None,
            error: None,
        };
        let observer = registry.observer();
        observer.observe(&mut event).unwrap();
        let mut client = TcpStream::connect(&event.view.take().unwrap().endpoint).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        rfb_handshake(&mut client);
        read_metadata(&mut client);
        actor.close(session.id).unwrap();
        drop(registry.observer()); // An unstarted concurrent call has no run to release.
        client.set_read_timeout(Some(Duration::from_millis(1200))).unwrap();
        assert!(
            matches!(client.read(&mut [0]), Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _observer = observer;
                panic!("synthetic progress callback failure");
            }))
            .is_err()
        );
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        read_metadata(&mut client);
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
        actor.release_upload(app.id).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn native_close_interrupts_a_partial_client_message_within_flush_grace() {
        let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
            .unwrap();
        let session = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let registry = Registry::new(&actor);
        let observer = registry.observer();
        let mut event = crate::runner::Progress {
            run: Uuid::new_v4(),
            matrix_index: Some(0),
            phase: "session_created",
            recipe: None,
            step: None,
            session: Some(session.id),
            view: None,
            error: None,
        };
        observer.observe(&mut event).unwrap();
        let view = event.view.take().unwrap();
        let mut client = TcpStream::connect(&view.endpoint).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        rfb_handshake(&mut client);
        read_metadata(&mut client);
        // Header declares 4096 bytes, but the body never finishes.
        client.write_all(&[6, 0, 0, 0, 0, 0, 16, 0, 65]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        actor.close(session.id).unwrap();
        let started = Instant::now();
        drop(observer);
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
        assert!(started.elapsed() < Duration::from_secs(2));
        actor.release_upload(app.id).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn progress_keeps_identity_and_recipe_results_across_reset() {
        let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
            .unwrap();
        let session = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let registry = Arc::new(Registry::new(&actor));
        let mut event = crate::runner::Progress {
            run: Uuid::new_v4(),
            matrix_index: Some(0),
            phase: "session_created",
            recipe: None,
            step: None,
            session: Some(session.id),
            view: None,
            error: None,
        };
        registry.observe(&mut event).unwrap();
        let initial = event.view.take().unwrap().metadata;
        assert_eq!(initial.model, "iPhone synthetic");
        assert_eq!(initial.os, "iOS 27.0");
        assert_eq!((initial.lane, initial.lanes), (1, 1));
        assert_eq!(initial.build_sha256, app.sha256);
        event.phase = "step";
        event.recipe = Some("login".into());
        event.step = Some("open".into());
        registry.observe(&mut event).unwrap();
        assert_eq!(
            registry.open(session.id).unwrap().metadata.step.as_deref(),
            Some("open")
        );
        // A blocked transport/shutdown lock must not delay progress publication.
        let client = Arc::clone(&registry.views.lock().unwrap()[&session.id].client);
        let held = client.lock().unwrap();
        let retained = Arc::clone(&registry);
        let mut update = event.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = retained.observe(&mut update);
            send.send(result).unwrap();
        });
        let published = receive.recv_timeout(Duration::from_millis(250));
        drop(held);
        worker.join().unwrap();
        assert!(published.unwrap().is_ok());
        event.phase = "recipe_failed";
        event.step = None;
        registry.observe(&mut event).unwrap();
        let replacement = actor.reset(session.id).unwrap();
        // Another lane must preserve the predecessor until replacement observation.
        let other = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        registry.open(other.id).unwrap();
        assert!(registry.views.lock().unwrap().contains_key(&session.id));
        event.phase = "session_created";
        event.session = Some(replacement.id);
        event.recipe = Some("next".into());
        event.step = Some("reset".into());
        registry.observe(&mut event).unwrap();
        let replaced = event.view.take().unwrap().metadata;
        assert_ne!(replaced.session_id, initial.session_id);
        assert_eq!(replaced.run_id, initial.run_id);
        assert_eq!(
            replaced.recipes,
            vec![NativeRecipeResult {
                recipe: "login".into(),
                passed: false,
                blocked: false,
            }]
        );
        assert_eq!(replaced.step.as_deref(), Some("reset"));
        event.phase = "lane_complete";
        registry.observe(&mut event).unwrap();
        actor.close(replacement.id).unwrap();
        actor.close(other.id).unwrap();
        actor.release_upload(app.id).unwrap();
    }

    #[test]
    // The actor fixture uses Unix process guardians and filesystem permissions.
    #[cfg(unix)]
    fn closing_native_session_disconnects_idle_viewer_without_reopening_registry() {
        let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(20))
            .unwrap();
        let session = actor.create(0, app.id, Duration::from_secs(10)).unwrap();
        let registry = Registry::new(&actor);
        let view = registry.open(session.id).unwrap();
        let mut client = TcpStream::connect(view.endpoint).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        assert_eq!(read(&mut client, 12).unwrap(), b"RFB 003.008\n");
        actor.close(session.id).unwrap();
        let start = std::time::Instant::now();
        let mut byte = [0];
        assert!(matches!(client.read(&mut byte), Ok(0)));
        assert!(start.elapsed() < Duration::from_secs(3));
        actor.release_upload(app.id).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn closed_view_releases_capacity_before_the_capture_thread_observes_closure() {
        let (_fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        let actor = Arc::new(actor);
        let app = actor
            .upload(horizon_app_testing::contract::Platform::Ios, Duration::from_secs(60))
            .unwrap();
        let first = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let second = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        let registry = Registry::new(&actor);
        // Retain two streams whose capture threads have not published stop. This
        // models the one-second polling window without depending on scheduling.
        for (session, port) in [(first.id, 10001), (second.id, 10002)] {
            registry.views.lock().unwrap().insert(
                session,
                View {
                    endpoint: format!("127.0.0.1:{port}"),
                    stop: Arc::new(AtomicBool::new(false)),
                    closed: actor.view_lifetime(session).unwrap(),
                    client: Arc::new(Mutex::new(None)),
                    metadata: Arc::new(Mutex::new(metadata())),
                    dirty: Arc::new(AtomicBool::new(false)),
                    pending: Arc::new(AtomicBool::new(false)),
                    finished: Arc::new(AtomicBool::new(false)),
                },
            );
        }
        actor.close(first.id).unwrap();
        assert!(!registry.views.lock().unwrap()[&first.id].stop.load(Ordering::Acquire));
        let third = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        assert!(registry.open(third.id).is_ok());
        assert_eq!(registry.open(second.id).unwrap().endpoint, "127.0.0.1:10002");
        assert_eq!(registry.views.lock().unwrap().len(), 3);
        assert!(registry.views.lock().unwrap().contains_key(&first.id));
        actor.close(second.id).unwrap();
        actor.close(third.id).unwrap();
        actor.release_upload(app.id).unwrap();
    }
    #[test]
    fn transparent_pixels_are_composited_onto_black() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 255, 255, 0]).unwrap();
        }
        assert!(pixels(&bytes).unwrap().iter().all(|value| *value == 0));
    }
    #[test]
    fn live_rfb_frames_change_and_input_is_discarded_without_a_native_command_channel() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let frame = Arc::new(Mutex::new(vec![1; usize::from(WIDTH) * usize::from(HEIGHT) * 4]));
        let retained = Arc::clone(&frame);
        let stop = Arc::new(AtomicBool::new(false));
        let held_stop = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Exercise the BSD inheritance path on every test platform.
            stream.set_nonblocking(true).unwrap();
            let _ = connection(
                &mut stream,
                &retained,
                &held_stop,
                &Mutex::new(metadata()),
                &AtomicBool::new(false),
                &AtomicBool::new(false),
                &AtomicBool::new(false),
            );
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        assert_eq!(read(&mut client, 12).unwrap(), b"RFB 003.008\n");
        std::thread::sleep(Duration::from_millis(100));
        finish_handshake(&mut client);
        assert_eq!(read_metadata(&mut client), metadata());
        client.write_all(&[4, 1, 0, 0, 0, 0, 0, 65, 5, 1, 0, 1, 0, 1]).unwrap();
        let request = [3, 0, 0, 0, 0, 0, 3, 0, 6, 0];
        client.write_all(&request).unwrap();
        let header = read(&mut client, 16).unwrap();
        assert_eq!(&header[..4], [0, 0, 0, 1]);
        let first = read(&mut client, usize::from(WIDTH) * usize::from(HEIGHT) * 4).unwrap();
        assert_eq!(first[0], 1);
        frame.lock().unwrap().fill(2);
        client.write_all(&request).unwrap();
        read(&mut client, 16).unwrap();
        let second = read(&mut client, usize::from(WIDTH) * usize::from(HEIGHT) * 4).unwrap();
        assert_eq!(second[0], 2);
        stop.store(true, Ordering::Release);
        client.shutdown(Shutdown::Both).unwrap();
        server.join().unwrap();
    }
}
