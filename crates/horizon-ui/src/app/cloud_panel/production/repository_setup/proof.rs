//! Horizon proves the setup agent's image itself: it builds the image, runs the worker
//! check and runs the CI jobs the agent named with localci in a container of the image,
//! then once more with `--fast` on the same volume. The agent only reads the result.
//! The timings are therefore the image's, not the agent's.
use horizon_core::cloud_runtime::Cancellation;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod bundle;
mod launch;
// Its only user, a UI test, runs where the setup agent is offered: on Linux.
#[cfg(all(test, target_os = "linux"))]
mod failed;
// The setup gate, an ignored test that scripts/setup-gate.py runs, on Linux likewise.
#[cfg(all(test, target_os = "linux"))]
mod gate;
mod localci;
mod needs;
mod preflight;
mod remote;
mod resources;
mod session_env;
mod slots;
pub(super) use session_env::arguments as session_environment;
mod stream;
use needs::distinct_builds;
pub(super) use needs::uses_nuget;
pub(super) use remote::{PortForward, close_forwards};
pub(super) use resources::{
    Daemon, Pinned, docker_host_of, expect_removal, keep_unremoved, preview_network, remove_now, remove_run_resources,
    retry_unremoved, run_id, run_names,
};
pub(super) use stream::end_group;

/// The step titles Horizon owns once the agent asks for a proof.
pub(super) const BUILD: &str = "Build image";
pub(super) const CI: &str = "Run CI in image";
pub(super) const FAST: &str = "Fast rerun";

/// Lines of proof output the dialog keeps.
const LOG_LINES: usize = 200;

/// Events waiting for the dialog; output lines beyond it are dropped, phases wait.
const EVENT_QUEUE: usize = 1024;

/// The longest an image build may take before the proof reports it hung.
const BUILD_TIMEOUT: Duration = Duration::from_mins(45);

/// The longest the CI jobs may run in the image before the proof reports them hung.
const CI_TIMEOUT: Duration = Duration::from_mins(90);

/// The longest one preparation step of the CI phase may take.
const STEP_TIMEOUT: Duration = Duration::from_mins(30);

/// Creates a file Horizon writes in the run's folder. The agent can write there too, so
/// whatever is at the path goes first (a link itself, never its target) and the file
/// is created new, never through a link someone put in its place.
pub(super) fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

pub(super) mod crates;
mod host;
mod spec;
pub(super) use spec::Spec;
use spec::commands_script;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Build,
    Ci,
    Fast,
}

impl Phase {
    /// Where the phase sits in [`Proof::phases`].
    const fn index(self) -> usize {
        match self {
            Self::Build => 0,
            Self::Ci => 1,
            Self::Fast => 2,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Build => BUILD,
            Self::Ci => CI,
            Self::Fast => FAST,
        }
    }
}

enum Event {
    Line(String),
    /// The fingerprint of the snapshot the proof read and tests.
    Inputs([u8; 32]),
    /// The setup the worker read asks for a GPU; the preview of its image gets it too.
    Gpu(bool),
    Started(Phase),
    /// Every build slot of the daemon is taken, by the setups of these repositories; an
    /// empty list once the proof has its slot.
    Waiting(Vec<String>),
    /// The failed build met an account problem; it comes before the build's `Finished`.
    Fix(super::access::Fix),
    Finished(Phase, Result<(), String>),
}

/// A phase as the dialog shows it.
#[derive(Clone, Debug, Default)]
pub(super) struct PhaseState {
    pub(super) started: Option<Instant>,
    pub(super) ended: Option<Instant>,
    pub(super) failure: Option<String>,
}

/// One proof run in the background.
pub(super) struct Proof {
    receiver: mpsc::Receiver<Event>,
    /// What the CI phases ran, as the report names it: the CI jobs or the commands.
    ran: &'static str,
    cancel: Cancellation,
    pub(super) log: VecDeque<String>,
    pub(super) phases: [(Phase, PhaseState); 3],
    /// Where the whole log is written, for the agent to read on a failure.
    pub(super) log_path: PathBuf,
    pub(super) image: String,
    /// The volume with the checkout and its build output.
    pub(super) volume: String,
    /// The proof ended, passed or not, and the agent has not been told yet.
    pub(super) unreported: bool,
    /// The proof's container, so a stopped or replaced proof also stops its run.
    container: String,
    /// The fingerprint of the setup and request this proof tests, once its worker read
    /// the setup.
    pub(super) inputs: Option<[u8; 32]>,
    /// Whether this proof restores private .NET packages on this computer: part of
    /// what it proves, so changing the choice afterwards needs a new proof.
    pub(super) host_restore: bool,
    /// The Rust git repositories it may clone here: part of what it proves, as above.
    pub(super) host_crates: Vec<String>,
    /// The proved setup's default profile asks for a GPU; its preview gets it too.
    pub(super) gpu: bool,
    /// The background run, so cleanup waits until it can create nothing more.
    worker: Option<std::thread::JoinHandle<()>>,
    /// The daemon the run checked and uses, for Stop and the run's cleanup.
    pinned: Pinned,
    /// An account problem the build met that the person fixes on GitHub.
    pub(super) fix: Option<super::access::Fix>,
    /// The repositories whose setups hold every build slot while this proof waits for one.
    pub(super) waiting: Vec<String>,
}

impl Proof {
    /// Starts the proof of the setup in `repository`. `log_path` keeps the whole output.
    /// Starts proof number `request` of the setup in `repository`; each gets its own
    /// container, so the cleanup of an earlier one never meets it. `host_restore` lets it
    /// restore private .NET packages on this computer, which the person allowed. It
    /// begins once `after`, the run of the proof it replaces, has ended.
    pub(super) fn start(
        (repository, run, identity): (&Path, &str, super::untrusted::Identity),
        build_on: Option<horizon_core::cloud_runtime::docker_host::Host>,
        (request, spec): (u64, Spec),
        log_path: PathBuf,
        (host_restore, root_dirty, host_crates): (bool, std::sync::Arc<std::sync::atomic::AtomicBool>, crates::Access),
        (after, ctx): (Option<std::thread::JoinHandle<()>>, egui::Context),
    ) -> Self {
        let approved = host_crates.repositories.clone();
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
        let cancel = Cancellation::default();
        let pinned = Pinned::default();
        let (image, volume) = run_names(repository, run);
        let container = format!("{volume}-proof-{request}");
        let ran = if spec.commands.is_empty() {
            "CI jobs"
        } else {
            "proof commands"
        };
        let job = Job {
            repository: repository.to_path_buf(),
            identity,
            build_on,
            spec,
            image: image.clone(),
            volume: volume.clone(),
            container: container.clone(),
            host_restore,
            root_dirty,
            setup: Err("the setup was not read yet".to_owned()),
            tail: std::cell::RefCell::default(),
            host_crates,
            mirrors: std::cell::RefCell::default(),
            log_path: log_path.clone(),
            cancel: cancel.clone(),
            sender,
            logged: std::cell::Cell::new(0),
            localci: std::cell::RefCell::new(Vec::new()),
            daemon: resources::Daemon::default(),
            pinned: pinned.clone(),
            planned: std::cell::RefCell::new(Vec::new()),
            ci_localci: std::cell::RefCell::new(None),
            restored: std::cell::RefCell::new(Vec::new()),
            packages_copied: std::cell::Cell::new(false),
            docker_config: std::cell::OnceCell::new(),
            ctx,
        };
        let worker = std::thread::spawn(move || {
            if let Some(after) = after {
                let _ = after.join();
            }
            job.run();
        });
        Self {
            receiver,
            ran,
            cancel,
            log: VecDeque::new(),
            phases: [
                (Phase::Build, PhaseState::default()),
                (Phase::Ci, PhaseState::default()),
                (Phase::Fast, PhaseState::default()),
            ],
            log_path,
            image,
            volume,
            unreported: false,
            container,
            inputs: None,
            host_restore,
            host_crates: approved,
            gpu: false,
            worker: Some(worker),
            pinned,
            fix: None,
            waiting: Vec::new(),
        }
    }

    /// The daemon the proof's run checked and uses, once it has.
    pub(super) fn daemon(&self) -> Pinned {
        self.pinned.clone()
    }

    /// Stops the proof and hands back its run, to wait for before its resources go.
    pub(super) fn stop(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.cancel.cancel();
        self.remove_container();
        // No one reads the proof's events any more: its receiver goes now, so a worker
        // waiting to send into a full queue fails at once instead of holding up whoever
        // waits for it to end.
        let (_, closed) = mpsc::sync_channel(1);
        drop(std::mem::replace(&mut self.receiver, closed));
        self.worker.take()
    }

    fn remove_container(&self) {
        // Killing the docker client leaves its container running; the run goes too. On
        // a thread that waits for it, so the dialog never blocks and no process is left
        // unreaped.
        // A run that has not checked its daemon has started no container yet; the run's
        // cleanup removes one it starts after all.
        let Some(daemon) = self.pinned.get().cloned() else {
            return;
        };
        let container = self.container.clone();
        super::run::keep_ending(std::thread::spawn(move || {
            let _ = resources::remove_now(&daemon, &["rm", "--force", "--volumes", &container]);
        }));
    }

    /// Takes what the background run reported since the last frame.
    pub(super) fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Line(line) => {
                    if self.log.len() == LOG_LINES {
                        self.log.pop_front();
                    }
                    self.log.push_back(line);
                }
                Event::Inputs(inputs) => self.inputs = Some(inputs),
                Event::Gpu(gpu) => self.gpu = gpu,
                Event::Started(phase) => self.phase_mut(phase).started = Some(Instant::now()),
                Event::Waiting(holders) => self.waiting = holders,
                Event::Fix(fix) => self.fix = Some(fix),
                Event::Finished(phase, result) => {
                    let state = self.phase_mut(phase);
                    state.ended = Some(Instant::now());
                    state.failure = result.err();
                    if phase == Phase::Fast || state.failure.is_some() {
                        self.unreported = true;
                    }
                }
            }
        }
    }

    pub(super) fn phase(&self, phase: Phase) -> &PhaseState {
        &self.phases[phase.index()].1
    }

    fn phase_mut(&mut self, phase: Phase) -> &mut PhaseState {
        &mut self.phases[phase.index()].1
    }

    pub(super) fn running(&self) -> bool {
        self.phases
            .iter()
            .any(|(_, state)| state.started.is_some() && state.ended.is_none())
    }

    /// The message the agent gets once the proof ended.
    pub(super) fn report(&self) -> String {
        let time = |phase: Phase| {
            let state = self.phase(phase);
            match (state.started, state.ended) {
                (Some(start), Some(end)) => format!("{} s", end.duration_since(start).as_secs()),
                _ => "not run".to_owned(),
            }
        };
        if let Some(fix) = &self.fix {
            return fix.report();
        }
        if let Some((phase, state)) = self.phases.iter().find(|(_, state)| state.failure.is_some()) {
            return format!(
                "Horizon proof: {} failed: {}. The whole log is in {}. Fix the Dockerfile or the proof, then increase proof_request by one.",
                phase.title(),
                state.failure.as_deref().unwrap_or_default(),
                self.log_path.display()
            );
        }
        format!(
            "Horizon proof passed: image {} built and checked in {}, {} passed in {}, fast rerun in {}. Use these times in your summary. Horizon now starts the desktop preview named in the status file; without a GUI app, mark Desktop preview skipped.",
            self.image,
            time(Phase::Build),
            self.ran,
            time(Phase::Ci),
            time(Phase::Fast)
        )
    }
}

impl Drop for Proof {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.remove_container();
    }
}

/// Horizon's own localci in its private folder, or why it is not there.
type TrustedLocalci = Result<(tempfile::TempDir, PathBuf), String>;

struct Job {
    repository: PathBuf,
    /// Which folder the repository was when the run started.
    identity: super::untrusted::Identity,
    /// The Docker host the person chose to build on; none for this computer.
    build_on: Option<horizon_core::cloud_runtime::docker_host::Host>,
    spec: Spec,
    image: String,
    volume: String,
    container: String,
    host_restore: bool,
    /// A preview of the run ran as root, so the volume's folders are taken back first.
    root_dirty: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The setup this proof tests, read once at its start, or why Horizon refuses it.
    setup: Result<super::snapshot::Setup, String>,
    /// The newest lines of the current phase, kept here so a diagnosis never reads the
    /// log file back from the run folder, where the agent can write.
    tail: std::cell::RefCell<super::access::Tail>,
    host_crates: crates::Access,
    mirrors: std::cell::RefCell<Option<crates::Mirrors>>,
    log_path: PathBuf,
    cancel: Cancellation,
    sender: mpsc::SyncSender<Event>,
    /// Bytes written to the whole log so far, against [`LOG_FILE_LIMIT`].
    logged: std::cell::Cell<u64>,
    /// Horizon's own localci for each platform the proof's images have, fetched once per
    /// proof.
    localci: std::cell::RefCell<Vec<(String, TrustedLocalci)>>,
    /// The daemon of this computer every docker command of the run goes to, checked
    /// before the first one, and shared with the proof for its Stop and cleanup.
    daemon: resources::Daemon,
    pinned: Pinned,
    /// The .NET solutions or projects the host restored, by their path in the checkout.
    restored: std::cell::RefCell<Vec<String>>,
    /// The host restore copied the packages it used into the volume.
    packages_copied: std::cell::Cell<bool>,
    /// The localci a worker session of the CI's image runs, which the CI runs too; none
    /// when the image has none, and then Horizon's own runs.
    ci_localci: std::cell::RefCell<Option<String>>,
    /// The legs localci's plan selected, which the full run's report must account for.
    planned: std::cell::RefCell<Vec<String>>,
    /// The throwaway Docker configuration every docker command of the run uses, made
    /// before the first one that can pull, and whether it holds Horizon's login.
    docker_config: std::cell::OnceCell<(tempfile::TempDir, bool)>,
    ctx: egui::Context,
}

impl Job {
    /// A docker command for the run's daemon.
    fn docker(&self) -> Command {
        let mut command = Command::new("docker");
        self.pin(&mut command);
        command
    }

    /// Points `command`, a docker command, at the run's daemon and Docker configuration
    /// only: no login or credential helper of this computer's is asked for anything.
    fn pin(&self, command: &mut Command) {
        self.daemon.pin(command);
        if let Some((config, _)) = self.docker_config.get() {
            command
                .env("DOCKER_CONFIG", config.path())
                .env_remove("DOCKER_AUTH_CONFIG");
        }
    }

    /// The commit the proof's snapshot names: the proof clones and tests that one.
    fn head(&self) -> Option<&str> {
        self.setup
            .as_ref()
            .ok()
            .map(|setup| setup.head.as_str())
            .filter(|head| !head.is_empty())
    }

    fn run(mut self) {
        // One snapshot of the setup, read here and not on the dialog's thread: the proof
        // fingerprints it and builds from it alone. A setup Horizon refuses to read fails
        // the build with the reason.
        self.setup = self.read_setup();
        if let Ok(setup) = &self.setup {
            let _ = self.sender.send(Event::Inputs(setup.fingerprint(&self.spec)));
            let _ = self.sender.send(Event::Gpu(default_wants_gpu(setup)));
        }
        // Without its log a failure could not tell the agent what to read: none runs.
        let mut log = match create_private(&self.log_path) {
            Ok(file) => Some(file),
            Err(error) => {
                let _ = self.sender.send(Event::Started(Phase::Build));
                let reason = format!("cannot write the proof log {}: {error}", self.log_path.display());
                let _ = self.sender.send(Event::Finished(Phase::Build, Err(reason)));
                self.ctx.request_repaint();
                return;
            }
        };
        // The daemon is checked before the first docker command, and every later one
        // goes to it.
        match resources::Daemon::on(self.build_on.as_ref()) {
            Ok(daemon) => {
                let _ = self.pinned.set(daemon.clone());
                self.daemon = daemon;
            }
            Err(reason) => {
                let _ = self.sender.send(Event::Started(Phase::Build));
                let _ = self.sender.send(Event::Finished(Phase::Build, Err(reason)));
                self.ctx.request_repaint();
                return;
            }
        }
        // An earlier proof of this repository may still be stopping: its container goes,
        // within the proof's limits, and a daemon that does not answer fails the build.
        if let Err(reason) = self.remove_earlier() {
            let _ = self.sender.send(Event::Started(Phase::Build));
            let _ = self.sender.send(Event::Finished(Phase::Build, Err(reason)));
            self.ctx.request_repaint();
            return;
        }
        // The heavy part waits for a build slot of the daemon, held until the proof ends.
        let Some(_slot) = self.take_slot() else {
            return;
        };
        for phase in [Phase::Build, Phase::Ci, Phase::Fast] {
            if self.cancel.is_cancelled() {
                return;
            }
            self.tail.borrow_mut().clear();
            let _ = self.sender.send(Event::Started(phase));
            self.ctx.request_repaint();
            let result = match phase {
                Phase::Build => match &self.setup {
                    Err(reason) => Err(format!("Horizon does not read this setup: {reason}")),
                    Ok(setup) => self.build(setup, &mut log),
                },
                Phase::Ci => self.prepare_ci(&mut log).and_then(|()| self.ci(false, &mut log)),
                Phase::Fast => self.ci(true, &mut log),
            };
            let failed = result.is_err();
            // A stopped or replaced proof asks GitHub nothing and offers no fix.
            if phase == Phase::Build
                && failed
                && !self.cancel.is_cancelled()
                && let Some(fix) = self
                    .tail
                    .borrow()
                    .diagnose(&horizon_docker(), &|| self.cancel.is_cancelled())
            {
                let _ = self.sender.send(Event::Fix(fix));
            }
            let _ = self.sender.send(Event::Finished(phase, result));
            self.ctx.request_repaint();
            if failed {
                return;
            }
        }
    }

    /// One of the daemon's build slots, once one is free; `None` once the proof stopped.
    /// While it waits, the dialog hears which setups hold them.
    fn take_slot(&self) -> Option<slots::Slot> {
        let tell = |holders: &[String]| {
            let _ = self.sender.send(Event::Waiting(holders.to_vec()));
            self.ctx.request_repaint();
        };
        slots::take_for(Some(&self.daemon.slot_key()), &self.repository, &self.cancel, tell)
    }

    /// Refuses a setup this proof cannot prove: same-worker companions, which deployment
    /// layers on the image and checks there, and a GPU profile on a Docker host or on a
    /// computer without an NVIDIA GPU.
    fn refuse_unprovable(&self, config: &horizon_core::cloud_panel::CloudConfig) -> Result<(), String> {
        // Deployment layers each same-worker companion's recipe on the image and checks
        // the layered result; this proof builds only the profiles' own images, so it
        // does not call such a setup proved.
        if let Some((name, _)) = config.same_worker_siblings().next() {
            return Err(format!(
                "the setup declares the same-worker companion {name}; Horizon cannot prove same-worker companions yet, so leave them out of this setup"
            ));
        }
        // A GPU profile is proved on a computer with an NVIDIA GPU or not at all; the CI
        // runs with it in the default profile's image.
        if let Some((name, _)) = config.profiles.iter().find(|(_, profile)| profile.gpu) {
            if let Some(remote) = self.daemon.remote() {
                return Err(format!(
                    "profile {name} needs a GPU, and Horizon proves a GPU setup only on this computer, not on the Docker host {}: build it on This computer",
                    remote.name()
                ));
            }
            if !Path::new("/dev/nvidiactl").exists() {
                return Err(format!(
                    "profile {name} needs a GPU and this computer has no NVIDIA GPU, so Horizon cannot prove this setup here"
                ));
            }
        }
        Ok(())
    }

    fn build(&self, setup: &super::snapshot::Setup, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // The pins first: no Docker, so a moved toolchain fails before any pull.
        self.check_pins(setup, log)?;
        // Made before the first command that can pull an image.
        let (docker_config, signed_in) = self.docker_config()?;
        let config = setup.config()?;
        self.refuse_unprovable(&config)?;
        let default_builds = config
            .profiles
            .get(&config.default)
            .ok_or("the default profile is missing")?
            .build
            .is_some();
        // The agent CLI releases deployment would pin, so every build gets its arguments.
        let releases = horizon_core::cloud_runtime::image::agents::latest(&self.cancel)
            .map_err(|error| format!("cannot look up the agent CLI releases: {error}"))?;
        let builds = distinct_builds(&config, |profile| {
            horizon_core::cloud_runtime::image::build_arguments(&profile.capabilities, &releases)
        })?;
        let _ = self.sender.try_send(Event::Line(if signed_in {
            "Pulling ghcr.io images with Horizon's GitHub login.".to_owned()
        } else {
            "No Horizon GitHub login for ghcr.io; images are pulled anonymously.".to_owned()
        }));
        // Every profile can be chosen in New cloud, so each distinct build is built; the
        // CI then runs in the default profile's image.
        let tag = |index: usize| {
            if index == 0 && default_builds {
                self.image.clone()
            } else {
                format!("{}-build{index}:candidate", self.volume)
            }
        };
        for (index, (build, arguments)) in builds.iter().enumerate() {
            self.build_one(setup, (build, arguments), &tag(index), docker_config, log)?;
        }
        // The agents on the worker, and the proof's CI as they would, run the localci a
        // worker session finds: every image the setup builds must have one, as any
        // profile with a build can be chosen in New cloud. A repository without GitHub
        // Actions is proved with commands, and its images are asked for no localci.
        let with_localci = if self.spec.commands.is_empty() {
            &builds[..]
        } else {
            &[]
        };
        for (index, (build, _)) in with_localci.iter().enumerate() {
            let place = self
                .session_localci(&tag(index), index)
                .and_then(|found| found.ok_or_else(|| NO_LOCALCI.to_owned()))
                .map_err(|error| {
                    let profiles = config
                        .profiles
                        .iter()
                        .filter(|(_, profile)| profile.build.as_ref() == Some(build))
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{error} (the image of profile {profiles})")
                })?;
            if tag(index) == self.image {
                *self.ci_localci.borrow_mut() = Some(place);
            }
        }
        // Each profile's worker contract, checked the way deployment checks it: its
        // capabilities go to horizon-worker-check and every marker it reports is validated.
        for (number, (name, profile)) in config.profiles.iter().enumerate() {
            let Some(build) = &profile.build else {
                // A published image runs as it is, and deployment checks its contract, so
                // the proof does too: pulled first unless it is Horizon's own known base.
                let trusted = horizon_core::cloud_runtime::repository::launch::quick_start::trusted_contract(
                    &profile.image,
                    &profile.capabilities,
                )
                .is_some();
                // The default profile's image is the one the CI runs in, so it must be here.
                let default = *name == config.default;
                if !trusted || default {
                    let mut pull = self.docker();
                    pull.env_remove("DOCKER_AUTH_CONFIG")
                        .arg("--config")
                        .arg(docker_config)
                        .args(["pull", "--platform", "linux/amd64", &profile.image]);
                    self.stream_within(pull, log, Some(BUILD_TIMEOUT))?;
                }
                self.check_contract((name, profile), &profile.image, number, docker_config, log)?;
                if default {
                    let mut retag = self.docker();
                    retag.args(["tag", &profile.image, &self.image]);
                    self.stream(retag, log)?;
                    // Without a localci of its own, its CI runs Horizon's; a proof with commands
                    // runs no localci, so it does not look for one.
                    if self.spec.commands.is_empty() {
                        *self.ci_localci.borrow_mut() = self.session_localci(&self.image, builds.len())?;
                    }
                }
                continue;
            };
            let arguments = horizon_core::cloud_runtime::image::build_arguments(&profile.capabilities, &releases);
            let index = builds
                .iter()
                .position(|(built, with)| built == build && *with == arguments)
                .ok_or_else(|| format!("profile {name} has no proof image"))?;
            self.check_contract((name, profile), &tag(index), number, docker_config, log)?;
        }
        Ok(())
    }

    /// Validates `profile`'s worker contract on the image `tag` with deployment's own
    /// check, which runs the image's horizon-worker-check with the profile's capabilities.
    fn check_contract(
        &self,
        (name, profile): (&str, &horizon_core::cloud_panel::Profile),
        tag: &str,
        number: usize,
        docker_config: &Path,
        log: &mut Option<std::fs::File>,
    ) -> Result<(), String> {
        let cell = std::cell::RefCell::new(log.take());
        let emit = |event: horizon_core::cloud_runtime::Event| {
            if let horizon_core::cloud_runtime::Event::Output(line) = event {
                self.record(&mut cell.borrow_mut(), line);
            }
        };
        let runner = horizon_core::cloud_runtime::command::Runner {
            cancel: &self.cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        let images = horizon_core::cloud_runtime::image::Images {
            docker_host: self.daemon.host(),
            isolated_registry: true,
            docker_config,
            runner: &runner,
            publisher: None,
        };
        // Named after the run, so the run's cleanup finds a check container that a
        // cancelled check left, and the next check of the same profile reuses the name.
        let operation = format!("{}-{number}", resources::check_operations(&self.volume));
        let result = images
            .validate_contract(tag, &operation, profile, true)
            .map(|_| ())
            .map_err(|error| format!("the worker check of profile {name} failed: {error}"));
        *log = cell.into_inner();
        result
    }

    fn build_one(
        &self,
        setup: &super::snapshot::Setup,
        (build, arguments): (&horizon_core::cloud_panel::Build, &[String]),
        tag: &str,
        docker_config: &Path,
        log: &mut Option<std::fs::File>,
    ) -> Result<(), String> {
        // The agent wrote these paths: both stay inside the repository, as deployment
        // demands, and the build reads the proof's snapshot, never the live folder.
        let (folder, context, dockerfile) = setup.context(&self.repository, build)?;
        // buildx numbers its steps per build, so a diagnosis reads this build's output only.
        self.tail.borrow_mut().clear();
        let mut command = self.docker();
        command
            .current_dir(folder.path())
            .env_remove("DOCKER_AUTH_CONFIG")
            .arg("--config")
            .arg(docker_config)
            .args(["buildx", "build", "--load", "--progress", "plain", "--platform"])
            .arg(&build.platform)
            .args(arguments)
            .arg("-f")
            .arg(&dockerfile)
            .arg("-t")
            .arg(tag)
            .arg(&context);
        self.stream_within(command, log, Some(BUILD_TIMEOUT))
    }

    /// Clones the committed repository into the proof volume, or brings the clone
    /// up to date, so build output and caches stay between runs.
    fn prepare(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // The image the agent wrote gets the tested commit and its history only, as a
        // bundle made here, never this computer's git folder with its other branches.
        let head = self.head().ok_or("the repository has no commit")?;
        // In a new private folder with a random name, never beside the agent's files.
        let folder = tempfile::Builder::new()
            .prefix("horizon-setup-bundle-")
            .tempdir()
            .map_err(|error| format!("cannot make a private folder for the bundle: {error}"))?;
        let (bundle, branch) = self.bundle(folder.path(), head, log)?;
        // Horizon's pinned base image, not the candidate, prepares the volume: what the
        // CI later runs on is the bundled commit, whatever the candidate's tools do.
        let mut own = self.docker();
        own.args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", TOOLS, "-c"]);
        let take_back = self.root_dirty.load(std::sync::atomic::Ordering::SeqCst);
        own.arg(if take_back {
            format!("{OWN}; {TAKE_BACK}")
        } else {
            OWN.to_owned()
        });
        self.stream(own, log)?;
        if take_back {
            self.root_dirty.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        // As root, so the checkout can be handed to the worker's account after. The
        // bundle goes into the container as a copy, so any daemon can run it.
        let mut clone = self.docker();
        clone
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-e")
            .arg(format!("HEAD_SHA={head}"))
            .arg("-e")
            .arg(format!("HEAD_BRANCH={}", branch.unwrap_or_default()))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg(CLONE);
        let mut clone = launch::Launch::new(clone);
        clone.copies.push(launch::Copy::new(bundle, "/src.bundle"));
        let result = self.stream_launch(clone, log, Some(STEP_TIMEOUT), false);
        drop(folder);
        result?;
        self.hydrate(head, log)
    }

    /// Fills the checkout's submodules and Git LFS files as `actions/checkout` would and
    /// deployment does: when the commit has submodules or LFS files, deployment's own
    /// snapshot of it, made on this computer, goes over the checkout in the volume.
    fn hydrate(&self, head: &str, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // Within the proof's limits and Stop: the repository's configuration is the agent's.
        let git = |args: &[&str]| {
            let mut command = super::commit::git_command(&self.repository);
            command.args(args);
            self.capture(command, STEP_TIMEOUT).is_ok()
        };
        let modules = git(&["cat-file", "-e", &format!("{head}:.gitmodules")]);
        let lfs = git(&[
            "grep",
            "-q",
            "filter=lfs",
            head,
            "--",
            ":(glob)**/.gitattributes",
            ".gitattributes",
        ]);
        if !modules && !lfs {
            return Ok(());
        }
        let folder = tempfile::Builder::new()
            .prefix("horizon-proof-checkout-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let runner = horizon_core::cloud_runtime::command::Runner {
            cancel: &self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let tree = horizon_core::cloud_runtime::repository::snapshot(&self.repository, head, folder.path(), &runner)
            .map_err(|error| format!("cannot make the committed tree with its submodules and LFS files: {error}"))?;
        let mut overlay = self.docker();
        overlay
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg("cp -a /snapshot/. /workspace/checkout/ && chown -R 10001:10001 /workspace/checkout");
        let mut overlay = launch::Launch::new(overlay);
        overlay.copies.push(launch::Copy::new(tree, "/snapshot"));
        self.stream_launch(overlay, log, Some(STEP_TIMEOUT), false).map(|_| ())
    }

    /// Whether the default profile, whose image the CI runs in, asks for a GPU.
    fn wants_gpu(&self) -> bool {
        self.setup.as_ref().is_ok_and(default_wants_gpu)
    }

    /// Runs the CI jobs the way the agents on the worker do: the container starts the
    /// localci a worker session finds, with the image's session environment read
    /// beforehand, and Horizon's own when the image has none; the steps then run in the
    /// image as the worker would run them.
    ///
    /// The proof shows that the setup works; it is no defence against an image made to
    /// lie. Every program the CI runs comes from the image, its shell included, so such
    /// an image passes with any localci. Horizon's own checks the plan, which reads only
    /// the repository's workflow.
    fn ci(&self, fast: bool, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let head = self.head().unwrap_or_default();
        let run = |command| self.capture(command, STEP_TIMEOUT);
        let session = session_env::arguments(&self.image, &format!("{}-env", self.container), &run)?;
        if !fast {
            self.forget_older_records(&session, log)?;
            self.restore_in_container(&session, log)?;
        }
        let mut launch = self.ci_container()?;
        launch.command.args(&session);
        if !self.spec.commands.is_empty() {
            // Without GitHub Actions the commands themselves are the proof, run with the
            // image's shell; a rerun on the warm volume shows what the caches save.
            launch.command.args([
                "--entrypoint",
                "bash",
                &self.image,
                "-c",
                &commands_script(&self.spec.commands),
            ]);
            return self.stream_launch(launch, log, Some(CI_TIMEOUT), false).map(|_| ());
        }
        launch
            .command
            .arg("--entrypoint")
            .arg(
                self.ci_localci
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| localci::IN_CONTAINER.to_owned()),
            )
            .arg(&self.image)
            .args(self.spec.localci_args(fast, head));
        if fast {
            return self.stream_launch(launch, log, Some(CI_TIMEOUT), false).map(|_| ());
        }
        let report = self.stream_launch(launch, log, Some(CI_TIMEOUT), true)?;
        localci::check_report(&report, &self.planned.borrow())
    }

    /// localci's step records do not know the image: before a full run after a rebuild
    /// Horizon's base image removes them from the volume, so no job passes on a record
    /// an older image left.
    fn forget_older_records(&self, session: &[String], log: &mut Option<std::fs::File>) -> Result<(), String> {
        let mut forget = self.docker();
        forget
            .args(["run", "--rm", "--name", &self.container, "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["-e", "HOME=/workspace/home", "-e"])
            .arg(format!(
                "HORIZON_LOCALCI_RECORDS={}",
                localci::records(session).unwrap_or_default()
            ))
            .arg("-e")
            .arg(format!("HORIZON_PROOF_IMAGE={}", self.image_id()))
            .args(["--entrypoint", "sh", TOOLS, "-c", FORGET_OLDER_IMAGE]);
        self.stream(forget, log)
    }

    /// The built image's ID, or its tag when Docker cannot say, asked within the
    /// proof's time limit.
    fn image_id(&self) -> String {
        let mut inspect = self.docker();
        inspect.args(["image", "inspect", "--format", "{{.Id}}", &self.image]);
        self.capture(inspect, STEP_TIMEOUT)
            .ok()
            .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| self.image.clone())
    }

    fn remove_container(&self) {
        let _ = resources::remove_now(&self.daemon, &["rm", "--force", "--volumes", &self.container]);
    }
}

/// Why a built image fails the proof when a worker session finds no localci.
const NO_LOCALCI: &str = "a worker session finds no localci on its PATH; install it for the agents on the worker, for example at /usr/local/bin/localci";

/// Fetches the tested commit from its bundle into the volume's checkout and checks it
/// out on its branch, as `actions/checkout` does for a push, or detached when it has no
/// branch without doubt, then removes everything untracked. It runs as root in Horizon's base image, so
/// it starts from a fresh git folder with no global settings and no hooks: an earlier
/// proof's CI could have written to the checkout's .git and to HOME, and none of that
/// may run here. Git LFS files stay pointers, as `actions/checkout` leaves them without
/// `lfs: true`; the bundle has no LFS objects, and `hydrate` brings them when the commit
/// has any.
const CLONE: &str = "set -e; export HOME=/tmp/horizon-clone GIT_CONFIG_GLOBAL=/dev/null GIT_LFS_SKIP_SMUDGE=1; mkdir -p \"$HOME\"; \
git config --system --add safe.directory '*'; rm -rf /workspace/checkout; mkdir /workspace/checkout; \
cd /workspace/checkout; git init -q; G='git -c core.hooksPath=/dev/null -c core.fsmonitor=false'; \
$G fetch -q /src.bundle HEAD '+refs/heads/*:refs/remotes/origin/*' '+refs/tags/*:refs/tags/*'; \
if [ -n \"$HEAD_BRANCH\" ]; then $G checkout -q --force -B \"$HEAD_BRANCH\" \"$HEAD_SHA\"; \
else $G checkout -q --force --detach \"$HEAD_SHA\"; fi; $G clean -q -ffdx; \
chown -R 10001:10001 /workspace/checkout";

/// Hands the volume to the worker's account, with the folders a worker session names:
/// HOME, the session's data folder and the GPU lock's folder. They stay between runs for
/// their caches, but a link an earlier run left in their place goes, so nothing is made
/// through it.
const OWN: &str = "for d in /workspace/home /workspace/session-data /workspace/session-data/proof /workspace/locks; do \
if [ -L \"$d\" ]; then rm -f \"$d\"; fi; mkdir -p \"$d\"; done; \
chown 10001:10001 /workspace /workspace/home /workspace/session-data /workspace/session-data/proof /workspace/locks";

/// Gives the worker's account back what a preview of the run that ran as root left in
/// those folders (links themselves, never followed). It runs only when Horizon knows
/// such a preview ran; nothing on the volume decides it.
const TAKE_BACK: &str =
    "find /workspace/home /workspace/session-data /workspace/locks ! -user 10001 -exec chown -h 10001:10001 {} +";

/// Forgets localci's step records when the volume last ran a proof in another image.
/// It runs with Horizon's settings only, never the image's: the records' folder comes
/// from [`localci::records`], is resolved through any link first, and is never inside
/// the checkout, and a link at the marker's place is replaced, not written through.
const FORGET_OLDER_IMAGE: &str = "marker=\"$HOME/.horizon-proof-image\"; \
if [ \"$(cat \"$marker\" 2>/dev/null)\" != \"$HORIZON_PROOF_IMAGE\" ]; then \
if [ -n \"$HORIZON_LOCALCI_RECORDS\" ]; then records=$(realpath -m \"$HORIZON_LOCALCI_RECORDS\"); \
case \"$records\" in /workspace/checkout|/workspace/checkout/*) ;; /workspace/?*) rm -rf \"$records\";; esac; fi; \
rm -f \"$marker\"; printf '%s' \"$HORIZON_PROOF_IMAGE\" > \"$marker\"; fi;";

/// The setup agent's instruction for the localci the image must hold.
pub(super) fn localci_instruction() -> String {
    localci::instruction()
}

/// The image that prepares the proof's volume: Horizon's pinned base, never the
/// candidate the agent wrote.
pub(super) const TOOLS: &str = horizon_core::cloud_runtime::repository::launch::quick_start::IMAGE;

/// Horizon's own Docker configuration directory, where its `ghcr.io` login is kept.
fn horizon_docker() -> PathBuf {
    horizon_core::HorizonHome::resolve().root().join("cloud").join("docker")
}

/// Whether the setup's default profile asks for a GPU. Its proof and preview then get
/// this computer's GPU, or fail without one; they never run without it unseen.
fn default_wants_gpu(setup: &super::snapshot::Setup) -> bool {
    setup
        .config()
        .ok()
        .and_then(|config| config.profiles.get(&config.default).map(|profile| profile.gpu))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The script runs in Horizon's Ubuntu base, whose realpath takes -m; macOS's does not.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_new_image_forgets_the_step_records_an_older_one_left() {
        // The volume's /workspace is a folder here.
        let volume = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(volume.path()).unwrap();
        let script = FORGET_OLDER_IMAGE.replace("/workspace", &root.to_string_lossy());
        let home = root.join("home");
        let run = |records: &std::path::Path, image: &str| {
            std::fs::create_dir_all(records).unwrap();
            std::fs::write(records.join("record"), "passed").unwrap();
            let status = Command::new("bash")
                .args(["-c", &script])
                .env("HOME", &home)
                .env("HORIZON_LOCALCI_RECORDS", records)
                .env("HORIZON_PROOF_IMAGE", image)
                .status()
                .unwrap();
            assert!(status.success());
            records.join("record").exists()
        };
        let records = home.join(".cache/localci");
        assert!(!run(&records, "sha256:one"), "the first proof in a volume starts clean");
        assert!(run(&records, "sha256:one"), "the same image keeps its records");
        assert!(!run(&records, "sha256:two"), "a rebuilt image forgets them");
        // A link an earlier CI left that leads into the checkout is never followed there.
        let tracked = root.join("checkout/localci");
        std::fs::create_dir_all(&tracked).unwrap();
        std::fs::remove_dir_all(home.join(".cache")).unwrap();
        std::os::unix::fs::symlink(root.join("checkout"), home.join(".cache")).unwrap();
        assert!(run(&records, "sha256:three"), "the checkout's localci folder stays");
        assert!(tracked.join("record").exists());
    }

    #[test]
    fn a_dotnet_project_below_the_top_is_found_but_not_in_a_build_folder() {
        let repository = tempfile::tempdir().unwrap();
        assert!(!uses_nuget(repository.path()));
        std::fs::create_dir_all(repository.path().join("node_modules/pkg")).unwrap();
        std::fs::write(repository.path().join("node_modules/pkg/x.csproj"), "").unwrap();
        assert!(!uses_nuget(repository.path()), "build folders are skipped");
        std::fs::create_dir_all(repository.path().join("src/App")).unwrap();
        std::fs::write(repository.path().join("src/App/App.csproj"), "").unwrap();
        assert!(uses_nuget(repository.path()));
        let large = tempfile::tempdir().unwrap();
        for index in 0..5001 {
            std::fs::write(large.path().join(format!("file{index}.txt")), "").unwrap();
        }
        assert!(uses_nuget(large.path()), "an unfinished walk may still find a project");
    }

    #[cfg(unix)]
    #[test]
    fn a_log_never_writes_through_a_link_the_agent_left() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("someone-elses-file");
        std::fs::write(&target, "keep").unwrap();
        let log = folder.path().join("status.proof-1.log");
        std::os::unix::fs::symlink(&target, &log).unwrap();
        std::io::Write::write_all(&mut create_private(&log).unwrap(), b"proof output").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(!std::fs::symlink_metadata(&log).unwrap().file_type().is_symlink());
    }

    #[test]
    fn every_distinct_profile_build_is_proved_and_the_default_first() {
        let config = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: gpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile.cpu}\n  gpu:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n  gpu-big:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n",
        )
        .unwrap();
        let builds = distinct_builds(&config, |_| Vec::new()).unwrap();
        assert_eq!(builds.len(), 2, "two profiles share one build");
        assert_eq!(builds[0].0.dockerfile, ".horizon/Dockerfile", "the default first");
        let calls = std::cell::Cell::new(0);
        let by_arguments = distinct_builds(&config, |_| {
            calls.set(calls.get() + 1);
            vec![format!("BASE={}", calls.get())]
        })
        .unwrap();
        assert_eq!(by_arguments.len(), 4, "other build arguments make another image");
        let published = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: base\nprofiles:\n  base:\n    provider: runpod\n    image: ghcr.io/o/base\n    cpu: 4\n    memory_gb: 8\n  built:\n    provider: runpod\n    image: ghcr.io/o/r-worker\n    cpu: 4\n    memory_gb: 8\n    build: {context: .horizon, dockerfile: .horizon/Dockerfile}\n",
        )
        .unwrap();
        let builds = distinct_builds(&published, |_| Vec::new()).unwrap();
        assert_eq!(builds.len(), 1, "a published default image is pulled, not built");
    }
}
