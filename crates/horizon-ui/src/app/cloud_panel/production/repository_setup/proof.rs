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
}

impl Proof {
    /// Starts the proof of the setup in `repository`. `log_path` keeps the whole output.
    pub(super) fn start(repository: &Path, spec: Spec, log_path: PathBuf, ctx: egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let image = format!("horizon-setup-{}:candidate", slug(repository));
        let job = Job {
            repository: repository.to_path_buf(),
            spec,
            image: image.clone(),
            volume: format!("horizon-setup-{}", slug(repository)),
            log_path: log_path.clone(),
            cancel: cancel.clone(),
            sender,
            ctx,
        };
        std::thread::spawn(move || job.run());
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
        }
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
    }
}

struct Job {
    repository: PathBuf,
    spec: Spec,
    image: String,
    volume: String,
    log_path: PathBuf,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl Job {
    fn run(self) {
        let mut log = std::fs::File::create(&self.log_path).ok();
        for phase in [Phase::Build, Phase::Ci, Phase::Fast] {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = self.sender.send(Event::Started(phase));
            self.ctx.request_repaint();
            let result = match phase {
                Phase::Build => self.build(&mut log),
                Phase::Ci => self.prepare(&mut log).and_then(|()| self.ci(false, &mut log)),
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
        let profile = config
            .profiles
            .get(&config.default)
            .ok_or("the default profile is missing")?;
        let build = profile
            .build
            .as_ref()
            .ok_or("the default profile has no build section")?;
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
        let mut command = Command::new("docker");
        command
            .current_dir(&self.repository)
            .env_remove("DOCKER_AUTH_CONFIG")
            .arg("--config")
            .arg(docker_config.path())
            .args(["buildx", "build", "--load", "--progress", "plain", "--platform"])
            .arg(&build.platform)
            .arg("-f")
            .arg(&build.dockerfile)
            .arg("-t")
            .arg(&self.image)
            .arg(&build.context);
        self.stream_within(command, log, Some(BUILD_TIMEOUT))?;
        let mut check = Command::new("docker");
        check.args([
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            "horizon-worker-check",
        ]);
        check.arg(&self.image).arg("--git-auth");
        self.stream(check, log)
    }

    /// Clones the committed repository into the proof volume, or brings the clone
    /// up to date, so build output and caches stay between runs.
    fn prepare(&self, log: &mut Option<std::fs::File>) -> Result<(), String> {
        // A worktree's .git only points into its repository's shared git folder, so the
        // clone reads that folder and checks out the worktree's own commit.
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&self.repository)
                .args(args)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let head = git(&["rev-parse", "HEAD"]).ok_or("the repository has no commit")?;
        let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok_or("cannot find the repository's git folder")?;
        let mut own = Command::new("docker");
        own.args(["run", "--rm", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["--entrypoint", "sh", &self.image, "-c"])
            .arg("mkdir -p /workspace/home && chown 10001:10001 /workspace /workspace/home");
        self.stream(own, log)?;
        let mut clone = Command::new("docker");
        clone
            .args(["run", "--rm", "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .arg("-v")
            .arg(format!("{common}:/src.git:ro"))
            .args(["-e", "HOME=/workspace/home", "-e"])
            .arg(format!("HEAD_SHA={head}"))
            .args(["--entrypoint", "sh", &self.image, "-c"])
            .arg(CLONE);
        self.stream(clone, log)
    }

    fn ci(&self, fast: bool, log: &mut Option<std::fs::File>) -> Result<(), String> {
        let head = Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["-w", "/workspace/checkout", "-e", "HOME=/workspace/home"]);
        // Packages this computer already restored, private ones included, need no feed
        // credential: NuGet reads them as a fallback folder before it asks a source.
        if let Some(packages) = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".nuget/packages"))
            && packages.is_dir()
        {
            command
                .arg("-v")
                .arg(format!("{}:/opt/host-nuget:ro", packages.display()))
                .args(["-e", "RestoreAdditionalProjectFallbackFolders=/opt/host-nuget"]);
        }
        let script = format!(
            "{SESSION_ENV} exec localci {}",
            self.spec
                .localci_args(fast, &head)
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        );
        command.args(["--entrypoint", "bash", &self.image, "-c", &script]);
        self.stream(command, log)
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
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    let _ = lines.send(line);
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
                return Err("stopped".to_owned());
            }
            if limit.is_some_and(|limit| started.elapsed() > limit) {
                let _ = child.kill();
                let _ = child.wait();
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

/// Clones the repository's git folder into the volume once, fetches it after, and checks
/// out the commit Horizon names; build output and caches stay between runs.
const CLONE: &str = "set -e; git config --global --add safe.directory '*'; \
if [ -d /workspace/checkout/.git ]; then git -C /workspace/checkout fetch -q /src.git '+refs/heads/*:refs/remotes/src/*'; \
else git clone -q --no-hardlinks --no-checkout /src.git /workspace/checkout; fi; \
git -C /workspace/checkout checkout -q --force --detach \"$HEAD_SHA\"";

/// Applies the image's session environment, as a worker session gets it.
const SESSION_ENV: &str = "for f in /etc/horizon-worker/session-env.d/*.env; do [ -f \"$f\" ] || continue; \
while IFS= read -r line || [ -n \"$line\" ]; do case \"$line\" in ''|'#'*) ;; \
PATH_PREPEND=*) PATH=\"${line#PATH_PREPEND=}:$PATH\";; PATH_APPEND=*) PATH=\"$PATH:${line#PATH_APPEND=}\";; \
*=*) export \"$line\";; esac; done < \"$f\"; done; export PATH;";

/// A name Docker accepts for the repository folder.
fn slug(repository: &Path) -> String {
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
        assert_eq!(slug(Path::new("/home/me/YouPay v2")), "youpay-v2");
        assert_eq!(slug(Path::new("/")), "repository");
    }

    #[test]
    fn quoted_arguments_survive_the_shell() {
        assert_eq!(shell_quote("Use isolated NuGet caches"), "'Use isolated NuGet caches'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
