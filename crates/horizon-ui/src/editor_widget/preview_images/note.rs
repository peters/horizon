//! The places of a picture in the text of a note.
//!
//! The preview asks this module where the pictures of a note are, and what the
//! preview must do with each one. The answer of a plane is text only: it names a
//! path or it holds the line that replaces the picture. Nothing here reads,
//! file-types or decodes a file, so a frame that plans the pictures of a note
//! stays on the UI thread.

use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::{Failure, MAX_PICTURES};

/// One picture of a note, and what the preview gives it.
pub(crate) struct Place {
    /// The bytes of the note text that this picture occupies.
    pub(crate) span: Range<usize>,
    /// The destination, as the note writes it.
    pub(crate) written: String,
    /// The text of the destination that a URI of the preview carries, so the
    /// picture resolves beside the note and not beside the working directory.
    pub(crate) tail: String,
    /// The alternative text of the picture, without its markup.
    pub(crate) alt: String,
    /// Whether the destination has no scheme, so the implicit scheme of the
    /// viewer resolves it and the text of the note stays the text of the editor.
    pub(crate) implicit: bool,
    pub(crate) fate: Fate,
}

impl Place {
    /// The name of the picture for the line that says that it is on its way.
    pub(crate) fn label(&self) -> &str {
        if self.alt.is_empty() { &self.written } else { &self.alt }
    }
}

/// What the preview must do with one picture.
pub(crate) enum Fate {
    /// A file of this computer: the paths that this file can have, the path as
    /// the note writes it first.
    File(Vec<PathBuf>),
    /// A line of text in the place of the picture, and no file work at all.
    Line(String),
}

/// The text that takes the place of one picture.
pub(crate) enum Replacement {
    /// An image of the private scheme of the preview.
    Picture(String),
    /// A line of text of the preview, without markup of the untrusted path.
    Line(String),
}

/// The text of one note and every picture in it.
pub(crate) struct NoteText {
    pub(crate) source: String,
    pub(crate) places: Vec<Place>,
}

impl NoteText {
    /// Every picture of `text`, with the folder that a relative path of the note
    /// belongs to.
    pub(crate) fn new(text: &str, folder: Option<&Path>) -> Self {
        let mut places = Vec::new();
        let mut files = 0_usize;
        let mut open = None;
        for (event, span) in Parser::new_ext(text, viewer_options()).into_offset_iter() {
            match event {
                Event::Start(Tag::Image { dest_url, .. }) => {
                    open = Some((span, dest_url.to_string(), String::new()));
                }
                Event::Text(alt_text) | Event::Code(alt_text) => {
                    if let Some((_, _, alt)) = open.as_mut() {
                        alt.push_str(&alt_text);
                    }
                }
                Event::End(TagEnd::Image) => {
                    let Some((span, written, alt)) = open.take() else {
                        continue;
                    };
                    let (fate, implicit, tail) = fate_of(&written, &alt, folder);
                    let fate = match fate {
                        Fate::File(_) if files >= MAX_PICTURES => Fate::Line(Failure::NoRoom.line(&written)),
                        Fate::File(paths) => {
                            files += 1;
                            Fate::File(paths)
                        }
                        Fate::Line(line) => Fate::Line(line),
                    };
                    places.push(Place {
                        span,
                        written,
                        tail,
                        alt,
                        implicit,
                        fate,
                    });
                }
                _ => {}
            }
        }
        Self {
            source: text.to_owned(),
            places,
        }
    }

    /// `out` becomes the text of the note with the place of each picture filled
    /// by its replacement, or by the picture that the note wrote.
    pub(crate) fn rewrite(&self, out: &mut String, replacements: &[Option<Replacement>]) {
        out.clear();
        let mut cursor = 0;
        for (place, replacement) in self.places.iter().zip(replacements) {
            if place.span.start < cursor || place.span.end > self.source.len() {
                continue;
            }
            out.push_str(&self.source[cursor..place.span.start]);
            match replacement {
                Some(Replacement::Picture(picture)) => out.push_str(picture),
                Some(Replacement::Line(line)) => out.push_str(line),
                None => out.push_str(&self.source[place.span.clone()]),
            }
            cursor = place.span.end;
        }
        if cursor <= self.source.len() {
            out.push_str(&self.source[cursor..]);
        }
    }
}

/// The options of the viewer, so this module finds the same pictures in a note.
fn viewer_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

/// `text` with the markup of the preview worked out, so a path or an alternative
/// text of an untrusted note shows as itself and starts no markup of its own.
pub(crate) fn escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for (index, character) in text.chars().enumerate() {
        let markup = matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '~' | '|' | '!'
        ) || (index == 0 && matches!(character, '#' | '-' | '+' | '=' | '.' | ':'));
        if markup {
            escaped.push('\\');
        }
        escaped.push(if character.is_control() { ' ' } else { character });
    }
    escaped
}

fn fate_of(written: &str, alt: &str, folder: Option<&Path>) -> (Fate, bool, String) {
    let own = written.to_owned();
    if let Some(rest) = written.strip_prefix("//") {
        return (remote(alt, host_of(rest), written), false, own);
    }
    if written.starts_with("data:") {
        return (remote(alt, None, written), false, own);
    }
    if let Some((scheme, rest)) = split_scheme(written) {
        if scheme.eq_ignore_ascii_case("file") {
            let path = rest.strip_prefix("//").unwrap_or(rest);
            let tail = path.to_owned();
            return match candidates(path, folder) {
                Some(paths) => (Fate::File(paths), false, tail),
                None => (Fate::Line(Failure::NotFound.line(written)), false, tail),
            };
        }
        return (remote(alt, host_of(rest), written), false, own);
    }
    match candidates(written, folder) {
        Some(paths) => (Fate::File(paths), true, own),
        None => (Fate::Line(Failure::NotFound.line(written)), true, own),
    }
}

fn remote(alt: &str, host: Option<&str>, written: &str) -> Fate {
    let label = if !alt.is_empty() {
        alt
    } else if let Some(host) = host {
        host
    } else {
        written
    };
    Fate::Line(format!("Remote image not loaded: {}", escaped(label)))
}

/// The scheme of a URI that has one, and the text after its `://`.
fn split_scheme(written: &str) -> Option<(&str, &str)> {
    let index = written.find("://")?;
    Some((&written[..index], &written[index + 3..]))
}

fn host_of(rest: &str) -> Option<&str> {
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

/// The paths that a destination can name: the text as written, and the text with
/// its `%XX` escapes worked out when it has any.
fn candidates(written: &str, folder: Option<&Path>) -> Option<Vec<PathBuf>> {
    let path = Path::new(written);
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        folder?.to_path_buf()
    };
    let mut paths = vec![base.join(path)];
    if let Some(decoded) = percent_decode(written) {
        let second = base.join(Path::new(&decoded));
        if second != paths[0] {
            paths.push(second);
        }
    }
    Some(paths)
}

/// `written` with each `%XX` escape worked out, when it has an escape and the
/// result is text.
fn percent_decode(written: &str) -> Option<String> {
    String::from_utf8(super::percent_decoded(written, false)?).ok()
}
