//! The private URI scheme of the pictures of a markdown preview.
//!
//! A URI of this module carries the canvas zoom of the preview, the folder of the
//! note and the destination as the note writes it, so the loader of a picture
//! needs no state of a panel, and every zoom of one note shares one texture.

use std::fmt::Write as _;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

use super::{PATH_TAIL, SCHEME, WRITTEN_TAIL};

fn zoom_of(zoom: f32) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    let hundredths = (zoom * 100.0).round() as i32;
    hundredths
}

#[allow(clippy::cast_precision_loss)]
fn zoom_of_hundredths(hundredths: i32) -> f32 {
    hundredths as f32 / 100.0
}

/// The prefix that resolves every relative picture of `folder` at `zoom`.
pub(crate) fn implicit_scheme(zoom: f32, folder: &Path) -> String {
    format!("{SCHEME}{}/{}?", zoom_of(zoom), encode_path(folder))
}

/// The URI of the picture that the note writes as `tail` in `folder`.
///
/// The tail of a URI that the viewer builds is the destination as the note
/// writes it, so the separator is `?`. This module builds the URI of a `file:`
/// picture itself, and it shares one texture with every zoom of the note: the
/// separator `!` carries an encoded tail, so the URI holds no space and no
/// bracket that a destination of a Markdown picture cannot carry.
pub(crate) fn picture_uri(zoom: f32, folder: &Path, tail: &str) -> String {
    format!(
        "{SCHEME}{}/{}{PATH_TAIL}{}",
        zoom_of(zoom),
        encode_path(folder),
        encode_path(Path::new(tail))
    )
}

/// The zoom and the file of a URI of this module.
pub(crate) fn picture_of_uri(uri: &str) -> Option<(f32, PathBuf)> {
    let rest = uri.strip_prefix(SCHEME)?;
    let (zoom, rest) = rest.split_once('/')?;
    let zoom: i32 = zoom.parse().ok()?;
    let file = if let Some((folder, written)) = rest.split_once(WRITTEN_TAIL) {
        let folder = decode_path(folder)?;
        let written = Path::new(written);
        if written.is_absolute() {
            written.to_path_buf()
        } else {
            folder.join(written)
        }
    } else {
        let (folder, tail) = rest.split_once(PATH_TAIL)?;
        let folder = decode_path(folder)?;
        let tail = decode_path(tail)?;
        if tail.is_absolute() { tail } else { folder.join(tail) }
    };
    Some((zoom_of_hundredths(zoom), file))
}

fn encode_path(path: &Path) -> String {
    let mut encoded = String::new();
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn decode_path(encoded: &str) -> Option<PathBuf> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = encoded.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}
