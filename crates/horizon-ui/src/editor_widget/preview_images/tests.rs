//! The pictures of a markdown preview, from the text of a note through the
//! viewer call of the editor panel.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::epaint::ClippedShape;
use egui::{Context, Pos2, Rect, Vec2};
use horizon_core::ui_thread::FrameScope;
use horizon_core::{Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::super::tests::painted_rows;
use super::super::{MarkdownPreviewCache, render_preview_pane};
use super::note::{Fate, NoteText, Replacement};
use super::uri::picture_of_uri;
use super::{MAX_FILE_BYTES, MAX_PICTURES, NoteImages};

/// The size of a test picture, in pixels and in points at a zoom of 100 percent.
const WIDE: (u32, u32) = (120, 60);

/// The width of the preview pane of a test, in points of a zoom of 100 percent.
const PANE: f32 = 300.0;

/// The side of the largest texture that a test pass reports, so a picture of the
/// tests is refused by the pixel bound and not by a small test display.
const MAX_SIDE: usize = 16_384;

fn write_png(path: &Path, width: u32, height: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("picture folder");
    }
    let file = std::fs::File::create(path).expect("picture file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("picture header");
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let red = u8::try_from(x % 256).unwrap_or_default();
            let green = u8::try_from(y % 256).unwrap_or_default();
            pixels.extend_from_slice(&[red, green, 0x40, 0xff]);
        }
    }
    writer.write_image_data(&pixels).expect("picture data");
}

/// A note whose header claims `width` by `height` pixels and that holds no
/// picture, so a decode of it fails and only a header check can refuse it.
fn write_png_header_only(path: &Path, width: u32, height: u32) {
    write_png(path, 1, 1);
    let mut bytes = std::fs::read(path).expect("picture bytes");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32fast::hash(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    std::fs::write(path, bytes).expect("picture header");
}

/// A note on this computer, and the folder that its pictures belong to.
struct Note {
    folder: tempfile::TempDir,
}

impl Note {
    fn new(markdown: &str) -> Self {
        Self::build(|_| markdown.to_owned())
    }

    /// A note whose text `create` writes into the folder of its pictures.
    fn build(create: impl FnOnce(&Path) -> String) -> Self {
        let folder = tempfile::TempDir::new().expect("folder of a note");
        let text = create(folder.path());
        std::fs::write(folder.path().join("note.md"), text).expect("note file");
        Self { folder }
    }

    fn picture(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.folder.path().join(name);
        write_png(&path, width, height);
        path
    }

    fn panel(&self) -> Panel {
        Panel::spawn(
            PanelId(7),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Editor,
                command: Some(self.folder.path().join("note.md").display().to_string()),
                cwd: Some(self.folder.path().to_path_buf()),
                ..PanelOptions::default()
            },
        )
        .expect("editor panel")
    }
}

fn test_context() -> Context {
    let ctx = Context::default();
    ctx.set_fonts(crate::app::bootstrap::configure_fonts());
    ctx
}

/// The shapes of one pass of the preview of `panel` at the canvas zoom `zoom`.
fn shapes_at(ctx: &Context, panel: &mut Panel, cache: &mut MarkdownPreviewCache, zoom: f32) -> Vec<ClippedShape> {
    let layer = egui::LayerId::new(egui::Order::Background, egui::Id::unique("canvas"));
    let input = egui::RawInput {
        max_texture_side: Some(MAX_SIDE),
        ..egui::RawInput::default()
    };
    let output = ctx.run_ui(input, |_| {
        ctx.set_transform_layer(layer, egui::emath::TSTransform::from_scaling(zoom));
        let builder = egui::UiBuilder::new()
            .layer_id(layer)
            .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(PANE, 600.0)));
        let mut child = egui::Ui::new(ctx.clone(), egui::Id::unique("canvas ui"), builder);
        child.set_clip_rect(child.max_rect());
        render_preview_pane(&mut child, panel, Some(cache));
    });
    let shapes = output.shapes.clone();
    let _ = crate::test_egui::DiscardTextures::discard_textures(output);
    shapes
}

/// The passes of the preview of `panel` until `wanted` holds, and no longer than
/// the deadline, so a picture that never arrives ends the test instead of the
/// test run.
fn draw_until(
    ctx: &Context,
    panel: &mut Panel,
    cache: &mut MarkdownPreviewCache,
    zoom: f32,
    within: Duration,
    wanted: impl Fn(&[ClippedShape]) -> bool,
) -> Vec<ClippedShape> {
    let start = Instant::now();
    loop {
        let shapes = shapes_at(ctx, panel, cache, zoom);
        if wanted(&shapes) {
            return shapes;
        }
        assert!(
            start.elapsed() < within,
            "the preview did not reach the state that the test waits for in {within:?}: {} / {:?}",
            text_of(&shapes),
            pictures_of(&shapes)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The painted rectangles of the textured pictures of a pass. The preview of
/// `egui` paints a picture as a rectangle with a texture.
fn pictures_of(shapes: &[ClippedShape]) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, rects: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Rect(rect) => {
                let textured = rect.brush.as_ref().is_some_and(|brush| brush.uv != Rect::ZERO);
                if textured && rect.rect.width() > 0.5 && rect.rect.height() > 0.5 {
                    rects.push(rect.rect);
                }
            }
            egui::Shape::Mesh(mesh) => {
                let mut rect = Rect::NOTHING;
                for vertex in &mesh.vertices {
                    rect = rect.union(Rect::from_min_max(vertex.pos, vertex.pos));
                }
                if rect.is_finite() && rect.width() > 0.5 && rect.height() > 0.5 {
                    rects.push(rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, rects);
                }
            }
            _ => {}
        }
    }
    let mut rects = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut rects);
    }
    rects
}

/// Every painted word of a pass, in paint order.
fn text_of(shapes: &[ClippedShape]) -> String {
    painted_rows(shapes).into_iter().flatten().collect::<Vec<_>>().join(" ")
}

fn shows_line(shapes: &[ClippedShape], line: &str) -> bool {
    text_of(shapes).contains(line)
}

#[test]
fn the_span_of_a_picture_is_the_whole_picture_of_the_note() {
    let note = NoteText::new("A line ![the alt text](picture.png) and more.", Some(Path::new("/tmp")));
    assert_eq!(note.places.len(), 1);
    let place = &note.places[0];
    assert_eq!(&note.source[place.span.clone()], "![the alt text](picture.png)");
    assert_eq!(place.written, "picture.png");
    assert_eq!(place.alt, "the alt text");
    assert!(place.implicit);
}

#[test]
fn a_picture_that_is_a_file_of_this_computer_becomes_a_path() {
    let note = NoteText::new(
        "![a](/tmp/a.png) ![b](pictures/b.png) ![c](<my picture.png>) ![d](my%20picture.png)",
        Some(Path::new("/notes")),
    );
    let paths: Vec<Vec<PathBuf>> = note
        .places
        .iter()
        .map(|place| match &place.fate {
            Fate::File(paths) => paths.clone(),
            Fate::Line(line) => panic!("this picture is a file: {line}"),
        })
        .collect();
    assert_eq!(paths[0], vec![PathBuf::from("/tmp/a.png")]);
    assert_eq!(paths[1], vec![PathBuf::from("/notes/pictures/b.png")]);
    assert_eq!(
        paths[2],
        vec![PathBuf::from("/notes/my picture.png")],
        "a space in a destination works"
    );
    assert_eq!(
        paths[3],
        vec![
            PathBuf::from("/notes/my%20picture.png"),
            PathBuf::from("/notes/my picture.png")
        ],
        "a percent-encoded destination names the file with the space"
    );
}

#[test]
fn a_picture_that_is_not_a_file_of_this_computer_becomes_a_line() {
    for (written, expected) in [
        (
            "https://example.test/shot.png",
            "Remote image not loaded: the alternative",
        ),
        ("//example.test/shot.png", "Remote image not loaded: the alternative"),
        ("data:image/png;base64,AAAA", "Remote image not loaded: the alternative"),
        (
            "http://example.test/shot.png",
            "Remote image not loaded: the alternative",
        ),
    ] {
        let note = NoteText::new(&format!("![the alternative](<{written}>)"), Some(Path::new("/notes")));
        match &note.places[0].fate {
            Fate::Line(line) => assert_eq!(line, expected, "the line of {written}"),
            Fate::File(paths) => panic!("{written} must not name a file: {paths:?}"),
        }
    }
}

#[test]
fn a_relative_picture_of_a_note_without_a_file_becomes_a_line() {
    let note = NoteText::new("![shot](picture.png)", None);
    match &note.places[0].fate {
        Fate::Line(line) => assert_eq!(line, "Image not found: picture.png"),
        Fate::File(paths) => panic!("a note without a file has no relative picture: {paths:?}"),
    }
}

#[test]
fn a_note_gives_its_pictures_a_line_after_the_last_picture_that_it_loads() {
    let markdown = "![shot](a.png) ".repeat(MAX_PICTURES + 1);
    let note = NoteText::new(&markdown, Some(Path::new("/notes")));
    assert_eq!(note.places.len(), MAX_PICTURES + 1);
    assert!(matches!(note.places[MAX_PICTURES - 1].fate, Fate::File(_)));
    match &note.places[MAX_PICTURES].fate {
        Fate::Line(line) => assert_eq!(line, "Too many pictures in this note: a.png"),
        Fate::File(_) => panic!("a picture after the last one must not be read"),
    }
}

#[test]
fn a_line_replaces_a_picture_without_the_markup_of_an_untrusted_path() {
    let note = NoteText::new("before ![x](<a *b* [c].png>) after", None);
    let replacements: Vec<_> = note
        .places
        .iter()
        .map(|place| match &place.fate {
            Fate::Line(line) => Some(Replacement::Line(line.clone())),
            Fate::File(_) => None,
        })
        .collect();
    let mut out = String::new();
    note.rewrite(&mut out, &replacements);
    assert_eq!(
        out, "before Image not found: a \\*b\\* \\[c\\].png after",
        "the markup of a path of the note is worked out"
    );
    let remote = NoteText::new("![x](https://example.test/shot.png)", None);
    match &remote.places[0].fate {
        Fate::Line(line) => assert_eq!(line, "Remote image not loaded: x"),
        Fate::File(paths) => panic!("a remote picture names no file: {paths:?}"),
    }
}

#[test]
fn the_uri_of_a_picture_keeps_the_folder_the_destination_and_the_zoom() {
    let uri = super::picture_uri(1.5, Path::new("/notes/a folder"), "pictures/my shot.png");
    assert_eq!(
        picture_of_uri(&uri),
        Some((1.5, PathBuf::from("/notes/a folder/pictures/my shot.png")))
    );
    let absolute = super::picture_uri(0.67, Path::new("/notes"), "/tmp/shot.png");
    assert_eq!(picture_of_uri(&absolute), Some((0.67, PathBuf::from("/tmp/shot.png"))));
}

#[test]
fn the_application_has_no_loader_that_could_read_a_network_uri() {
    let ctx = test_context();
    let images = NoteImages::shared(&ctx);
    let texture = |uri: &str| {
        egui::load::TextureLoader::load(
            &*images,
            &ctx,
            uri,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        )
    };
    for uri in ["https://example.test/shot.png", "file:///tmp/shot.png", "data:,x"] {
        assert!(texture(uri).is_err(), "{uri} has no texture of a note");
        assert_eq!(picture_of_uri(uri), None, "{uri} is not a picture of a note");
    }
    assert!(matches!(
        ctx.try_load_image("https://example.test/shot.png", egui::load::SizeHint::default()),
        Err(egui::load::LoadError::NoImageLoaders)
    ));
    assert!(
        ctx.try_load_texture(
            "https://example.test/shot.png",
            egui::TextureOptions::default(),
            egui::load::SizeHint::default()
        )
        .is_err(),
        "no texture loader of this application reads a remote picture"
    );
}

#[test]
fn a_picture_beside_the_note_shows_with_its_size() {
    let note = Note::new("The picture:\n\n![shot](shot.png)\n");
    note.picture("shot.png", WIDE.0, WIDE.1);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    let rects = pictures_of(&shapes);
    assert_eq!(rects.len(), 1, "the preview draws one picture: {rects:?}");
    let rect = rects[0];
    assert!(
        (rect.width() - 120.0).abs() <= 1.0 && (rect.height() - 60.0).abs() <= 1.0,
        "the picture shows at its natural size: {rect:?}"
    );
    assert!(text_of(&shapes).contains("The picture:"));
}

#[test]
fn a_picture_in_a_sub_folder_a_picture_outside_the_note_and_a_file_uri_show() {
    let outside = tempfile::TempDir::new().expect("folder of a picture");
    let away = outside.path().join("away.png");
    write_png(&away, 30, 10);
    let note = Note::build(|folder| {
        let inside = folder.join("pictures/shot.png");
        write_png(&inside, 40, 20);
        format!(
            "![inside](pictures/shot.png)\n\n![away]({})\n\n![uri](file://{})\n",
            away.display(),
            inside.display()
        )
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        pictures_of(shapes).len() == 3
    });
    let mut widths: Vec<f32> = pictures_of(&shapes).iter().map(Rect::width).collect();
    widths.sort_by(f32::total_cmp);
    assert!(
        (widths[0] - 30.0).abs() <= 1.0 && (widths[1] - 40.0).abs() <= 1.0 && (widths[2] - 40.0).abs() <= 1.0,
        "each picture shows with its own size: {widths:?}"
    );
}

#[test]
fn a_picture_whose_destination_holds_a_space_shows() {
    let note = Note::build(|folder| {
        write_png(&folder.join("my shot.png"), 50, 25);
        "![one](<my shot.png>)\n\n![two](my%20shot.png)\n".to_owned()
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        pictures_of(shapes).len() == 2
    });
    for rect in pictures_of(&shapes) {
        assert!(
            (rect.width() - 50.0).abs() <= 1.0,
            "a destination with a space shows its picture: {rect:?}"
        );
    }
}

#[test]
fn a_remote_picture_makes_no_request_and_shows_its_line() {
    let note = Note::new("![the alternative](https://example.test/shot.png)\n");
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
    assert!(
        shows_line(&shapes, "Remote image not loaded: the alternative"),
        "the preview shows the line of a remote picture: {}",
        text_of(&shapes)
    );
    assert!(pictures_of(&shapes).is_empty(), "a remote picture paints no picture");
}

#[test]
fn a_picture_that_is_not_there_shows_its_path_as_the_note_writes_it() {
    let note = Note::new("![shot](pictures/gone.png)\n");
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(2), |shapes| {
        shows_line(shapes, "Image not found")
    });
    assert!(
        shows_line(&shapes, "Image not found: pictures/gone.png"),
        "the line names the path of the note: {}",
        text_of(&shapes)
    );
}

#[test]
fn a_pipe_a_device_a_folder_and_a_text_file_are_refused() {
    let note = Note::build(|folder| {
        let pipe = folder.join("pipe.png");
        rustix::fs::mkfifoat(rustix::fs::CWD, &pipe, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
            .expect("the test makes a pipe");
        std::fs::create_dir(folder.join("folder.png")).expect("the test makes a folder");
        std::fs::write(folder.join("text.png"), "this is not a picture\n").expect("the test writes a text file");
        "![pipe](pipe.png)\n\n![zero](/dev/zero)\n\n![folder](folder.png)\n\n![text](text.png)\n".to_owned()
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let start = Instant::now();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(2), |shapes| {
        shows_line(shapes, "Not a regular file: pipe.png")
            && shows_line(shapes, "Not a regular file: /dev/zero")
            && shows_line(shapes, "Not a regular file: folder.png")
            && shows_line(shapes, "Not a picture file: text.png")
    });
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "a pipe, a device, a folder and a text file answer at once: {:?}",
        start.elapsed()
    );
    assert!(pictures_of(&shapes).is_empty(), "none of them paints a picture");
}

#[test]
fn a_file_above_the_bound_is_refused_without_a_read_of_its_content() {
    let note = Note::build(|folder| {
        let big = folder.join("big.png");
        let file = std::fs::File::create(&big).expect("big file");
        file.set_len(MAX_FILE_BYTES + 1).expect("big file length");
        "![big](big.png)\n".to_owned()
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(5), |shapes| {
        shows_line(shapes, "Picture too large")
    });
    assert!(
        shows_line(&shapes, "Picture too large: big.png"),
        "the line of a file above the bound: {}",
        text_of(&shapes)
    );
}

#[test]
fn a_picture_header_above_the_pixel_bound_is_refused_before_the_decode() {
    let note = Note::build(|folder| {
        write_png_header_only(&folder.join("huge.png"), 5_000, 5_000);
        "![huge](huge.png)\n".to_owned()
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(5), |shapes| {
        shows_line(shapes, "Picture too large")
    });
    assert!(
        shows_line(&shapes, "Picture too large: huge.png"),
        "the pixel bound refuses the header before the decode: {}",
        text_of(&shapes)
    );
}

#[test]
fn a_note_shows_the_pictures_that_it_loads_and_a_line_after_them() {
    let note = Note::build(|folder| {
        let mut markdown = String::new();
        for index in 0..=MAX_PICTURES {
            let name = format!("shot-{index}.png");
            write_png(&folder.join(&name), 8, 4);
            let _ = writeln!(markdown, "![shot]({name})");
        }
        markdown
    });
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(30), |shapes| {
        pictures_of(shapes).len() == MAX_PICTURES && shows_line(shapes, "Too many pictures")
    });
    assert!(
        shows_line(
            &shapes,
            &format!("Too many pictures in this note: shot-{MAX_PICTURES}.png")
        ),
        "the picture after the last one shows a line: {}",
        text_of(&shapes)
    );
}

#[test]
fn a_frame_that_starts_and_shows_a_picture_does_no_file_work_on_the_ui_thread() {
    horizon_core::ui_thread::use_strict_mode();
    let note = Note::new("![shot](shot.png)\n");
    note.picture("shot.png", WIDE.0, WIDE.1);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    {
        let _frame = FrameScope::enter();
        let first = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
        assert!(
            shows_line(&first, "Loading picture"),
            "the first frame gives the picture one line: {}",
            text_of(&first)
        );
    }
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    assert_eq!(pictures_of(&shapes).len(), 1);
    {
        let _frame = FrameScope::enter();
        let again = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
        assert_eq!(pictures_of(&again).len(), 1, "the frame draws the picture of the note");
    }
}

#[test]
fn a_picture_keeps_its_size_against_the_text_at_every_zoom() {
    let note = Note::new(
        "A paragraph of the preview that is long enough to wrap over more than one line of a narrow panel.\n\n![shot](shot.png)\n\nA last line.\n",
    );
    note.picture("shot.png", WIDE.0, WIDE.1);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let at = |ctx: &Context, panel: &mut Panel, cache: &mut MarkdownPreviewCache, zoom: f32| {
        let shapes = draw_until(ctx, panel, cache, zoom, Duration::from_secs(20), |shapes| {
            !pictures_of(shapes).is_empty()
        });
        let rect = pictures_of(&shapes)[0];
        (rect, painted_rows(&shapes))
    };
    let (one, rows_one) = at(&ctx, &mut panel, &mut cache, 1.0);
    assert!(rows_one.len() > 3, "the preview paints its words and its picture");
    for zoom in [0.67_f32, 1.5] {
        let (rect, rows) = at(&ctx, &mut panel, &mut cache, zoom);
        assert!(
            (rect.width() - one.width() * zoom).abs() <= 1.0,
            "at zoom {zoom} the picture is {} points wide and not {}",
            rect.width(),
            one.width() * zoom
        );
        assert!(
            (rect.height() - one.height() * zoom).abs() <= 1.0,
            "at zoom {zoom} the picture is {} points high and not {}",
            rect.height(),
            one.height() * zoom
        );
        assert_eq!(rows, rows_one, "at zoom {zoom} the text keeps the lines of zoom 1");
    }
}

#[test]
fn a_picture_wider_than_the_pane_shrinks_and_keeps_its_proportion() {
    let note = Note::new("![wide](wide.png)\n");
    note.picture("wide.png", 900, 60);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    let rect = pictures_of(&shapes)[0];
    assert!(
        rect.width() <= PANE,
        "the picture is not wider than the preview pane: {rect:?}"
    );
    assert!(
        (rect.width() / rect.height() - 15.0).abs() < 0.5,
        "the picture keeps its proportion: {rect:?}"
    );
}

#[test]
fn a_note_without_a_picture_keeps_the_plain_path_of_the_preview() {
    let note = Note::new("# Notes\n\nA paragraph that names no picture.\n");
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
    assert!(text_of(&shapes).contains("A paragraph"), "the note draws");
    assert!(
        !NoteImages::installed(&ctx),
        "a note without a picture installs no loader and starts no thread"
    );
    assert!(cache.pictures.is_none(), "a note without a picture keeps no cache");
}

#[test]
fn a_picture_above_the_folder_of_the_note_shows() {
    let root = tempfile::TempDir::new().expect("folder above the note");
    let folder = root.path().join("sub");
    std::fs::create_dir(&folder).expect("folder of the note");
    write_png(&root.path().join("away.png"), 20, 10);
    std::fs::write(folder.join("note.md"), "![away](../away.png)\n").expect("note file");
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = Panel::spawn(
        PanelId(8),
        WorkspaceId(1),
        PanelOptions {
            kind: PanelKind::Editor,
            command: Some(folder.join("note.md").display().to_string()),
            cwd: Some(folder),
            ..PanelOptions::default()
        },
    )
    .expect("editor panel");
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    let rect = pictures_of(&shapes)[0];
    assert!(
        (rect.width() - 20.0).abs() <= 1.0,
        "a path with a parent step shows its picture: {rect:?}"
    );
}

#[test]
fn a_picture_that_changes_on_disk_keeps_its_picture_until_the_note_is_saved() {
    let note = Note::new("![shot](shot.png)\n");
    note.picture("shot.png", 40, 20);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    assert!((pictures_of(&shapes)[0].width() - 40.0).abs() <= 1.0);
    note.picture("shot.png", 60, 30);
    let shapes = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
    assert!(
        (pictures_of(&shapes)[0].width() - 40.0).abs() <= 1.0,
        "the preview keeps the picture that it read: {:?}",
        pictures_of(&shapes)
    );
    for saving in [true, false] {
        if let Some(editor) = panel.content.editor_mut() {
            editor.saving = saving;
        }
        let _ = shapes_at(&ctx, &mut panel, &mut cache, 1.0);
    }
    let shapes = draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        pictures_of(shapes)
            .first()
            .is_some_and(|rect| (rect.width() - 60.0).abs() <= 1.0)
    });
    assert!((pictures_of(&shapes)[0].width() - 60.0).abs() <= 1.0);
}

#[test]
fn the_pictures_of_a_note_end_with_its_panel() {
    let note = Note::new("![shot](shot.png)\n");
    note.picture("shot.png", WIDE.0, WIDE.1);
    let ctx = test_context();
    let mut cache = MarkdownPreviewCache::default();
    let mut panel = note.panel();
    draw_until(&ctx, &mut panel, &mut cache, 1.0, Duration::from_secs(20), |shapes| {
        !pictures_of(shapes).is_empty()
    });
    let images = Arc::clone(&cache.pictures.as_ref().expect("the pictures of the note").images);
    assert_eq!(images.held(), 1, "the loader holds the picture of the note");
    drop(cache);
    assert_eq!(
        images.held(),
        0,
        "the cache of the pictures of a note ends with its panel"
    );
}
