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

/// The longest an image build may take before the proof reports it hung.
const BUILD_TIMEOUT: Duration = Duration::from_mins(45);

/// The longest the CI jobs may run in the image before the proof reports them hung.
const CI_TIMEOUT: Duration = Duration::from_mins(90);

/// The longest the worker check may take; it only reads the image.
const CHECK_TIMEOUT: Duration = Duration::from_mins(5);

/// What a proof tests: every file in `.horizon`, the commit and the proof request, so
/// a passed proof says nothing about a setup that changed after it. SHA-256 over framed
/// records (kind, path, mode and content, each with its length), so no two setups read
/// as the same bytes.
pub(super) fn fingerprint(repository: &Path, spec: &Spec) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut entries: Vec<(String, std::fs::DirEntry)> = Vec::new();
    let mut pending = vec![repository.join(".horizon")];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()) {
                pending.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(repository)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            entries.push((relative, entry));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    let mut record = |kind: &[u8], bytes: &[u8]| {
        hash.update(kind);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    };
    record(b"spec", format!("{spec:?}").as_bytes());
    // The commit too: a context outside .horizon builds from the committed tree.
    let head = super::commit::git_command(repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|output| output.stdout)
        .unwrap_or_default();
    record(b"head", &head);
    for (relative, entry) in &entries {
        record(b"path", relative.as_bytes());
        // A link counts by its target's name, never followed: no loop, nothing outside.
        match entry.file_type() {
            Ok(kind) if kind.is_symlink() => {
                let target = std::fs::read_link(entry.path()).unwrap_or_default();
                record(b"link", target.to_string_lossy().as_bytes());
            }
            _ => {
                record(b"mode", if executable(entry) { b"755" } else { b"644" });
                record(b"file", &std::fs::read(entry.path()).unwrap_or_default());
            }
        }
    }
    hash.finalize().into()
}

#[cfg(unix)]
fn executable(entry: &std::fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    entry
        .metadata()
        .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_: &std::fs::DirEntry) -> bool {
    false
}

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
        if workflow.is_empty() || workflow.contains("..") || jobs.is_empty() {
            return None;
        }
        Some(Self {
            workflow,
            jobs,
            event: value
                .get("event")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("push")
                .to_owned(),
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
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let inputs = fingerprint(repository, &spec);
        let (image, volume) = run_names(repository, run);
        let container = format!("{volume}-proof-{request}");
        let job = Job {
            repository: repository.to_path_buf(),
            spec,
            image: image.clone(),
            volume,
            container: container.clone(),
            host_restore,
            log_path: log_path.clone(),
            cancel: cancel.clone(),
            sender,
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
    log_path: PathBuf,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl Job {
    fn run(self) {
        let mut log = create_private(&self.log_path).ok();
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
                Phase::Build => self.build(&mut log),
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

    fn build(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let config = std::fs::read_to_string(self.repository.join(".horizon/cloud.yml"))
            .map_err(|error| format!("cannot read .horizon/cloud.yml: {error}"))?;
        let config = horizon_core::cloud_panel::CloudConfig::parse(&config).map_err(|error| error.to_string())?;
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
        let _ = self.sender.send(Event::Line(if signed_in {
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
            self.build_one((build, arguments), &tag, docker_config.path(), log)?;
        }
        Ok(())
    }

    fn build_one(
        &self,
        (build, arguments): (&horizon_core::cloud_panel::Build, &[String]),
        tag: &str,
        docker_config: &Path,
        log: &mut Option<std::fs::File>,
    ) -> Result<(), String> {
        // The agent wrote these paths: both stay inside the repository, as deployment
        // demands. A context outside .horizon builds from the committed tree with the
        // proposed .horizon laid over it, the image that deployment will build.
        let root = std::fs::canonicalize(&self.repository).map_err(|error| error.to_string())?;
        let context = inside(&root, &build.context, "build.context")?;
        let dockerfile = inside(&root, &build.dockerfile, "build.dockerfile")?;
        let snapshot = if context.starts_with(root.join(".horizon")) {
            None
        } else {
            Some(committed_snapshot(&root)?)
        };
        let (context, dockerfile) = match &snapshot {
            Some((_, tree)) => {
                let moved = |path: &Path| tree.join(path.strip_prefix(&root).unwrap_or(path));
                (moved(&context), moved(&dockerfile))
            }
            None => (context, dockerfile),
        };
        let mut command = Command::new("docker");
        command
            .current_dir(&self.repository)
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
        let head = super::commit::git_command(&self.repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .ok_or("the repository has no commit")?;
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
        let wanted = std::fs::read_to_string(self.repository.join(".horizon/cloud.yml"))
            .ok()
            .and_then(|config| horizon_core::cloud_panel::CloudConfig::parse(&config).ok())
            .and_then(|config| config.profiles.get(&config.default).map(|profile| profile.gpu))
            .unwrap_or(false);
        wanted && Path::new("/dev/nvidiactl").exists()
    }

    fn ci(&self, fast: bool, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let head = super::commit::git_command(&self.repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
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
                .localci_args(fast, &head)
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
            .map_err(|error| format!("cannot start docker: {error}"))?;
        let (lines, received) = mpsc::channel::<String>();
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
                while reader.read_until(b'\n', &mut bytes).is_ok_and(|read| read > 0) {
                    let line = String::from_utf8_lossy(&bytes);
                    let _ = lines.send(line.trim_end_matches(['\n', '\r']).to_owned());
                    bytes.clear();
                }
            });
        }
        drop(lines);
        loop {
            match received.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    if let Some(file) = log.as_mut() {
                        let _ = writeln!(file, "{line}");
                    }
                    let _ = self.sender.send(Event::Line(line));
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

/// `path` of the repository at `root`, refused when it leaves the repository.
fn inside(root: &Path, path: &str, field: &str) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(root.join(path)).map_err(|error| format!("{field} {path}: {error}"))?;
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(format!("{field} {path} leaves the repository"))
    }
}

/// The committed tree of the repository at `root`, made the way deployment makes it
/// (Git LFS files and submodules included), with its working `.horizon` laid over it.
/// The folder is private and deleted on drop; the second value is the tree inside it.
fn committed_snapshot(root: &Path) -> Result<(tempfile::TempDir, PathBuf), String> {
    let folder = tempfile::Builder::new()
        .prefix("horizon-setup-context-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let cancel = horizon_core::cloud_runtime::Cancellation::default();
    let runner = horizon_core::cloud_runtime::command::Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let tree = horizon_core::cloud_runtime::repository::snapshot(root, "HEAD", folder.path(), &runner)
        .map_err(|error| format!("cannot make the committed tree: {error}"))?;
    // The proposed .horizon replaces the committed one exactly: deletions and links too.
    let _ = std::fs::remove_dir_all(tree.join(".horizon"));
    copy_tree(&root.join(".horizon"), &tree.join(".horizon")).map_err(|error| error.to_string())?;
    Ok((folder, tree))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &target)?;
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

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

    #[test]
    fn a_proof_fingerprint_follows_the_setup_and_the_request() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\n").unwrap();
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let first = fingerprint(repository.path(), &spec);
        assert_eq!(first, fingerprint(repository.path(), &spec), "stable");
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\nRUN true\n").unwrap();
        let edited = fingerprint(repository.path(), &spec);
        assert_ne!(first, edited, "an edited Dockerfile is not the proved one");
        let other = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["lint"]})).unwrap();
        assert_ne!(edited, fingerprint(repository.path(), &other), "nor another request");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_setup_counts_by_its_target_and_is_never_followed() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\n").unwrap();
        std::os::unix::fs::symlink(
            repository.path().join(".horizon"),
            repository.path().join(".horizon/loop"),
        )
        .unwrap();
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let first = fingerprint(repository.path(), &spec);
        std::fs::remove_file(repository.path().join(".horizon/loop")).unwrap();
        std::os::unix::fs::symlink("/etc", repository.path().join(".horizon/loop")).unwrap();
        assert_ne!(
            first,
            fingerprint(repository.path(), &spec),
            "a link to elsewhere is a change"
        );
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
    fn a_context_outside_the_setup_must_stay_in_the_repository() {
        let repository = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(repository.path()).unwrap();
        std::fs::create_dir_all(root.join(".horizon")).unwrap();
        assert!(inside(&root, ".horizon", "build.context").is_ok());
        assert!(inside(&root, ".", "build.context").is_ok());
        assert!(
            inside(&root, "..", "build.context")
                .unwrap_err()
                .contains("leaves the repository")
        );
    }

    #[test]
    fn the_fingerprint_frames_each_file() {
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let read = |files: &[(&str, &str)]| {
            let repository = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
            for (name, content) in files {
                std::fs::write(repository.path().join(".horizon").join(name), content).unwrap();
            }
            (fingerprint(repository.path(), &spec), repository)
        };
        let (one, _kept) = read(&[("a", "b")]);
        let (other, repository) = read(&[("ab", "")]);
        assert_ne!(one, other, "a path and its content never run together");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let file = repository.path().join(".horizon/ab");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_ne!(
                fingerprint(repository.path(), &spec),
                other,
                "the executable bit counts"
            );
        }
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
