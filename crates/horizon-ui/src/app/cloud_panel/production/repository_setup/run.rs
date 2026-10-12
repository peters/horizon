//! A setup agent that works in a hidden panel while the New cloud dialog shows its
//! progress. The agent keeps a small status file current; the person reads the steps
//! here and steers the agent by typing into its terminal from the dialog.
use super::Mode;
use super::proof::{self, Phase, Proof};
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

/// The largest status file Horizon reads.
const STATUS_LIMIT: u64 = 1024 * 1024;

/// `text` as one pasted message, so a terminal agent takes it whole even mid-turn.
pub(super) fn pasted(text: &str) -> Vec<u8> {
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend(text.replace(['\r', '\n'], " ").as_bytes());
    bytes.extend(b"\x1b[201~");
    bytes
}

/// Enter follows a pasted message after this pause, so the terminal app takes the
/// text as one message and the key as sending it.
pub(super) const SUBMIT_DELAY: Duration = Duration::from_millis(150);

/// Pastes `text` as one message and schedules its Enter. An Enter still waiting for an
/// earlier message goes first, so two messages never arrive as one.
pub(super) fn paste(panel: &mut horizon_core::Panel, submit_at: &mut Option<Instant>, text: &str) {
    if submit_at.take().is_some() {
        panel.write_input(b"\r");
    }
    panel.write_input(&pasted(text));
    *submit_at = Some(Instant::now() + SUBMIT_DELAY);
}

pub(in crate::app::cloud_panel::production) struct Run {
    pub(super) panel: PanelId,
    /// The preset's name, as the dialog shows the agent.
    pub(super) agent: String,
    /// Instructions still to paste, for an agent that takes no starting prompt argument.
    pub(super) pending_prompt: Option<String>,
    pub(super) repository: PathBuf,
    pub(super) status_path: PathBuf,
    pub(super) started: Instant,
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
    /// Horizon's own proof of the image, and the agent's request it answers.
    pub(super) proof: Option<Proof>,
    pub(super) proved_request: u64,
    /// The summary the agent was last told needs a proof Horizon can run.
    pub(super) missing_proof_summary: Option<String>,
    /// The setup changed after its proof passed, and the agent was told once.
    pub(super) proof_stale_reported: bool,
    /// The person allowed restoring private .NET packages on this computer for the proof.
    pub(super) host_restore: bool,
    /// The current fingerprint of the setup and proof request, and when it was read.
    inputs: Option<(Instant, Option<[u8; 32]>)>,
    /// The files outside `.horizon` when the run started, to find what changed since.
    pub(super) outside: super::commit::Outside,
    /// Whether the repository restores .NET packages, read once when the run starts.
    pub(super) dotnet: bool,
    /// Set once the run's proof resources and folder are being removed.
    released: bool,
    /// The agent's process, kept when Use closes its panel, so Use waits until it and
    /// everything it started have ended before it reads and commits the setup.
    pub(super) agent_process: Option<u32>,
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
    /// What the agent asks Horizon to prove, and how often it asked.
    proof: Option<proof::Spec>,
    proof_request: u64,
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
            agent: kind.display_name().to_owned(),
            pending_prompt: None,
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
            proof: None,
            proved_request: 0,
            missing_proof_summary: None,
            proof_stale_reported: false,
            host_restore: false,
            inputs: None,
            outside: super::commit::Outside::default(),
            dotnet: false,
            released: false,
            agent_process: None,
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
    /// A newer proof the agent asked for than the one Horizon ran last.
    pub(super) fn proof_wanted(&self) -> Option<(u64, proof::Spec)> {
        let request = self.status.proof_request;
        (request > self.proved_request)
            .then(|| self.status.proof.clone())
            .flatten()
            .map(|spec| (request, spec))
    }

    /// The step Horizon owns for `title`, once a proof exists.
    fn proof_phase(&self, title: &str) -> Option<&proof::PhaseState> {
        let proof = self.proof.as_ref()?;
        [Phase::Build, Phase::Ci, Phase::Fast]
            .into_iter()
            .find(|phase| phase.title() == title)
            .map(|phase| proof.phase(phase))
    }

    fn elapsed(&self, title: &str) -> Option<Duration> {
        if let Some(phase) = self.proof_phase(title) {
            let start = phase.started?;
            return Some(
                phase
                    .ended
                    .unwrap_or_else(Instant::now)
                    .saturating_duration_since(start),
            );
        }
        let (start, end) = self.times.get(title)?;
        Some(end.unwrap_or_else(Instant::now).saturating_duration_since(*start))
    }

    /// Rereads the status file when it changed. A file the agent is still writing, or
    /// one that does not parse, keeps the last good status.
    pub(super) fn poll(&mut self) {
        self.refresh_inputs();
        let Ok(modified) = std::fs::metadata(&self.status_path).and_then(|meta| meta.modified()) else {
            return;
        };
        if self.modified == Some(modified) {
            return;
        }
        // The agent writes this file, so it is read as untrusted: no link, no pipe, small.
        if let Some(status) = super::untrusted::read_regular(&self.status_path, STATUS_LIMIT)
            .ok()
            .and_then(|bytes| parse(&String::from_utf8_lossy(&bytes)))
        {
            self.status = status;
            self.modified = Some(modified);
            self.time_steps();
        }
    }

    /// The agent wrote its summary and Horizon proved this setup: the proof it asked for
    /// last ran, passed, and tested the files as they are now. An agent that cannot wait
    /// may write a summary first.
    pub(super) fn finished(&self) -> bool {
        self.status.summary.is_some() && self.passed() && !self.changed_since_proof() && self.agent_steps_done()
    }

    /// Every step the agent owns is reported done or skipped, and no step failed; the
    /// proof's own steps are Horizon's to judge.
    fn agent_steps_done(&self) -> bool {
        let horizons = [proof::BUILD, proof::CI, proof::FAST];
        let ended = |title: &str| {
            self.status
                .steps
                .iter()
                .any(|step| step.title == title && matches!(step.state, State::Done | State::Skipped))
        };
        !self
            .status
            .steps
            .iter()
            .any(|step| step.state == State::Failed && !horizons.contains(&step.title.as_str()))
            && STEPS
                .iter()
                .filter(|title| !horizons.contains(title))
                .all(|title| ended(title))
    }

    /// The last proof the agent asked for ran and passed: its current request is valid
    /// and is the one Horizon ran, so removing the request does not keep an old pass.
    fn passed(&self) -> bool {
        self.status.proof.is_some()
            && self.status.proof_request == self.proved_request
            && self.proof.as_ref().is_some_and(|proof| {
                proof
                    .phases
                    .iter()
                    .all(|(_, state)| state.ended.is_some() && state.failure.is_none())
            })
    }

    /// The `.horizon` files or the proof request changed after the proof that passed.
    pub(super) fn changed_since_proof(&self) -> bool {
        match (&self.proof, self.inputs) {
            (Some(proof), Some((_, now))) => now != Some(proof.inputs),
            _ => false,
        }
    }

    /// The agent wrote its summary over a proof that no longer matches the setup.
    pub(super) fn summary_over_stale_proof(&self) -> bool {
        self.status.summary.is_some() && self.passed() && self.changed_since_proof()
    }

    /// Reads the setup's fingerprint again, at most once a second.
    fn refresh_inputs(&mut self) {
        if self.inputs.is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.refresh_inputs_now();
    }

    /// Reads the setup's fingerprint again now, as Use does before it commits.
    pub(super) fn refresh_inputs_now(&mut self) {
        if let Some(spec) = self.status.proof.as_ref() {
            self.inputs = Some((
                Instant::now(),
                super::snapshot::fingerprint(&self.repository, spec).ok(),
            ));
        }
    }

    /// The agent wrote its summary without a proof request Horizon can run, and was not
    /// told yet for this summary; a new summary without one is told again. An earlier
    /// proof does not count once the status no longer asks for it.
    pub(super) fn take_summary_without_proof(&mut self) -> bool {
        let ran_current =
            self.proof.is_some() && self.status.proof.is_some() && self.status.proof_request == self.proved_request;
        let without = self.status.summary.is_some() && self.proof_wanted().is_none() && !ran_current;
        if !without || self.missing_proof_summary == self.status.summary {
            return false;
        }
        self.missing_proof_summary.clone_from(&self.status.summary);
        true
    }

    /// The proof the agent asks for now.
    pub(super) fn proof_spec(&self) -> Option<&proof::Spec> {
        self.status.proof.as_ref()
    }

    /// Stops the proof of a run that ended unfinished and removes its containers, image
    /// and volume in the background. The run's folder stays for its view until Stop.
    pub(super) fn abandon_proof(&mut self) {
        if let Some(proof) = self.proof.as_mut() {
            let worker = proof.stop();
            proof::remove_run_resources(&self.repository, &proof::run_id(&self.status_path), worker, false);
        }
    }

    /// Ends the run in the background: its proof stops, then the proof's containers,
    /// image and volume and the run's folder go.
    pub(super) fn discard(mut self) {
        self.release(false);
    }

    fn release(&mut self, wait: bool) {
        if std::mem::replace(&mut self.released, true) {
            return;
        }
        if let Some(proof) = self.proof.as_mut() {
            let worker = proof.stop();
            proof::remove_run_resources(&self.repository, &proof::run_id(&self.status_path), worker, wait);
        }
        remove_run_folder(&self.status_path);
    }

    /// The steps to draw: always the whole expected sequence, each with what the agent
    /// last reported for it (pending until then; the first running before any report)
    /// and Horizon's own state for the proof steps. Rows the agent added follow.
    fn steps(&self) -> Vec<Step> {
        let reported = &self.status.steps;
        let mut steps: Vec<Step> = STEPS
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let step = reported.iter().find(|step| step.title == *title).map_or_else(
                    || Step {
                        title: (*title).to_owned(),
                        state: if index == 0 && reported.is_empty() {
                            State::Running
                        } else {
                            State::Pending
                        },
                        detail: String::new(),
                    },
                    Step::clone_view,
                );
                self.proof_step(&step).unwrap_or(step)
            })
            .collect();
        steps.extend(
            reported
                .iter()
                .filter(|step| !STEPS.contains(&step.title.as_str()))
                .map(Step::clone_view),
        );
        steps
    }
}

impl Run {
    /// Horizon's own state of a step it owns, in place of what the agent wrote.
    fn proof_step(&self, step: &Step) -> Option<Step> {
        let phase = self.proof_phase(&step.title)?;
        let (state, detail) = match (phase.started, phase.ended, phase.failure.as_deref()) {
            (_, Some(_), Some(failure)) => (State::Failed, failure.to_owned()),
            (_, Some(_), None) => (State::Done, "proved by Horizon".to_owned()),
            (Some(_), None, _) => (State::Running, "Horizon is running it".to_owned()),
            (None, _, _) => (State::Pending, String::new()),
        };
        Some(Step {
            title: step.title.clone(),
            state,
            detail,
        })
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

/// A run dropped without Stop or Use, as when Horizon exits, still leaves nothing
/// behind: Horizon waits for its proof to stop and removes what it made.
impl Drop for Run {
    fn drop(&mut self) {
        self.release(true);
    }
}

/// Removes a finished run's folder: its status, proof logs and screenshots.
pub(super) fn remove_run_folder(status: &std::path::Path) {
    if let Some(folder) = status.parent().filter(|folder| {
        folder
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("horizon-cloud-setup-"))
    }) {
        let _ = std::fs::remove_dir_all(folder);
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
        proof: value.get("proof").and_then(proof::Spec::parse),
        proof_request: value
            .get("proof_request")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
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
    fn a_failed_or_missing_agent_step_keeps_the_run_from_being_ready() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        let steps = |preview: &str| {
            format!(
                r#"{{"steps":[{{"title":"Read CI","state":"done"}},{{"title":"Find tools","state":"done"}},{{"title":"Write Dockerfile","state":"done"}},{{"title":"Build image","state":"running"}},{{"title":"Desktop preview","state":"{preview}"}}]}}"#
            )
        };
        run.status = parse(&steps("skipped")).unwrap();
        assert!(run.agent_steps_done(), "Horizon's own steps are not the agent's to end");
        run.status = parse(&steps("failed")).unwrap();
        assert!(!run.agent_steps_done(), "a failed step is not a finished setup");
        run.status = parse(r#"{"steps":[{"title":"Read CI","state":"done"}]}"#).unwrap();
        assert!(!run.agent_steps_done(), "nor is a missing one");
    }

    #[test]
    fn the_whole_sequence_shows_whatever_the_agent_reported() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status =
            parse(r#"{"steps":[{"title":"Read CI","state":"running"},{"title":"Extra","state":"done"}]}"#).unwrap();
        let titles: Vec<String> = run.steps().into_iter().map(|step| step.title).collect();
        let mut expected: Vec<String> = STEPS.iter().map(|title| (*title).to_owned()).collect();
        expected.push("Extra".to_owned());
        assert_eq!(
            titles, expected,
            "the rows the agent left out still show, its own extra follows"
        );
        assert!(
            run.steps()[1..STEPS.len()]
                .iter()
                .all(|step| step.state == State::Pending)
        );
    }

    #[test]
    fn a_summary_before_the_proof_is_not_ready() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status =
            parse(r#"{"steps":[],"summary":"done","proof":{"workflow":"ci.yml","jobs":["test"]},"proof_request":1}"#)
                .unwrap();
        assert!(!run.finished(), "the proof the agent asked for has not run");
        run.proved_request = 1;
        assert!(!run.finished(), "a summary is not a proof");
        run.proved_request = 0;
        run.status = parse(r#"{"steps":[],"summary":"done"}"#).unwrap();
        assert!(run.take_summary_without_proof(), "the agent is told to ask for a proof");
        assert!(!run.take_summary_without_proof(), "once per summary");
        run.status = parse(r#"{"steps":[],"summary":"done again"}"#).unwrap();
        assert!(
            run.take_summary_without_proof(),
            "a new summary without one is told again"
        );
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
