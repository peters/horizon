//! A live preview belongs to the person who watches it: a desktop that no Device panel
//! shows any more keeps no container. The desktop it did show still counts for the
//! setup, so a closed panel never turns a finished setup back into a waiting one.
use super::{Run, joined, preview, proof};

impl Run {
    /// The agent named no desktop preview, or Horizon showed the one it named last, or
    /// skipped it because this computer lacks the hardware it needs: `true` does not
    /// always mean a live desktop.
    pub(super) fn preview_shown_or_none(&self) -> bool {
        !self.status.preview_invalid
            && (self.status.preview.is_none()
                || (self.previewed_request == self.status.preview_request
                    && self.previewed_spec == self.status.preview
                    && (self.preview_closed
                        || self.preview.as_ref().is_some_and(|preview| {
                            matches!(
                                preview.outcome,
                                super::preview::Outcome::Live { .. } | super::preview::Outcome::Unavailable(_)
                            )
                        }))))
    }

    /// A preview to start: the proof passed, the agent named one, and it is newer than
    /// the last one started or differs from it.
    pub(super) fn preview_wanted(&self) -> Option<(u64, preview::Spec, String, String, proof::Pinned)> {
        let proof = self.proof.as_ref()?;
        // Only the proof of the setup as it is now: never an image a newer request or a
        // changed .horizon left behind, measured after the newest status was taken.
        let current = self.passed() && self.inputs_proved() && self.inputs_read_for_status();
        let request = self.status.preview_request;
        let changed = self.status.preview.is_some() && self.status.preview != self.previewed_spec;
        (current && (request > self.previewed_request || changed))
            .then(|| self.status.preview.clone())
            .flatten()
            .map(|spec| (request, spec, proof.image.clone(), proof.volume.clone(), proof.daemon()))
    }

    /// The agent took its preview back (`preview` is null now) or wrote one Horizon cannot
    /// start: the live one stops and its panel closes. A preview named again starts once
    /// it differs from the last one or has a higher request.
    pub(super) fn stop_withdrawn_preview(&mut self) {
        if self.status.preview.is_none() && self.preview.is_some() {
            if let Some(mut preview) = self.preview.take() {
                let stopping = joined(self.preview_stopping.take(), Some(preview.stop()));
                self.preview_stopping = stopping;
            }
            self.previewed_request = self.status.preview_request;
            self.previewed_spec = None;
            self.preview_closed = false;
        }
    }

    /// Stops a live preview whose Device panel the person closed: a desktop nobody watches
    /// holds a container, ports and GPU memory for nothing. The run keeps its files, its
    /// proof and its card, and counts this desktop as shown, so the setup stays finished
    /// and **Use this setup** still commits it. Only a newer request or a changed spec
    /// starts the next desktop.
    pub(in crate::app) fn stop_unwatched_preview(&mut self) {
        if let Some(mut preview) = self.preview.take() {
            let stopping = joined(self.preview_stopping.take(), Some(preview.stop()));
            self.preview_stopping = stopping;
        }
        self.preview_panel = None;
        self.preview_shown = None;
        self.preview_closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PanelId, PanelKind, Run, parse};
    use crate::app::cloud_panel::production::repository_setup::Mode;
    use crate::app::cloud_panel::production::repository_setup::run::Preview;
    use std::path::PathBuf;

    fn run_with_preview(request: u64) -> Run {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status = parse(&format!(
            r#"{{"steps":[],"preview":{{"command":"./app"}},"preview_request":{request}}}"#
        ))
        .unwrap();
        run.previewed_request = request;
        run.previewed_spec = run.status.preview.clone();
        run
    }

    /// A person who watched the desktop and closed its panel leaves a finished setup
    /// finished: **Use this setup** must still commit it.
    #[test]
    fn a_preview_a_person_closed_still_counts_as_shown() {
        let mut run = run_with_preview(3);
        run.preview = Some(Preview::live(32768));
        run.preview_panel = Some(PanelId(7));
        run.preview_shown = Some("127.0.0.1:32768".to_owned());
        assert!(run.preview_shown_or_none(), "the desktop is live");

        run.stop_unwatched_preview();

        assert!(run.preview.is_none(), "the desktop stops");
        assert!(run.preview_panel.is_none(), "the panel goes with the desktop");
        assert!(run.preview_shown.is_none(), "nothing shows this desktop");
        assert!(
            run.preview_shown_or_none(),
            "the desktop was shown, so the setup does not wait for it again"
        );
    }

    /// A newer request, or a preview the agent changed, is a desktop Horizon has not shown.
    #[test]
    fn a_changed_preview_is_not_served_by_the_closed_one() {
        let mut run = run_with_preview(3);
        run.preview = Some(Preview::live(32768));
        run.stop_unwatched_preview();
        assert!(run.preview_shown_or_none(), "the closed desktop is served");

        run.status = parse(r#"{"steps":[],"preview":{"command":"./other"},"preview_request":3}"#).unwrap();
        assert!(!run.preview_shown_or_none(), "a changed spec is a new desktop");

        run.status = parse(r#"{"steps":[],"preview":{"command":"./other"},"preview_request":4}"#).unwrap();
        assert!(!run.preview_shown_or_none(), "a newer request is a new desktop");
    }

    /// A withdrawn desktop stops, and the closed mark of an earlier one does not stay to
    /// serve the preview the agent names next.
    #[test]
    fn a_withdrawn_preview_clears_the_closed_mark() {
        let mut run = run_with_preview(3);
        run.preview = Some(Preview::live(32768));
        run.stop_unwatched_preview();
        assert!(run.preview_closed, "the person closed this desktop");

        run.preview = Some(Preview::live(32769));
        run.status = parse(r#"{"steps":[],"preview":null,"preview_request":3}"#).unwrap();
        run.stop_withdrawn_preview();

        assert!(!run.preview_closed, "the withdrawn preview is not a closed one");
        assert!(run.preview.is_none(), "the desktop stops");
        assert!(run.preview_shown_or_none(), "no preview is nothing to wait for");
    }

    #[test]
    fn a_preview_that_never_started_owes_no_panel() {
        let mut run = run_with_preview(3);
        run.preview_panel = Some(PanelId(7));
        run.stop_unwatched_preview();
        assert!(run.preview_panel.is_none(), "the panel id goes either way");
    }
}
