//! The pictures of a markdown preview.
//!
//! A note can name pictures that are files on this computer. The preview shows
//! them, and it shows one line of text in the place of every picture that it
//! does not show. The trust boundary of this module is
//! [`docs/review/trust-boundaries/markdown-preview-images.md`](../../../../docs/review/trust-boundaries/markdown-preview-images.md),
//! and
//! [`docs/testing/procedures/markdown-preview.md`](../../../../docs/testing/procedures/markdown-preview.md)
//! gives the steps that a person does.
//!
//! The viewer of `egui_commonmark` draws a picture with `egui::Image::from_uri`
//! and offers no hook, so this module supplies the two halves that the viewer
//! needs. The first half is a private URI scheme: the viewer prefixes a relative
//! destination with its implicit scheme, and a picture that the note writes with
//! `file:` is rewritten into the same scheme. The second half is the one texture
//! loader of that scheme, and it is the only loader that this module installs.
//! No byte loader, image loader or file loader of `egui_extras` ever enters the
//! application, so no loader of the preview reads a network URI, and no stock
//! loader answers an `egui::Image` of another panel.
//!
//! The URI of one picture carries the canvas zoom and the folder of the note, so
//! the loader needs no state of a panel. It returns the natural size of the
//! picture times the zoom of the URI, and every zoom of a note shares one
//! texture.
//!
//! Every open, file type, read and decode runs on the [`loader`] thread. The UI
//! thread changes a state under one mutex and reads an answer that the worker
//! wrote.

mod loader;
mod note;
#[cfg(test)]
mod tests;
mod uri;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Context, Vec2};

pub(crate) use loader::NoteImages;
pub(crate) use note::{Fate, NoteText, Place, Replacement};
pub(crate) use uri::{implicit_scheme, picture_uri};

/// The most pictures that one note loads. A later picture shows a line.
pub(crate) const MAX_PICTURES: usize = 32;

/// The most bytes that the preview reads of one picture file.
pub(crate) const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The most pixels of one picture, so a small file cannot ask for a large
/// allocation.
pub(crate) const MAX_PICTURE_PIXELS: u64 = 16_777_216;

/// The most pixels that the pictures on screen hold together. A picture that
/// does not fit shows a line and is not decoded, so the memory of the preview
/// has a bound that an untrusted note cannot pass.
const MAX_SHOWN_PIXELS: u64 = 67_108_864;

/// The private scheme of a picture of a note.
pub(crate) const SCHEME: &str = "horizon-note-picture://";

/// The tail of a URI that the viewer builds: the destination as written.
const WRITTEN_TAIL: char = '?';

/// The tail of a URI that this module builds: an encoded path.
const PATH_TAIL: char = '!';

/// The bytes of `written` with each `%XX` escape worked out.
///
/// A strict decode reads an escape only whole and answers `None` for one that is
/// not two hex digits or that ends the text. A lenient decode keeps such an escape
/// as written, and answers `None` when nothing was escaped.
fn percent_decoded(written: &str, strict: bool) -> Option<Vec<u8>> {
    let bytes = written.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        // A lenient decode keeps the `%` of an escape that ends the text, so it
        // reads only an escape that has one byte after its two hex digits.
        let end = index + 3;
        if bytes[index] == b'%'
            && end <= bytes.len()
            && (strict || end < bytes.len())
            && let Some(byte) = written
                .get(index + 1..end)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            escaped = true;
            index = end;
            continue;
        }
        if strict && bytes[index] == b'%' {
            return None;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    (strict || escaped).then_some(decoded)
}

/// The marker of a picture in the text of a note. A note without it runs the
/// plain path of the preview and starts no thread.
pub(crate) const PICTURE_MARKER: &str = "![";

/// The thread that opens, reads and decodes the pictures of a note.
const WORKER: &str = "note-picture";

/// The formats that the workspace already decodes: the clipboard of the editor
/// and the image transfer of the compositor decode them.
const FORMATS: [image::ImageFormat; 2] = [image::ImageFormat::Png, image::ImageFormat::Jpeg];

/// Why the preview gives a line of text in the place of a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    NotFound,
    Unreadable,
    NotRegular,
    TooLarge,
    NotAPicture,
    NoRoom,
}

impl Failure {
    /// The line of the preview for the path that the note writes.
    fn line(self, written: &str) -> String {
        let reason = match self {
            Self::NotFound => "Image not found",
            Self::Unreadable => "Cannot read",
            Self::NotRegular => "Not a regular file",
            Self::TooLarge => "Picture too large",
            Self::NotAPicture => "Not a picture file",
            Self::NoRoom => "Too many pictures in this note",
        };
        format!("{reason}: {}", note::escaped(written))
    }
}

/// What the preview knows about one picture of a note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Seen {
    /// The file is on its way, and its size is not known yet.
    Reading,
    /// The file is on its way, and its natural size in points is known.
    Size(Vec2),
    /// The picture is there.
    Ready,
    /// The picture becomes a line of text.
    Failed(Failure),
}

/// What the viewer of the preview draws.
pub(crate) struct Prepared<'a> {
    /// The text of this module, with a line or a URI in the place of a picture,
    /// while the viewer must draw it instead of the text of the editor. The
    /// viewer can change the text of the editor itself, for example through the
    /// check box of a task list, and it cannot change this text.
    pub(crate) text: Option<&'a str>,
    /// The prefix that resolves a relative picture of the note.
    pub(crate) scheme: Option<String>,
}

/// The pictures of one editor panel: the plan of its note and its textures.
/// Dropping it drops the textures that its pictures hold.
pub(crate) struct PreviewImages {
    ctx: Context,
    images: Arc<NoteImages>,
    note: Option<NoteText>,
    folder: Option<PathBuf>,
    rendered: String,
    paths: Vec<PathBuf>,
    was_saving: bool,
}

impl PreviewImages {
    pub(crate) fn new(ctx: &Context) -> Self {
        Self {
            ctx: ctx.clone(),
            images: NoteImages::shared(ctx),
            note: None,
            folder: None,
            rendered: String::new(),
            paths: Vec::new(),
            was_saving: false,
        }
    }

    /// The text that the viewer draws, and what it needs for the pictures of the
    /// note.
    pub(crate) fn prepare(&mut self, text: &str, folder: Option<&Path>, zoom: f32, saving: bool) -> Prepared<'_> {
        if self.folder.as_deref() != folder || self.note.as_ref().is_none_or(|note| note.source != text) {
            self.forget();
            self.folder = folder.map(Path::to_path_buf);
            self.note = Some(NoteText::new(text, folder));
        } else if self.was_saving && !saving {
            // The file of the note was just written, so its pictures are read
            // again.
            self.forget();
        }
        self.was_saving = saving;
        let max_side = max_texture_side(&self.ctx);
        let resolved = self.folder.clone().unwrap_or_default();
        let Some(note) = self.note.as_ref() else {
            return Prepared {
                text: None,
                scheme: None,
            };
        };
        let images = Arc::clone(&self.images);
        let ctx = &self.ctx;
        let mut wanted = Vec::new();
        let mut replacements = Vec::with_capacity(note.places.len());
        for place in &note.places {
            replacements.push(replacement_of(
                &images,
                ctx,
                place,
                zoom,
                &resolved,
                max_side,
                &mut wanted,
            ));
        }
        self.paths = wanted;
        let scheme = || self.folder.as_deref().map(|folder| implicit_scheme(zoom, folder));
        if replacements.iter().all(Option::is_none) {
            // Every picture of this note resolves through the implicit scheme, so
            // the viewer draws the text of the editor and its check boxes stay
            // live.
            return Prepared {
                text: None,
                scheme: scheme(),
            };
        }
        let mut rendered = std::mem::take(&mut self.rendered);
        note.rewrite(&mut rendered, &replacements);
        self.rendered = rendered;
        Prepared {
            text: Some(&self.rendered),
            scheme: scheme(),
        }
    }

    fn forget(&mut self) {
        self.images.forget(&self.paths);
        self.paths.clear();
    }
}

impl Drop for PreviewImages {
    fn drop(&mut self) {
        self.forget();
    }
}

/// The text that takes the place of one picture, or `None` while the note text
/// already resolves it.
fn replacement_of(
    images: &NoteImages,
    ctx: &Context,
    place: &Place,
    zoom: f32,
    folder: &Path,
    max_side: u32,
    wanted: &mut Vec<PathBuf>,
) -> Option<Replacement> {
    match &place.fate {
        Fate::Line(line) => Some(Replacement::Line(line.clone())),
        Fate::File(paths) => {
            let key = paths.first()?;
            wanted.push(key.clone());
            match images.request(ctx, key, paths, max_side) {
                Seen::Reading => Some(Replacement::Line(format!(
                    "Loading picture: {}",
                    note::escaped(place.label())
                ))),
                Seen::Size(_) | Seen::Ready if place.implicit => None,
                Seen::Size(_) | Seen::Ready => Some(Replacement::Picture(format!(
                    "![{}]({})",
                    note::escaped(&place.alt),
                    picture_uri(zoom, folder, &place.tail)
                ))),
                Seen::Failed(failure) => Some(Replacement::Line(failure.line(&place.written))),
            }
        }
    }
}

fn max_texture_side(ctx: &Context) -> u32 {
    let side = ctx.input(|input| input.max_texture_side);
    u32::try_from(side).unwrap_or(u32::MAX)
}
