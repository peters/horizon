//! A live preview belongs to the person who watches it: a desktop that no Device panel
//! shows any more keeps no container.
use super::{Run, joined};

impl Run {
    /// Stops a live preview that no panel shows any more, and keeps it stopped: a
    /// desktop nobody watches holds a container, ports and GPU memory for nothing. The
    /// run keeps its files, its proof and its card, and the same desktop starts again
    /// only when the agent names a new one.
    /// The agent named no desktop preview, or Horizon showed the one it named last, or
    /// skipped it because this computer lacks the hardware it needs: `true` does not
    /// always mean a live desktop.
    pub(super) fn preview_shown_or_none(&self) -> bool {
        !self.status.preview_invalid
            && (self.status.preview.is_none()
                || (self.previewed_request == self.status.preview_request
                    && self.previewed_spec == self.status.preview
                    && self.preview.as_ref().is_some_and(|preview| {
                        matches!(
                            preview.outcome,
                            super::preview::Outcome::Live { .. } | super::preview::Outcome::Unavailable(_)
                        )
                    })))
    }

    pub(in crate::app) fn stop_unwatched_preview(&mut self) {
        if let Some(mut preview) = self.preview.take() {
            let stopping = joined(self.preview_stopping.take(), Some(preview.stop()));
            self.preview_stopping = stopping;
        }
        self.preview_panel = None;
        self.preview_shown = None;
        // Even a desktop that already ended is served: the next frame must not start it
        // again, so only a newer request or a changed spec brings one back.
        self.previewed_request = self.status.preview_request;
        self.previewed_spec = self.status.preview.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PanelId, PanelKind, Run, parse};
    use crate::app::cloud_panel::production::repository_setup::Mode;
    use std::path::PathBuf;

    #[test]
    fn a_preview_nobody_watches_stays_stopped() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status = parse(r#"{"steps":[],"preview":{"command":"./app"},"preview_request":3}"#).unwrap();
        run.preview_panel = Some(PanelId(7));
        run.preview_shown = Some("127.0.0.1:32768".to_owned());
        run.previewed_request = 3;
        run.previewed_spec = run.status.preview.clone();

        run.stop_unwatched_preview();

        assert!(run.preview_panel.is_none(), "the panel goes with the desktop");
        assert!(run.preview_shown.is_none(), "nothing shows this desktop");
        assert_eq!(
            run.previewed_request, run.status.preview_request,
            "the same request never starts the desktop again"
        );
        assert_eq!(run.previewed_spec, run.status.preview, "the same spec is served");

        // A newer request does want it: the agent asked for a fresh desktop.
        run.status = parse(r#"{"steps":[],"preview":{"command":"./app"},"preview_request":4}"#).unwrap();
        assert!(
            run.previewed_request < run.status.preview_request,
            "a newer request is a new desktop"
        );
    }

    #[test]
    fn a_preview_that_never_started_owes_no_panel() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.preview_panel = Some(PanelId(7));
        run.stop_unwatched_preview();
        assert!(run.preview_panel.is_none(), "the panel id goes either way");
    }
}
