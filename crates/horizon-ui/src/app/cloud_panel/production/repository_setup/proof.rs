//! Horizon proves the setup agent's image itself: it builds the image, runs the worker
//! check and runs the CI jobs the agent named with localci in a container of the image,
//! then once more with `--fast` on the same volume. The agent only reads the result.
//! The timings are therefore the image's, not the agent's.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The step titles Horizon owns once the agent asks for a proof.
pub(super) const BUILD: &str = "Build image";
pub(super) const CI: &str = "Run CI in image";
pub(super) const FAST: &str = "Fast rerun";

/// Lines of proof output the dialog keeps.
const LOG_LINES: usize = 200;

/// The longest output line Horizon keeps; the rest of a longer line is dropped.
const LINE_LIMIT: usize = 16 * 1024;

/// Output lines waiting between a command's pipes and the proof.
const LINE_QUEUE: usize = 256;

/// Events waiting for the dialog; output lines beyond it are dropped, phases wait.
const EVENT_QUEUE: usize = 1024;

/// The most a proof writes to its whole log.
const LOG_FILE_LIMIT: u64 = 64 * 1024 * 1024;

/// Reads one line of untrusted output into `line`, keeping at most [`LINE_LIMIT`]
/// bytes of it, and returns how many bytes it consumed.
fn read_bounded_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut consumed = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(consumed);
        }
        let (chunk, done) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(end) => (&buffer[..=end], true),
            None => (buffer, false),
        };
        let room = LINE_LIMIT.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let used = chunk.len();
        reader.consume(used);
        consumed += used;
        if done {
            return Ok(consumed);
        }
    }
}

/// The longest an image build may take before the proof reports it hung.
const BUILD_TIMEOUT: Duration = Duration::from_mins(45);

/// The longest the CI jobs may run in the image before the proof reports them hung.
const CI_TIMEOUT: Duration = Duration::from_mins(90);

/// The longest the worker check may take; it only reads the image.
const CHECK_TIMEOUT: Duration = Duration::from_mins(5);

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
        let valid = workflow_name(&workflow)
            && !jobs.is_empty()
            && jobs.iter().all(|job| job_name(job))
            && !event.is_empty()
            && event.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_');
        valid.then(|| Self {
            workflow,
            jobs,
            event,
            skip: strings("skip").unwrap_or_default(),
        })
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
    cancel: Arc<AtomicBool>,
    pub(super) log: VecDeque<String>,
    pub(super) phases: [(Phase, PhaseState); 3],
    /// Where the whole log is written, for the agent to read on a failure.
    pub(super) log_path: PathBuf,
    pub(super) image: String,
    /// The proof ended, passed or not, and the agent has not been told yet.
    pub(super) unreported: bool,
    /// The proof's container, so a stopped or replaced proof also stops its run.
    container: String,
    /// The fingerprint of the setup and request this proof tests.
    pub(super) inputs: [u8; 32],
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
        (repository, run): (&Path, &str),
        (request, spec): (u64, Spec),
        log_path: PathBuf,
        host_restore: bool,
        (after, ctx): (Option<std::thread::JoinHandle<()>>, egui::Context),
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE);
        let cancel = Arc::new(AtomicBool::new(false));
        // One snapshot of the setup: the proof fingerprints it and builds from it alone.
        // A setup Horizon refuses to read fails the build with the reason.
        let setup = super::snapshot::Setup::read(repository);
        let inputs = setup.as_ref().map_or([0; 32], |setup| setup.fingerprint(&spec));
        let (image, volume) = run_names(repository, run);
        let container = format!("{volume}-proof-{request}");
        let job = Job {
            repository: repository.to_path_buf(),
            spec,
            image: image.clone(),
            volume,
            container: container.clone(),
            host_restore,
            setup,
            log_path: log_path.clone(),
            cancel: cancel.clone(),
            sender,
            logged: std::cell::Cell::new(0),
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
            inputs,
            worker: Some(worker),
        }
    }

    /// Stops the proof and hands back its run, to wait for before its resources go.
    pub(super) fn stop(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.cancel.store(true, Ordering::Relaxed);
        self.remove_container();
        self.worker.take()
    }

    fn remove_container(&self) {
        // Killing the docker client leaves its container running; the run goes too.
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
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
        self.cancel.store(true, Ordering::Relaxed);
        self.remove_container();
    }
}

struct Job {
    repository: PathBuf,
    spec: Spec,
    image: String,
    volume: String,
    container: String,
    host_restore: bool,
    /// The setup this proof tests, read once at its start, or why Horizon refuses it.
    setup: Result<super::snapshot::Setup, String>,
    log_path: PathBuf,
    cancel: Arc<AtomicBool>,
    sender: mpsc::SyncSender<Event>,
    /// Bytes written to the whole log so far, against [`LOG_FILE_LIMIT`].
    logged: std::cell::Cell<u64>,
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

    fn run(self) {
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
            .arg(format!("name=^{}-proof-", self.volume))
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
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = self.sender.send(Event::Started(phase));
            self.ctx.request_repaint();
            let result = match phase {
                Phase::Build => match &self.setup {
                    Err(reason) => Err(format!("Horizon does not read this setup: {reason}")),
                    Ok(setup) => self.build(setup, &mut log),
                },
                Phase::Ci => self.prepare(&mut log).and_then(|()| {
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
        let config = setup.config()?;
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
        // Every profile can be chosen in New cloud, so each distinct build is built and
        // checked; the CI then runs in the default profile's image.
        for (index, (build, arguments)) in builds.iter().enumerate() {
            let tag = if index == 0 {
                self.image.clone()
            } else {
                format!("{}-build{index}:candidate", self.volume)
            };
            self.build_one(setup, (build, arguments), &tag, docker_config.path(), log)?;
        }
        Ok(())
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
        self.stream_within(command, log, Some(BUILD_TIMEOUT))?;
        // Named, so a check that hangs goes with its container on the timeout.
        let mut check = Command::new("docker");
        check.args([
            "run",
            "--rm",
            "--name",
            &self.container,
            "--network",
            "none",
            "--entrypoint",
        ]);
        check.arg("horizon-worker-check").arg(tag).arg("--git-auth");
        self.stream_within(check, log, Some(CHECK_TIMEOUT))
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
        let mut own = Command::new("docker");
        own.args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", &self.image, "-c"])
            .arg("mkdir -p /workspace/home && chown 10001:10001 /workspace /workspace/home");
        self.stream(own, log)?;
        // As root, so the checkout can be handed to the worker's account after.
        let mut clone = Command::new("docker");
        clone
            .args(["run", "--rm", "--name", &self.container, "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{}:/src.bundle:ro", bundle.display()))
            .args(["-e", "HOME=/workspace/home", "-e"])
            .arg(format!("HEAD_SHA={head}"))
            .args(["--entrypoint", "sh", &self.image, "-c"])
            .arg(CLONE);
        let result = self.stream(clone, log);
        drop(folder);
        result
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

    /// Whether the default profile asks for a GPU and this computer has an NVIDIA one;
    /// Docker refuses `--gpus` without it, and that is no fault of the Dockerfile.
    fn gpu(&self) -> bool {
        let wanted = self
            .setup
            .as_ref()
            .ok()
            .and_then(|setup| setup.config().ok())
            .and_then(|config| config.profiles.get(&config.default).map(|profile| profile.gpu))
            .unwrap_or(false);
        wanted && Path::new("/dev/nvidiactl").exists()
    }

    fn ci(&self, fast: bool, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let head = self.head().unwrap_or_default();
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "--name", &self.container, "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["-w", "/workspace/checkout", "-e", "HOME=/workspace/home"]);
        if self.gpu() {
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
        // localci's step records do not know the image: after a rebuild the full run
        // forgets them, so no job passes on a record an older image left.
        let forget = if fast {
            ""
        } else {
            command
                .arg("-e")
                .arg(format!("HORIZON_PROOF_IMAGE={}", self.image_id()));
            FORGET_OLDER_IMAGE
        };
        let script = format!(
            "{SESSION_ENV} {forget} exec localci {}",
            self.spec
                .localci_args(fast, head)
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        );
        command.args(["--entrypoint", "bash", &self.image, "-c", &script]);
        self.stream_within(command, log, Some(CI_TIMEOUT))
    }

    /// The built image's ID, or its tag when Docker cannot say.
    fn image_id(&self) -> String {
        Command::new("docker")
            .args(["image", "inspect", "--format", "{{.Id}}", &self.image])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
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

    /// Runs `command`, sending each output line to the dialog and the log file.
    fn stream(&self, command: Command, log: &mut Option<std::fs::File>) -> Result<(), String> {
        self.stream_within(command, log, None)
    }

    /// As [`Self::stream`], stopping the command once it has run for `limit`.
    fn stream_within(
        &self,
        mut command: Command,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
    ) -> Result<(), String> {
        use std::io::Write;
        let started = Instant::now();
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", command.get_program().to_string_lossy()))?;
        // Bounded, so output faster than Horizon writes it waits in the pipe instead.
        let (lines, received) = mpsc::sync_channel::<String>(LINE_QUEUE);
        for pipe in [
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = lines.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut bytes = Vec::new();
                // Bytes that are not UTF-8 become replacement characters, not the end of the log.
                while read_bounded_line(&mut reader, &mut bytes).is_ok_and(|read| read > 0) {
                    let line = String::from_utf8_lossy(&bytes);
                    if lines.send(line.trim_end_matches(['\n', '\r']).to_owned()).is_err() {
                        break;
                    }
                    bytes.clear();
                }
            });
        }
        drop(lines);
        loop {
            match received.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    if let Some(file) = log.as_mut() {
                        let logged = self.logged.get();
                        if logged < LOG_FILE_LIMIT {
                            let _ = writeln!(file, "{line}");
                            self.logged.set(logged + line.len() as u64 + 1);
                            if self.logged.get() >= LOG_FILE_LIMIT {
                                let _ = writeln!(file, "[Horizon stopped writing this log at {LOG_FILE_LIMIT} bytes]");
                            }
                        }
                    }
                    // The dialog shows the last lines only; a full queue drops one.
                    let _ = self.sender.try_send(Event::Line(line));
                    self.ctx.request_repaint();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if self.cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                self.remove_container();
                return Err("stopped".to_owned());
            }
            if limit.is_some_and(|limit| started.elapsed() > limit) {
                let _ = child.kill();
                let _ = child.wait();
                // Killing the docker client leaves its container running.
                self.remove_container();
                return Err(format!(
                    "still running after {} minutes; stopped",
                    started.elapsed().as_secs() / 60
                ));
            }
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "exit {} after {} s",
                status.code().unwrap_or(-1),
                started.elapsed().as_secs()
            ))
        }
    }
}

/// Fetches the tested commit from its bundle into the volume's checkout and checks it
/// out; build output stays for the fast rerun. It runs as root, so its Git settings go
/// to the container's system file, not the shared home.
const CLONE: &str = "set -e; git config --system --add safe.directory '*'; \
mkdir -p /workspace/checkout; cd /workspace/checkout; [ -d .git ] || git init -q; \
git fetch -q /src.bundle HEAD; git checkout -q --force --detach \"$HEAD_SHA\"; git clean -q -ffdx; \
chown -R 10001:10001 /workspace/checkout";

/// Forgets localci's step records when the volume last ran a proof in another image.
const FORGET_OLDER_IMAGE: &str = "marker=\"$HOME/.horizon-proof-image\"; \
if [ \"$(cat \"$marker\" 2>/dev/null)\" != \"$HORIZON_PROOF_IMAGE\" ]; then \
rm -rf \"${XDG_CACHE_HOME:-$HOME/.cache}/localci\"; printf '%s' \"$HORIZON_PROOF_IMAGE\" > \"$marker\"; fi;";

/// Applies the image's session environment, as a worker session gets it.
const SESSION_ENV: &str = "for f in /etc/horizon-worker/session-env.d/*.env; do [ -f \"$f\" ] || continue; \
while IFS= read -r line || [ -n \"$line\" ]; do case \"$line\" in ''|'#'*) ;; \
PATH_PREPEND=*) PATH=\"${line#PATH_PREPEND=}:$PATH\";; PATH_APPEND=*) PATH=\"$PATH:${line#PATH_APPEND=}\";; \
*=*) export \"$line\";; esac; done < \"$f\"; done; export PATH;";

/// The image and volume of one setup run's proofs: unique to the run, so two runs or
/// two Horizon processes on the same checkout never share or remove each other's.
pub(super) fn run_names(repository: &Path, run: &str) -> (String, String) {
    let volume = format!("horizon-setup-{}-{run}", slug(repository));
    (format!("{volume}:candidate"), volume)
}

/// A short name for the setup run whose status file is `status`.
pub(super) fn run_id(status: &Path) -> String {
    let folder = status.parent().unwrap_or(status).to_string_lossy();
    let hash = folder.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{:08x}", hash & 0xffff_ffff)
}

/// Removes a finished run's containers, image and volume, in the background, once
/// `after`, the run's stopped proof, has ended and can create nothing more.
pub(super) fn remove_run_resources(repository: &Path, run: &str, after: Option<std::thread::JoinHandle<()>>) {
    let (image, volume) = run_names(repository, run);
    std::thread::spawn(move || {
        if let Some(after) = after {
            let _ = after.join();
        }
        let containers = Command::new("docker")
            .args(["ps", "--all", "--quiet", "--filter"])
            .arg(format!("name=^{volume}-"))
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !containers.is_empty() {
            let _ = Command::new("docker")
                .args(["rm", "--force"])
                .args(&containers)
                .output();
        }
        let images = Command::new("docker")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter"])
            .arg(format!("reference={volume}-build*"))
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let _ = Command::new("docker")
            .args(["rmi", "--force", &image])
            .args(&images)
            .output();
        let _ = Command::new("docker")
            .args(["volume", "rm", "--force", &volume])
            .output();
    });
}

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

/// Whether the repository restores .NET packages, possibly from private feeds.
pub(super) fn uses_nuget(repository: &Path) -> bool {
    ["nuget.config", "NuGet.Config", "Directory.Packages.props"]
        .iter()
        .any(|file| repository.join(file).is_file())
}

/// A name Docker accepts for the repository: its folder name and a short hash of its
/// path, so two checkouts with the same folder name never share an image or a volume.
fn slug(repository: &Path) -> String {
    let hash = repository
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{}-{:06x}", folder_slug(repository), hash & 0xff_ffff)
}

fn folder_slug(repository: &Path) -> String {
    let name = repository
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "repository".to_owned()
    } else {
        slug.to_owned()
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
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
    }

    #[test]
    fn the_image_and_volume_take_a_name_docker_accepts() {
        assert_eq!(folder_slug(Path::new("/home/me/YouPay v2")), "youpay-v2");
        assert_eq!(folder_slug(Path::new("/")), "repository");
        let (a, b) = (slug(Path::new("/home/a/app")), slug(Path::new("/home/b/app")));
        assert!(a.starts_with("app-") && b.starts_with("app-"));
        assert_ne!(a, b, "two checkouts named app keep their own image and volume");
        assert_eq!(a, slug(Path::new("/home/a/app")), "the same path keeps its volume");
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
    fn an_endless_output_line_is_kept_short() {
        let mut input = std::io::Cursor::new([vec![b'x'; LINE_LIMIT * 3], b"\nnext\n".to_vec()].concat());
        let mut line = Vec::new();
        assert_eq!(read_bounded_line(&mut input, &mut line).unwrap(), LINE_LIMIT * 3 + 1);
        assert_eq!(line.len(), LINE_LIMIT, "the rest of the line is dropped");
        line.clear();
        read_bounded_line(&mut input, &mut line).unwrap();
        assert_eq!(line, b"next\n", "the next line reads whole");
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

    #[test]
    fn each_setup_run_names_its_own_image_and_volume() {
        let repository = Path::new("/home/me/app");
        let (first, second) = (
            run_id(Path::new("/tmp/horizon-cloud-setup-1-10/status.json")),
            run_id(Path::new("/tmp/horizon-cloud-setup-1-11/status.json")),
        );
        assert_ne!(first, second);
        let (image, volume) = run_names(repository, &first);
        assert_eq!(image, format!("{volume}:candidate"));
        assert_ne!(
            volume,
            run_names(repository, &second).1,
            "two runs never share a volume"
        );
    }

    #[test]
    fn quoted_arguments_survive_the_shell() {
        assert_eq!(shell_quote("Use isolated NuGet caches"), "'Use isolated NuGet caches'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
