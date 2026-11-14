//! The loader and the worker of the pictures of a markdown preview.
//!
//! One loader of the private scheme of [`super::SCHEME`] serves every picture of
//! every note of one `egui::Context`, and one worker thread opens, file-types,
//! reads and decodes them. This module states the states of one picture and the
//! bounds of the trust boundary
//! [`docs/review/trust-boundaries/markdown-preview-images.md`](../../../../../docs/review/trust-boundaries/markdown-preview-images.md).

use std::collections::HashMap;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::thread;

use egui::load::{LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll};
use egui::{ColorImage, Context, Id, TextureHandle, TextureOptions, Vec2};

use super::uri::picture_of_uri;
use super::{FORMATS, Failure, MAX_FILE_BYTES, MAX_PICTURE_PIXELS, MAX_SHOWN_PIXELS, Seen, WORKER};

impl Seen {
    fn of(state: &State) -> Self {
        match state {
            State::Queued => Self::Reading,
            State::Size(size) => Self::Size(*size),
            State::Decoded(_) | State::Ready { .. } => Self::Ready,
            State::Failed(failure) => Self::Failed(*failure),
        }
    }
}
enum State {
    /// The job of this picture waits for the worker.
    Queued,
    /// The worker read the size of the picture and the decode runs.
    Size(Vec2),
    /// The worker decoded the picture, and the UI thread makes its texture.
    Decoded(Arc<ColorImage>),
    /// The texture of the picture, and its natural size in points.
    Ready {
        texture: TextureHandle,
        natural: Vec2,
    },
    Failed(Failure),
}

struct Entry {
    state: State,
    seen: u64,
}

struct Job {
    ctx: Context,
    key: PathBuf,
    paths: Vec<PathBuf>,
    max_side: u32,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<PathBuf, Entry>,
    jobs: Option<mpsc::Sender<Job>>,
    tick: u64,
    pixels: u64,
}

/// The pictures of every note that one [`Context`] draws.
pub(crate) struct NoteImages {
    inner: Arc<Mutex<Inner>>,
}

impl NoteImages {
    const ID: &'static str = egui::generate_loader_id!(NoteImages);

    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Whether a `Context` holds the pictures of a note. A test proves that a
    /// note without a picture installs nothing.
    #[cfg(test)]
    pub(crate) fn installed(ctx: &Context) -> bool {
        ctx.data(|data| data.get_temp::<Arc<Self>>(shared_id()).is_some())
    }

    /// The one instance of these pictures for `ctx`, with its texture loader
    /// installed.
    pub(crate) fn shared(ctx: &Context) -> Arc<Self> {
        let images: Arc<Self> = ctx.data_mut(|data| {
            data.get_temp_mut_or_insert_with(shared_id(), || Arc::new(Self::new()))
                .clone()
        });
        if !ctx.is_loader_installed(Self::ID) {
            ctx.add_texture_loader(Arc::clone(&images) as Arc<dyn TextureLoader + Send + Sync>);
        }
        images
    }

    /// The state of one picture, with a request to read it when the preview asks
    /// about it for the first time.
    pub(crate) fn request(&self, ctx: &Context, key: &Path, paths: &[PathBuf], max_side: u32) -> Seen {
        let mut inner = lock(&self.inner);
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(entry) = inner.entries.get_mut(key) {
            entry.seen = tick;
            return Seen::of(&entry.state);
        }
        inner.entries.insert(
            key.to_path_buf(),
            Entry {
                state: State::Queued,
                seen: tick,
            },
        );
        let jobs = inner
            .jobs
            .get_or_insert_with(|| spawn_worker(Arc::downgrade(&self.inner)));
        let job = Job {
            ctx: ctx.clone(),
            key: key.to_path_buf(),
            paths: paths.to_vec(),
            max_side,
        };
        if jobs.send(job).is_err()
            && let Some(entry) = inner.entries.get_mut(key)
        {
            entry.state = State::Failed(Failure::Unreadable);
        }
        Seen::Reading
    }

    /// Drops the texture and the state of each path, so the next frame reads the
    /// file again. The panel calls this when its note or its folder changes and
    /// when it closes.
    pub(crate) fn forget(&self, paths: &[PathBuf]) {
        let mut inner = lock(&self.inner);
        for path in paths {
            if let Some(entry) = inner.entries.remove(path) {
                inner.pixels = inner.pixels.saturating_sub(pixels_of(&entry.state));
            }
        }
    }
}

impl TextureLoader for NoteImages {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, options: TextureOptions, _size_hint: SizeHint) -> TextureLoadResult {
        let Some((zoom, key)) = picture_of_uri(uri) else {
            return Err(LoadError::NotSupported);
        };
        let mut inner = lock(&self.inner);
        inner.tick += 1;
        let tick = inner.tick;
        let Some(entry) = inner.entries.get_mut(&key) else {
            return Ok(TexturePoll::Pending { size: None });
        };
        entry.seen = tick;
        match &mut entry.state {
            State::Queued => Ok(TexturePoll::Pending { size: None }),
            State::Size(natural) => Ok(TexturePoll::Pending {
                size: Some(*natural * zoom),
            }),
            State::Decoded(image) => {
                let natural = natural_size(image);
                let texture = ctx.load_texture(key.display().to_string(), Arc::clone(image), options);
                entry.state = State::Ready {
                    texture: texture.clone(),
                    natural,
                };
                Ok(TexturePoll::Ready {
                    texture: SizedTexture::new(texture.id(), natural * zoom),
                })
            }
            State::Ready { texture, natural } => Ok(TexturePoll::Ready {
                texture: SizedTexture::new(texture.id(), *natural * zoom),
            }),
            State::Failed(failure) => Err(LoadError::Loading(failure.line(&key.display().to_string()))),
        }
    }

    fn forget(&self, uri: &str) {
        if let Some((_, key)) = picture_of_uri(uri) {
            self.forget(&[key]);
        }
    }

    fn forget_all(&self) {
        let mut inner = lock(&self.inner);
        inner.entries.clear();
        inner.pixels = 0;
    }

    fn byte_size(&self) -> usize {
        let inner = lock(&self.inner);
        usize::try_from(inner.pixels).unwrap_or_default() * 4
    }
}

/// The key of the pictures of one `Context`.
fn shared_id() -> Id {
    Id::unique("horizon_note_pictures")
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn spawn_worker(shared: Weak<Mutex<Inner>>) -> mpsc::Sender<Job> {
    let (jobs, receiver) = mpsc::channel::<Job>();
    let spawned = thread::Builder::new()
        .name(WORKER.to_owned())
        .spawn(move || worker(receiver, &shared));
    if spawned.is_err() {
        tracing::warn!("the thread of the pictures of a note did not start");
    }
    jobs
}

fn worker(receiver: mpsc::Receiver<Job>, shared: &Weak<Mutex<Inner>>) {
    for job in receiver {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        decode(&shared, &job);
    }
}

/// Reads, checks and decodes one picture on the worker thread.
fn decode(shared: &Mutex<Inner>, job: &Job) {
    let outcome = picture(shared, job);
    {
        let mut inner = lock(shared);
        let state = match outcome {
            Ok((image, pixels)) => {
                inner.pixels += pixels;
                State::Decoded(image)
            }
            Err(failure) => State::Failed(failure),
        };
        let Some(entry) = inner.entries.get_mut(&job.key) else {
            return;
        };
        entry.state = state;
    }
    job.ctx.request_repaint();
}

/// The decoded picture of one job, and its pixels, or the reason of its line.
fn picture(shared: &Mutex<Inner>, job: &Job) -> Result<(Arc<ColorImage>, u64), Failure> {
    let bytes = read_picture(&job.paths)?;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| Failure::NotAPicture)?;
    let Some(format) = reader.format().filter(|format| FORMATS.contains(format)) else {
        return Err(Failure::NotAPicture);
    };
    let (width, height) = reader.into_dimensions().map_err(|_| Failure::NotAPicture)?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PICTURE_PIXELS || width > job.max_side || height > job.max_side {
        return Err(Failure::TooLarge);
    }
    let natural = Vec2::new(points_of(width), points_of(height));
    {
        let mut inner = lock(shared);
        let shown = inner.pixels;
        let Some(entry) = inner.entries.get_mut(&job.key) else {
            return Err(Failure::NotFound);
        };
        if shown + pixels > MAX_SHOWN_PIXELS {
            return Err(Failure::NoRoom);
        }
        entry.state = State::Size(natural);
    }
    job.ctx.request_repaint();
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(job.max_side);
    limits.max_image_height = Some(job.max_side);
    limits.max_alloc = Some(MAX_PICTURE_PIXELS * 8);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| Failure::NotAPicture)?.into_rgba8();
    let (width, height) = decoded.dimensions();
    let image = ColorImage::from_rgba_unmultiplied(
        [
            usize::try_from(width).unwrap_or_default(),
            usize::try_from(height).unwrap_or_default(),
        ],
        decoded.as_raw(),
    );
    Ok((Arc::new(image), pixels))
}

/// The bytes of one picture file: never a pipe, never a device and never above
/// the bound.
fn read_picture(paths: &[PathBuf]) -> Result<Vec<u8>, Failure> {
    let mut opened = None;
    for path in paths {
        match open_readonly(path) {
            Ok(file) => {
                opened = Some(file);
                break;
            }
            Err(Failure::NotFound) => {}
            Err(failure) => return Err(failure),
        }
    }
    let Some(file) = opened else {
        return Err(Failure::NotFound);
    };
    // The file type and the length come from the open handle, so a race cannot
    // pass a pipe or a device in the place of the file that the preview checked.
    let meta = file.metadata().map_err(|_| Failure::Unreadable)?;
    if !meta.is_file() {
        return Err(Failure::NotRegular);
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(Failure::TooLarge);
    }
    let capacity = usize::try_from(meta.len()).unwrap_or_default();
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::Unreadable)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(Failure::TooLarge);
    }
    Ok(bytes)
}

/// Opens `path` for reading without a wait, so a pipe with no writer answers at
/// once instead of holding the worker.
fn open_readonly(path: &Path) -> Result<std::fs::File, Failure> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Failure::NotFound,
            _ => Failure::Unreadable,
        })
}

fn pixels_of(state: &State) -> u64 {
    match state {
        State::Decoded(image) => u64::try_from(image.size[0])
            .unwrap_or_default()
            .saturating_mul(u64::try_from(image.size[1]).unwrap_or_default()),
        State::Ready { natural, .. } => {
            let width = u64::try_from(texels(natural.x)).unwrap_or_default();
            let height = u64::try_from(texels(natural.y)).unwrap_or_default();
            width.saturating_mul(height)
        }
        State::Queued | State::Size(_) | State::Failed(_) => 0,
    }
}

fn natural_size(image: &ColorImage) -> Vec2 {
    Vec2::new(points(image.size[0]), points(image.size[1]))
}

/// The point size of a pixel count, at one point for one pixel.
#[allow(clippy::cast_precision_loss)]
fn points(texels: usize) -> f32 {
    texels as f32
}

/// The pixel count of a point size.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn texels(points: f32) -> usize {
    points.max(0.0) as usize
}

/// The point size of a pixel count of a picture header.
#[allow(clippy::cast_precision_loss)]
fn points_of(texels: u32) -> f32 {
    texels as f32
}
