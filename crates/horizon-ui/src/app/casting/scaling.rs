use std::{
    io,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

struct Input {
    image: Arc<egui::ColorImage>,
    rect: egui::Rect,
    pixels_per_point: f32,
}
pub(super) struct ScaledFrame {
    pub(super) rect: egui::Rect,
    pub(super) rgba: Vec<u8>,
}
pub(super) struct Scaler {
    input: Option<SyncSender<Input>>,
    output: Receiver<ScaledFrame>,
    worker: Option<JoinHandle<()>>,
}
impl Scaler {
    pub(super) fn new(dimensions: (usize, usize), repaint: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Input>(1);
        let (scaled, output) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().name("cast-scale".into()).spawn(move || {
            while let Ok(input) = receive.recv() {
                let crop = input.image.region(&input.rect, Some(input.pixels_per_point));
                if crop.width() == 0 || crop.height() == 0 {
                    continue;
                }
                let frame = ScaledFrame {
                    rect: input.rect,
                    rgba: letterbox(&crop, dimensions),
                };
                match scaled.try_send(frame) {
                    Ok(()) => repaint(),
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                    Err(mpsc::TrySendError::Full(_)) => {}
                }
            }
        })?;
        Ok(Self {
            input: Some(send),
            output,
            worker: Some(worker),
        })
    }
    pub(super) fn submit(&self, image: Arc<egui::ColorImage>, rect: egui::Rect, pixels_per_point: f32) {
        if let Some(input) = &self.input {
            let _ = input.try_send(Input {
                image,
                rect,
                pixels_per_point,
            });
        }
    }
    pub(super) fn take(&self) -> Option<ScaledFrame> {
        self.output.try_recv().ok()
    }
}
impl Drop for Scaler {
    fn drop(&mut self) {
        self.input = None;
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}

pub(super) fn letterbox(image: &egui::ColorImage, (width, height): (usize, usize)) -> Vec<u8> {
    let mut rgba = [0, 0, 0, 255].repeat(width * height);
    let (fit_width, fit_height) = if image.width() * height > image.height() * width {
        (width, image.height() * width / image.width())
    } else {
        (image.width() * height / image.height(), height)
    };
    if fit_width == 0 || fit_height == 0 {
        return rgba;
    }
    let left = (width - fit_width) / 2;
    let top = (height - fit_height) / 2;
    let source_x: Vec<_> = (0..fit_width).map(|x| x * image.width() / fit_width).collect();
    let mut previous_source_y = None;
    for y in 0..fit_height {
        let source_y = y * image.height() / fit_height;
        let at = ((y + top) * width + left) * 4;
        let end = at + fit_width * 4;
        if previous_source_y == Some(source_y) {
            rgba.copy_within(at - width * 4..end - width * 4, at);
        } else {
            let row = &image.pixels[source_y * image.width()..(source_y + 1) * image.width()];
            for (pixel, source_x) in rgba[at..end].as_chunks_mut::<4>().0.iter_mut().zip(&source_x) {
                *pixel = row[*source_x].to_array();
            }
            previous_source_y = Some(source_y);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_crop_preserves_region_and_letterbox_dimensions() {
        let scaler = Scaler::new((8, 8), || {}).expect("scaler");
        let rect = egui::Rect::from_min_size(egui::pos2(2.0, 1.0), egui::vec2(2.0, 1.0));
        let mut image = egui::ColorImage::filled([6, 4], egui::Color32::BLUE);
        image[(2, 1)] = egui::Color32::RED;
        image[(3, 1)] = egui::Color32::GREEN;
        scaler.submit(Arc::new(image), rect, 1.0);
        let frame = scaler
            .output
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("scaled frame");
        assert_eq!(frame.rect, rect);
        assert_eq!(frame.rgba.len(), 8 * 8 * 4);
        assert_eq!(&frame.rgba[..8 * 2 * 4], &[0, 0, 0, 255].repeat(16));
        assert_eq!(&frame.rgba[8 * 2 * 4..8 * 2 * 4 + 4], &egui::Color32::RED.to_array());
        assert_eq!(
            &frame.rgba[8 * 2 * 4 + 4 * 4..8 * 2 * 4 + 5 * 4],
            &egui::Color32::GREEN.to_array()
        );
    }
    #[test]
    fn dropping_scaler_disconnects_its_idle_worker() {
        let mut scaler = Scaler::new((8, 8), || {}).expect("scaler");
        let worker = scaler.worker.take().expect("worker");
        drop(scaler);
        worker.join().expect("worker finished");
    }

    #[test]
    fn completed_scaling_wakes_the_host_with_a_frame_ready() {
        let (wake, notified) = mpsc::channel();
        let scaler = Scaler::new((8, 8), move || {
            let _ = wake.send(());
        })
        .expect("scaler");
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2.0, 2.0));
        scaler.submit(
            Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::RED)),
            rect,
            1.0,
        );
        notified
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("host wake");
        let frame = scaler.take().expect("frame available at wake");
        assert_eq!(frame.rect, rect);
        assert_eq!(frame.rgba, egui::Color32::RED.to_array().repeat(64));
    }
}
