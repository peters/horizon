//! The live preview of a desktop app before anything is rented: Horizon starts the app
//! in a container of the proved image, on the proof's volume so its build output is
//! reused, with a virtual desktop and a VNC server on this computer's loopback. Once a
//! window appears it takes one screenshot when the image can, and the dialog opens a
//! native Device panel on the live desktop.
use super::proof::{LocalDocker, Pinned};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// How long the app gets to show a window.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(60);

/// How long one docker command may take; the image's own tools answer within it.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The most output Horizon keeps from one docker command; the image writes it.
const OUTPUT_LIMIT: usize = 256 * 1024;

/// The largest screenshot Horizon copies out of the container.
const SCREENSHOT_LIMIT: u64 = 32 * 1024 * 1024;

/// The most output Horizon reads from one docker command that reads the session
/// environment.
const SESSION_LIMIT: u64 = 4 * 1024 * 1024;

/// How long a live desktop's VNC server gets to greet when Use checks it.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// The longest preview command the agent may name.
const MAX_COMMAND_LENGTH: usize = 4096;

/// What the agent asks Horizon to start, from the status file's `preview`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Spec {
    /// A shell command, run in /workspace/checkout with the session environment.
    pub(super) command: String,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Spec {
    pub(super) fn parse(value: &serde_json::Value) -> Option<Self> {
        let command = value.get("command")?.as_str()?.trim().to_owned();
        if command.is_empty() || command.chars().count() > MAX_COMMAND_LENGTH || command.contains('\0') {
            return None;
        }
        let (width, height) = value
            .get("size")
            .and_then(serde_json::Value::as_str)
            .and_then(|size| size.split_once('x'))
            .and_then(|(width, height)| Some((width.trim().parse().ok()?, height.trim().parse().ok()?)))
            .filter(|&(width, height): &(u32, u32)| (320..=7680).contains(&width) && (240..=4320).contains(&height))
            .unwrap_or((1280, 800));
        Some(Self { command, width, height })
    }
}

/// What the preview reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Starting,
    /// A window drew; the desktop is served on this loopback port. The screenshot is
    /// there when the image has the `import` tool of `ImageMagick`.
    Live {
        port: u16,
        window: String,
        after: Duration,
        screenshot: Option<PathBuf>,
    },
    Failed(String),
}

pub(super) struct Preview {
    receiver: mpsc::Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
    container: String,
    pub(super) outcome: Outcome,
    pub(super) started: Instant,
    pub(super) ended: Option<Instant>,
    /// The outcome is new and the agent has not been told yet.
    pub(super) unreported: bool,
    /// The background run, so cleanup waits until it can create nothing more.
    worker: Option<std::thread::JoinHandle<()>>,
    /// The proof's pinned Docker daemon, which every docker command of the preview uses.
    pinned: Pinned,
}

impl Preview {
    /// Starts preview number `request`; each gets its own container, so the removal of
    /// an earlier one never meets this one. It begins once `after`, the run of the
    /// preview or proof it follows, has ended.
    pub(super) fn start(
        (image, volume, pinned): (&str, &str, Pinned),
        (request, spec): (u64, Spec),
        screenshot: PathBuf,
        (after, ctx): (Option<std::thread::JoinHandle<()>>, egui::Context),
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let container = format!("{volume}-preview-{request}");
        let job = Job {
            image: image.to_owned(),
            volume: volume.to_owned(),
            container: container.clone(),
            spec,
            screenshot,
            cancel: cancel.clone(),
            sender,
            ctx,
            pinned: pinned.clone(),
            config: std::sync::OnceLock::new(),
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
            container,
            outcome: Outcome::Starting,
            started: Instant::now(),
            ended: None,
            unreported: false,
            worker: Some(worker),
            pinned,
        }
    }

    /// Stops the preview and hands back one run that ends once its worker ended and its
    /// container is gone, so a proof or cleanup that waits for it finds the volume free.
    pub(super) fn stop(&mut self) -> std::thread::JoinHandle<()> {
        self.cancel.store(true, Ordering::Relaxed);
        let worker = self.worker.take();
        let container = self.container.clone();
        let pinned = self.pinned.clone();
        std::thread::spawn(move || {
            if let Some(worker) = worker {
                let _ = worker.join();
            }
            // A preview whose run never reached its daemon started no container.
            if let Some(daemon) = pinned.get() {
                let _ = super::proof::remove_now(daemon, &["rm", "--force", &container]);
            }
        })
    }

    /// Removes the container in the background and waits for docker there, so the
    /// finished client leaves no zombie process behind.
    fn remove_container(&self) {
        let container = self.container.clone();
        let pinned = self.pinned.clone();
        // Registered, so Horizon's exit and the next setup wait for it.
        super::run::keep_ending(std::thread::spawn(move || {
            if let Some(daemon) = pinned.get() {
                let _ = super::proof::remove_now(daemon, &["rm", "--force", &container]);
            }
        }));
    }

    pub(super) fn poll(&mut self) {
        while let Ok(outcome) = self.receiver.try_recv() {
            self.outcome = outcome;
            self.ended = Some(Instant::now());
            self.unreported = true;
        }
    }

    /// The loopback port of the live desktop.
    pub(super) fn live_port(&self) -> Option<u16> {
        match self.outcome {
            Outcome::Live { port, .. } => Some(port),
            _ => None,
        }
    }

    /// The loopback endpoint of the live desktop.
    pub(super) fn endpoint(&self) -> Option<String> {
        self.live_port().map(|port| format!("127.0.0.1:{port}"))
    }

    /// A preview whose desktop is live on `port`, with no run behind it, for the Unix
    /// tests of Use.
    #[cfg(all(test, unix))]
    pub(super) fn live(port: u16) -> Self {
        let (_, receiver) = mpsc::channel();
        Self {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            container: "horizon-test-preview".to_owned(),
            outcome: Outcome::Live {
                port,
                window: "640x400".to_owned(),
                after: Duration::ZERO,
                screenshot: None,
            },
            started: Instant::now(),
            ended: Some(Instant::now()),
            unreported: false,
            worker: None,
            pinned: Pinned::default(),
        }
    }

    pub(super) fn report(&self) -> String {
        match &self.outcome {
            Outcome::Starting => String::new(),
            Outcome::Live {
                window,
                after,
                screenshot,
                ..
            } => format!(
                "Horizon desktop preview passed: a {window} window appeared {} s after the start; {}, and the person sees the app live in a Device panel. Mark Desktop preview done and write your summary.",
                after.as_secs(),
                screenshot.as_ref().map_or_else(
                    || "there is no screenshot because the image has no ImageMagick import".to_owned(),
                    |path| format!("the screenshot is {}", path.display())
                )
            ),
            Outcome::Failed(reason) => format!(
                "Horizon desktop preview failed: {reason} Fix the preview command and increase preview_request by one; when the image or cloud.yml needs a change, change it and increase proof_request instead, and Horizon starts the preview again once that proof passed."
            ),
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // The live desktop goes with the run, unless `stop` already removes it and a
        // second removal would only race the one that is waited for.
        if let Some(worker) = self.worker.take() {
            super::run::keep_ending(worker);
            self.remove_container();
        }
    }
}

struct Job {
    image: String,
    volume: String,
    container: String,
    spec: Spec,
    screenshot: PathBuf,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<Outcome>,
    ctx: egui::Context,
    pinned: Pinned,
    /// The preview's own empty Docker configuration, made on first use.
    config: std::sync::OnceLock<tempfile::TempDir>,
}

impl Job {
    /// The preview's docker: the proof's pinned daemon, resolved here when the proof
    /// never did, and a private, empty Docker configuration made for this preview.
    fn daemon(&self) -> Result<Docker, String> {
        let daemon = if let Some(daemon) = self.pinned.get() {
            daemon.clone()
        } else {
            let daemon = LocalDocker::resolve()?;
            self.pinned.get_or_init(|| daemon).clone()
        };
        let config = if let Some(config) = self.config.get() {
            config
        } else {
            let folder = tempfile::Builder::new()
                .prefix("horizon-preview-docker-")
                .tempdir()
                .map_err(|error| format!("cannot make a private Docker configuration: {error}"))?;
            self.config.get_or_init(|| folder)
        };
        Ok(Docker {
            daemon,
            config: config.path().to_path_buf(),
        })
    }

    /// One docker command against the pinned daemon.
    fn docker(&self, args: &[&str]) -> Result<String, String> {
        docker(&self.daemon()?, args)
    }

    fn run(self) {
        let outcome = self.serve().unwrap_or_else(Outcome::Failed);
        // A stop can come while `docker run` starts the container, and a failed preview
        // keeps no app or desktop running: the run removes its container in both cases.
        let stopped = self.cancel.load(Ordering::Relaxed);
        if stopped || matches!(outcome, Outcome::Failed(_)) {
            let _ = self.docker(&["rm", "--force", &self.container]);
        }
        if stopped {
            return;
        }
        let live = match &outcome {
            Outcome::Live { port, .. } => Some(*port),
            _ => None,
        };
        let _ = self.sender.send(outcome);
        self.ctx.request_repaint();
        if let Some(port) = live {
            self.watch(port);
        }
    }

    /// Watches a live desktop until the preview stops: when the app's container or its
    /// VNC server ends, the preview fails, so Use and the panel never rest on a desktop
    /// that is gone.
    fn watch(&self, port: u16) {
        loop {
            for _ in 0..20 {
                if self.cancel.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Only a container docker reports stopped or gone counts: a slow or busy daemon
            // that does not answer in time is no reason to end a live preview.
            let running = match self.docker(&["inspect", "--format", "{{.State.Running}}", &self.container]) {
                Ok(running) => running.trim() == "true",
                Err(error) => !error.contains("No such"),
            };
            let reason = if !running {
                format!("the app stopped while it was live. Its output: {}", tail(&self.logs()))
            } else if vnc_answers(port).is_err() {
                "the desktop's VNC server stopped answering.".to_owned()
            } else {
                continue;
            };
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = self.docker(&["rm", "--force", &self.container]);
            let _ = self.sender.send(Outcome::Failed(reason));
            self.ctx.request_repaint();
            return;
        }
    }

    /// Starts the app's container on a virtual desktop, with the image's session
    /// environment, against the proof's pinned daemon.
    fn start_app(&self) -> Result<(), String> {
        // Its desktop is published on this computer's loopback, so the daemon must be
        // this computer's: the proof's pinned one.
        let daemon = self.daemon()?;
        let _ = self.docker(&["rm", "--force", &self.container]);
        // The image's session environment, read without running the image, as the
        // proof's CI gets it.
        let capture = |mut command: Command| {
            daemon.pin(&mut command);
            capture(command, SESSION_LIMIT)
        };
        let session = super::proof::session_environment(&self.image, &format!("{}-env", self.container), &capture)?;
        if self.cancel.load(Ordering::Relaxed) {
            return Err("stopped.".to_owned());
        }
        let script = format!(
            "Xvfb :99 -screen 0 {}x{}x24 -nolisten tcp >/tmp/xvfb.log 2>&1 & \
             for i in $(seq 1 50); do xdpyinfo -display :99 >/dev/null 2>&1 && break; sleep 0.1; done; \
             (openbox >/dev/null 2>&1 &); \
             (x11vnc -display :99 -forever -shared -nopw -rfbport 5900 -quiet >/tmp/x11vnc.log 2>&1 &); \
             cd /workspace/checkout && exec bash -c {}",
            self.spec.width,
            self.spec.height,
            shell_quote(&self.spec.command)
        );
        let volume = format!("{}:/workspace", self.volume);
        // The run's own bridge network: only its previews are on it, so no other container
        // reaches the desktop's VNC server, which has no password.
        let network = super::proof::preview_network(&self.volume);
        if docker(&daemon, &["network", "inspect", &network]).is_err() {
            docker(&daemon, &["network", "create", "--driver", "bridge", &network])?;
        }
        // The image's session environment first, then the variables the preview needs:
        // docker keeps the last value, so the image cannot move the app off :99.
        // Never pulled: the proof built or tagged this image here, and a pull would reach a
        // registry this preview has no business with.
        let mut run = vec![
            "run",
            "--detach",
            "--pull",
            "never",
            "--name",
            &self.container,
            "--network",
            &network,
            // Bounded logs, as the worker's own launcher keeps them: a chatty app cannot
            // fill this computer's disk through the daemon.
            "--log-driver",
            "local",
            "--log-opt",
            "max-size=10m",
            "--log-opt",
            "max-file=2",
            "--user",
            "10001:10001",
            "-v",
            &volume,
        ];
        run.extend(session.iter().map(String::as_str));
        run.extend([
            "-e",
            "HOME=/workspace/home",
            "-e",
            "DISPLAY=:99",
            "-e",
            "LIBGL_ALWAYS_SOFTWARE=1",
            "-p",
            "127.0.0.1::5900",
            "--entrypoint",
            "bash",
            &self.image,
            "-c",
            &script,
        ]);
        docker(&daemon, &run).map_err(|error| {
            if error.contains("No such image") || error.contains("Unable to find image") {
                "the proved image is no longer on this computer. Increase proof_request by one so Horizon proves the setup again, and the preview starts after it.".to_owned()
            } else {
                error
            }
        })?;
        Ok(())
    }

    fn serve(&self) -> Result<Outcome, String> {
        // A preview stopped while it waited for the run before it never starts its app.
        if self.cancel.load(Ordering::Relaxed) {
            return Err("stopped.".to_owned());
        }
        self.start_app()?;
        let started = Instant::now();
        let window = loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err("stopped.".to_owned());
            }
            if started.elapsed() > WINDOW_TIMEOUT {
                return Err(format!(
                    "no window appeared within {} s. The app's output: {}",
                    WINDOW_TIMEOUT.as_secs(),
                    tail(&self.logs())
                ));
            }
            if self
                .docker(&["inspect", "--format", "{{.State.Running}}", &self.container])
                .is_ok_and(|running| running.trim() != "true")
            {
                return Err(format!(
                    "the app exited before it showed a window. Its output: {}",
                    tail(&self.logs())
                ));
            }
            if let Some(window) = self
                .docker(&["exec", &self.container, "sh", "-c", SHOWN_WINDOWS])
                .ok()
                .and_then(|listing| largest_shown(&listing))
            {
                break window;
            }
            std::thread::sleep(Duration::from_millis(400));
        };
        let after = started.elapsed();
        // The window drew; give it a moment to paint before the screenshot.
        std::thread::sleep(Duration::from_millis(800));
        // A screenshot when the image can take one; the live desktop is the proof.
        let screenshot = self
            .docker(&["exec", &self.container, "import", "-window", "root", "/tmp/preview.png"])
            .and_then(|_| self.copy_screenshot())
            .ok()
            .map(|()| self.screenshot.clone());
        let port = self
            .docker(&["port", &self.container, "5900/tcp"])?
            .lines()
            .find_map(|line| line.rsplit_once(':').and_then(|(_, port)| port.trim().parse().ok()))
            .ok_or("the VNC port was not published")?;
        vnc_answers(port)?;
        Ok(Outcome::Live {
            port,
            window,
            after,
            screenshot,
        })
    }
}

impl Job {
    /// Reads the screenshot out of the container, at most `SCREENSHOT_LIMIT` bytes, and
    /// writes it into the run's folder as a new file: the agent can write there, so
    /// nothing follows its links, and nothing large reaches this computer's disk.
    fn copy_screenshot(&self) -> Result<(), String> {
        let bytes = docker_bytes(
            &self.daemon()?,
            &["exec", &self.container, "cat", "/tmp/preview.png"],
            SCREENSHOT_LIMIT,
        )
        .map_err(|error| format!("the screenshot cannot be read: {error}"))?;
        let mut file = super::proof::create_private(&self.screenshot)
            .map_err(|error| format!("cannot keep the screenshot: {error}"))?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|error| format!("cannot keep the screenshot: {error}"))
    }
}

impl Job {
    /// The app's last output, both streams: `docker logs` writes its stderr to its own.
    fn logs(&self) -> String {
        self.daemon()
            .and_then(|daemon| run_docker(&daemon, &["logs", "--tail", "20", &self.container]))
            .map(|(stdout, stderr)| format!("{stdout}\n{stderr}"))
            .unwrap_or_default()
    }
}

/// Runs one docker command within `COMMAND_TIMEOUT` and keeps at most `OUTPUT_LIMIT`
/// bytes of each stream: the container's own tools write what comes back.
/// The preview's docker commands: the proof's pinned daemon and a private, empty Docker
/// configuration, so no command reads or uses this computer's registry credentials.
struct Docker {
    daemon: LocalDocker,
    config: PathBuf,
}

impl Docker {
    fn command(&self) -> Command {
        let mut command = Command::new("docker");
        self.pin(&mut command);
        command
    }

    fn pin(&self, command: &mut Command) {
        self.daemon.pin(command);
        command
            .env("DOCKER_CONFIG", &self.config)
            .env_remove("DOCKER_AUTH_CONFIG");
    }
}

fn docker(daemon: &Docker, args: &[&str]) -> Result<String, String> {
    run_docker(daemon, args).map(|(stdout, _)| stdout)
}

/// Whether a VNC server answers on the loopback `port` within ten seconds: it greets
/// with `RFB `. A desktop without one is not live, whatever drew on it.
fn vnc_answers(port: u16) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if greets(port, Duration::from_secs(2)) {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err(
                "the app drew a window, but no VNC server answered on the desktop; the image needs x11vnc.".to_owned(),
            );
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Whether the live desktop on the loopback `port` still answers: its VNC server greets
/// within [`ANSWER_WAIT`], connection included. The app runs as the container's first process, so a stopped app
/// takes the server with it. It waits on the network, so only a worker thread asks.
pub(super) fn answers(port: u16) -> bool {
    greets(port, ANSWER_WAIT)
}

/// Whether the VNC server on the loopback `port` greets with `RFB ` within `wait`, one
/// deadline for the connection and every read, so a server that sends its greeting a
/// byte at a time cannot stretch it.
fn greets(port: u16, wait: Duration) -> bool {
    use std::io::Read as _;
    let deadline = Instant::now() + wait;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, wait) else {
        return false;
    };
    let mut greeting = [0_u8; 4];
    let mut read = 0;
    while read < greeting.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        match stream.read(&mut greeting[read..]) {
            Ok(0) | Err(_) => return false,
            Ok(count) => read += count,
        }
    }
    &greeting == b"RFB "
}

/// Starts one docker command in its own process group, so a timeout or the end of its
/// output ends everything the client started too.
fn spawn_docker(daemon: &Docker, args: &[&str], stderr: Stdio) -> Result<std::process::Child, String> {
    let mut command = daemon.command();
    command.args(args);
    spawn(command, stderr)
}

/// Starts `command` in its own process group with its output piped.
fn spawn(mut command: Command, stderr: Stdio) -> Result<std::process::Child, String> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .map_err(|error| format!("cannot run docker: {error}"))
}

/// Waits until every reader ended, at most until `COMMAND_TIMEOUT` after `started`.
fn wait_for_readers(readers: &[&std::thread::JoinHandle<impl Send>], started: Instant) -> bool {
    while !readers.iter().all(|reader| reader.is_finished()) {
        if started.elapsed() >= COMMAND_TIMEOUT {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// The command's output closed: its group goes before it is reaped, while its group
/// id is still its own, and then its status is read.
fn finish(child: &mut std::process::Child) -> Result<std::process::ExitStatus, String> {
    super::proof::end_group(child.id());
    child.wait().map_err(|error| error.to_string())
}

/// Runs one docker command within `COMMAND_TIMEOUT` and returns its standard output as
/// bytes, refused once it passes `limit`: the command is stopped then, never read on.
fn docker_bytes(daemon: &Docker, args: &[&str], limit: u64) -> Result<Vec<u8>, String> {
    let mut command = daemon.command();
    command.args(args);
    capture(command, limit)
}

/// Runs `command` within `COMMAND_TIMEOUT` and returns its standard output, refused once
/// it passes `limit`.
fn capture(command: Command, limit: u64) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let mut child = spawn(command, Stdio::null())?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(limit + 1).read_to_end(&mut bytes);
        }
        bytes
    });
    if !wait_for_readers(&[&reader], started) {
        super::bounded::end(&mut child);
        return Err(format!("docker did not answer within {} s", COMMAND_TIMEOUT.as_secs()));
    }
    let bytes = reader.join().unwrap_or_default();
    if bytes.len() as u64 > limit {
        super::bounded::end(&mut child);
        return Err(format!("it is larger than {limit} bytes"));
    }
    match finish(&mut child)? {
        status if status.success() => Ok(bytes),
        _ => Err("docker failed".to_owned()),
    }
}

/// Runs one docker command like [`docker`] and returns both of its streams.
fn run_docker(daemon: &Docker, args: &[&str]) -> Result<(String, String), String> {
    let started = Instant::now();
    let mut child = spawn_docker(daemon, args, Stdio::piped())?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0_u8; 8192];
            if let Some(mut pipe) = pipe {
                while let Ok(read) = pipe.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    };
    let stdout = read(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let stderr = read(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let name = args.first().copied().unwrap_or_default();
    if !wait_for_readers(&[&stdout, &stderr], started) {
        super::bounded::end(&mut child);
        return Err(format!(
            "docker {name} did not answer within {} s",
            COMMAND_TIMEOUT.as_secs()
        ));
    }
    let status = finish(&mut child)?;
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    if status.success() {
        Ok((stdout, stderr))
    } else {
        Err(format!("docker {name} failed: {}", tail(&stderr)))
    }
}

/// Lists every top-level window of the desktop, at most 256, as `ID STATE WIDTH HEIGHT`
/// lines, in one command inside the container: each window's map state and size.
const SHOWN_WINDOWS: &str = "xwininfo -root -children | sed -n 's/^ *\\(0x[0-9a-fA-F]*\\) .*/\\1/p' | head -n 256 | \
while read -r id; do xwininfo -id \"$id\" | awk -v id=\"$id\" \
'/Map State:/ {s=$3} /Width:/ {w=$2} /Height:/ {h=$2} END {print id, s, w, h}'; done";

/// The largest shown window bigger than a stray helper window, as `WIDTHxHEIGHT`, from
/// the [`SHOWN_WINDOWS`] listing: one created hidden does not count as the app showing
/// a window, however large.
fn largest_shown(listing: &str) -> Option<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (_, state) = (words.next()?, words.next()?);
            let (width, height) = (words.next()?.parse::<u32>().ok()?, words.next()?.parse::<u32>().ok()?);
            (state == "IsViewable" && width >= 200 && height >= 150).then_some((width, height))
        })
        .max_by_key(|&(width, height)| u64::from(width) * u64::from(height))
        .map(|(width, height)| format!("{width}x{height}"))
}

/// The last lines of output, each cut short, as one line for the agent and the dialog.
/// The app writes them; `run::pasted` takes every control character out before any
/// of it reaches the agent's terminal.
fn tail(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(240).collect())
        .collect();
    lines[lines.len().saturating_sub(6)..].join(" | ")
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Where the preview screenshot of a run goes, beside its status file.
pub(super) fn screenshot_path(status: &Path) -> PathBuf {
    status.with_extension("preview.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_names_a_command_and_maybe_a_size() {
        let spec = Spec::parse(&serde_json::json!({"command": "dotnet youpay.dll", "size": "1280x1024"})).unwrap();
        assert_eq!((spec.width, spec.height), (1280, 1024));
        let plain = Spec::parse(&serde_json::json!({"command": "./app"})).unwrap();
        assert_eq!((plain.width, plain.height), (1280, 800), "a default size");
        assert!(Spec::parse(&serde_json::json!({"command": " "})).is_none());
        let odd = Spec::parse(&serde_json::json!({"command": "./app", "size": "9x9"})).unwrap();
        assert_eq!((odd.width, odd.height), (1280, 800), "an unusable size falls back");
        let wide = "é".repeat(MAX_COMMAND_LENGTH);
        assert!(
            Spec::parse(&serde_json::json!({"command": wide})).is_some(),
            "the limit counts characters"
        );
        let long = "x".repeat(MAX_COMMAND_LENGTH + 1);
        assert!(
            Spec::parse(&serde_json::json!({"command": long})).is_none(),
            "a command is bounded"
        );
        assert!(Spec::parse(&serde_json::json!({"command": "./app\u{0}"})).is_none());
    }

    #[test]
    fn the_output_the_agent_reads_is_cut_short() {
        let long = format!("{}\n", "y".repeat(10_000)).repeat(20);
        let tail = tail(&long);
        assert_eq!(tail.matches(" | ").count(), 5, "six lines");
        assert!(tail.len() < 6 * 240 + 6 * 3, "each line cut short");
    }

    #[test]
    fn a_desktop_greets_within_one_deadline() {
        use std::io::Write as _;
        let serve = |bytes: &'static [u8], every: Duration| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    for byte in bytes {
                        std::thread::sleep(every);
                        if stream.write_all(&[*byte]).is_err() {
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
            port
        };
        assert!(greets(serve(b"RFB 003.008\n", Duration::ZERO), Duration::from_secs(5)));
        assert!(!greets(serve(b"HTTP", Duration::ZERO), Duration::from_secs(5)));
        let started = Instant::now();
        assert!(
            !greets(serve(b"RFB ", Duration::from_millis(300)), Duration::from_millis(700)),
            "a greeting a byte at a time past the deadline"
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "one deadline in all, not one per read: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_app_window_is_the_largest_shown_one() {
        let listing = "0x200001 IsViewable 10 10\n\
             0x300001 IsUnMapped 1920 1080\n0x300002 IsUnMapped 1600 900\n0x300003 IsUnMapped 1500 900\n\
             0x300004 IsUnMapped 1400 900\n0x300005 IsUnMapped 1300 900\n0x300006 IsUnMapped 1290 900\n\
             0x400005 IsViewable 640 400\n";
        assert_eq!(
            largest_shown(listing).as_deref(),
            Some("640x400"),
            "hidden helper windows, however many, never hide the shown one"
        );
        assert_eq!(largest_shown("0x1 IsViewable 20 20\n"), None);
        assert_eq!(largest_shown("garbage\n"), None);
    }
}
