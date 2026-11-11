//! The start of a Device video recording, on the coordination worker.
//!
//! The start takes the manifest lock of the capture, which waits up to 2 s for
//! another writer, and it makes the folder of the video. A frame records the
//! request and the coordination worker starts the encoder; the panel reports
//! the recording state from the handle, as it does after a start on the frame.

use horizon_core::PanelId;

use super::HorizonApp;

impl HorizonApp {
    /// Queues the start of every recording whose control asked for it.
    pub(in crate::app) fn queue_device_video_starts(&mut self) {
        let starts: Vec<(PanelId, _)> = self
            .panel_render_caches
            .device_ui_state
            .iter_mut()
            .filter_map(|(id, state)| state.take_video_start().map(|start| (*id, start)))
            .collect();
        for (id, start) in starts {
            self.browser_create_host.io.then(
                move || crate::device_widget::recording::start_video(start),
                move |app, started| {
                    let result = started.unwrap_or_else(|| Err("the start of the video did not finish".to_owned()));
                    if let Some(state) = app.panel_render_caches.device_ui_state.get_mut(&id) {
                        state.finish_video_start(result);
                    }
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use horizon_core::browser::manifest::device::{Operation, VideoAction};

    use crate::app::device_requests::tests::app_with_a_viewer;

    #[test]
    fn a_recording_starts_on_the_coordination_worker_and_the_frame_does_not_wait() {
        let (_temp, _ctx, mut app, panel_id) = app_with_a_viewer();

        // The click of the person records a request; no frame starts an encoder.
        let started = Instant::now();
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            let state = app
                .panel_render_caches
                .device_ui_state
                .get_mut(&panel_id)
                .expect("the state of the viewer");
            assert!(
                matches!(
                    state.video(VideoAction::Start),
                    horizon_core::browser::manifest::device::Outcome::Failed { .. }
                ),
                "the click answers before the encoder exists"
            );
            app.queue_device_video_starts();
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the frame that starts a recording does not wait for its manifest lock"
        );

        app.settle_browser_host_io();
        let state = app
            .panel_render_caches
            .device_ui_state
            .get_mut(&panel_id)
            .expect("the state of the viewer");
        let status = match state.video(VideoAction::Status) {
            horizon_core::browser::manifest::device::Outcome::Video { recording } => recording,
            other => panic!("the worker started no recording: {other:?}"),
        };
        assert!(
            std::path::Path::new(&status.capture.path).exists(),
            "the worker made the folder of the video"
        );
        let _ = Operation::Video {
            panel_id: String::new(),
            action: VideoAction::Stop,
        };
    }
}
