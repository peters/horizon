//! Build on: where the next setup's proof builds its image and runs the CI, this
//! computer's Docker, a Docker host saved in Cloud settings and reached over SSH, or a
//! builder Horizon rents from Hetzner. The settings are read on a thread, again every few
//! seconds while the dialog asks.
use crate::theme;
use egui::{RichText, Stroke, Vec2};
use horizon_core::cloud_runtime::docker_host::{Access, Host};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the saved Docker hosts are read again while the setup choices show.
const READ_EVERY: Duration = Duration::from_secs(3);

/// Where a setup builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Target {
    /// A saved Docker host on another machine.
    Host(Host),
    /// The builder Horizon rents for setups.
    Rented,
}

impl Target {
    /// The name its proofs and their leftovers record.
    pub(in crate::app::cloud_panel::production) fn name(&self) -> &str {
        match self {
            Self::Host(host) => &host.name,
            Self::Rented => horizon_core::cloud_runtime::setup_builder::NAME,
        }
    }
}

/// What the settings offer: the saved Docker hosts on other machines, and whether a
/// builder can be rented.
#[derive(Default)]
struct Offered {
    hosts: Vec<Host>,
    rentable: bool,
}

/// The places a setup can build on, and the one chosen for the next setup.
#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    hosts: Vec<Host>,
    rentable: bool,
    /// The chosen place; none for this computer.
    chosen: Option<Choice>,
    /// The read in flight, with the opening of the dialog it started in.
    job: Option<(u64, mpsc::Receiver<Offered>)>,
    read_at: Option<Instant>,
    /// Counts the openings of the dialog; each one reads the settings again.
    opening: u64,
    /// The opening whose read finished last; a host chosen earlier is used only after it
    /// is this opening.
    read_for: Option<u64>,
    /// The Starts that wait for this opening's read, with what each submitted; one for
    /// each repository.
    start_after_read: Vec<Submitted>,
    /// Why a waiting Start does not go on: the read found its place gone.
    gone: Option<String>,
}

/// What a Start submitted: the repository entered, the agent's preset and the mode. A
/// Start that waits goes on with these, whatever the dialog shows by then.
pub(in crate::app::cloud_panel::production) struct Submitted {
    pub(super) entered: String,
    pub(super) preset: horizon_core::PresetConfig,
    pub(super) mode: super::Mode,
}

/// What becomes of a Start that waited for the dialog's read of the Docker hosts.
pub(in crate::app::cloud_panel::production) enum Deferred {
    /// No Start waits, or its read has not finished.
    Waiting,
    /// The read finished and the Start goes on with what it submitted.
    Start(Box<Submitted>),
    /// The Starts do not go on: their place is gone, or the person changed Build on
    /// while they waited. The text says which.
    Gone(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Choice {
    /// A saved host, by name.
    Host(String),
    Rented,
}

impl State {
    /// Takes the hosts a read found and starts the next read when it is due, from the
    /// Cloud settings in `root`.
    pub(in crate::app::cloud_panel::production) fn refresh(&mut self, root: Option<&Path>, ctx: &egui::Context) {
        if let Some((opening, job)) = &self.job
            && let Ok(offered) = job.try_recv()
        {
            // A read of an earlier opening only frees the slot: the settings may have
            // changed again since it began, and this opening's read follows at once.
            let current = *opening == self.opening;
            self.job = None;
            if current {
                self.hosts = offered.hosts;
                self.rentable = offered.rentable;
                self.read_for = Some(self.opening);
                let gone = match &self.chosen {
                    Some(Choice::Host(chosen)) => !self.hosts.iter().any(|host| &host.name == chosen),
                    Some(Choice::Rented) => !self.rentable,
                    None => false,
                };
                if gone {
                    self.drop_choice();
                }
            }
        }
        let Some(root) = root else {
            // Without Cloud settings there is no host to build on.
            self.hosts.clear();
            self.drop_choice();
            self.read_for = Some(self.opening);
            return;
        };
        if self.job.is_some() {
            return;
        }
        if let Some(remaining) = self.read_at.and_then(|at| READ_EVERY.checked_sub(at.elapsed()))
            && !remaining.is_zero()
        {
            // The next read is due without any input.
            ctx.request_repaint_after(remaining);
            return;
        }
        self.read_at = Some(Instant::now());
        let (sender, receiver) = mpsc::channel();
        let (path, wake) = (root.join("settings.json"), ctx.clone());
        std::thread::spawn(move || {
            let _ = sender.send(offered(&path));
            wake.request_repaint();
        });
        self.job = Some((self.opening, receiver));
    }

    /// Builds on this computer from now on. A waiting Start does not go on then: the
    /// person chose another machine for it.
    fn drop_choice(&mut self) {
        if let Some(chosen) = self.chosen.take()
            && !std::mem::take(&mut self.start_after_read).is_empty()
        {
            self.gone = Some(match chosen {
                Choice::Host(name) => format!(
                    "The Docker host {name} is no longer in Cloud settings. Choose where the setup builds, then start it again."
                ),
                Choice::Rented => "Hetzner is no longer connected in Cloud settings, so no builder can be rented. Choose where the setup builds, then start it again.".into(),
            });
        }
    }

    /// Reads the settings again on the next refresh, for a new opening of the dialog. A
    /// Start that waited for the previous opening's read is dropped.
    pub(in crate::app::cloud_panel::production) fn read_again(&mut self) {
        self.read_at = None;
        self.opening += 1;
        self.start_after_read.clear();
        self.gone = None;
    }

    /// True when a host is chosen and this opening's read has not finished, so a Start
    /// waits for it: the host may have changed or gone in Cloud settings since.
    fn waiting(&self) -> bool {
        self.chosen.is_some() && self.read_for != Some(self.opening)
    }

    /// Keeps a Start for after this opening's read when it must wait for it; gives it
    /// back when it goes on now.
    pub(in crate::app::cloud_panel::production) fn defer_start(&mut self, submitted: Submitted) -> Option<Submitted> {
        if !self.waiting() {
            return Some(submitted);
        }
        self.start_after_read
            .retain(|waiting| waiting.entered != submitted.entered);
        self.start_after_read.push(submitted);
        None
    }

    /// What becomes of the deferred Starts once their read finished: one Start at a call,
    /// or once why none goes on.
    pub(in crate::app::cloud_panel::production) fn deferred(&mut self) -> Deferred {
        if let Some(reason) = self.gone.take() {
            return Deferred::Gone(reason);
        }
        if self.waiting() {
            return Deferred::Waiting;
        }
        if self.start_after_read.is_empty() {
            return Deferred::Waiting;
        }
        Deferred::Start(Box::new(self.start_after_read.remove(0)))
    }

    /// Where the next setup builds; none for this computer.
    pub(in crate::app::cloud_panel::production) fn chosen(&self) -> Option<Target> {
        match self.chosen.as_ref()? {
            Choice::Host(chosen) => self
                .hosts
                .iter()
                .find(|host| &host.name == chosen)
                .cloned()
                .map(Target::Host),
            Choice::Rented => Some(Target::Rented),
        }
    }

    /// Chooses where the next setup builds. A waiting Start does not go on then: it was
    /// submitted for the place chosen before.
    fn choose(&mut self, pick: Option<Choice>) {
        if pick == self.chosen {
            return;
        }
        self.chosen = pick;
        if !std::mem::take(&mut self.start_after_read).is_empty() {
            self.gone = Some("Build on changed before the setup started, so it did not start. Start it again.".into());
        }
    }

    /// The choice as chips under the agent chips: This computer, each saved Docker host
    /// on another machine, then Rented builder. Nothing shows while there is no other
    /// place than this computer.
    pub(in crate::app::cloud_panel::production) fn show(&mut self, ui: &mut egui::Ui) {
        if self.hosts.is_empty() && !self.rentable {
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
                let selected = self.chosen == Some(Choice::Host(host.name.clone()));
                let address = host.address.as_deref().unwrap_or_default();
                let picked = chip(ui, &host.name, selected)
                    .on_hover_text(format!(
                        "The proof builds the image and runs the CI with the Docker of {address}, reached over SSH. The agent still runs here."
                    ))
                    .clicked();
                if picked {
                    pick = Some(Some(Choice::Host(host.name.clone())));
                }
            }
            if self.rentable {
                let rented = chip(ui, "Rented builder", self.chosen == Some(Choice::Rented)).on_hover_text(
                    "Horizon rents a Hetzner server with Docker for the proof, shares it among the setups that build on it and deletes it 10 minutes after the last one. Its cost shows on the setup board. The agent still runs here.",
                );
                if rented.clicked() {
                    pick = Some(Some(Choice::Rented));
                }
            }
            if let Some(pick) = pick {
                self.choose(pick);
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

/// The saved Docker hosts on another machine, when Docker hosts are on in Cloud settings,
/// and whether a builder can be rented; nothing when the settings cannot be read.
fn offered(path: &Path) -> Offered {
    let Ok(settings) = horizon_core::cloud_runtime::settings::Settings::load(path) else {
        return Offered::default();
    };
    Offered {
        rentable: horizon_core::cloud_runtime::setup_builder::available(&settings),
        hosts: if settings.docker_hosts_on() {
            on_other_machines(settings.docker_hosts)
        } else {
            Vec::new()
        },
    }
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

    fn submitted(entered: &str) -> Submitted {
        Submitted {
            entered: entered.into(),
            preset: horizon_core::PresetConfig {
                name: "Claude".into(),
                alias: None,
                kind: horizon_core::PanelKind::Claude,
                command: None,
                args: Vec::new(),
                resume: horizon_core::PanelResume::default(),
                ssh_connection: None,
            },
            mode: super::super::Mode::default(),
        }
    }

    fn host(name: &str, access: Access) -> Host {
        Host {
            name: name.into(),
            access,
            address: (access != Access::ThisPc).then(|| format!("me@{name}")),
            docker: None,
            pinned: None,
        }
    }

    #[test]
    fn a_chosen_host_that_is_no_longer_saved_falls_back_to_this_computer() {
        let mut state = State {
            hosts: vec![host("builder", Access::Ssh)],
            chosen: Some(Choice::Host("builder".into())),
            ..State::default()
        };
        assert_eq!(state.chosen(), Some(Target::Host(host("builder", Access::Ssh))));
        let (sender, receiver) = mpsc::channel();
        sender.send(Offered::default()).unwrap();
        state.job = Some((0, receiver));
        state.read_at = Some(Instant::now());
        state.refresh(None, &egui::Context::default());
        assert!(state.chosen().is_none());
        assert!(state.chosen.is_none(), "the next setup builds on this computer");
    }

    #[test]
    fn a_start_waits_for_the_read_of_this_opening() {
        let mut state = State {
            hosts: vec![host("builder", Access::Ssh)],
            chosen: Some(Choice::Host("builder".into())),
            read_for: Some(0),
            ..State::default()
        };
        assert!(
            state.defer_start(submitted("/work/a")).is_some(),
            "a host this opening read starts at once"
        );
        state.read_again();
        // A read of the previous opening is still in flight when the dialog opens again.
        let (sender, receiver) = mpsc::channel();
        state.job = Some((0, receiver));
        assert!(
            state.defer_start(submitted("/work/a")).is_none(),
            "a host chosen in an earlier opening waits"
        );
        let folder = tempfile::tempdir().unwrap();
        // That read saw the host removed; it was saved again before this opening.
        sender.send(Offered::default()).unwrap();
        let ctx = egui::Context::default();
        state.refresh(Some(folder.path()), &ctx);
        assert!(
            matches!(state.deferred(), Deferred::Waiting),
            "the previous opening's read does not count"
        );
        assert!(state.chosen.is_some(), "nor does it drop the choice");
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender
            .send(Offered {
                hosts: vec![host("builder", Access::Ssh)],
                rentable: false,
            })
            .unwrap();
        state.refresh(Some(folder.path()), &ctx);
        assert!(
            matches!(state.deferred(), Deferred::Start(start) if start.entered == "/work/a"),
            "this opening's read lets the Start go on with what it submitted"
        );
        assert!(matches!(state.deferred(), Deferred::Waiting), "the Start goes on once");
    }

    #[test]
    fn each_repository_keeps_its_waiting_start() {
        let mut state = State {
            hosts: vec![host("builder", Access::Ssh)],
            chosen: Some(Choice::Host("builder".into())),
            ..State::default()
        };
        state.read_again();
        for entered in ["/work/a", "/work/b", "/work/a"] {
            assert!(state.defer_start(submitted(entered)).is_none());
        }
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender
            .send(Offered {
                hosts: vec![host("builder", Access::Ssh)],
                rentable: false,
            })
            .unwrap();
        state.refresh(Some(tempfile::tempdir().unwrap().path()), &egui::Context::default());
        let mut started = Vec::new();
        while let Deferred::Start(start) = state.deferred() {
            started.push(start.entered);
        }
        assert_eq!(
            started,
            ["/work/b", "/work/a"],
            "a second Start of a repository replaces its first"
        );
    }

    #[test]
    fn a_waiting_start_whose_host_was_removed_does_not_go_on() {
        let mut state = State {
            hosts: vec![host("builder", Access::Ssh)],
            chosen: Some(Choice::Host("builder".into())),
            read_for: Some(0),
            ..State::default()
        };
        state.read_again();
        assert!(state.defer_start(submitted("/work/a")).is_none());
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender.send(Offered::default()).unwrap();
        state.refresh(None, &egui::Context::default());
        assert!(matches!(state.deferred(), Deferred::Gone(reason) if reason.contains("builder")));
        assert!(state.chosen().is_none(), "the next Start builds on this computer");
        assert!(matches!(state.deferred(), Deferred::Waiting), "it is told once");
    }

    #[test]
    fn changing_build_on_ends_the_waiting_starts() {
        let mut state = State {
            hosts: vec![host("a", Access::Ssh), host("b", Access::Ssh)],
            chosen: Some(Choice::Host("a".into())),
            ..State::default()
        };
        state.read_again();
        assert!(state.defer_start(submitted("/work/a")).is_none());
        state.choose(Some(Choice::Host("a".into())));
        assert!(matches!(state.deferred(), Deferred::Waiting), "the same place keeps it");
        state.choose(None);
        assert!(
            matches!(state.deferred(), Deferred::Gone(_)),
            "this computer does not take it over"
        );
        assert!(
            state.defer_start(submitted("/work/a")).is_some(),
            "a new Start goes on at once here"
        );
        state.choose(Some(Choice::Host("b".into())));
        assert!(state.defer_start(submitted("/work/a")).is_none());
        state.choose(Some(Choice::Host("a".into())));
        assert!(matches!(state.deferred(), Deferred::Gone(_)), "nor does another host");
        assert!(matches!(state.deferred(), Deferred::Waiting), "it is told once");
    }

    #[test]
    fn only_hosts_on_another_machine_are_offered() {
        let names: Vec<String> = on_other_machines(vec![host("local", Access::ThisPc), host("builder", Access::Ssh)])
            .into_iter()
            .map(|host| host.name)
            .collect();
        assert_eq!(names, ["builder"]);
        let folder = tempfile::tempdir().unwrap();
        let offered = offered(&folder.path().join("missing.json"));
        assert!(offered.hosts.is_empty() && !offered.rentable);
    }

    #[test]
    fn the_rented_builder_is_offered_only_while_hetzner_is_connected() {
        let mut state = State {
            rentable: true,
            chosen: Some(Choice::Rented),
            ..State::default()
        };
        assert_eq!(state.chosen(), Some(Target::Rented));
        let (sender, receiver) = mpsc::channel();
        sender.send(Offered::default()).unwrap();
        state.job = Some((0, receiver));
        state.read_at = Some(Instant::now());
        state.refresh(None, &egui::Context::default());
        assert_eq!(state.chosen(), None, "Hetzner was disconnected");
    }
}
