//! A setup run as one row of the setup board.
use super::{Run, STEPS, State};
use crate::app::cloud_panel::production::repository_setup::proof;
use horizon_core::setup_board::{Row, RowState, Step};

impl Run {
    /// The run as the setup board shows it: its steps, its state and what it does now.
    pub(in crate::app::cloud_panel::production::repository_setup) fn board_row(&self) -> Row {
        let steps = self.steps();
        let waiting = self.proof.as_ref().is_some_and(|proof| !proof.waiting.is_empty());
        let segments = steps
            .iter()
            .take(STEPS.len())
            .map(|step| match step.state {
                State::Pending if waiting && step.title == proof::BUILD => Step::Waiting,
                State::Pending => Step::Pending,
                State::Running => Step::Running,
                State::Done => Step::Done,
                State::Skipped => Step::Skipped,
                State::Failed => Step::Failed,
            })
            .collect();
        let failed = steps.iter().find(|step| step.state == State::Failed);
        let state = if self.finished() {
            RowState::Ready
        } else if self.stopped {
            RowState::Stopped
        } else if failed.is_some() {
            RowState::Failed
        } else if waiting {
            RowState::Waiting
        } else {
            RowState::Working
        };
        let line = |title: &str, detail: &str| {
            if detail.is_empty() {
                title.to_owned()
            } else {
                format!("{title}: {detail}")
            }
        };
        let committing = self.ending();
        // A notice can run over several lines, such as Git's own words; the row shows the first.
        let notice = self
            .notice
            .as_deref()
            .and_then(|notice| notice.lines().map(str::trim).find(|line| !line.is_empty()));
        let now = if committing {
            "Use this setup commits the setup".to_owned()
        } else if let Some(notice) = notice {
            notice.to_owned()
        } else {
            match state {
                RowState::Ready => self.status.summary.as_deref().map_or_else(
                    || "Ready: Use this setup in New cloud".to_owned(),
                    |summary| format!("Ready: {summary}"),
                ),
                RowState::Stopped => "Stopped: the agent's panel closed before the setup was ready".to_owned(),
                RowState::Failed => {
                    failed.map_or_else(String::new, |step| format!("{} failed: {}", step.title, step.detail))
                }
                RowState::Waiting => steps
                    .iter()
                    .find(|step| step.title == proof::BUILD)
                    .map_or_else(String::new, |step| step.detail.clone()),
                RowState::Working => steps
                    .iter()
                    .find(|step| step.state == State::Running)
                    .or_else(|| steps.iter().rev().find(|step| step.state == State::Done))
                    .map_or_else(|| "Starting".to_owned(), |step| line(&step.title, &step.detail)),
            }
        };
        Row {
            run: self.panel,
            name: self.repository.file_name().map_or_else(
                || self.repository.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: if self.entered.is_empty() {
                self.repository.display().to_string()
            } else {
                self.entered.clone()
            },
            agent: self.agent.clone(),
            steps: segments,
            now,
            state,
            committing,
            alert: !committing && notice.is_some(),
            agent_open: !self.agent_closed,
        }
    }
}

// The proof's test doubles are built where the setup agent is offered: on Linux.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::super::{Mode, Run};
    use super::*;
    use crate::app::cloud_panel::production::repository_setup::proof::Proof;
    use horizon_core::{PanelId, PanelKind};

    fn run() -> Run {
        let mut run = Run::new(
            PanelId(7),
            PanelKind::Usage,
            std::path::PathBuf::from("/code/web-api"),
            std::path::PathBuf::from("missing.json"),
            Mode::New,
        );
        run.entered = "~/code/web-api".into();
        run
    }

    #[test]
    fn a_new_run_reads_its_ci_first() {
        let row = run().board_row();
        assert_eq!(row.run, PanelId(7));
        assert_eq!((row.name.as_str(), row.path.as_str()), ("web-api", "~/code/web-api"));
        assert_eq!(row.state, RowState::Working);
        assert_eq!(row.now, "Read CI");
        assert_eq!(row.steps.len(), STEPS.len());
        assert_eq!(row.steps[0], Step::Running);
        assert!(row.steps[1..].iter().all(|step| *step == Step::Pending));
    }

    #[test]
    fn a_run_whose_proof_waits_shows_who_builds() {
        let mut run = run();
        run.proof = Some(Proof::waiting_on(&["mobile-app"]));
        let row = run.board_row();
        assert_eq!(row.state, RowState::Waiting);
        assert_eq!(row.now, "Waiting for a build slot; mobile-app builds");
        let build = STEPS.iter().position(|step| *step == proof::BUILD).unwrap();
        assert_eq!(row.steps[build], Step::Waiting);
    }

    #[test]
    fn a_notice_and_a_closed_agent_show_on_the_row() {
        let mut run = run();
        run.notice =
            Some("\nCould not commit .horizon: Author identity unknown\n\n*** Please tell me who you are.".into());
        let row = run.board_row();
        assert_eq!(
            row.now, "Could not commit .horizon: Author identity unknown",
            "the first line only"
        );
        assert!(row.alert);
        run.notice = None;
        run.stopped = true;
        run.agent_closed = true;
        let row = run.board_row();
        assert_eq!(row.state, RowState::Stopped);
        assert!(!row.alert && !row.agent_open, "a closed agent cannot be shown");
        assert!(row.now.starts_with("Stopped"), "{}", row.now);
    }
}
