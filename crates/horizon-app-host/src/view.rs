//! Read-only loopback RFB adapter; the controller remains the sole owner of every native session.
use crate::{Error, Result, actor::Actor};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
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
}
pub struct Registry {
    actor: Weak<Actor>,
    views: Mutex<BTreeMap<Uuid, View>>,
}
impl Registry {
    #[must_use]
    pub fn new(actor: &Arc<Actor>) -> Self {
        Self {
            actor: Arc::downgrade(actor),
            views: Mutex::new(BTreeMap::new()),
        }
    }
    /// # Errors
    /// Reuse the exact owned session's viewer; at most two live streams exist per host.
    pub fn open(&self, session: Uuid) -> Result<Handle> {
        let mut views = self.views.lock().map_err(|_| Error::Unavailable)?;
        views.retain(|_, view| !view.stop.load(Ordering::Acquire) && !view.closed.load(Ordering::Acquire));
        if let Some(view) = views.get(&session) {
            return Ok(view.handle());
        }
        if views.len() >= 2 {
            return Err(Error::Unavailable);
        }
        drop(views);
        let actor = self.actor.upgrade().ok_or(Error::SessionUnknown)?;
        let closed = actor.view_lifetime(session)?;
        let first = actor.screenshot(session)?;
        let mut views = self.views.lock().map_err(|_| Error::Unavailable)?;
        views.retain(|_, view| !view.stop.load(Ordering::Acquire) && !view.closed.load(Ordering::Acquire));
        if let Some(view) = views.get(&session) {
            return Ok(view.handle());
        }
        if views.len() >= 2 {
            return Err(Error::Unavailable);
        }
        if closed.load(Ordering::Acquire) {
            return Err(Error::SessionUnknown);
        }
        let view = View::new(self.actor.clone(), session, &first, closed)?;
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
}
impl View {
    fn new(actor: Weak<Actor>, session: Uuid, first: &[u8], closed: Arc<AtomicBool>) -> Result<Self> {
        let frame = Arc::new(Mutex::new(pixels(first)?));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| Error::Unavailable)?;
        listener.set_nonblocking(true).map_err(|_| Error::Unavailable)?;
        let endpoint = listener.local_addr().map_err(|_| Error::Unavailable)?.to_string();
        let view = Self {
            endpoint,
            stop: Arc::new(AtomicBool::new(false)),
            closed: Arc::clone(&closed),
            client: Arc::new(Mutex::new(None)),
        };
        let retained_stop = Arc::clone(&view.stop);
        let retained_frame = Arc::clone(&frame);
        let client = Arc::clone(&view.client);
        std::thread::Builder::new()
            .name("native-live-view".into())
            .spawn(move || {
                while !retained_stop.load(Ordering::Acquire) {
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
                            let _ = connection(&mut stream, &retained_frame, &retained_stop);
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
        let stop = Arc::clone(&view.stop);
        let capture_client = Arc::clone(&view.client);
        std::thread::Builder::new()
            .name("native-live-capture".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    for _ in 0..20 {
                        if stop.load(Ordering::Acquire) || closed.load(Ordering::Acquire) {
                            stop_view(&stop, &capture_client);
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
                        Err(_) => {
                            stop_view(&stop, &capture_client);
                        }
                    }
                }
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(view)
    }
    fn handle(&self) -> Handle {
        Handle {
            endpoint: self.endpoint.clone(),
            width: WIDTH,
            height: HEIGHT,
            read_only: true,
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        stop_view(&self.stop, &self.client);
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
fn connection(stream: &mut TcpStream, frame: &Mutex<Vec<u8>>, stop: &AtomicBool) -> std::io::Result<()> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "native_view_protocol_invalid");
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"RFB 003.008\n")?;
    if read(stream, 12)? != b"RFB 003.008\n" {
        return Err(invalid());
    }
    stream.write_all(&[1, 1])?;
    if read(stream, 1)? != [1] {
        return Err(invalid());
    }
    stream.write_all(&[0; 4])?;
    read(stream, 1)?;
    let name = b"Native app (read-only)";
    let mut initial = Vec::new();
    initial.extend(WIDTH.to_be_bytes());
    initial.extend(HEIGHT.to_be_bytes());
    initial.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    initial.extend(u32::try_from(name.len()).map_err(|_| invalid())?.to_be_bytes());
    initial.extend(name);
    stream.write_all(&initial)?;
    let mut swapped = false;
    while !stop.load(Ordering::Acquire) {
        match read(stream, 1)?[0] {
            0 => {
                read(stream, 3)?;
                let format = read(stream, 16)?;
                if format == [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0] {
                    swapped = false;
                } else if format == [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0] {
                    swapped = true;
                } else {
                    return Err(invalid());
                }
            }
            2 => {
                read(stream, 1)?;
                let count = read(stream, 2)?;
                let count = usize::from(u16::from_be_bytes([count[0], count[1]]));
                if count > 64 {
                    return Err(invalid());
                }
                let encodings = read(stream, count * 4)?;
                if !encodings.as_chunks::<4>().0.iter().any(|bytes| bytes == &[0, 0, 0, 0]) {
                    return Err(invalid());
                }
            }
            3 => {
                read(stream, 9)?;
                let mut image = frame.lock().map_err(|_| invalid())?.clone();
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
                    if stop.load(Ordering::Acquire) {
                        return Ok(());
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
                    return Err(invalid());
                }
                read(stream, usize::try_from(count).map_err(|_| invalid())?)?;
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
                },
            );
        }
        actor.close(first.id).unwrap();
        assert!(!registry.views.lock().unwrap()[&first.id].stop.load(Ordering::Acquire));
        let third = actor.create(0, app.id, Duration::from_secs(30)).unwrap();
        assert!(registry.open(third.id).is_ok());
        assert_eq!(registry.open(second.id).unwrap().endpoint, "127.0.0.1:10002");
        assert_eq!(registry.views.lock().unwrap().len(), 2);
        assert!(!registry.views.lock().unwrap().contains_key(&first.id));
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
            let _ = connection(&mut stream, &retained, &held_stop);
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        assert_eq!(read(&mut client, 12).unwrap(), b"RFB 003.008\n");
        client.write_all(b"RFB 003.008\n").unwrap();
        assert_eq!(read(&mut client, 2).unwrap(), [1, 1]);
        client.write_all(&[1]).unwrap();
        assert_eq!(read(&mut client, 4).unwrap(), [0; 4]);
        client.write_all(&[1]).unwrap();
        let init = read(&mut client, 24).unwrap();
        assert_eq!(&init[..2], WIDTH.to_be_bytes());
        let length = u32::from_be_bytes([init[20], init[21], init[22], init[23]]);
        assert_eq!(
            read(&mut client, usize::try_from(length).unwrap()).unwrap(),
            b"Native app (read-only)"
        );
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
