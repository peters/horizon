//! `horizon --browser-vm-local up|down|status`: the browser image of
//! `packaging/browser-vm` on the loopback of this computer.
//!
//! This launch opens no window. Every action talks to Docker and waits for a container, so
//! it runs before the event loop starts and reports its result as an exit status.

use std::path::{Path, PathBuf};

use horizon_core::cloud_runtime::browser_vm::local::{DEFAULT_VNC_PORT, LocalBrowserVm};

/// The action of the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Up,
    Down,
    Status,
}

impl Action {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "status" => Ok(Self::Status),
            _ => Err("--browser-vm-local needs up, down or status".into()),
        }
    }
}

/// The command and the container it acts on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocalCommand {
    action: Action,
    build: Option<PathBuf>,
    name: Option<String>,
    image: Option<String>,
    vnc_port: Option<u16>,
    ssh_port: Option<u16>,
}

impl LocalCommand {
    pub(crate) fn new(action: Action) -> Self {
        Self {
            action,
            build: None,
            name: None,
            image: None,
            vnc_port: None,
            ssh_port: None,
        }
    }

    /// Whether `flag` belongs to this command anywhere on the command line.
    pub(crate) fn handles(flag: &str) -> bool {
        matches!(flag, "--build" | "--name" | "--image" | "--vnc-port" | "--ssh-port")
    }

    /// Read the value of a flag of this command from `args`.
    pub(crate) fn apply(&mut self, flag: &str, args: &mut impl Iterator<Item = String>) -> Result<(), String> {
        match flag {
            "--build" => self.build = Some(operand(flag, args)?),
            "--name" | "--image" => {
                let value = match args.next() {
                    Some(value) if !value.is_empty() && !value.starts_with('-') => value,
                    _ => return Err(format!("{flag} needs a value")),
                };
                if flag == "--name" {
                    self.name = Some(value);
                } else {
                    self.image = Some(value);
                }
            }
            "--vnc-port" | "--ssh-port" => {
                let port = match args.next().map(|value| value.parse::<u16>()) {
                    Some(Ok(port)) if port > 0 => port,
                    _ => return Err(format!("{flag} needs a port between 1 and 65535")),
                };
                if flag == "--vnc-port" {
                    self.vnc_port = Some(port);
                } else {
                    self.ssh_port = Some(port);
                }
            }
            _ => return Err(format!("{flag} belongs to --browser-vm-local")),
        }
        Ok(())
    }

    /// Run the action and print what it did. `settings` is the path of the settings store,
    /// whose directory holds the key and the host keys of the container.
    pub(crate) fn run(&self, settings: &Path) -> i32 {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let root = settings.parent().unwrap_or(Path::new(".")).join("browser-vm");
        let mut vm = LocalBrowserVm::new(&root, home.as_deref());
        if let Some(name) = self.name.as_deref() {
            vm = vm.named(name);
        }
        if let Some(image) = self.image.as_deref() {
            vm = vm.with_image(image);
        }
        if let (Some(vnc), Some(ssh)) = (self.vnc_port, self.ssh_port) {
            vm = vm.with_ports(vnc, ssh);
        }
        let vnc_port = self.vnc_port.unwrap_or(DEFAULT_VNC_PORT);
        let result = match self.action {
            Action::Up => vm.up(self.build.as_deref()).map(|route| {
                println!(
                    "the container {} runs\nthe ssh target is {}\nthe Device panel takes 127.0.0.1:{vnc_port}",
                    vm.name(),
                    route.ssh_target
                );
                if let Some(note) = route.note {
                    println!("{note}");
                }
            }),
            Action::Down => vm.down().map(|()| {
                println!(
                    "the container {} stopped; the state volume {} keeps the sign-ins",
                    vm.name(),
                    vm.volume()
                );
            }),
            Action::Status => vm.status().map(|status| {
                println!(
                    "container: {}\nrunning: {}\nvnc: {}\nssh: {}\nvolume: {}\nimage: {}",
                    vm.name(),
                    if status.running { "yes" } else { "no" },
                    status.vnc_address,
                    status.ssh_address,
                    status.volume,
                    status.image
                );
            }),
        };
        match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("horizon: {error}");
                1
            }
        }
    }
}

/// The value of a flag that takes one operand.
fn operand(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<PathBuf, String> {
    match args.next() {
        Some(value) if !value.is_empty() && !value.starts_with('-') => Ok(PathBuf::from(value)),
        _ => Err(format!("{flag} needs a path")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, LocalCommand};

    fn apply(command: &mut LocalCommand, args: &[&str]) -> Result<(), String> {
        let mut args = args.iter().map(|arg| (*arg).to_string());
        let flag = args.next().expect("a flag");
        command.apply(&flag, &mut args)
    }

    #[test]
    fn every_action_has_a_word() {
        assert_eq!(Action::parse("up"), Ok(Action::Up));
        assert_eq!(Action::parse("down"), Ok(Action::Down));
        assert_eq!(Action::parse("status"), Ok(Action::Status));
        assert!(Action::parse("restart").is_err());
    }

    #[test]
    fn the_flags_of_the_command_fill_its_container() {
        let mut command = LocalCommand::new(Action::Up);
        for line in [
            ["--name", "probe"],
            ["--image", "horizon-browser-vm-local:test"],
            ["--vnc-port", "5991"],
            ["--ssh-port", "2291"],
            ["--build", "/src/horizon"],
        ] {
            apply(&mut command, &line).expect("a flag");
        }
        assert_eq!(command.name.as_deref(), Some("probe"));
        assert_eq!(command.image.as_deref(), Some("horizon-browser-vm-local:test"));
        assert_eq!(command.vnc_port, Some(5991));
        assert_eq!(command.ssh_port, Some(2291));
        assert_eq!(command.build.as_deref(), Some(std::path::Path::new("/src/horizon")));
    }

    #[test]
    fn a_flag_without_a_value_and_a_port_of_zero_are_refused() {
        let mut command = LocalCommand::new(Action::Up);
        assert!(apply(&mut command, &["--name"]).is_err());
        assert!(
            apply(&mut command, &["--name", "--image"]).is_err(),
            "a flag is not a name"
        );
        assert!(apply(&mut command, &["--vnc-port", "0"]).is_err());
        assert!(apply(&mut command, &["--ssh-port", "70000"]).is_err());
        assert!(apply(&mut command, &["--vnc-port", "x"]).is_err());
        assert!(apply(&mut command, &["--build"]).is_err());
        assert_eq!(command, LocalCommand::new(Action::Up), "a refused flag changes nothing");
    }

    #[test]
    fn the_command_owns_exactly_its_own_flags() {
        for flag in ["--build", "--name", "--image", "--vnc-port", "--ssh-port"] {
            assert!(LocalCommand::handles(flag), "{flag}");
        }
        for flag in ["--config", "--ephemeral", "--new-session", "--blank", "--export-config"] {
            assert!(!LocalCommand::handles(flag), "{flag}");
        }
    }
}
