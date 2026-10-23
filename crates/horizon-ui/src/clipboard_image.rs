//! Clipboard images pasted into a terminal panel.
//!
//! A terminal receives text only, so a clipboard image becomes a private PNG
//! file and the terminal receives its quoted path, as the Linux terminals do.
//! The program in the shell, not Horizon, decides what to do with the file.
//!
//! Encoding and disk writes run on a worker thread; the UI thread only clones
//! the image and later receives the finished path.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::ColorImage;
use horizon_core::PanelId;

use crate::screenshot::PrivatePngStore;

/// How many pasted images stay on disk. A shell command can still read any of
/// them; the oldest goes away only after this many newer pastes.
const STORED_IMAGES: usize = 64;
const IMAGE_DIRECTORY: &str = "horizon-pasted-images-";

/// The result of one clipboard image paste, addressed to the panel that
/// received it. `path` is `None` when the image could not be stored.
pub(crate) struct PastedImage {
    pub(crate) panel: PanelId,
    pub(crate) path: Option<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct ClipboardImages {
    sender: Sender<PastedImage>,
    receiver: Arc<Mutex<Receiver<PastedImage>>>,
    files: PrivatePngStore,
    next_index: Arc<AtomicU64>,
}

impl ClipboardImages {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            files: PrivatePngStore::new(IMAGE_DIRECTORY, STORED_IMAGES),
            next_index: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Store `image` for `panel` on a worker thread and wake the UI to paste it.
    pub(crate) fn persist(&self, ctx: &egui::Context, panel: PanelId, image: Arc<ColorImage>) {
        let index = self.next_index.fetch_add(1, Ordering::Relaxed);
        let files = self.files.clone();
        let sender = self.sender.clone();
        let repaint_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("horizon-paste-image".to_string())
            .spawn(move || {
                let result = files.write(&image, &format!("pasted-{index}-"));
                if let Err(error) = &result {
                    tracing::warn!(%error, "cannot store the pasted clipboard image");
                }
                let _ = sender.send(PastedImage {
                    panel,
                    path: result.ok(),
                });
                repaint_ctx.request_repaint();
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "cannot start the clipboard image writer");
            let _ = self.sender.send(PastedImage { panel, path: None });
            ctx.request_repaint();
        }
    }

    /// The pastes that finished since the last call.
    pub(crate) fn take_finished(&self) -> Vec<PastedImage> {
        let receiver = self.receiver.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        receiver.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipboardImages, PastedImage};
    use egui::ColorImage;
    use horizon_core::PanelId;

    fn image() -> std::sync::Arc<ColorImage> {
        std::sync::Arc::new(ColorImage::filled([2, 1], egui::Color32::RED))
    }

    fn wait_for_paste(images: &ClipboardImages) -> Vec<PastedImage> {
        for _ in 0..500 {
            let finished = images.take_finished();
            if !finished.is_empty() {
                return finished;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the clipboard image writer did not finish");
    }

    #[test]
    fn a_pasted_image_becomes_a_readable_png_for_its_panel() {
        let ctx = egui::Context::default();
        let images = ClipboardImages::new();
        images.persist(&ctx, PanelId(7), image());

        let finished = wait_for_paste(&images);
        assert_eq!(finished.len(), 1);
        let paste = &finished[0];
        assert_eq!(paste.panel, PanelId(7));
        let path = paste.path.as_ref().expect("stored image");
        assert_eq!(path.extension().and_then(std::ffi::OsStr::to_str), Some("png"));
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(path).expect("open pasted image"),
        ));
        let mut reader = decoder.read_info().expect("png info");
        let mut pixels = vec![0; reader.output_buffer_size().expect("png size")];
        reader.next_frame(&mut pixels).expect("png frame");
        assert_eq!(pixels, [255, 0, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn every_paste_stays_available_until_the_store_is_full() {
        let ctx = egui::Context::default();
        let images = ClipboardImages::new();
        for _ in 0..3 {
            images.persist(&ctx, PanelId(1), image());
        }
        let finished = wait_for_paste(&images);
        for paste in &finished {
            assert!(paste.path.as_ref().is_some_and(|path| path.exists()));
        }
        assert!(images.take_finished().is_empty(), "each paste is reported once");
    }

    #[test]
    fn an_oversized_image_leaves_no_file_and_no_path() {
        let ctx = egui::Context::default();
        let images = ClipboardImages::new();
        let mut oversized = ColorImage::filled([1, 1], egui::Color32::RED);
        oversized.size = [usize::MAX, 2];
        images.persist(&ctx, PanelId(1), std::sync::Arc::new(oversized));

        let finished = wait_for_paste(&images);
        assert_eq!(finished.len(), 1);
        assert!(finished[0].path.is_none());
    }
}
