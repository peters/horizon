//! Build on: where the next setup's proof builds its image and runs the CI, this
//! computer's Docker or a Docker host saved in Cloud settings and reached over SSH. The
//! saved hosts are read on a thread, again every few seconds while the dialog asks.
use crate::theme;
use egui::{RichText, Stroke, Vec2};
use horizon_core::cloud_runtime::docker_host::{Access, Host};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the saved Docker hosts are read again while the setup choices show.
const READ_EVERY: Duration = Duration::from_secs(10);

/// The Docker hosts a setup can build on, and the one chosen for the next setup.
#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    hosts: Vec<Host>,
    /// The chosen host by name; none for this computer.
    chosen: Option<String>,
    job: Option<mpsc::Receiver<Vec<Host>>>,
    read_at: Option<Instant>,
}

impl State {
    /// Takes the hosts a read found and starts the next read when it is due, from the
    /// Cloud settings in `root`.
    pub(in crate::app::cloud_panel::production) fn refresh(&mut self, root: Option<&Path>, ctx: &egui::Context) {
        if let Some(job) = &self.job
            && let Ok(hosts) = job.try_recv()
        {
            self.hosts = hosts;
            self.job = None;
            if self
                .chosen
                .as_ref()
                .is_some_and(|chosen| !self.hosts.iter().any(|host| &host.name == chosen))
            {
                self.chosen = None;
            }
        }
        let Some(root) = root else {
            return;
        };
        if self.job.is_some() || self.read_at.is_some_and(|at| at.elapsed() < READ_EVERY) {
            return;
        }
        self.read_at = Some(Instant::now());
        let (sender, receiver) = mpsc::channel();
        let (path, wake) = (root.join("settings.json"), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(remote_hosts(&path));
            wake.request_repaint();
        });
        self.job = Some(receiver);
    }

    /// The Docker host the next setup builds on; none for this computer.
    pub(in crate::app::cloud_panel::production) fn chosen(&self) -> Option<Host> {
        let chosen = self.chosen.as_ref()?;
        self.hosts.iter().find(|host| &host.name == chosen).cloned()
    }

    /// The choice as chips under the agent chips: This computer, then each saved Docker
    /// host on another machine. Nothing shows while there is no such host.
    pub(in crate::app::cloud_panel::production) fn show(&mut self, ui: &mut egui::Ui) {
        if self.hosts.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            ui.label(RichText::new("Build on").size(12.5).color(theme::FG_SOFT()));
            let mut pick = None;
            let this = chip(ui, "This computer", self.chosen.is_none())
                .on_hover_text("The proof builds the image and runs the CI with this computer's Docker.");
            if this.clicked() {
                pick = Some(None);
            }
            for host in &self.hosts {
                let selected = self.chosen.as_deref() == Some(host.name.as_str());
                let address = host.address.as_deref().unwrap_or_default();
                let picked = chip(ui, &host.name, selected)
                    .on_hover_text(format!(
                        "The proof builds the image and runs the CI with the Docker of {address}, reached over SSH. The agent still runs here."
                    ))
                    .clicked();
                if picked {
                    pick = Some(Some(host.name.clone()));
                }
            }
            if let Some(pick) = pick {
                self.chosen = pick;
            }
        });
    }
}

fn chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let fill = if selected {
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22)
    } else {
        theme::PANEL_BG_ALT()
    };
    let text = RichText::new(label)
        .size(12.5)
        .color(if selected { theme::FG() } else { theme::FG_SOFT() });
    let stroke = if selected {
        theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)
    } else {
        theme::BORDER_SUBTLE()
    };
    ui.add(
        egui::Button::new(text)
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 28.0))
            .truncate(),
    )
}

/// The saved Docker hosts on another machine, when Docker hosts are on in Cloud settings;
/// none when the settings cannot be read.
fn remote_hosts(path: &Path) -> Vec<Host> {
    let Ok(settings) = horizon_core::cloud_runtime::settings::Settings::load(path) else {
        return Vec::new();
    };
    if !settings.docker_hosts_on() {
        return Vec::new();
    }
    on_other_machines(settings.docker_hosts)
}

/// The valid hosts of `hosts` that are not this computer.
fn on_other_machines(hosts: Vec<Host>) -> Vec<Host> {
    hosts
        .into_iter()
        .filter(|host| host.access != Access::ThisPc && host.validate().is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, access: Access) -> Host {
        Host {
            name: name.into(),
            access,
            address: (access != Access::ThisPc).then(|| format!("me@{name}")),
            docker: None,
        }
    }

    #[test]
    fn a_chosen_host_that_is_no_longer_saved_falls_back_to_this_computer() {
        let mut state = State {
            hosts: vec![host("builder", Access::Ssh)],
            chosen: Some("builder".into()),
            ..State::default()
        };
        assert_eq!(state.chosen().map(|host| host.name), Some("builder".into()));
        let (sender, receiver) = mpsc::channel();
        sender.send(Vec::new()).unwrap();
        state.job = Some(receiver);
        state.read_at = Some(Instant::now());
        state.refresh(None, &egui::Context::default());
        assert!(state.chosen().is_none());
        assert!(state.chosen.is_none(), "the next setup builds on this computer");
    }

    #[test]
    fn only_hosts_on_another_machine_are_offered() {
        let names: Vec<String> = on_other_machines(vec![host("local", Access::ThisPc), host("builder", Access::Ssh)])
            .into_iter()
            .map(|host| host.name)
            .collect();
        assert_eq!(names, ["builder"]);
        let folder = tempfile::tempdir().unwrap();
        assert!(remote_hosts(&folder.path().join("missing.json")).is_empty());
    }
}
