//! The Docker host card of Cloud settings, beside `RunPod` and Hetzner: the saved hosts
//! with what each has free, and an editor that adds or changes one and checks it
//! read-only on a worker thread. Saving keeps a name and how to reach the host; no
//! secret is stored.
use super::dashboard::{Tone, caption, field, header, label, surface};
use crate::{app::util::chrome_button, theme};
use egui::{Align, Color32, Layout, RichText, Sense, Ui, vec2};
use horizon_core::cloud_runtime::{
    Cancellation,
    docker_host::{self, Access, Capacity, Check, CheckState, Host, hosts::Reading},
    provider,
    setup::Draft,
};
use std::sync::mpsc::{Receiver, channel};

const ACCESS: [(Access, &str, &str); 3] = [
    (Access::ThisPc, "This PC", "Docker on this computer"),
    (Access::Ssh, "SSH", "Your keys and agent"),
    (Access::TailscaleSsh, "Tailscale SSH", "Your tailnet's SSH rules"),
];

enum Step {
    Row(Check),
    Done(Option<Capacity>),
}

/// One host being added or changed. Dropping it cancels a check that is still running.
struct Editor {
    /// The saved name of the host being changed; `None` adds one.
    original: Option<String>,
    name: String,
    access: Access,
    address: String,
    docker: String,
    cancel: Cancellation,
    check: Option<Receiver<Step>>,
    rows: Vec<Check>,
    /// Whether the last check ran to its end.
    checked: bool,
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Editor {
    fn new(saved: Option<&Host>) -> Self {
        Self {
            original: saved.map(|host| host.name.clone()),
            name: saved.map(|host| host.name.clone()).unwrap_or_default(),
            access: saved.map_or(Access::Ssh, |host| host.access),
            address: saved.and_then(|host| host.address.clone()).unwrap_or_default(),
            docker: saved.and_then(|host| host.docker.clone()).unwrap_or_default(),
            cancel: Cancellation::default(),
            check: None,
            rows: Vec::new(),
            checked: false,
        }
    }

    fn host(&self) -> Host {
        let text = |value: &str| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        Host {
            name: self.name.trim().to_owned(),
            access: self.access,
            address: (self.access != Access::ThisPc).then(|| text(&self.address)).flatten(),
            docker: text(&self.docker),
        }
    }

    /// Why the host cannot be kept as typed, if it cannot.
    fn problem(&self, saved: &[Host]) -> Option<String> {
        let host = self.host();
        if let Err(error) = host.validate() {
            return Some(error.to_string());
        }
        if self.original.is_none() && saved.len() >= horizon_core::cloud_runtime::settings::MAX_DOCKER_HOSTS {
            return Some("Horizon keeps at most 16 Docker hosts. Remove one first.".into());
        }
        let taken = saved
            .iter()
            .any(|other| other.name == host.name && self.original.as_ref() != Some(&other.name));
        taken.then(|| format!("Another saved host is named {}.", host.name))
    }

    fn passed(&self) -> bool {
        self.checked && self.rows.iter().all(|row| row.state != CheckState::Blocked)
    }

    fn start(&mut self, root: &std::path::Path, ctx: &egui::Context) {
        self.cancel.cancel();
        self.cancel = Cancellation::default();
        self.rows.clear();
        self.checked = false;
        let (sender, receiver) = channel();
        self.check = Some(receiver);
        let (host, root, cancel, ctx) = (self.host(), root.to_owned(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let capacity = docker_host::check(&host, &root, &cancel, &mut |row| {
                let _ = sender.send(Step::Row(row));
                ctx.request_repaint();
            });
            let _ = sender.send(Step::Done(capacity));
            ctx.request_repaint();
        });
    }

    /// Collects the rows sent so far, and keeps what a finished check read.
    fn poll(&mut self, hosts: &mut super::super::prices::hosts::State) {
        let Some(receiver) = &self.check else { return };
        for step in receiver.try_iter().collect::<Vec<_>>() {
            match step {
                Step::Row(row) => self.rows.push(row),
                Step::Done(capacity) => {
                    self.check = None;
                    self.checked = true;
                    if let Some(capacity) = capacity {
                        hosts.record(Reading {
                            host: self.host(),
                            capacity: Ok(capacity),
                        });
                    }
                }
            }
        }
    }
}

/// The card's own state: the host being edited, and whether the card was switched on
/// before any host was saved.
#[derive(Default)]
pub(super) struct Card {
    editor: Option<Editor>,
    turned_on: bool,
}

impl Card {
    /// A host being added or changed is not in the settings yet, so Save waits for it.
    pub(super) fn editing(&self) -> bool {
        self.editor.is_some()
    }
}

pub(super) fn card(ui: &mut Ui, draft: &mut Draft, card: &mut Card, hosts: &mut super::super::prices::hosts::State) {
    if let Some(editor) = &mut card.editor {
        editor.poll(hosts);
    }
    let saved = !draft.settings.docker_hosts.is_empty();
    surface(ui, |ui| {
        header(
            ui,
            "Docker host",
            "Your own machine · no rent",
            Some(status(draft, card, hosts)),
        );
        let mut on = if saved {
            !draft.settings.docker_hosts_off
        } else {
            card.turned_on || card.editor.is_some()
        };
        let text = RichText::new("Use a Docker host for clouds")
            .size(13.0)
            .color(theme::FG());
        if ui.checkbox(&mut on, text).changed() {
            card.turned_on = on;
            draft.settings.docker_hosts_off = !on;
            card.editor = (on && !saved).then(|| Editor::new(None));
        }
        if !on {
            caption(
                ui,
                "This PC, a Linux server over SSH, or a machine on your tailnet. Each cloud gets its own container and \
                 volume there. Saved hosts are kept while this is off, and clouds on them keep working.",
            );
            return;
        }
        for host in draft.settings.docker_hosts.clone() {
            if card
                .editor
                .as_ref()
                .is_some_and(|editor| editor.original.as_ref() == Some(&host.name))
            {
                continue;
            }
            if row(ui, &host, hosts.of(&host)) && card.editor.is_none() {
                card.editor = Some(Editor::new(Some(&host)));
            }
        }
        match &mut card.editor {
            Some(editor) => {
                let draft_root = draft.root().to_owned();
                if let Some(done) = edit(ui, editor, &draft.settings.docker_hosts, &draft_root) {
                    apply(draft, editor, done);
                    card.editor = None;
                }
            }
            None => {
                if ui.add(chrome_button("Add host").min_size(vec2(96.0, 28.0))).clicked() {
                    card.editor = Some(Editor::new(None));
                }
            }
        }
    });
}

fn status(draft: &Draft, card: &Card, hosts: &super::super::prices::hosts::State) -> (Tone, &'static str) {
    let saved = &draft.settings.docker_hosts;
    if card.editor.is_some() {
        (Tone::Attention, "Unsaved host")
    } else if saved.is_empty() || draft.settings.docker_hosts_off {
        (Tone::Idle, "Off")
    } else if saved
        .iter()
        .any(|host| hosts.of(host).is_some_and(|reading| reading.capacity.is_err()))
    {
        (Tone::Attention, "Not reachable")
    } else if saved.iter().any(|host| {
        hosts
            .of(host)
            .is_some_and(|reading| reading.capacity.as_ref().is_ok_and(|capacity| !capacity.native()))
    }) {
        (Tone::Attention, "Architecture not supported")
    } else {
        (Tone::Ready, "Ready")
    }
}

/// A saved host: how it is reached and what it has free. Returns whether Edit was clicked.
fn row(ui: &mut Ui, host: &Host, reading: Option<&Reading>) -> bool {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        super::super::creation::provider::mark(ui, &provider::DOCKER_HOST, 28.0);
        let width = (ui.available_width() - 76.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(&host.name).size(14.0).strong().color(theme::FG()));
            let way = ACCESS
                .iter()
                .find(|(access, ..)| *access == host.access)
                .map_or("", |(_, name, _)| name);
            let reach = host
                .address
                .as_ref()
                .map_or_else(|| way.to_owned(), |address| format!("{address} · {way}"));
            ui.label(RichText::new(reach).size(12.5).color(theme::FG_SOFT()));
            let (facts, color) = match reading.map(|reading| &reading.capacity) {
                Some(Ok(capacity)) if !capacity.native() => (
                    format!("{}: the worker image has no build for it", capacity.architecture),
                    theme::PALETTE_RED(),
                ),
                Some(Ok(capacity)) => (
                    format!(
                        "{} · Docker {} · {}",
                        capacity.architecture,
                        capacity.version,
                        capacity.free()
                    ),
                    theme::FG_DIM(),
                ),
                Some(Err(error)) => (error.clone(), theme::PALETTE_RED()),
                None => ("Not checked yet".to_owned(), theme::FG_DIM()),
            };
            ui.add(egui::Label::new(RichText::new(facts).size(12.0).color(color)).wrap());
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.add(chrome_button("Edit").min_size(vec2(64.0, 28.0)))
                .on_hover_text(format!("Change or check {}", host.name))
                .clicked()
        })
        .inner
    })
    .inner
}

#[derive(Clone, Copy)]
enum Done {
    Keep,
    Remove,
    Cancel,
}

/// The editor's fields, its check and its buttons. Returns what ended the edit.
fn edit(ui: &mut Ui, editor: &mut Editor, saved: &[Host], root: &std::path::Path) -> Option<Done> {
    ui.separator();
    let checking = editor.check.is_some();
    let mut changed = false;
    ui.add_enabled_ui(!checking, |ui| {
        changed |= field(ui, "Name", &mut editor.name, "build-box").changed();
        label(ui, "How Horizon reaches it");
        ui.columns(ACCESS.len(), |columns| {
            for (ui, (access, title, detail)) in columns.iter_mut().zip(ACCESS) {
                let text = format!("{title}\n{detail}");
                let tile = egui::Button::selectable(editor.access == access, RichText::new(text).size(12.5))
                    .min_size(vec2(ui.available_width(), 44.0));
                if ui.add(tile).clicked() && editor.access != access {
                    editor.access = access;
                    changed = true;
                }
            }
        });
        if editor.access != Access::ThisPc {
            changed |= field(ui, "Address", &mut editor.address, "deploy@build-box").changed();
        }
        changed |= field(
            ui,
            "Docker path (optional)",
            &mut editor.docker,
            "/usr/local/bin/docker",
        )
        .changed();
        caption(ui, explain(editor.access));
    });
    if changed {
        // A check of what was typed before says nothing about the host now.
        editor.cancel.cancel();
        editor.check = None;
        editor.rows.clear();
        editor.checked = false;
    }
    let problem = editor.problem(saved);
    ui.horizontal(|ui| {
        let start = ui
            .add_enabled(
                problem.is_none() && !checking,
                egui::Button::new("Check host").min_size(vec2(112.0, 30.0)),
            )
            .clicked();
        if start {
            editor.start(root, ui.ctx());
        }
        caption(ui, "Read-only: Horizon looks, and starts or installs nothing.");
    });
    if let Some(problem) = &problem {
        ui.colored_label(theme::PALETTE_YELLOW(), problem);
    }
    progress(ui, editor);
    let mut done = None;
    ui.horizontal(|ui| {
        let keep = egui::Button::new(if editor.original.is_some() {
            "Keep changes"
        } else {
            "Keep host"
        });
        if ui
            .add_enabled(problem.is_none() && editor.passed(), keep.min_size(vec2(112.0, 30.0)))
            .on_disabled_hover_text("Check the host first. A host whose check found no blocker can be kept.")
            .clicked()
        {
            done = Some(Done::Keep);
        }
        if ui.button("Cancel").clicked() {
            done = Some(Done::Cancel);
        }
        if editor.original.is_some() && ui.button("Remove host").clicked() {
            done = Some(Done::Remove);
        }
    });
    caption(
        ui,
        "Save settings to keep the change. Horizon keeps the name and how to reach the host; no secret is stored. \
         A cloud on a removed host cannot stop, resume or be deleted until the host is saved again under its name.",
    );
    done
}

fn explain(access: Access) -> &'static str {
    match access {
        Access::ThisPc => "Uses this computer's Docker engine. Nothing is published on the network.",
        Access::Ssh => {
            "Uses your SSH configuration, keys and agent; Horizon copies no key. The user needs Docker access \
             on the host, and Docker 28 or later."
        }
        Access::TailscaleSsh => {
            "Uses tailscale ssh. The host's Tailscale SSH rule must use action accept, as check mode waits for a \
             browser. The user needs Docker access on the host."
        }
    }
}

/// The rows as they arrive, then only what needs attention.
fn progress(ui: &mut Ui, editor: &Editor) {
    if editor.check.is_some() {
        for row in &editor.rows {
            check_row(ui, row, false);
        }
        ui.horizontal(|ui| {
            ui.spinner();
            caption(ui, "Checking…");
        });
    } else if editor.checked {
        let passed = editor.rows.iter().filter(|row| row.state == CheckState::Ready).count();
        for row in editor.rows.iter().filter(|row| row.state != CheckState::Ready) {
            check_row(ui, row, true);
        }
        if passed > 0 {
            let text = format!("{passed} of {} checks passed", editor.rows.len());
            ui.label(RichText::new(text).size(12.5).color(theme::PALETTE_GREEN()));
        }
    }
}

fn check_row(ui: &mut Ui, check: &Check, remedy: bool) {
    let color: Color32 = match check.state {
        CheckState::Ready => theme::PALETTE_GREEN(),
        CheckState::Warning => theme::PALETTE_YELLOW(),
        CheckState::Blocked => theme::PALETTE_RED(),
        CheckState::Unknown => theme::FG_DIM(),
    };
    ui.horizontal_wrapped(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.label(RichText::new(&check.name).size(12.5).strong().color(theme::FG()));
        ui.label(RichText::new(&check.detail).size(12.5).color(theme::FG_SOFT()));
    });
    if let Some(text) = check.remedy.as_ref().filter(|_| remedy) {
        ui.label(RichText::new(text).size(12.0).color(color));
    }
}

fn apply(draft: &mut Draft, editor: &Editor, done: Done) {
    let hosts = &mut draft.settings.docker_hosts;
    let position = editor
        .original
        .as_ref()
        .and_then(|name| hosts.iter().position(|host| host.name == *name));
    match (done, position) {
        (Done::Keep, Some(index)) => hosts[index] = editor.host(),
        (Done::Keep, None) => hosts.push(editor.host()),
        (Done::Remove, Some(index)) => drop(hosts.remove(index)),
        (Done::Remove | Done::Cancel, _) => {}
    }
    if matches!(done, Done::Keep) {
        draft.settings.docker_hosts_off = false;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    fn painted(draft: &mut Draft, card: &mut Card, hosts: &mut super::super::super::prices::hosts::State) -> String {
        let ctx = egui::Context::default();
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| super::card(ui, draft, card, hosts))
            .discard_textures();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn hosts_are_added_after_a_check_without_blockers_and_show_what_they_have_free() {
        let temp = tempfile::tempdir().unwrap();
        let mut draft = Draft::load(temp.path()).unwrap();
        let (mut card, mut hosts) = (Card::default(), super::super::super::prices::hosts::State::default());
        assert!(painted(&mut draft, &mut card, &mut hosts).contains("Off"));
        let mut editor = Editor::new(None);
        editor.name = "build box".into();
        editor.address = "deploy@build-box".into();
        assert!(editor.problem(&[]).is_some(), "a name with a space is refused");
        editor.name = "build-box".into();
        assert!(editor.problem(&[]).is_none() && !editor.passed());
        let row = |name: &str, state| Check {
            name: name.into(),
            state,
            detail: format!("{name} detail"),
            remedy: Some(format!("{name} remedy")),
        };
        editor.rows = vec![
            row("Connection", CheckState::Ready),
            row("Free capacity", CheckState::Warning),
        ];
        editor.checked = true;
        assert!(editor.passed(), "a warning does not block");
        card.editor = Some(editor);
        let text = painted(&mut draft, &mut card, &mut hosts);
        assert!(
            text.contains("Free capacity remedy") && text.contains("1 of 2 checks passed"),
            "{text}"
        );
        assert!(
            !text.contains("Connection detail"),
            "passed rows fold away once the check ends"
        );
        apply(&mut draft, card.editor.as_ref().unwrap(), Done::Keep);
        card.editor = None;
        assert_eq!(
            draft.settings.docker_hosts[0].address.as_deref(),
            Some("deploy@build-box")
        );
        // A Docker host alone is somewhere for clouds to run.
        draft.settings.default_agents = vec![horizon_core::cloud_runtime::setup::Agent::Claude];
        assert!(draft.validate().is_ok());
        let text = painted(&mut draft, &mut card, &mut hosts);
        assert!(
            text.contains("deploy@build-box · SSH") && text.contains("Not checked yet"),
            "{text}"
        );
        // A host whose architecture has no worker image build is not ready.
        hosts.answered(vec![Reading {
            host: draft.settings.docker_hosts[0].clone(),
            capacity: Ok(Capacity {
                cpus: 4,
                memory_bytes: 8 << 30,
                free_nano_cpus: 4_000_000_000,
                free_memory_bytes: 8 << 30,
                architecture: "riscv64".into(),
                version: "28.5.1".into(),
            }),
        }]);
        let text = painted(&mut draft, &mut card, &mut hosts);
        assert!(
            text.contains("Architecture not supported") && text.contains("riscv64: the worker image"),
            "{text}"
        );
        hosts.answered(Vec::new());
        // Switched off, the host stays saved and is not offered, and the switch saves.
        draft.settings.docker_hosts_off = true;
        assert!(!draft.settings.docker_hosts_on() && draft.validate().is_ok());
        assert!(painted(&mut draft, &mut card, &mut hosts).contains("Off"));
        draft.settings.docker_hosts_off = false;
        let saved = Editor::new(Some(&draft.settings.docker_hosts[0]));
        apply(&mut draft, &saved, Done::Remove);
        assert!(draft.settings.docker_hosts.is_empty());
        // Each reading asks every saved host at once, so a machine keeps at most 16.
        let full: Vec<Host> = (0..16)
            .map(|index| Host {
                name: format!("host-{index}"),
                access: Access::ThisPc,
                address: None,
                docker: None,
            })
            .collect();
        let mut extra = Editor::new(None);
        extra.name = "one-more".into();
        extra.access = Access::ThisPc;
        assert!(extra.problem(&full).unwrap().contains("at most 16"));
        draft.settings.docker_hosts = full;
        draft.settings.docker_hosts.push(extra.host());
        assert!(draft.validate().is_err());
    }
}
