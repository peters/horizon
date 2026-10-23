//! Panel pixels only: native clipboard images and bounded private PNG exports.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use egui::{ColorImage, Context, Ui};
use horizon_core::browser::{BrowserPanelState, BrowserStatus, manifest::device::Screenshot};

const MAX_PIXELS: usize = 8_294_400;
const MAX_EXPORTS: usize = 8;
const EXPORT_DIRECTORY: &str = "horizon-screenshots-";

#[derive(Default)]
pub(crate) struct Screenshots {
    files: Option<PrivatePngStore>,
    feedback: Option<(String, Instant)>,
}

#[derive(Default)]
struct ExportFiles {
    exports: VecDeque<tempfile::TempPath>,
    directory: Option<tempfile::TempDir>,
}

// Horizon's normal exit uses process::exit, which does not run destructors.
static EXPORTS: Mutex<Vec<Weak<Mutex<ExportFiles>>>> = Mutex::new(Vec::new());

pub(crate) fn clear_exports() {
    clear_registry(&EXPORTS);
}

fn clear_registry(registry: &Mutex<Vec<Weak<Mutex<ExportFiles>>>>) {
    let mut exports = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for files in exports.drain(..).filter_map(|files| files.upgrade()) {
        *files.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = ExportFiles::default();
    }
}

/// A private directory of PNG files that Horizon keeps for the session and
/// removes when it exits. A clipboard image is encoded here on a worker
/// thread, so the store is shared by [`Clone`] and is `Send + Sync`.
#[derive(Clone)]
pub(crate) struct PrivatePngStore {
    files: Arc<Mutex<ExportFiles>>,
    directory_prefix: &'static str,
    capacity: usize,
}

impl PrivatePngStore {
    pub(crate) fn new(directory_prefix: &'static str, capacity: usize) -> Self {
        let files = Arc::new(Mutex::new(ExportFiles::default()));
        let mut exports = EXPORTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        exports.retain(|files| files.strong_count() > 0);
        exports.push(Arc::downgrade(&files));
        Self {
            files,
            directory_prefix,
            capacity,
        }
    }

    /// The shared export state, for tests that count or clear stored files.
    #[cfg(test)]
    fn handle(&self) -> Arc<Mutex<ExportFiles>> {
        Arc::clone(&self.files)
    }

    /// Encode `image` into a new PNG and keep the file until the process exits
    /// or the store holds more than its capacity.
    pub(crate) fn write(&self, image: &ColorImage, file_prefix: &str) -> Result<PathBuf, String> {
        let mut files = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if files.directory.is_none() {
            files.directory = Some(private_directory(self.directory_prefix)?);
        }
        let directory = files.directory.as_ref().ok_or("Image directory unavailable")?;
        let path = write_png(directory.path(), file_prefix, image)?.into_temp_path();
        let stored = path.to_path_buf();
        files.exports.push_back(path);
        while files.exports.len() > self.capacity {
            files.exports.pop_front();
        }
        Ok(stored)
    }
}

fn private_directory(prefix: &str) -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().map_err(|error| error.to_string())
}

fn write_png(directory: &Path, file_prefix: &str, image: &ColorImage) -> Result<tempfile::NamedTempFile, String> {
    let (width, height) = validate(image)?;
    let mut file = tempfile::Builder::new()
        .prefix(file_prefix)
        .suffix(".png")
        .tempfile_in(directory)
        .map_err(|error| error.to_string())?;
    let rgba: Vec<_> = image
        .pixels
        .iter()
        .flat_map(egui::Color32::to_srgba_unmultiplied)
        .collect();
    let mut encoder = png::Encoder::new(file.as_file_mut(), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder
        .write_header()
        .and_then(|mut writer| {
            writer.write_image_data(&rgba)?;
            writer.finish()
        })
        .map_err(|error| error.to_string())?;
    Ok(file)
}

impl Screenshots {
    pub(crate) fn copy_button(
        &mut self,
        ui: &mut Ui,
        enabled: bool,
        image: impl FnOnce() -> Result<ColorImage, String>,
    ) -> bool {
        let response =
            crate::icon_button::icon_button(ui, enabled, "Copy screenshot", crate::icon_button::paint_screenshot)
                .on_hover_text("Copy the panel image to your clipboard at its source resolution");
        let clicked = response.clicked();
        if clicked {
            let result = image().and_then(|image| {
                validate(&image)?;
                ui.ctx().copy_image(image);
                Ok(())
            });
            self.feedback = Some((
                result.err().unwrap_or_else(|| "Screenshot copied".into()),
                Instant::now(),
            ));
        }
        if let Some((message, at)) = &self.feedback
            && at.elapsed() < Duration::from_secs(3)
        {
            response.show_tooltip_text(message);
            ui.ctx()
                .request_repaint_after(Duration::from_secs(3).saturating_sub(at.elapsed()));
        }
        clicked
    }

    pub(crate) fn export(
        &mut self,
        ctx: &Context,
        panel_id: String,
        image: ColorImage,
        copy_to_clipboard: bool,
    ) -> Result<Screenshot, String> {
        let (width, height) = validate(&image)?;
        let files = self
            .files
            .get_or_insert_with(|| PrivatePngStore::new(EXPORT_DIRECTORY, MAX_EXPORTS));
        let path = files.write(&image, "capture-")?;
        let capture = Screenshot {
            panel_id,
            path,
            width,
            height,
            clipboard_requested: copy_to_clipboard,
        };
        if copy_to_clipboard {
            ctx.copy_image(image);
            ctx.request_repaint();
        }
        Ok(capture)
    }
}

pub(crate) fn browser_available(browser: &BrowserPanelState) -> bool {
    matches!(browser.status, BrowserStatus::Ready) && browser.frame_slot.latest().is_some()
}

pub(crate) fn browser_image(browser: &BrowserPanelState) -> Result<ColorImage, String> {
    if !matches!(browser.status, BrowserStatus::Ready) {
        return Err("Browser panel is not ready".into());
    }
    let frame = browser.frame_slot.latest().ok_or("Browser has not received a frame")?;
    let size = [frame.width as usize, frame.height as usize];
    let count = size[0]
        .checked_mul(size[1])
        .filter(|count| *count > 0 && *count <= MAX_PIXELS);
    if count.and_then(|count| count.checked_mul(3)) != Some(frame.rgb.len()) {
        return Err("Browser frame has invalid dimensions or pixels".into());
    }
    Ok(ColorImage::from_rgb(size, &frame.rgb))
}

fn validate(image: &ColorImage) -> Result<(u32, u32), String> {
    let [width, height] = image.size;
    let count = width
        .checked_mul(height)
        .filter(|count| *count > 0 && *count <= MAX_PIXELS);
    if count != Some(image.pixels.len()) {
        return Err("Screenshot has invalid dimensions or pixels".into());
    }
    Ok((
        u32::try_from(width).map_err(|error| error.to_string())?,
        u32::try_from(height).map_err(|error| error.to_string())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures as _;
    use egui::Color32;

    #[test]
    fn copy_feedback_does_not_resize_the_panel_content() {
        let ctx = Context::default();
        let mut screenshots = Screenshots::default();
        let mut remaining = Vec::new();
        for feedback in [None, Some(("Screenshot copied".into(), Instant::now()))] {
            screenshots.feedback = feedback;
            let output = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    screenshots.copy_button(ui, false, || panic!("disabled button captured pixels"));
                    remaining.push(ui.available_rect_before_wrap());
                })
                .discard_textures();
            drop(output);
        }
        assert!(remaining.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn copy_button_keeps_its_name_when_enabled_and_disabled() {
        let labels = |enabled| {
            let mut screenshots = Screenshots::default();
            crate::test_egui::accesskit_labels(|ui| {
                screenshots.copy_button(ui, enabled, || Err("no image".into()));
            })
        };
        let enabled = labels(true);
        assert!(
            enabled
                .iter()
                .any(|(label, disabled)| label == "Copy screenshot" && !disabled)
        );
        let disabled = labels(false);
        assert!(
            disabled
                .iter()
                .any(|(label, disabled)| label == "Copy screenshot" && *disabled)
        );
    }

    #[test]
    fn explicit_exit_cleanup_removes_files_even_when_ui_state_is_still_alive() {
        let mut screenshots = Screenshots::default();
        let capture = screenshots
            .export(
                &Context::default(),
                "panel".into(),
                ColorImage::filled([1, 1], Color32::RED),
                false,
            )
            .unwrap();
        let registry = Mutex::new(vec![Arc::downgrade(&screenshots.files.as_ref().unwrap().handle())]);
        clear_registry(&registry);
        assert!(!capture.path.exists());
        assert!(!capture.path.parent().unwrap().exists());
        assert!(
            screenshots
                .files
                .as_ref()
                .unwrap()
                .handle()
                .lock()
                .unwrap()
                .exports
                .is_empty()
        );
    }

    #[test]
    fn browser_capture_requires_ready_current_pixels_and_preserves_dimensions() {
        let ctx = Context::default();
        let mut screenshots = Screenshots::default();
        let original = screenshots
            .export(&ctx, "source".into(), ColorImage::filled([3, 2], Color32::BLUE), false)
            .unwrap();
        let mut browser = BrowserPanelState::inert();
        browser
            .frame_slot
            .store_png(&std::fs::read(&original.path).unwrap())
            .unwrap();
        assert!(browser_image(&browser).is_err());
        browser.status = BrowserStatus::Ready;
        assert_eq!(
            browser_image(&browser).unwrap(),
            ColorImage::filled([3, 2], Color32::BLUE)
        );
        browser.frame_slot.clear();
        assert!(browser_image(&browser).is_err());
    }

    #[test]
    fn export_encodes_original_pixels_and_queues_clipboard_only_when_requested() {
        let ctx = Context::default();
        let image = ColorImage::new([2, 1], vec![Color32::RED, Color32::BLUE]);
        let mut screenshots = Screenshots::default();
        let mut capture = None;
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                capture = Some(
                    screenshots
                        .export(ui.ctx(), "panel".into(), image.clone(), true)
                        .unwrap(),
                );
            })
            .discard_textures();
        assert!(
            matches!(&output.platform_output.commands[..], [egui::OutputCommand::CopyImage(copied)] if copied == &image)
        );
        let capture = capture.unwrap();
        assert_eq!((capture.width, capture.height), (2, 1));
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&capture.path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!(pixels, [255, 0, 0, 255, 0, 0, 255, 255]);
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                screenshots
                    .export(ui.ctx(), "panel".into(), image.clone(), false)
                    .unwrap();
            })
            .discard_textures();
        assert!(output.platform_output.commands.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&capture.path).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_eq!(
                std::fs::metadata(capture.path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        drop(screenshots);
        assert!(!capture.path.exists());
    }

    #[test]
    fn exports_are_bounded_and_invalid_images_leave_no_artifacts_or_clipboard_commands() {
        let ctx = Context::default();
        let mut screenshots = Screenshots::default();
        let image = ColorImage::filled([1, 1], Color32::GREEN);
        let first = screenshots.export(&ctx, "panel".into(), image.clone(), false).unwrap();
        for _ in 0..MAX_EXPORTS {
            screenshots.export(&ctx, "panel".into(), image.clone(), false).unwrap();
        }
        assert!(!first.path.exists());
        assert_eq!(
            screenshots
                .files
                .as_ref()
                .unwrap()
                .handle()
                .lock()
                .unwrap()
                .exports
                .len(),
            MAX_EXPORTS
        );
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                let mut malformed = image.clone();
                malformed.size = [usize::MAX, 2];
                assert!(screenshots.export(ui.ctx(), "panel".into(), malformed, true).is_err());
            })
            .discard_textures();
        assert!(output.platform_output.commands.is_empty());
        assert_eq!(
            screenshots
                .files
                .as_ref()
                .unwrap()
                .handle()
                .lock()
                .unwrap()
                .exports
                .len(),
            MAX_EXPORTS
        );
    }
}
