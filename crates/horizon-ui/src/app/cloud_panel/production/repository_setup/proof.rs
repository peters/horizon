//! Horizon proves the setup agent's image itself: it builds the image, runs the worker
//! check and runs the CI jobs the agent named with localci in a container of the image,
//! then once more with `--fast` on the same volume. The agent only reads the result.
//! The timings are therefore the image's, not the agent's.
use horizon_core::cloud_runtime::Cancellation;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod localci;
mod resources;
mod session_env;
mod stream;
pub(super) use resources::{remove_run_resources, run_id, run_names};

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
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// What the agent asks Horizon to prove, from the status file's `proof`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Spec {
    pub(super) workflow: String,
    pub(super) jobs: Vec<String>,
    pub(super) event: String,
    /// Steps that throw caches away on purpose; a worker leaves them out.
    pub(super) skip: Vec<String>,
}

/// A workflow file name in `.github/workflows`: no folder, no option, YAML.
fn workflow_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    let yaml = Path::new(name)
        .extension()
        .is_some_and(|extension| extension == "yml" || extension == "yaml");
    !name.starts_with(['.', '-']) && yaml && name.bytes().all(allowed)
}

/// The most steps a proof may leave out.
const SKIP_LIMIT: usize = 20;

/// A step to leave out, which localci matches as a part of a step's name: never blank or
/// so short that it matches nearly every step, so leaving steps out cannot empty a job.
fn skip_name(step: &str) -> bool {
    let length = step.trim().chars().count();
    (3..=200).contains(&length)
}

/// A localci job: its id, `#N` for a matrix entry, never an option or a word localci
/// reads as a command.
fn job_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'#' | b':');
    !name.is_empty()
        && !name.starts_with(['-', '.', '#'])
        && !matches!(name, "help" | "version" | "list" | "plan" | "run" | "check")
        && name.bytes().all(allowed)
}

impl Spec {
    pub(super) fn parse(value: &serde_json::Value) -> Option<Self> {
        let strings = |key: &str| {
            value.get(key).and_then(serde_json::Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
        };
        let workflow = value.get("workflow")?.as_str()?.trim().to_owned();
        let jobs = strings("jobs").unwrap_or_default();
        let event = value
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("push")
            .to_owned();
        // The agent writes these and localci reads them as its arguments: a workflow is a
        // file name in .github/workflows, a job and the event are names, never options.
        let skip = strings("skip").unwrap_or_default();
        let valid = workflow_name(&workflow)
            && !jobs.is_empty()
            && jobs.iter().all(|job| job_name(job))
            && skip.len() <= SKIP_LIMIT
            && skip.iter().all(|step| skip_name(step))
            && !event.is_empty()
            && event.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_');
        valid.then_some(Self {
            workflow,
            jobs,
            event,
            skip,
        })
    }

    /// The localci arguments for this proof's JSON plan, which shows what would run.
    fn plan_args(&self) -> Vec<String> {
        let mut args = vec!["plan".to_owned(), "-w".to_owned(), self.workflow.clone()];
        args.extend(self.jobs.iter().cloned());
        args.extend(["--event".to_owned(), self.event.clone()]);
        for step in &self.skip {
            args.extend(["--skip".to_owned(), step.clone()]);
        }
        args.push("--json".to_owned());
        args
    }

    /// The localci arguments for this proof; `fast` for the warm rerun, which compares
    /// against `base`, the proved commit, since the clone's own default branch is
    /// whatever this computer's checkout last fetched.
    fn localci_args(&self, fast: bool, base: &str) -> Vec<String> {
        let mut args = vec!["run".to_owned(), "-w".to_owned(), self.workflow.clone()];
        args.extend(self.jobs.iter().cloned());
        args.extend(["--event".to_owned(), self.event.clone()]);
        for step in &self.skip {
            args.extend(["--skip".to_owned(), step.clone()]);
        }
        if fast {
            args.extend(["--fast".to_owned(), "--base".to_owned(), base.to_owned()]);
        } else {
            args.push("--cache".to_owned());
        }
        args
    }
}

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
    Started(Phase),
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
    cancel: Cancellation,
    pub(super) log: VecDeque<String>,
    pub(super) phases: [(Phase, PhaseState); 3],
    /// Where the whole log is written, for the agent to read on a failure.
    pub(super) log_path: PathBuf,
    pub(super) image: String,
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
    /// The background run, so cleanup waits until it can create nothing more.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Proof {
    /// Starts the proof of the setup in `repository`. `log_path` keeps the whole output.
    /// Starts proof number `request` of the setup in `repository`; each gets its own
    /// container, so the cleanup of an earlier one never meets it. `host_restore` lets it
    /// restore private .NET packages on this computer, which the person allowed. It
    /// begins once `after`, the run of the proof it replaces, has ended.
    pub(super) fn start(
        (repository, run, identity): (&Path, &str, super::untrusted::Identity),
        (request, spec): (u64, Spec),
        log_path: PathBuf,
        host_restore: bool,
        (after, ctx): (Option<std::thread::JoinHandle<()>>, egui::Context),
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
        let cancel = Cancellation::default();
        let (image, volume) = run_names(repository, run);
        let container = format!("{volume}-proof-{request}");
        let job = Job {
            repository: repository.to_path_buf(),
            identity,
            spec,
            image: image.clone(),
            volume,
            container: container.clone(),
            host_restore,
            setup: Err("the setup was not read yet".to_owned()),
            log_path: log_path.clone(),
            cancel: cancel.clone(),
            sender,
            logged: std::cell::Cell::new(0),
            localci: std::cell::OnceCell::new(),
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
            cancel,
            log: VecDeque::new(),
            phases: [
                (Phase::Build, PhaseState::default()),
                (Phase::Ci, PhaseState::default()),
                (Phase::Fast, PhaseState::default()),
            ],
            log_path,
            image,
            unreported: false,
            container,
            inputs: None,
            host_restore,
            worker: Some(worker),
        }
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
        let container = self.container.clone();
        super::run::keep_ending(std::thread::spawn(move || {
            let _ = Command::new("docker")
                .args(["rm", "--force", &container])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
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
                Event::Started(phase) => self.phase_mut(phase).started = Some(Instant::now()),
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
        if let Some((phase, state)) = self.phases.iter().find(|(_, state)| state.failure.is_some()) {
            return format!(
                "Horizon proof: {} failed: {}. The whole log is in {}. Fix the Dockerfile or the proof, then increase proof_request by one.",
                phase.title(),
                state.failure.as_deref().unwrap_or_default(),
                self.log_path.display()
            );
        }
        format!(
            "Horizon proof passed: image {} built and checked in {}, CI jobs passed in {}, fast rerun in {}. Use these times in your summary. Now do Desktop preview with image {}.",
            self.image,
            time(Phase::Build),
            time(Phase::Ci),
            time(Phase::Fast),
            self.image
        )
    }
}

impl Drop for Proof {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.remove_container();
    }
}

struct Job {
    repository: PathBuf,
    /// Which folder the repository was when the run started.
    identity: super::untrusted::Identity,
    spec: Spec,
    image: String,
    volume: String,
    container: String,
    host_restore: bool,
    /// The setup this proof tests, read once at its start, or why Horizon refuses it.
    setup: Result<super::snapshot::Setup, String>,
    log_path: PathBuf,
    cancel: Cancellation,
    sender: mpsc::SyncSender<Event>,
    /// Bytes written to the whole log so far, against [`LOG_FILE_LIMIT`].
    logged: std::cell::Cell<u64>,
    /// Horizon's own localci for the CI, fetched once per proof.
    localci: std::cell::OnceCell<Result<(tempfile::TempDir, PathBuf), String>>,
    ctx: egui::Context,
}

impl Job {
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
        self.setup = super::snapshot::Setup::read(&self.repository, self.identity);
        if let Ok(setup) = &self.setup {
            let _ = self.sender.send(Event::Inputs(setup.fingerprint(&self.spec)));
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
        // An earlier proof of this repository may still be stopping: its container goes.
        let earlier = Command::new("docker")
            .args(["ps", "--all", "--quiet", "--filter"])
            .arg(format!("name=^/{}-proof-", self.volume))
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !earlier.is_empty() {
            let _ = Command::new("docker").args(["rm", "--force"]).args(&earlier).output();
        }
        for phase in [Phase::Build, Phase::Ci, Phase::Fast] {
            if self.cancel.is_cancelled() {
                return;
            }
            let _ = self.sender.send(Event::Started(phase));
            self.ctx.request_repaint();
            let result = match phase {
                Phase::Build => match &self.setup {
                    Err(reason) => Err(format!("Horizon does not read this setup: {reason}")),
                    Ok(setup) => self.build(setup, &mut log),
                },
                Phase::Ci => self.prepare(&mut log).and_then(|()| self.check_plan()).and_then(|()| {
                    self.warm_packages(&mut log);
                    self.ci(false, &mut log)
                }),
                Phase::Fast => self.ci(true, &mut log),
            };
            let failed = result.is_err();
            let _ = self.sender.send(Event::Finished(phase, result));
            self.ctx.request_repaint();
            if failed {
                return;
            }
        }
    }

    fn build(&self, setup: &super::snapshot::Setup, log: &mut Option<std::fs::File>) -> Result<(), String> {
        resources::local_docker()?;
        let config = setup.config()?;
        // Deployment layers each same-worker companion's recipe on the image and checks
        // the layered result; this proof builds only the profiles' own images, so it
        // does not call such a setup proved.
        if let Some((name, _)) = config.same_worker_siblings().next() {
            return Err(format!(
                "the setup declares the same-worker companion {name}; Horizon cannot prove same-worker companions yet, so leave them out of this setup"
            ));
        }
        // The agent CLI releases deployment would pin, so every build gets its arguments.
        let releases =
            horizon_core::cloud_runtime::image::agents::latest(&horizon_core::cloud_runtime::Cancellation::default())
                .map_err(|error| format!("cannot look up the agent CLI releases: {error}"))?;
        let builds = distinct_builds(&config, |profile| {
            horizon_core::cloud_runtime::image::build_arguments(&profile.capabilities, &releases)
        })?;
        // Only Horizon's own ghcr.io login, never this computer's credential helpers: a
        // private ghcr.io base pulls with it, and every other image anonymously.
        let horizon_docker = horizon_core::HorizonHome::resolve().root().join("cloud").join("docker");
        let (docker_config, signed_in) = horizon_core::cloud_runtime::github::publish::build_config(&horizon_docker)
            .map_err(|error| error.to_string())?;
        let _ = self.sender.try_send(Event::Line(if signed_in {
            "Pulling ghcr.io images with Horizon's GitHub login.".to_owned()
        } else {
            "No Horizon GitHub login for ghcr.io; images are pulled anonymously.".to_owned()
        }));
        // Every profile can be chosen in New cloud, so each distinct build is built; the
        // CI then runs in the default profile's image.
        let tag = |index: usize| {
            if index == 0 {
                self.image.clone()
            } else {
                format!("{}-build{index}:candidate", self.volume)
            }
        };
        for (index, (build, arguments)) in builds.iter().enumerate() {
            self.build_one(setup, (build, arguments), &tag(index), docker_config.path(), log)?;
        }
        // The proof runs Horizon's own localci, but the agents on the worker run the
        // image's: it must be the same pinned release.
        let run = |command| self.capture(command, STEP_TIMEOUT);
        localci::check_installed(
            &self.image,
            &format!("{}-localci", self.container),
            self.trusted_localci()?,
            &run,
        )?;
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
                if !trusted {
                    let mut pull = Command::new("docker");
                    pull.env_remove("DOCKER_AUTH_CONFIG")
                        .arg("--config")
                        .arg(docker_config.path())
                        .args(["pull", "--platform", "linux/amd64", &profile.image]);
                    self.stream_within(pull, log, Some(BUILD_TIMEOUT))?;
                }
                self.check_contract((name, profile), &profile.image, number, docker_config.path(), log)?;
                continue;
            };
            let arguments = horizon_core::cloud_runtime::image::build_arguments(&profile.capabilities, &releases);
            let index = builds
                .iter()
                .position(|(built, with)| built == build && *with == arguments)
                .ok_or_else(|| format!("profile {name} has no proof image"))?;
            self.check_contract((name, profile), &tag(index), number, docker_config.path(), log)?;
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
            docker_host: None,
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
        let mut command = Command::new("docker");
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
        let bundle = folder.path().join("head.bundle");
        let mut create = super::commit::git_command(&self.repository);
        create.args(["bundle", "create", "--quiet"]).arg(&bundle).arg("HEAD");
        self.stream(create, log)?;
        // Horizon's pinned base image, not the candidate, prepares the volume: what the
        // CI later runs on is the bundled commit, whatever the candidate's tools do.
        let mut own = Command::new("docker");
        own.args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg("mkdir -p /workspace/home && chown 10001:10001 /workspace /workspace/home");
        self.stream(own, log)?;
        // As root, so the checkout can be handed to the worker's account after.
        let mut clone = Command::new("docker");
        clone
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:/src.bundle:ro", bundle.display()))
            .arg("-e")
            .arg(format!("HEAD_SHA={head}"))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg(CLONE);
        let result = self.stream(clone, log);
        drop(folder);
        result?;
        self.hydrate(head, log)
    }

    /// Fills the checkout's submodules and Git LFS files as `actions/checkout` would and
    /// deployment does: when the commit has submodules or LFS files, deployment's own
    /// snapshot of it, made on this computer, goes over the checkout in the volume.
    fn hydrate(&self, head: &str, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let git = |args: &[&str]| {
            super::commit::git_command(&self.repository)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
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
        let mut overlay = Command::new("docker");
        overlay
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:/snapshot:ro", tree.display()))
            .args(["--entrypoint", "sh", TOOLS, "-c"])
            .arg("cp -a /snapshot/. /workspace/checkout/ && chown -R 10001:10001 /workspace/checkout");
        self.stream(overlay, log)
    }

    /// Restores a .NET repository's packages on this computer first, with its own feed
    /// credentials, so the container finds private packages in the read-only fallback
    /// folder and no credential ever enters it. Best effort: a failure here shows up in
    /// the CI step instead.
    fn warm_packages(&self, log: &mut Option<std::fs::File>) {
        // It runs the repository's MSBuild files here, with this computer's feed logins,
        // so only when the person allowed it in the dialog.
        if !self.host_restore || !uses_nuget(&self.repository) {
            return;
        }
        let mut restore = Command::new("dotnet");
        restore
            .current_dir(&self.repository)
            .args(["restore", "--nologo", "-v", "quiet"])
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1");
        let _ = self.stream(restore, log);
    }

    /// Whether the default profile, whose image the CI runs in, asks for a GPU.
    fn wants_gpu(&self) -> bool {
        self.setup
            .as_ref()
            .ok()
            .and_then(|setup| setup.config().ok())
            .and_then(|config| config.profiles.get(&config.default).map(|profile| profile.gpu))
            .unwrap_or(false)
    }

    /// Horizon's own localci for the image's platform, fetched and checked once.
    fn trusted_localci(&self) -> Result<&Path, String> {
        self.localci
            .get_or_init(|| {
                let platform = self
                    .setup
                    .as_ref()
                    .ok()
                    .and_then(|setup| setup.config().ok())
                    .and_then(|config| {
                        let build = config.profiles.get(&config.default)?.build.as_ref()?;
                        Some(build.platform.clone())
                    })
                    .unwrap_or_else(|| "linux/amd64".to_owned());
                localci::trusted(&platform)
            })
            .as_ref()
            .map(|(_, binary)| binary.as_path())
            .map_err(Clone::clone)
    }

    /// Checks with the trusted localci, before the CI runs, that the proof keeps a step
    /// that runs a command in every job it selects.
    fn check_plan(&self) -> Result<(), String> {
        let mut command = self.ci_container()?;
        command
            .args(["--entrypoint", localci::MOUNTED, &self.image])
            .args(self.spec.plan_args());
        let plan = self.capture(command, STEP_TIMEOUT)?;
        localci::check_plan(&plan)
    }

    /// A container of the image as the worker account on the proof's volume, with the
    /// GPU, this computer's packages and Horizon's localci that the proof needs.
    fn ci_container(&self) -> Result<Command, String> {
        let localci = self.trusted_localci()?;
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "--name", &self.container, "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:{}:ro", localci.display(), localci::MOUNTED))
            .args(["-w", "/workspace/checkout", "-e", "HOME=/workspace/home"]);
        // A GPU profile is proved with a GPU or not at all: CI that passes without one
        // says nothing about the worker that has one.
        if self.wants_gpu() {
            if !Path::new("/dev/nvidiactl").exists() {
                return Err(
                    "the default profile needs a GPU and this computer has no NVIDIA GPU, so Horizon cannot prove it here"
                        .to_owned(),
                );
            }
            command.args(["--gpus", "all"]);
        }
        // Packages this computer already restored, private ones included, need no feed
        // credential: NuGet reads them as a fallback folder before it asks a source. Only
        // with the person's consent, since the repository's CI could read every one.
        if self.host_restore
            && uses_nuget(&self.repository)
            && let Some(packages) = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".nuget/packages"))
            && packages.is_dir()
        {
            command
                .arg("-v")
                .arg(format!("{}:/opt/host-nuget:ro", packages.display()))
                .args(["-e", "RestoreAdditionalProjectFallbackFolders=/opt/host-nuget"]);
        }
        Ok(command)
    }

    /// Runs the CI jobs: the container starts Horizon's localci itself, with the
    /// image's session environment read beforehand, so no program of the candidate runs
    /// before it; the steps then run in the image as the worker would run them.
    fn ci(&self, fast: bool, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let head = self.head().unwrap_or_default();
        let run = |command| self.capture(command, STEP_TIMEOUT);
        let session = session_env::arguments(&self.image, &format!("{}-env", self.container), &run)?;
        if !fast {
            self.forget_older_records(&session, log)?;
        }
        let mut command = self.ci_container()?;
        command
            .args(&session)
            .args(["--entrypoint", localci::MOUNTED, &self.image])
            .args(self.spec.localci_args(fast, head));
        self.stream_within(command, log, Some(CI_TIMEOUT))
    }

    /// localci's step records do not know the image: before a full run after a rebuild
    /// Horizon's base image removes them from the volume, so no job passes on a record
    /// an older image left.
    fn forget_older_records(&self, session: &[String], log: &mut Option<std::fs::File>) -> Result<(), String> {
        let mut forget = Command::new("docker");
        forget
            .args(["run", "--rm", "--name", &self.container, "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["-e", "HOME=/workspace/home"])
            .args(session)
            .arg("-e")
            .arg(format!("HORIZON_PROOF_IMAGE={}", self.image_id()))
            .args(["--entrypoint", "sh", TOOLS, "-c", FORGET_OLDER_IMAGE]);
        self.stream(forget, log)
    }

    /// The built image's ID, or its tag when Docker cannot say, asked within the
    /// proof's time limit.
    fn image_id(&self) -> String {
        let mut inspect = Command::new("docker");
        inspect.args(["image", "inspect", "--format", "{{.Id}}", &self.image]);
        self.capture(inspect, STEP_TIMEOUT)
            .ok()
            .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| self.image.clone())
    }

    fn remove_container(&self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
}

/// Fetches the tested commit from its bundle into the volume's checkout and checks it
/// out, then removes everything untracked. It runs as root in Horizon's base image, so
/// it starts from a fresh git folder with no global settings and no hooks: an earlier
/// proof's CI could have written to the checkout's .git and to HOME, and none of that
/// may run here.
const CLONE: &str = "set -e; export HOME=/tmp/horizon-clone GIT_CONFIG_GLOBAL=/dev/null; mkdir -p \"$HOME\"; \
git config --system --add safe.directory '*'; mkdir -p /workspace/checkout; cd /workspace/checkout; \
rm -rf .git; git init -q; G='git -c core.hooksPath=/dev/null -c core.fsmonitor=false'; \
$G fetch -q /src.bundle HEAD; $G checkout -q --force --detach \"$HEAD_SHA\"; $G clean -q -ffdx; \
chown -R 10001:10001 /workspace/checkout";

/// Forgets localci's step records when the volume last ran a proof in another image.
const FORGET_OLDER_IMAGE: &str = "marker=\"$HOME/.horizon-proof-image\"; \
if [ \"$(cat \"$marker\" 2>/dev/null)\" != \"$HORIZON_PROOF_IMAGE\" ]; then \
rm -rf \"${XDG_CACHE_HOME:-$HOME/.cache}/localci\"; printf '%s' \"$HORIZON_PROOF_IMAGE\" > \"$marker\"; fi;";

/// The setup agent's instruction for the localci the image must hold.
pub(super) fn localci_instruction() -> String {
    localci::instruction()
}

/// The image that prepares the proof's volume: Horizon's pinned base, never the
/// candidate the agent wrote.
const TOOLS: &str = horizon_core::cloud_runtime::repository::launch::quick_start::IMAGE;

/// The images deployment would build: each profile's build with the build arguments
/// its capabilities give, the default profile's first, every other one that differs
/// from all before it. A profile without a build section runs a published image.
fn distinct_builds(
    config: &horizon_core::cloud_panel::CloudConfig,
    arguments: impl Fn(&horizon_core::cloud_panel::Profile) -> Vec<String>,
) -> Result<Vec<(horizon_core::cloud_panel::Build, Vec<String>)>, String> {
    let default = config
        .profiles
        .get(&config.default)
        .ok_or("the default profile is missing")?;
    let build = default
        .build
        .clone()
        .ok_or("the default profile has no build section")?;
    let mut builds = vec![(build, arguments(default))];
    for profile in config.profiles.values() {
        if let Some(build) = &profile.build {
            let candidate = (build.clone(), arguments(profile));
            if !builds.contains(&candidate) {
                builds.push(candidate);
            }
        }
    }
    Ok(builds)
}

/// Whether the repository restores .NET packages, possibly from private feeds: a package
/// configuration, or a solution or project file near the top. A bounded walk that never
/// follows links and skips hidden and build folders.
pub(super) fn uses_nuget(repository: &Path) -> bool {
    const CONFIGS: [&str; 3] = ["nuget.config", "NuGet.Config", "Directory.Packages.props"];
    const PROJECTS: [&str; 5] = ["sln", "slnx", "csproj", "fsproj", "vbproj"];
    const SKIPPED: [&str; 5] = ["node_modules", "target", "bin", "obj", "vendor"];
    let mut pending = vec![(repository.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((folder, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 5000 {
                return false;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < 3 && !name.starts_with('.') && !SKIPPED.contains(&name.as_ref()) {
                    pending.push((entry.path(), depth + 1));
                }
            } else if kind.is_file()
                && (CONFIGS.contains(&name.as_ref())
                    || Path::new(name.as_ref())
                        .extension()
                        .is_some_and(|extension| PROJECTS.iter().any(|project| extension == *project)))
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_names_a_workflow_and_jobs() {
        let spec = Spec::parse(&serde_json::json!({
            "workflow": "dotnet-core.yml",
            "jobs": ["setup", "test"],
            "skip": ["Use isolated NuGet caches"]
        }))
        .unwrap();
        assert_eq!(spec.event, "push", "push by default");
        assert_eq!(
            spec.localci_args(true, "abc123"),
            [
                "run",
                "-w",
                "dotnet-core.yml",
                "setup",
                "test",
                "--event",
                "push",
                "--skip",
                "Use isolated NuGet caches",
                "--fast",
                "--base",
                "abc123"
            ]
        );
        assert!(Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": []})).is_none());
        assert!(Spec::parse(&serde_json::json!({"workflow": "../x.yml", "jobs": ["a"]})).is_none());
        for job in ["--help", "help", "version", "-k"] {
            assert!(
                Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": [job]})).is_none(),
                "{job} would end localci without a job"
            );
        }
        for workflow in ["/etc/ci.yml", "sub/ci.yml", "-w.yml", "ci.txt"] {
            assert!(Spec::parse(&serde_json::json!({"workflow": workflow, "jobs": ["a"]})).is_none());
        }
        assert!(Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["rust-test#2"]})).is_some());
        for skip in ["", "   ", "t"] {
            assert!(
                Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"], "skip": [skip]})).is_none(),
                "{skip:?} would leave out every step"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_new_image_forgets_the_step_records_an_older_one_left() {
        let home = tempfile::tempdir().unwrap();
        let records = home.path().join(".cache/localci");
        let run = |image: &str| {
            std::fs::create_dir_all(&records).unwrap();
            std::fs::write(records.join("record"), "passed").unwrap();
            let status = Command::new("bash")
                .args(["-c", FORGET_OLDER_IMAGE])
                .env("HOME", home.path())
                .env_remove("XDG_CACHE_HOME")
                .env("HORIZON_PROOF_IMAGE", image)
                .status()
                .unwrap();
            assert!(status.success());
            records.join("record").exists()
        };
        assert!(!run("sha256:one"), "the first proof in a volume starts clean");
        assert!(run("sha256:one"), "the same image keeps its records");
        assert!(!run("sha256:two"), "a rebuilt image forgets them");
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
    }
}
