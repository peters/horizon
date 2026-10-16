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
const READ_EVERY: Duration = Duration::from_secs(3);

/// The Docker hosts a setup can build on, and the one chosen for the next setup.
#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    hosts: Vec<Host>,
    /// The chosen host by name; none for this computer.
    chosen: Option<String>,
    /// The read in flight, with the opening of the dialog it started in.
    job: Option<(u64, mpsc::Receiver<Vec<Host>>)>,
    read_at: Option<Instant>,
    /// Counts the openings of the dialog; each one reads the settings again.
    opening: u64,
    /// The opening whose read finished last; a host chosen earlier is used only after it
    /// is this opening.
    read_for: Option<u64>,
    /// The Starts that wait for this opening's read, with what each submitted; one for
    /// each repository.
    start_after_read: Vec<Submitted>,
    /// The host a waiting Start chose, when the read found it no longer saved.
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
    /// The Starts do not go on: their host is no longer saved, or the person changed
    /// Build on while they waited. The text says which.
    Gone(String),
}

impl State {
    /// Takes the hosts a read found and starts the next read when it is due, from the
    /// Cloud settings in `root`.
    pub(in crate::app::cloud_panel::production) fn refresh(&mut self, root: Option<&Path>, ctx: &egui::Context) {
        if let Some((opening, job)) = &self.job
            && let Ok(hosts) = job.try_recv()
        {
            // A read of an earlier opening only frees the slot: the settings may have
            // changed again since it began, and this opening's read follows at once.
            let current = *opening == self.opening;
            self.job = None;
            if current {
                self.hosts = hosts;
                self.read_for = Some(self.opening);
                if self
                    .chosen
                    .as_ref()
                    .is_some_and(|chosen| !self.hosts.iter().any(|host| &host.name == chosen))
                {
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
            let _ = sender.send(remote_hosts(&path));
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
            self.gone = Some(format!(
                "The Docker host {chosen} is no longer in Cloud settings. Choose where the setup builds, then start it again."
            ));
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

    /// The Docker host the next setup builds on; none for this computer.
    pub(in crate::app::cloud_panel::production) fn chosen(&self) -> Option<Host> {
        let chosen = self.chosen.as_ref()?;
        self.hosts.iter().find(|host| &host.name == chosen).cloned()
    }

    /// Chooses where the next setup builds. A waiting Start does not go on then: it was
    /// submitted for the place chosen before.
    fn choose(&mut self, pick: Option<String>) {
        if pick == self.chosen {
            return;
        }
        self.chosen = pick;
        if !std::mem::take(&mut self.start_after_read).is_empty() {
            self.gone = Some("Build on changed before the setup started, so it did not start. Start it again.".into());
        }
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
            chosen: Some("builder".into()),
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
        sender.send(Vec::new()).unwrap();
        let ctx = egui::Context::default();
        state.refresh(Some(folder.path()), &ctx);
        assert!(
            matches!(state.deferred(), Deferred::Waiting),
            "the previous opening's read does not count"
        );
        assert!(state.chosen.is_some(), "nor does it drop the choice");
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender.send(vec![host("builder", Access::Ssh)]).unwrap();
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
            chosen: Some("builder".into()),
            ..State::default()
        };
        state.read_again();
        for entered in ["/work/a", "/work/b", "/work/a"] {
            assert!(state.defer_start(submitted(entered)).is_none());
        }
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender.send(vec![host("builder", Access::Ssh)]).unwrap();
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
            chosen: Some("builder".into()),
            read_for: Some(0),
            ..State::default()
        };
        state.read_again();
        assert!(state.defer_start(submitted("/work/a")).is_none());
        let (sender, receiver) = mpsc::channel();
        state.job = Some((1, receiver));
        sender.send(Vec::new()).unwrap();
        state.refresh(None, &egui::Context::default());
        assert!(matches!(state.deferred(), Deferred::Gone(reason) if reason.contains("builder")));
        assert!(state.chosen().is_none(), "the next Start builds on this computer");
        assert!(matches!(state.deferred(), Deferred::Waiting), "it is told once");
    }

    #[test]
    fn changing_build_on_ends_the_waiting_starts() {
        let mut state = State {
            hosts: vec![host("a", Access::Ssh), host("b", Access::Ssh)],
            chosen: Some("a".into()),
            ..State::default()
        };
        state.read_again();
        assert!(state.defer_start(submitted("/work/a")).is_none());
        state.choose(Some("a".into()));
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
        state.choose(Some("b".into()));
        assert!(state.defer_start(submitted("/work/a")).is_none());
        state.choose(Some("a".into()));
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
        assert!(remote_hosts(&folder.path().join("missing.json")).is_empty());
    }
}
