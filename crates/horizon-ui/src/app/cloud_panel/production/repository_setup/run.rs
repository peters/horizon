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
pub(super) const STEPS: [&str; 6] = [
    "Read CI",
    "Find tools",
    "Write Dockerfile",
    "Build image",
    "Run CI in image",
    "Fast rerun",
];

/// The largest status file Horizon reads.
const STATUS_LIMIT: u64 = 1024 * 1024;

/// How often the status file's thread looks at it.
const STATUS_EVERY: Duration = Duration::from_millis(250);

/// Reads the agent's status file on a thread whenever it changed and sends each status
/// that parses, waking the dialog; the thread ends once `alive` has no run behind it.
/// The agent writes this file, so it is read as untrusted: no link, no pipe, small.
fn watch_status(path: PathBuf, ctx: egui::Context, alive: std::sync::Weak<()>) -> std::sync::mpsc::Receiver<Status> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen: Option<SystemTime> = None;
        while alive.strong_count() > 0 {
            let modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
            if modified.is_some()
                && modified != seen
                && let Some(status) = super::untrusted::read_regular(&path, STATUS_LIMIT)
                    .ok()
                    .and_then(|bytes| parse(&String::from_utf8_lossy(&bytes)))
            {
                seen = modified;
                if sender.send(status).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
            std::thread::sleep(STATUS_EVERY);
        }
    });
    receiver
}

/// `text` as one pasted message, so a terminal agent takes it whole even mid-turn.
/// Line breaks become spaces and every other control character goes, so text that
/// quotes the agent's own files, such as a proof failure, can never end the paste
/// early with ESC and type into the agent's terminal.
pub(super) fn pasted(text: &str) -> Vec<u8> {
    let clean: String = text
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend(clean.as_bytes());
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
    /// The repository as the person entered it, so the dialog knows it without the disk.
    pub(super) entered: String,
    /// Which folder the repository was when the run started; reads check it.
    pub(super) identity: super::untrusted::Identity,
    pub(super) status_path: PathBuf,
    pub(super) started: Instant,
    status: Status,
    /// The agent's status as a thread reads it from the status file, started on the
    /// first poll; the thread ends once the run is gone.
    status_feed: Option<std::sync::mpsc::Receiver<Status>>,
    watching: std::sync::Arc<()>,
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
    /// The list of changed files being made on a thread.
    pub(super) files_job: Option<std::sync::mpsc::Receiver<String>>,
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
    /// A fingerprint being read off the dialog's thread.
    inputs_job: Option<std::sync::mpsc::Receiver<Option<[u8; 32]>>>,
    /// The proof's containers, image and volume are already being removed.
    proof_released: bool,
    /// The files outside `.horizon` when the run started, to find what changed since.
    pub(super) outside: super::commit::Outside,
    /// Whether the repository restores .NET packages, read once when the run starts.
    pub(super) dotnet: bool,
    /// Set once the run's proof resources and folder are being removed.
    released: bool,
    /// The agent's process until it is known to have ended: Use, Stop and the end of
    /// the run end it, with its process group, before the run's setup is read or let go.
    pub(super) agent_process: Option<super::agent::AgentProcess>,
    /// Use's thread, ending the agent's processes: it holds the agent meanwhile, so the
    /// run waits for it before it lets go.
    pub(super) ending: Option<super::accept::Using>,
    /// Use already said that ignored files past the reading's limit were not compared.
    pub(super) ignored_partial_told: bool,
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
            // Read on the thread that prepares the run, never here.
            identity: None,
            entered: String::new(),
            repository,
            status_path,
            started: Instant::now(),
            status: Status::default(),
            status_feed: None,
            watching: std::sync::Arc::new(()),
            steer: String::new(),
            submit_at: None,
            stopped: false,
            times: HashMap::from([(STEPS[0].to_owned(), (Instant::now(), None))]),
            output: Vec::new(),
            sampled: None,
            mode,
            files: None,
            files_job: None,
            proof: None,
            proved_request: 0,
            missing_proof_summary: None,
            proof_stale_reported: false,
            host_restore: false,
            inputs: None,
            inputs_job: None,
            proof_released: false,
            outside: super::commit::Outside::default(),
            dotnet: false,
            released: false,
            agent_process: None,
            ending: None,
            ignored_partial_told: false,
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

    /// Takes the newest status the status file's thread read. A file the agent is still
    /// writing, or one that does not parse, keeps the last good status.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        self.refresh_inputs(ctx);
        let feed = self.status_feed.get_or_insert_with(|| {
            watch_status(
                self.status_path.clone(),
                ctx.clone(),
                std::sync::Arc::downgrade(&self.watching),
            )
        });
        if let Some(status) = std::iter::from_fn(|| feed.try_recv().ok()).last() {
            self.status = status;
            self.time_steps();
        }
    }

    /// The agent wrote its summary and Horizon proved this setup: the proof it asked for
    /// last ran, passed, and tested the files as they are now. An agent that cannot wait
    /// may write a summary first.
    pub(super) fn finished(&self) -> bool {
        self.status.summary.is_some() && self.passed() && self.inputs_proved() && self.agent_steps_done()
    }

    /// The setup as last read is the one the proof that passed tested.
    fn inputs_proved(&self) -> bool {
        matches!((&self.proof, self.inputs), (Some(proof), Some((_, Some(now))))
            if Some(now) == proof.inputs && proof.host_restore == self.host_restore)
    }

    /// A new proof starts: the fingerprint read for the earlier one says nothing about it.
    pub(super) fn forget_inputs(&mut self) {
        self.inputs = None;
        self.inputs_job = None;
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
            (Some(proof), Some((_, now))) => {
                proof.host_restore != self.host_restore || (proof.inputs.is_some() && now != proof.inputs)
            }
            _ => false,
        }
    }

    /// The agent wrote its summary over a proof that no longer matches the setup.
    pub(super) fn summary_over_stale_proof(&self) -> bool {
        self.status.summary.is_some() && self.passed() && self.changed_since_proof()
    }

    /// Reads the setup's fingerprint again, at most once a second and only while a proof
    /// passed, on a thread of its own: the setup can hold many files, and the dialog
    /// must not wait for them. Use reads it once more itself before it commits.
    fn refresh_inputs(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.inputs_job {
            match job.try_recv() {
                Ok(now) => self.inputs = Some((Instant::now(), now)),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
            self.inputs_job = None;
            return;
        }
        if !self.passed() || self.inputs.is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        let Some(spec) = self.status.proof.clone() else {
            return;
        };
        let repository = self.repository.clone();
        let identity = self.identity;
        let (sender, receiver) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(super::snapshot::fingerprint(&repository, &spec, identity).ok());
            wake.request_repaint();
        });
        self.inputs_job = Some(receiver);
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

    /// Use is waiting for the agent's processes to end.
    pub(super) fn ending(&self) -> bool {
        self.ending.is_some()
    }

    /// The proof the agent asks for now.
    pub(super) fn proof_spec(&self) -> Option<&proof::Spec> {
        self.status.proof.as_ref()
    }

    /// Ends a run whose agent panel closed before it finished, in the background: the
    /// agent's processes end, the proof stops and its containers, image and volume go.
    /// The run's folder stays for its view until Stop.
    pub(super) fn abandon(&mut self) {
        let agent = self
            .agent_process
            .take()
            .map(|agent| std::thread::spawn(move || agent.end_completely()));
        self.release_proof(agent, false);
    }

    /// Stops the proof and removes the run's containers, image and volume once `before`,
    /// the agent ending, and the proof's run have ended, so neither creates one after.
    /// A run without a proof has them removed too, such as the agent's preview
    /// container. Only once: a second, unordered removal could race the first.
    fn release_proof(&mut self, before: Option<std::thread::JoinHandle<()>>, wait: bool) {
        if std::mem::replace(&mut self.proof_released, true) {
            if let Some(before) = before {
                keep_ending(before);
            }
            return;
        }
        let (worker, pinned) = self
            .proof
            .as_mut()
            .map_or((None, None), |proof| (proof.stop(), Some(proof.daemon())));
        let after = match (before, worker) {
            (Some(before), Some(worker)) => Some(std::thread::spawn(move || {
                let _ = before.join();
                let _ = worker.join();
            })),
            (before, worker) => before.or(worker),
        };
        proof::remove_run_resources(&self.repository, &proof::run_id(&self.status_path), after, pinned, wait);
    }

    /// Ends the run in the background: its agent's processes end, its proof stops, then
    /// the proof's containers, image and volume and the run's folder go. A new setup
    /// waits for that, so two setups never run at once.
    pub(super) fn discard(self) {
        // Noted before the thread starts, so an exit that comes first keeps them.
        proof::expect_removal(&self.repository, &proof::run_id(&self.status_path));
        keep_ending(std::thread::spawn(move || drop(self)));
    }

    fn release(&mut self, wait: bool) {
        if std::mem::replace(&mut self.released, true) {
            return;
        }
        // Use may still be ending the agent on its thread: that ends first, and an agent
        // it could not end is ended here.
        if let Some(using) = self.ending.take() {
            let agent = using.wait();
            self.agent_process = self.agent_process.take().or(agent);
        }
        if let Some(agent) = self.agent_process.take() {
            agent.end_completely();
        }
        self.release_proof(None, wait);
        remove_run_folder_now(&self.status_path);
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

/// Runs, agents and proof resources being ended in the background.
static ENDING: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Keeps `ending`, a thread that ends a run, an agent or what a proof made, so a new
/// setup and Horizon's exit wait for it.
pub(in crate::app::cloud_panel::production::repository_setup) fn keep_ending(ending: std::thread::JoinHandle<()>) {
    ENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(ending);
}

/// Waits until every run, agent and proof resource being ended in the background has
/// ended, with what their ending started in turn: a new setup starts only then, and
/// Horizon exits only then. No waiter takes a handle out before its thread has
/// finished, so every waiter sees each running one: a thread leaves the list only once
/// its work has returned, and any ending it started by then is in the list already.
pub(super) fn wait_for_ended_runs() {
    let _ = wait_for_ended_runs_within(None);
}

/// As [`wait_for_ended_runs`], for at most `limit`; whether every one ended in time.
pub(super) fn wait_for_ended_runs_within(limit: Option<Duration>) -> bool {
    let deadline = limit.map(|limit| Instant::now() + limit);
    loop {
        {
            let mut ending = ENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            ending.retain(|run| !run.is_finished());
            if ending.is_empty() {
                return true;
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A run dropped without Stop or Use, as when Horizon exits, still leaves nothing
/// behind: Horizon waits for its proof to stop and removes what it made.
impl Drop for Run {
    fn drop(&mut self) {
        self.release(true);
    }
}

/// Removes a finished run's folder, its status, proof logs and screenshots, on a thread
/// that a new setup and Horizon's exit wait for.
pub(super) fn remove_run_folder(status: &std::path::Path) {
    let status = status.to_path_buf();
    keep_ending(std::thread::spawn(move || remove_run_folder_now(&status)));
}

/// As [`remove_run_folder`], here and now, on a thread that may wait for the disk.
pub(super) fn remove_run_folder_now(status: &std::path::Path) {
    if let Some(folder) = status.parent().filter(|folder| {
        folder
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("horizon-cloud-setup-"))
    }) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// The most steps the dialog takes from the agent's status.
const STATUS_STEPS: usize = 32;

/// `text` trimmed and cut to `limit` characters: the agent writes the status file, so
/// nothing in it is drawn at any length.
fn clip(text: &str, limit: usize) -> String {
    text.trim().chars().take(limit).collect()
}

fn parse(text: &str) -> Option<Status> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let line = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(|text| clip(text, 4000))
            .filter(|text| !text.is_empty())
    };
    let steps = value
        .get("steps")?
        .as_array()?
        .iter()
        .take(STATUS_STEPS)
        .filter_map(|step| {
            let title = clip(step.get("title")?.as_str()?, 120);
            let state = match step.get("state")?.as_str()? {
                "running" => State::Running,
                "done" => State::Done,
                "skipped" => State::Skipped,
                "failed" => State::Failed,
                _ => State::Pending,
            };
            let detail = clip(
                step.get("detail")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                600,
            );
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
    fn every_waiter_waits_for_an_ending_and_what_it_started() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let nested_done = std::sync::Arc::new(AtomicBool::new(false));
        let done = nested_done.clone();
        keep_ending(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            keep_ending(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                done.store(true, Ordering::SeqCst);
            }));
        }));
        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let done = nested_done.clone();
                std::thread::spawn(move || {
                    wait_for_ended_runs();
                    done.load(Ordering::SeqCst)
                })
            })
            .collect();
        for waiter in waiters {
            assert!(
                waiter.join().unwrap(),
                "no waiter returns before the nested ending finished"
            );
        }
    }

    #[test]
    fn a_paste_carries_no_control_character() {
        let bytes = pasted("Horizon proof failed: \x1b[201~rm -rf ~\r\nnext\u{9b}");
        let text = String::from_utf8(bytes).unwrap();
        let inner = &text["\x1b[200~".len()..text.len() - "\x1b[201~".len()];
        assert!(!inner.chars().any(char::is_control), "{inner:?}");
        assert!(inner.contains("[201~rm -rf ~  next"));
    }

    #[test]
    fn a_status_file_cannot_grow_the_dialog_without_bound() {
        let steps: Vec<_> = (0..1000)
            .map(|index| serde_json::json!({"title": format!("Step {index} {}", "x".repeat(500)), "state": "done"}))
            .collect();
        let status = parse(&serde_json::json!({"steps": steps, "summary": "y".repeat(10_000)}).to_string()).unwrap();
        assert_eq!(status.steps.len(), STATUS_STEPS);
        assert!(status.steps.iter().all(|step| step.title.chars().count() <= 120));
        assert_eq!(status.summary.unwrap().chars().count(), 4000);
    }

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
        let steps = |dockerfile: &str| {
            format!(
                r#"{{"steps":[{{"title":"Read CI","state":"done"}},{{"title":"Find tools","state":"done"}},{{"title":"Write Dockerfile","state":"{dockerfile}"}},{{"title":"Build image","state":"running"}}]}}"#
            )
        };
        run.status = parse(&steps("done")).unwrap();
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
