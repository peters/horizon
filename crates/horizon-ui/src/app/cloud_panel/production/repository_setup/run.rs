//! A setup agent that works in a hidden panel while the New cloud dialog shows its
//! progress. The agent keeps a small status file current; the person reads the steps
//! here and steers the agent by typing into its terminal from the dialog.
use super::Mode;
use super::preview::{self, Preview};
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

/// The step a live preview owns.
const DESKTOP_PREVIEW: &str = "Desktop preview";

/// The steps the prompt asks the agent to report, in order. They show as pending
/// before the agent writes its first status. The preview starts once the whole proof,
/// fast rerun included, passed.
pub(super) const STEPS: [&str; 7] = [
    "Read CI",
    "Find tools",
    "Write Dockerfile",
    "Build image",
    "Run CI in image",
    "Fast rerun",
    "Desktop preview",
];

/// One run that ends after both `first` and `second`.
pub(super) fn joined(
    first: Option<std::thread::JoinHandle<()>>,
    second: Option<std::thread::JoinHandle<()>>,
) -> Option<std::thread::JoinHandle<()>> {
    match (first, second) {
        (Some(first), Some(second)) => Some(std::thread::spawn(move || {
            let _ = first.join();
            let _ = second.join();
        })),
        (first, second) => first.or(second),
    }
}

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

/// Whether the status file at `path` names a desktop preview other than `shown` now:
/// another spec or request, one Horizon cannot start, or none. A file that cannot be
/// read or parsed changes nothing, as for the dialog, which keeps the last good status.
pub(super) fn preview_changed_in(path: &std::path::Path, shown: Option<&(preview::Spec, u64)>) -> bool {
    let Some(status) = super::untrusted::read_regular(path, STATUS_LIMIT)
        .ok()
        .and_then(|bytes| parse(&String::from_utf8_lossy(&bytes)))
    else {
        return false;
    };
    status.preview_invalid || status.preview.map(|spec| (spec, status.preview_request)).as_ref() != shown
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
    /// The summary the agent was last told that its preview cannot start for.
    invalid_preview_summary: Option<String>,
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
    /// The live preview of a desktop app, the request it answers, and its Device panel.
    pub(super) preview: Option<Preview>,
    pub(super) previewed_request: u64,
    /// The preview spec Horizon started last, so a changed one starts again.
    previewed_spec: Option<preview::Spec>,
    pub(super) preview_panel: Option<PanelId>,
    /// A withdrawn preview still being removed, so what waits for the preview waits
    /// for it too.
    preview_stopping: Option<std::thread::JoinHandle<()>>,
    /// The preview request the agent was told needs a new proof first.
    stale_preview_reported: u64,
    /// The live desktop the preview panel shows, so a new one replaces it.
    pub(super) preview_shown: Option<String>,
    /// The agent's panel closed; whether the run was finished then decided its end, so a
    /// preview that stops later never discards a setup Use can still commit.
    pub(super) agent_closed: bool,
}

/// What the person did in the progress view.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Action {
    Steer(String),
    Show,
    /// Open the live preview of the desktop app.
    ShowPreview,
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
    /// What the agent asks Horizon to start live, and how often it asked.
    preview: Option<preview::Spec>,
    preview_request: u64,
    /// The agent wrote a preview Horizon cannot start; it is told, and Use waits.
    preview_invalid: bool,
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
            invalid_preview_summary: None,
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
            preview: None,
            previewed_request: 0,
            previewed_spec: None,
            preview_panel: None,
            preview_stopping: None,
            stale_preview_reported: 0,
            preview_shown: None,
            agent_closed: false,
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

    /// A preview to start: the proof passed, the agent named one, and it is newer than
    /// the last one started or differs from it.
    pub(super) fn preview_wanted(&self) -> Option<(u64, preview::Spec, String, String, proof::Pinned)> {
        let proof = self.proof.as_ref()?;
        // Only the proof of the setup as it is now: never an image a newer request or a
        // changed .horizon left behind.
        let current = self.passed() && self.inputs_proved();
        let request = self.status.preview_request;
        let changed = self.status.preview.is_some() && self.status.preview != self.previewed_spec;
        (current && (request > self.previewed_request || changed))
            .then(|| self.status.preview.clone())
            .flatten()
            .map(|spec| (request, spec, proof.image.clone(), proof.volume.clone(), proof.daemon()))
    }

    /// After a passed proof, starts the desktop app the agent named, live, takes what the
    /// preview reported, and returns the message for the agent when it ended.
    pub(super) fn sync_preview(&mut self, ctx: &egui::Context) -> Option<String> {
        self.stop_withdrawn_preview();
        if let Some((request, spec, image, volume, pinned)) = self.preview_wanted() {
            self.previewed_request = request;
            self.previewed_spec = Some(spec.clone());
            let screenshot = super::preview::screenshot_path(&self.status_path);
            let after = joined(
                self.preview.as_mut().map(super::preview::Preview::stop),
                self.preview_stopping.take(),
            );
            self.preview = Some(super::preview::Preview::start(
                (&image, &volume, pinned),
                (request, spec),
                screenshot,
                (after, ctx.clone()),
            ));
        }
        if let Some(message) = self.take_preview_over_changed_setup() {
            return Some(message);
        }
        self.preview.as_mut().and_then(|preview| {
            preview.poll();
            if preview.ended.is_none() {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            std::mem::take(&mut preview.unreported).then(|| preview.report())
        })
    }

    /// A newer preview request over a setup that changed after its proof passed waits
    /// for a new proof: the image would not have the change. The agent is told once.
    fn take_preview_over_changed_setup(&mut self) -> Option<String> {
        let request = self.status.preview_request;
        let changed =
            matches!((&self.proof, self.inputs), (Some(proof), Some((_, Some(now)))) if Some(now) != proof.inputs);
        let waiting = self.status.preview.is_some() && request > self.previewed_request && self.passed() && changed;
        (waiting && std::mem::replace(&mut self.stale_preview_reported, request) != request).then(|| {
            "The .horizon files changed after Horizon's proof passed, so the preview would run an image without the change. Increase proof_request by one; Horizon starts the preview again once that proof passed.".to_owned()
        })
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
        }
    }

    /// Stops the proof and the preview, and hands back one run that ends after both:
    /// their containers and volume go only then.
    pub(super) fn stop_all(&mut self) -> Option<std::thread::JoinHandle<()>> {
        let proof = self.proof.as_mut().and_then(Proof::stop);
        // The preview goes with its desktop, so nothing shows it as live any more.
        let preview = self.preview.take().map(|mut preview| preview.stop());
        joined(joined(proof, preview), self.preview_stopping.take())
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
        if title == DESKTOP_PREVIEW
            && let Some(preview) = &self.preview
        {
            return Some(
                preview
                    .ended
                    .unwrap_or_else(Instant::now)
                    .saturating_duration_since(preview.started),
            );
        }
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
    /// may write a summary first. A desktop preview the agent named is live.
    pub(super) fn finished(&self) -> bool {
        self.status.summary.is_some()
            && self.passed()
            && self.inputs_proved()
            && self.agent_steps_done()
            && self.preview_shown_or_none()
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

    /// Takes the preview's queued outcomes, then says whether the agent named no preview
    /// or the one it named last is live: the latest outcome counts, not the one last drawn.
    pub(super) fn preview_live_or_none(&mut self) -> bool {
        if let Some(preview) = self.preview.as_mut() {
            preview.poll();
        }
        self.preview_shown_or_none()
    }

    /// The loopback port of the live desktop the agent named, which Use checks again.
    pub(super) fn named_preview_port(&self) -> Option<u16> {
        self.status.preview.as_ref()?;
        self.preview.as_ref()?.live_port()
    }

    /// The desktop preview the status names, with its request, as the dialog last read it.
    pub(super) fn named_preview(&self) -> Option<(preview::Spec, u64)> {
        self.status
            .preview
            .clone()
            .map(|spec| (spec, self.status.preview_request))
    }

    /// Shows the preview the status names as live on `port`, with no run behind it.
    #[cfg(test)]
    pub(super) fn show_live_preview(&mut self, port: u16) {
        self.status = parse(r#"{"steps":[],"preview":{"command":"./app"}}"#).unwrap_or_default();
        self.previewed_request = self.status.preview_request;
        self.previewed_spec.clone_from(&self.status.preview);
        self.preview = Some(Preview::live(port));
    }

    /// The agent named no desktop preview, or Horizon showed the one it named last.
    fn preview_shown_or_none(&self) -> bool {
        !self.status.preview_invalid
            && (self.status.preview.is_none()
                || (self.previewed_request == self.status.preview_request
                    && self.previewed_spec == self.status.preview
                    && self
                        .preview
                        .as_ref()
                        .is_some_and(|preview| matches!(preview.outcome, preview::Outcome::Live { .. }))))
    }

    /// Every step the agent owns is reported done or skipped, and no step failed; the
    /// proof's own steps are Horizon's to judge.
    fn agent_steps_done(&self) -> bool {
        // Horizon runs a desktop preview the agent named; without one, Desktop preview is
        // the agent's step and must be marked skipped like any other.
        let mut horizons = vec![proof::BUILD, proof::CI, proof::FAST];
        if self.status.preview.is_some() {
            horizons.push(DESKTOP_PREVIEW);
        }
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

    /// The agent wrote its summary over a preview Horizon cannot start, and was not told
    /// yet for this summary.
    pub(super) fn take_summary_with_invalid_preview(&mut self) -> bool {
        let invalid = self.status.summary.is_some() && self.status.preview_invalid;
        if !invalid || self.invalid_preview_summary == self.status.summary {
            return false;
        }
        self.invalid_preview_summary.clone_from(&self.status.summary);
        true
    }

    /// The proof the agent asks for now.
    pub(super) fn proof_spec(&self) -> Option<&proof::Spec> {
        self.status.proof.as_ref()
    }

    /// The agent's panel closed. A setup that had not finished then ends: its agent's
    /// processes end, and its proof and preview stop and give back their containers,
    /// image and volume, as Stop does. Only the moment of closing decides, so a preview
    /// that stops later never discards a setup Use can still commit.
    pub(super) fn agent_panel_closed(&mut self) {
        let just_closed = !std::mem::replace(&mut self.agent_closed, true);
        if !self.finished() {
            if just_closed && !self.stopped {
                self.abandon();
            }
            self.stopped = true;
        }
    }

    /// Ends a run whose agent panel closed before it finished, in the background: the
    /// agent's processes end, the proof and the preview on its volume stop, and their
    /// containers, image and volume go. The run's folder stays for its view until Stop.
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
        // The preview's container runs on the proof's volume: both stop before it goes.
        let pinned = self.proof.as_ref().map(Proof::daemon);
        let worker = self.stop_all();
        let after = joined(before, worker);
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
        if step.title == DESKTOP_PREVIEW
            && let Some(preview) = &self.preview
        {
            let (state, detail) = match &preview.outcome {
                preview::Outcome::Starting => (State::Running, "Horizon is starting the app".to_owned()),
                preview::Outcome::Live { window, after, .. } => (
                    State::Done,
                    format!("{window} window after {} s · live in Horizon", after.as_secs()),
                ),
                preview::Outcome::Failed(reason) => (State::Failed, reason.clone()),
            };
            return Some(Step {
                title: step.title.clone(),
                state,
                detail,
            });
        }
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
    let preview_value = value.get("preview").filter(|preview| !preview.is_null());
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
        preview: preview_value.and_then(preview::Spec::parse),
        preview_invalid: preview_value.is_some_and(|preview| preview::Spec::parse(preview).is_none()),
        // The first preview is request 1, also when the agent wrote 0.
        preview_request: value
            .get("preview_request")
            .and_then(serde_json::Value::as_u64)
            .map_or(1, |request| request.max(1)),
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
    fn a_joined_run_ends_after_both() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let slow = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            sender.send(()).unwrap();
        });
        let quick = std::thread::spawn(|| {});
        joined(Some(quick), Some(slow)).unwrap().join().unwrap();
        assert!(receiver.try_recv().is_ok(), "the slower run had ended");
        assert!(joined(None, None).is_none());
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
        let steps = |preview: &str| {
            format!(
                r#"{{"steps":[{{"title":"Read CI","state":"done"}},{{"title":"Find tools","state":"done"}},{{"title":"Write Dockerfile","state":"done"}},{{"title":"Build image","state":"running"}},{{"title":"Desktop preview","state":"{preview}"}}]}}"#
            )
        };
        run.status = parse(&steps("skipped")).unwrap();
        assert!(run.agent_steps_done(), "Horizon's own steps are not the agent's to end");
        run.status = parse(&steps("failed").replacen('{', r#"{"preview":{"command":"./app"},"#, 1)).unwrap();
        assert!(
            run.agent_steps_done(),
            "Horizon owns a named preview and reports it itself"
        );
        run.status = parse(&steps("pending")).unwrap();
        assert!(
            !run.agent_steps_done(),
            "without a preview the agent marks the step skipped"
        );
        run.status = parse(&steps("skipped").replace(
            r#""Write Dockerfile","state":"done""#,
            r#""Write Dockerfile","state":"failed""#,
        ))
        .unwrap();
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
    fn use_compares_the_preview_the_ended_agent_left_with_the_live_one() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("status.json");
        let live = preview::Spec::parse(&serde_json::json!({"command": "./app"})).unwrap();
        let shown = Some((live, 1));
        assert!(
            !preview_changed_in(&path, shown.as_ref()),
            "a status that cannot be read changes nothing"
        );
        let write = |text: &str| std::fs::write(&path, text).unwrap();
        write(r#"{"steps":[],"preview":{"command":"./app"},"preview_request":1}"#);
        assert!(!preview_changed_in(&path, shown.as_ref()), "the live preview");
        write(r#"{"steps":[],"preview":{"command":"./app"},"preview_request":2}"#);
        assert!(preview_changed_in(&path, shown.as_ref()), "a newer request");
        write(r#"{"steps":[],"preview":{"command":"./app --demo"},"preview_request":1}"#);
        assert!(preview_changed_in(&path, shown.as_ref()), "another command");
        write(r#"{"steps":[],"preview":{"command":""},"preview_request":1}"#);
        assert!(preview_changed_in(&path, shown.as_ref()), "one Horizon cannot start");
        write(r#"{"steps":[],"preview":null}"#);
        assert!(preview_changed_in(&path, shown.as_ref()), "a withdrawn preview");
        assert!(!preview_changed_in(&path, None), "no preview, as before");
    }

    #[test]
    fn a_named_preview_must_be_live_and_its_step_belongs_to_horizon() {
        let mut run = Run::new(
            PanelId(1),
            PanelKind::Codex,
            PathBuf::from("."),
            PathBuf::from("missing.json"),
            Mode::New,
        );
        run.status = parse(r#"{"steps":[]}"#).unwrap();
        assert!(run.preview_shown_or_none(), "no preview named");
        run.status = parse(r#"{"steps":[],"preview":{"command":"./app"}}"#).unwrap();
        assert!(!run.preview_shown_or_none(), "a named preview that never ran");
        run.status = parse(r#"{"steps":[],"summary":"done","preview":{"command":""}}"#).unwrap();
        assert!(
            !run.preview_shown_or_none(),
            "a preview Horizon cannot start is not none"
        );
        assert!(run.take_summary_with_invalid_preview(), "the agent is told");
        assert!(!run.take_summary_with_invalid_preview(), "once per summary");
        let agent_steps = STEPS
            .iter()
            .filter(|title| ![proof::BUILD, proof::CI, proof::FAST, DESKTOP_PREVIEW].contains(title))
            .map(|title| format!(r#"{{"title":"{title}","state":"done"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        run.status = parse(&format!(
            r#"{{"steps":[{agent_steps}],"preview":{{"command":"./app"}}}}"#
        ))
        .unwrap();
        assert!(run.agent_steps_done(), "a named preview is not the agent's to finish");
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
