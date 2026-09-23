//! Completed-pass context for diagnosing host exclusion without changing the view.
use horizon_core::browser::manifest::device::{HostCanvas, HostExclusion, HostPresentation, HostViewport};

#[derive(Default)]
pub(crate) struct HostState {
    observed_this_frame: bool,
    observation: Option<HostPresentation>,
    last_view: Option<(HostViewport, HostCanvas)>,
    revealed_view: Option<(HostViewport, HostCanvas)>,
    revision: u64,
    requests: u64,
    applied_request: u64,
}

impl HostState {
    pub(crate) fn begin_frame(&mut self) {
        self.observed_this_frame = false;
    }

    pub(crate) fn observed_this_frame(&self) -> bool {
        self.observed_this_frame
    }

    pub(crate) fn requested(&mut self) {
        self.requests = self.requests.saturating_add(1);
    }

    pub(crate) fn applied(&mut self, viewport: HostViewport, canvas: HostCanvas) {
        self.applied_request = self.requests;
        self.revealed_view = Some((viewport, canvas));
    }

    pub(crate) fn record(
        &mut self,
        viewport: HostViewport,
        canvas: Option<HostCanvas>,
        exclusion: Option<HostExclusion>,
    ) {
        let view = canvas.map(|canvas| (viewport, canvas));
        if let Some(view) = view {
            if self.last_view.is_some_and(|previous| previous != view) {
                self.revision = self.revision.saturating_add(1);
            }
            self.last_view = Some(view);
        }
        self.observation = Some(HostPresentation {
            observed_at_millis: horizon_core::browser::manifest::now_millis(),
            viewport,
            exclusion,
            canvas,
            canvas_after_pass: canvas,
            ui_pass: 0,
            discarded: false,
            view_revision: self.revision,
            reveal_requests: self.requests,
            applied_reveal_request: self.applied_request,
            view_changed_since_reveal: self
                .revealed_view
                .zip(view)
                .map(|(revealed, current)| revealed != current),
        });
        self.observed_this_frame = true;
    }

    pub(crate) fn observation(&self) -> Option<HostPresentation> {
        self.observation.clone()
    }

    pub(crate) fn finish(&mut self, canvas: Option<HostCanvas>, rendered: bool, ui_pass: u64, discarded: bool) {
        let Some(observation) = &mut self.observation else {
            return;
        };
        let view = canvas.map(|canvas| (observation.viewport, canvas));
        if let Some(view) = view {
            if self.last_view.is_some_and(|previous| previous != view) {
                self.revision = self.revision.saturating_add(1);
            }
            self.last_view = Some(view);
        }
        observation.observed_at_millis = horizon_core::browser::manifest::now_millis();
        observation.canvas_after_pass = canvas;
        observation.ui_pass = ui_pass;
        observation.discarded = discarded;
        observation.view_revision = self.revision;
        observation.view_changed_since_reveal = self
            .revealed_view
            .zip(view)
            .map(|(revealed, current)| revealed != current);
        if rendered {
            observation.exclusion = None;
        } else if observation.exclusion.is_none() {
            observation.exclusion = Some(HostExclusion::Unclassified);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(x: f32) -> HostCanvas {
        HostCanvas {
            pan_offset: [x, 0.0],
            zoom: 1.0,
            rect: [0.0, 0.0, 800.0, 600.0],
        }
    }

    #[test]
    fn dispatch_is_separate_from_applied_reveal_and_later_camera_movement() {
        let mut state = HostState::default();
        state.requested();
        state.record(
            HostViewport::Root,
            Some(canvas(0.0)),
            Some(HostExclusion::OutsideCanvas),
        );
        let requested = state.observation().unwrap();
        assert_eq!(requested.reveal_requests, 1);
        assert_eq!(requested.applied_reveal_request, 0);
        assert_eq!(requested.view_changed_since_reveal, None);
        state.applied(HostViewport::Root, canvas(100.0));
        state.record(HostViewport::Root, Some(canvas(100.0)), None);
        assert_eq!(state.observation().unwrap().view_changed_since_reveal, Some(false));
        state.begin_frame();
        assert!(!state.observed_this_frame());
        assert_eq!(state.observation().unwrap().applied_reveal_request, 1);
        state.record(
            HostViewport::Root,
            Some(canvas(200.0)),
            Some(HostExclusion::OutsideCanvas),
        );
        let moved = state.observation().unwrap();
        assert_eq!(moved.view_revision, 2);
        assert_eq!(moved.view_changed_since_reveal, Some(true));
    }

    #[test]
    fn unobserved_detached_canvas_does_not_reuse_root_or_stale_geometry() {
        let mut state = HostState::default();
        state.applied(HostViewport::Detached, canvas(0.0));
        state.record(
            HostViewport::Detached,
            None,
            Some(HostExclusion::DetachedViewportNotRendered),
        );
        let observation = state.observation().unwrap();
        assert_eq!(observation.canvas, None);
        assert_eq!(observation.view_changed_since_reveal, None);
        state.record(HostViewport::Root, Some(canvas(0.0)), None);
        assert_eq!(state.observation().unwrap().view_changed_since_reveal, Some(true));
    }

    #[test]
    fn late_navigation_and_discard_keep_render_camera_separate() {
        let mut state = HostState::default();
        state.requested();
        state.applied(HostViewport::Root, canvas(0.0));
        state.record(HostViewport::Root, Some(canvas(0.0)), None);
        state.finish(Some(canvas(100.0)), true, 7, true);
        let first = state.observation().unwrap();
        assert_eq!(first.canvas, Some(canvas(0.0)));
        assert_eq!(first.canvas_after_pass, Some(canvas(100.0)));
        assert_eq!(first.ui_pass, 7);
        assert!(first.discarded);
        assert_eq!(first.view_changed_since_reveal, Some(true));
        state.begin_frame();
        state.record(HostViewport::Root, Some(canvas(100.0)), None);
        state.finish(Some(canvas(0.0)), true, 8, false);
        let second = state.observation().unwrap();
        assert_eq!(second.canvas, Some(canvas(100.0)));
        assert_eq!(second.canvas_after_pass, Some(canvas(0.0)));
        assert!(!second.discarded);
        assert_eq!(second.view_changed_since_reveal, Some(false));
    }

    #[test]
    fn coalesced_requests_and_independent_viewports_do_not_claim_extra_applications() {
        let mut first = HostState::default();
        let mut second = HostState::default();
        first.requested();
        first.requested();
        first.applied(HostViewport::Detached, canvas(10.0));
        first.record(HostViewport::Detached, Some(canvas(10.0)), None);
        second.record(HostViewport::Detached, Some(canvas(20.0)), None);
        assert_eq!(first.observation().unwrap().applied_reveal_request, 2);
        assert_eq!(second.observation().unwrap().applied_reveal_request, 0);
        assert_eq!(second.observation().unwrap().view_changed_since_reveal, None);
        assert_eq!(first.observation().unwrap().canvas, Some(canvas(10.0)));
        assert_eq!(second.observation().unwrap().canvas, Some(canvas(20.0)));
    }
}
