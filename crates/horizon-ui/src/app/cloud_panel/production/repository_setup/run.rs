//! A setup agent that works in a hidden panel while the New cloud dialog shows its
//! progress. The agent keeps a small status file current; the person reads the steps
//! here and steers the agent by typing into its terminal from the dialog.
use super::Mode;
use horizon_core::{PanelId, PanelKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

mod view;
pub(super) use view::{readable_rows, render};

/// How often the agent's screen is sampled for the output well.
pub(super) const OUTPUT_EVERY: Duration = Duration::from_millis(400);
/// Rows of the agent's screen the output well keeps.
pub(super) const OUTPUT_ROWS: usize = 60;

/// The steps the prompt asks the agent to report, in order. They show as pending
/// before the agent writes its first status.
pub(super) const STEPS: [&str; 7] = [
    "Read CI",
    "Find tools",
    "Write Dockerfile",
    "Build image",
    "Run CI in image",
    "Desktop preview",
    "Fast rerun",
];

/// Enter follows a pasted message after this pause, so the terminal app takes the
/// text as one message and the key as sending it.
pub(super) const SUBMIT_DELAY: Duration = Duration::from_millis(150);

pub(in crate::app::cloud_panel::production) struct Run {
    pub(super) panel: PanelId,
    pub(super) kind: PanelKind,
    pub(super) repository: PathBuf,
    pub(super) status_path: PathBuf,
    started: Instant,
    status: Status,
    modified: Option<SystemTime>,
    steer: String,
    /// When Enter goes to the terminal after a pasted message.
    pub(super) submit_at: Option<Instant>,
    /// The panel closed before the agent reported a summary.
    pub(super) stopped: bool,
    /// When each step was first seen running and when it ended, by title.
    times: HashMap<String, (Instant, Option<Instant>)>,
    /// The agent's screen as last sampled, and when.
    pub(super) output: Vec<String>,
    pub(super) sampled: Option<Instant>,
    /// Where the setup started from; it names the heading and the commit.
    pub(super) mode: Mode,
    /// The `.horizon` files changed or added, once the agent is done.
    pub(super) files: Option<String>,
}

/// What the person did in the progress view.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Action {
    Steer(String),
    Show,
    Stop,
    Use,
}

#[derive(Default, Debug, PartialEq)]
struct Status {
    steps: Vec<Step>,
    question: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Step {
    title: String,
    state: State,
    detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
}

impl Run {
    pub(super) fn new(panel: PanelId, kind: PanelKind, repository: PathBuf, status_path: PathBuf, mode: Mode) -> Self {
        Self {
            panel,
            kind,
            repository,
            status_path,
            started: Instant::now(),
            status: Status::default(),
            modified: None,
            steer: String::new(),
            submit_at: None,
            stopped: false,
            times: HashMap::from([(STEPS[0].to_owned(), (Instant::now(), None))]),
            output: Vec::new(),
            sampled: None,
            mode,
            files: None,
        }
    }

    /// Notes when steps start and end. A step reported done without being seen
    /// running starts where the previous step ended.
    fn time_steps(&mut self) {
        let now = Instant::now();
        let mut previous_end = self.started;
        for step in &self.status.steps {
            let entry = self.times.entry(step.title.clone());
            match step.state {
                State::Pending => {}
                State::Running => {
                    entry.or_insert((now, None));
                }
                State::Done | State::Skipped | State::Failed => {
                    let times = entry.or_insert((previous_end.min(now), None));
                    times.1.get_or_insert(now);
                }
            }
            if let Some((_, Some(end))) = self.times.get(&step.title) {
                previous_end = *end;
            }
        }
    }

    /// How long a step has taken so far, or took.
    fn elapsed(&self, title: &str) -> Option<Duration> {
        let (start, end) = self.times.get(title)?;
        Some(end.unwrap_or_else(Instant::now).saturating_duration_since(*start))
    }

    /// Rereads the status file when it changed. A file the agent is still writing, or
    /// one that does not parse, keeps the last good status.
    pub(super) fn poll(&mut self) {
        let Ok(modified) = std::fs::metadata(&self.status_path).and_then(|meta| meta.modified()) else {
            return;
        };
        if self.modified == Some(modified) {
            return;
        }
        if let Some(status) = std::fs::read_to_string(&self.status_path)
            .ok()
            .and_then(|text| parse(&text))
        {
            self.status = status;
            self.modified = Some(modified);
            self.time_steps();
        }
    }

    pub(super) fn finished(&self) -> bool {
        self.status.summary.is_some()
    }

    /// The steps to draw: the agent's own, or the expected ones before its first report.
    fn steps(&self) -> Vec<Step> {
        if !self.status.steps.is_empty() {
            return self.status.steps.iter().map(Step::clone_view).collect();
        }
        STEPS
            .iter()
            .enumerate()
            .map(|(index, title)| Step {
                title: (*title).to_owned(),
                state: if index == 0 { State::Running } else { State::Pending },
                detail: String::new(),
            })
            .collect()
    }
}

impl Step {
    fn clone_view(&self) -> Self {
        Self {
            title: self.title.clone(),
            state: self.state,
            detail: self.detail.clone(),
        }
    }
}

fn parse(text: &str) -> Option<Status> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let line = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let steps = value
        .get("steps")?
        .as_array()?
        .iter()
        .filter_map(|step| {
            let title = step.get("title")?.as_str()?.trim().to_owned();
            let state = match step.get("state")?.as_str()? {
                "running" => State::Running,
                "done" => State::Done,
                "skipped" => State::Skipped,
                "failed" => State::Failed,
                _ => State::Pending,
            };
            let detail = step
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            Some(Step { title, state, detail })
        })
        .collect();
    Some(Status {
        steps,
        question: line("question"),
        summary: line("summary"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_file_reads_steps_question_and_summary() {
        let status = parse(
            r#"{"steps":[{"title":"Read CI","state":"done","detail":"dotnet-core.yml"},
                {"title":"Build image","state":"running"}],"question":" ","summary":"1,463 tests passed"}"#,
        )
        .unwrap();
        assert_eq!(status.steps.len(), 2);
        assert_eq!(status.steps[0].state, State::Done);
        assert_eq!(status.steps[0].detail, "dotnet-core.yml");
        assert_eq!(status.steps[1].state, State::Running);
        assert_eq!(status.question, None, "a blank question is no question");
        assert_eq!(status.summary.as_deref(), Some("1,463 tests passed"));
    }

    #[test]
    fn a_half_written_status_file_keeps_the_last_good_status() {
        assert_eq!(parse(r#"{"steps":[{"title":"Read"#), None);
        assert_eq!(parse(r#"{"question":"which feed?"}"#), None, "steps are required");
    }

    #[test]
    fn the_output_well_drops_blank_rows_and_rules() {
        let rows = readable_rows("● Reading CI\n\n────────────\n  dotnet-core.yml   \n│ │\n");
        assert_eq!(rows, ["● Reading CI", "  dotnet-core.yml"]);
    }

    #[test]
    fn a_step_reported_done_without_running_starts_where_the_last_one_ended() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Claude,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status =
            parse(r#"{"steps":[{"title":"Read CI","state":"done"},{"title":"Find tools","state":"done"}]}"#).unwrap();
        run.time_steps();
        assert!(run.elapsed("Read CI").is_some());
        let (start, end) = run.times["Find tools"];
        assert_eq!(Some(start), run.times["Read CI"].1, "it starts where Read CI ended");
        assert!(end.is_some());
        assert_eq!(run.elapsed("Write Dockerfile"), None, "pending steps have no time");
    }

    #[test]
    fn before_the_first_report_the_expected_steps_show_with_the_first_running() {
        let run = Run::new(
            PanelId(1),
            PanelKind::Claude,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        let steps = run.steps();
        assert_eq!(steps.len(), STEPS.len());
        assert_eq!(steps[0].state, State::Running);
        assert!(steps[1..].iter().all(|step| step.state == State::Pending));
    }
}
