//! The browser VM of one computer: the image of `packaging/browser-vm` in a container on this
//! machine instead of a rented one.
//!
//! The container publishes its VNC server and its SSH server on the loopback only, and Horizon
//! writes the same ssh entry that a rented machine gets, so every route that runs
//! `ssh horizon-browser-vm` reaches this container without a copy of a key and without a manual
//! host-key step. The named volume keeps the browser profiles, and with them the sign-ins.
//!
//! Every function here runs off the UI thread: a Docker call blocks for seconds.

use std::io::Read as _;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::ssh_entry::{self, ALIAS};
use super::{Error, Result};

/// Container and volume name of the local browser VM.
pub const DEFAULT_NAME: &str = "horizon-browser-vm-local";
/// Image that [`LocalBrowserVm`] runs unless a person names another.
pub const DEFAULT_IMAGE: &str = "horizon-browser-vm-local:latest";
/// Loopback port of the VNC server of the container.
pub const DEFAULT_VNC_PORT: u16 = 5900;
/// Loopback port of the SSH server of the container.
pub const DEFAULT_SSH_PORT: u16 = 2222;
/// How long one start waits for the SSH server of the container to name its host key.
const KEY_WAIT: Duration = Duration::from_secs(30);
/// Pause between two reads of the host key.
const KEY_PAUSE: Duration = Duration::from_millis(500);
/// How long one start waits for the VNC greeting of the container.
const PROBE: Duration = Duration::from_secs(5);
/// The variable that turns the SSH server of the image on.
const SSH_ENV: &str = "HORIZON_BROWSER_VM_SSH=1";
/// The variable that names the one key the SSH server of the image accepts.
const KEY_ENV: &str = "HORIZON_BROWSER_VM_SSH_PUBKEY";
/// The first four bytes of a VNC server, RFC 6143 section 7.1.1.
const VNC_GREETING: [u8; 4] = *b"RFB ";
/// The port inside the container, which the image declares.
const CONTAINER_SSH_PORT: u16 = 2222;
/// The port inside the container, which the image declares.
const CONTAINER_VNC_PORT: u16 = 5900;

/// Where the local browser VM of this computer is addressed, as every route names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Route {
    /// The ssh target: the alias of the managed entry, or the address when Horizon could not
    /// write an entry because it does not know the home directory of this computer.
    pub ssh_target: String,
    /// Why the managed entry is missing, when it is.
    pub note: Option<String>,
}

/// What the local browser VM is doing now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalStatus {
    /// Whether the container runs.
    pub running: bool,
    pub image: String,
    /// `127.0.0.1:<port>`, as a Device panel takes it.
    pub vnc_address: String,
    /// `127.0.0.1:<port>` of the SSH server of the container.
    pub ssh_address: String,
    /// The state volume, which holds the browser profiles and their sign-ins.
    pub volume: String,
}

/// The local container of the browser image, with the programs it runs.
#[derive(Clone, Debug)]
pub struct LocalBrowserVm {
    root: PathBuf,
    home: Option<PathBuf>,
    name: String,
    image: String,
    vnc_port: u16,
    ssh_port: u16,
    docker: PathBuf,
    keygen: PathBuf,
}

impl LocalBrowserVm {
    /// The local browser VM of this computer. `root` holds its key and the ssh entry of its
    /// container; `home` is the home directory that the ssh entry belongs to, when known.
    #[must_use]
    pub fn new(root: &Path, home: Option<&Path>) -> Self {
        Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
            name: DEFAULT_NAME.to_owned(),
            image: DEFAULT_IMAGE.to_owned(),
            vnc_port: DEFAULT_VNC_PORT,
            ssh_port: DEFAULT_SSH_PORT,
            docker: PathBuf::from("docker"),
            keygen: PathBuf::from("ssh-keygen"),
        }
    }

    #[cfg(test)]
    pub(super) fn with_programs(mut self, docker: &Path) -> Self {
        self.docker = docker.to_path_buf();
        self
    }

    /// A name other than [`DEFAULT_NAME`], for a second container of the same computer.
    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        self
    }

    /// Another image reference, such as the tag of a build of the recipe.
    #[must_use]
    pub fn with_image(mut self, image: &str) -> Self {
        image.clone_into(&mut self.image);
        self
    }

    /// Another loopback port of the display.
    #[must_use]
    pub fn with_vnc_port(mut self, port: u16) -> Self {
        self.vnc_port = port;
        self
    }

    /// Another loopback port of the SSH server.
    #[must_use]
    pub fn with_ssh_port(mut self, port: u16) -> Self {
        self.ssh_port = port;
        self
    }

    /// Both loopback ports at once, when the default ones belong to another container.
    #[must_use]
    pub fn with_ports(self, vnc_port: u16, ssh_port: u16) -> Self {
        self.with_vnc_port(vnc_port).with_ssh_port(ssh_port)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn image(&self) -> &str {
        &self.image
    }

    /// The state volume that keeps the browser profiles and their sign-ins.
    #[must_use]
    pub fn volume(&self) -> String {
        format!("{}-state", self.name)
    }

    /// Start the container, then write the ssh entry of this computer for it.
    ///
    /// `build` names the repository root whose `packaging/browser-vm` recipe makes the image,
    /// and is used only when the image is absent.
    ///
    /// # Errors
    /// Refuses a container that already exists, an absent image with no recipe to build it,
    /// a Docker call that fails, and a start whose SSH server names no host key within
    /// [`KEY_WAIT`].
    pub fn up(&self, build: Option<&Path>) -> Result<Route> {
        let key = self.ensure_key()?;
        // A run that broke after its container started is retried by adopting that container,
        // so a second start of the same name is never a dead end.
        let public_key = std::fs::read_to_string(key.with_extension("pub"))?;
        if let Some(image) = self.container_image()? {
            self.adopt(&image, &public_key)?;
        } else {
            if !self.image_exists()? {
                let Some(repository) = build else {
                    return Err(Error::Local(format!(
                        "the image {} is absent; build it with `horizon --browser-vm-local up --build <repository>`",
                        self.image
                    )));
                };
                self.build(repository)?;
            }
            self.run(&public_key)?;
        }
        let host_key = self.wait_for_host_key()?;
        tracing::info!(container = %self.name, host_key = %host_key.len(), "the local browser VM answers");
        self.greets()?;
        let route = self.write_entry(&key, &host_key)?;
        Ok(route)
    }

    /// Use the container that already holds this name: it must run the same image, it must
    /// accept the key of this run, and a container that stopped is started again.
    fn adopt(&self, image: &str, public_key: &str) -> Result<()> {
        if image != self.image {
            return Err(Error::Local(format!(
                "the container {} holds the image {image}, not {}; stop it with `horizon --browser-vm-local down` first",
                self.name, self.image
            )));
        }
        // A container that another Horizon started accepts another key, so adopting it would
        // write an ssh entry that its SSH server refuses.
        let accepted = self.container_env()?;
        let expected = format!("{KEY_ENV}={}", public_key.trim());
        if !accepted.lines().any(|line| line.trim() == expected) {
            return Err(Error::Local(format!(
                "the container {} accepts a key of another Horizon; stop it with `horizon --browser-vm-local down` first",
                self.name
            )));
        }
        if self.running()? {
            return Ok(());
        }
        let started = self.output(&["start", &self.name])?;
        if !started.status.success() {
            return Err(self.failure("start", &started));
        }
        Ok(())
    }

    /// Read the greeting of the VNC server on the loopback address.
    ///
    /// A route that this start reports must answer, or a person would open a Device panel on a
    /// closed port.
    fn greets(&self) -> Result<()> {
        let address = self.vnc_socket();
        let Ok(mut stream) = TcpStream::connect_timeout(&address, PROBE) else {
            return Err(Error::Local(format!(
                "the VNC server of {} does not answer on {address}",
                self.name
            )));
        };
        let _ = stream.set_read_timeout(Some(PROBE));
        let mut greeting = [0_u8; VNC_GREETING.len()];
        match stream.read_exact(&mut greeting) {
            Ok(()) if greeting == VNC_GREETING => Ok(()),
            _ => Err(Error::Local(format!(
                "the VNC server of {} answers on {address} with no VNC greeting",
                self.name
            ))),
        }
    }

    /// The loopback address of the VNC server, as a Device panel takes it.
    #[must_use]
    pub fn vnc_address(&self) -> String {
        self.vnc_socket().to_string()
    }

    /// The private key that this run presents to the container.
    #[must_use]
    pub fn key_path(&self) -> PathBuf {
        self.root.join("browser-vm-local").join("id_ed25519")
    }

    /// The public key that this run presents, which the SSH server of the container accepts.
    ///
    /// # Errors
    /// Reports a key that cannot be read, which happens before a start makes one.
    pub fn accepted_public_key(&self) -> Result<String> {
        Ok(std::fs::read_to_string(self.key_path().with_extension("pub"))?)
    }

    /// The loopback address of the SSH server of the container.
    #[must_use]
    pub fn ssh_address(&self) -> String {
        format!("127.0.0.1:{}", self.ssh_port)
    }

    /// The loopback address of the VNC server.
    fn vnc_socket(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.vnc_port))
    }

    /// Stop and remove the container. The state volume, the key and the ssh entry stay, so the
    /// sign-ins of the profiles survive and the next start finds the same host key.
    ///
    /// # Errors
    /// Reports a Docker call that fails.
    pub fn down(&self) -> Result<()> {
        if !self.container_exists()? {
            return Ok(());
        }
        let stopped = self.output(&["stop", &self.name])?;
        // A container that stopped on its own is not running, which is the state the caller
        // asked for.
        if !stopped.status.success() && !says(&stopped, "is not running") {
            return Err(self.failure("stop", &stopped));
        }
        let removed = self.output(&["rm", &self.name])?;
        if !removed.status.success() && !says(&removed, "No such container") {
            return Err(self.failure("rm", &removed));
        }
        Ok(())
    }

    /// What the container is doing now.
    ///
    /// # Errors
    /// Reports a Docker call that fails, other than the absence of the container.
    pub fn status(&self) -> Result<LocalStatus> {
        let running = self.running()?;
        // The image of the container that runs, not the one this call would start:
        // a person who passed another tag must read what is actually there.
        let image = self
            .container_image()?
            .filter(|image| !image.is_empty())
            .unwrap_or_else(|| self.image.clone());
        Ok(LocalStatus {
            running,
            image,
            vnc_address: self.vnc_socket().to_string(),
            ssh_address: self.ssh_address(),
            volume: self.volume(),
        })
    }

    /// The private key that opens the container, made once and kept.
    fn ensure_key(&self) -> Result<PathBuf> {
        let key = self.key_path();
        let Some(folder) = key.parent() else {
            return Err(Error::Local("the key of the container has no folder".into()));
        };
        std::fs::create_dir_all(folder)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        }
        if key.exists() {
            return Ok(key);
        }
        let output = Command::new(&self.keygen)
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "horizon-browser-vm-local", "-f"])
            .arg(&key)
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(Error::Local(format!(
                "ssh-keygen could not write {}. {}",
                key.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(key)
    }

    fn build(&self, repository: &Path) -> Result<()> {
        let output = self.docker(
            &[
                "build",
                "--file",
                "packaging/browser-vm/Dockerfile",
                "--target",
                "browser-vm-local",
                "--tag",
                self.image.as_str(),
                ".",
            ],
            repository,
        )?;
        if !output.status.success() {
            return Err(Error::Local(format!(
                "the image {} did not build: {}",
                self.image,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn run(&self, public_key: &str) -> Result<()> {
        let vnc = format!("127.0.0.1:{}:{}", self.vnc_port, CONTAINER_VNC_PORT);
        let ssh = format!("127.0.0.1:{}:{}", self.ssh_port, CONTAINER_SSH_PORT);
        let volume = format!("{}:/state", self.volume());
        let key = format!("{KEY_ENV}={}", public_key.trim());
        let output = self.docker(
            &[
                "run",
                "--detach",
                "--name",
                self.name.as_str(),
                "--volume",
                volume.as_str(),
                "--publish",
                vnc.as_str(),
                "--publish",
                ssh.as_str(),
                "--shm-size",
                "1g",
                "--env",
                SSH_ENV,
                "--env",
                key.as_str(),
                self.image.as_str(),
            ],
            Path::new("."),
        )?;
        if !output.status.success() {
            return Err(Error::Local(format!(
                "the container {} did not start: {}",
                self.name,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// The host key that the SSH server of the container presents, read through Docker so that
    /// no connection is trusted before the key is known.
    fn wait_for_host_key(&self) -> Result<String> {
        let deadline = Instant::now() + KEY_WAIT;
        let mut last = String::new();
        while Instant::now() < deadline && self.running()? {
            let output = self.docker(
                &["exec", self.name.as_str(), "cat", "/state/ssh/ssh_host_ed25519_key.pub"],
                Path::new("."),
            )?;
            let key = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if output.status.success() && key.starts_with("ssh-ed25519 ") {
                return Ok(key);
            }
            String::from_utf8_lossy(&output.stderr).trim().clone_into(&mut last);
            std::thread::sleep(KEY_PAUSE);
        }
        Err(Error::Local(format!(
            "the SSH server of {} named no host key within {} s. {last}",
            self.name,
            KEY_WAIT.as_secs()
        )))
    }

    fn write_entry(&self, key: &Path, host_key: &str) -> Result<Route> {
        let Some(home) = self.home.as_deref() else {
            return Ok(Route {
                ssh_target: "root@127.0.0.1".to_owned(),
                note: Some(format!(
                    "Horizon did not write the ssh entry {ALIAS}: it does not know the home directory of this computer"
                )),
            });
        };
        // The host key comes from the container itself through Docker, so the entry trusts the
        // key that the container presents and no key that the network named first.
        let entry = ssh_entry::Entry::with_port_and_keys(home, &self.root, self.ssh_port, host_key);
        let trust = entry.write("127.0.0.1", key)?;
        Ok(Route {
            ssh_target: ALIAS.to_owned(),
            note: trust.note,
        })
    }

    fn running(&self) -> Result<bool> {
        if !self.container_exists()? {
            return Ok(false);
        }
        let output = self.output(&["container", "inspect", "--format", "{{.State.Running}}", &self.name])?;
        if !output.status.success() {
            return Err(self.failure("inspect", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    /// The image of the container that holds this name, or nothing when no container does.
    fn container_image(&self) -> Result<Option<String>> {
        if !self.container_exists()? {
            return Ok(None);
        }
        let output = self.output(&["container", "inspect", "--format", "{{.Config.Image}}", &self.name])?;
        if !output.status.success() {
            return Err(self.failure("inspect", &output));
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
    }

    /// The environment of the container that holds this name, one `NAME=value` per line.
    fn container_env(&self) -> Result<String> {
        let output = self.output(&[
            "container",
            "inspect",
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
            &self.name,
        ])?;
        if !output.status.success() {
            return Err(self.failure("inspect", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether the container exists. An absent container is a no; a Docker that cannot answer
    /// is an error, so a stop never reports success when the daemon is down.
    fn container_exists(&self) -> Result<bool> {
        let output = self.output(&["container", "inspect", &self.name])?;
        if output.status.success() {
            return Ok(true);
        }
        if says(&output, "No such object") || says(&output, "No such container") {
            return Ok(false);
        }
        Err(self.failure("inspect", &output))
    }

    /// Whether the image is local. As with the container, a Docker that cannot answer is an
    /// error rather than an absent image that Horizon would try to build.
    fn image_exists(&self) -> Result<bool> {
        let output = self.output(&["image", "inspect", &self.image])?;
        if output.status.success() {
            return Ok(true);
        }
        if says(&output, "No such image") {
            return Ok(false);
        }
        Err(self.failure("image inspect", &output))
    }

    fn failure(&self, call: &str, output: &std::process::Output) -> Error {
        Error::Local(format!(
            "docker {call} of {} failed: {}",
            self.name,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }

    fn output(&self, args: &[&str]) -> Result<std::process::Output> {
        self.docker(args, Path::new("."))
    }

    fn docker(&self, args: &[&str], current_dir: &Path) -> Result<std::process::Output> {
        let output = Command::new(&self.docker)
            .args(args)
            .current_dir(current_dir)
            .stdin(Stdio::null())
            .output()?;
        Ok(output)
    }
}

/// Whether the message of a Docker call names `needle`, which is how a call reports the state
/// the caller asked for instead of a failure.
fn says(output: &std::process::Output, needle: &str) -> bool {
    String::from_utf8_lossy(&output.stderr).contains(needle)
}

#[cfg(all(test, unix))]
#[path = "local/tests.rs"]
mod tests;
