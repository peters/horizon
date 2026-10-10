//! Restart Docker from a failure that a stuck or stopped Docker caused. A computer has
//! one Docker, so every card shows the same restart: confirmed once, it serves each
//! cloud's own Retry.
use super::{Failure, confirmation_card};
use crate::app::cloud_panel::runtime::{action_button, danger_button};
use crate::theme;
use egui::RichText;
use horizon_core::cloud_runtime::{
    diagnosis,
    docker_daemon::{self, Plan, RestartError, Target},
    settings::Settings,
};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// What the person sees of a restart.
#[derive(Clone, Debug, Default)]
enum Step {
    #[default]
    Idle,
    Checking,
    Confirm {
        target: Target,
        plan: Plan,
    },
    Restarting {
        doing: String,
        since: Instant,
    },
    Answered(String),
    Failed {
        error: String,
        command: Option<String>,
    },
}

/// Finds the cloud's Docker and how to restart it.
type Inspect = fn() -> (Target, Plan);
type Restart = fn(&Target, &[String], &dyn Fn()) -> Result<String, RestartError>;

struct State {
    step: Step,
    inspect: Inspect,
    restart: Restart,
}

impl Default for State {
    fn default() -> Self {
        Self {
            step: Step::Idle,
            inspect,
            restart: Target::restart,
        }
    }
}

/// The one restart of this window, kept in egui's memory so every card sees it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<State>>);

impl Shared {
    fn of(ctx: &egui::Context) -> Self {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Self>(egui::Id::new("cloud-docker-restart"))
                .clone()
        })
    }

    fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn set(&self, ctx: &egui::Context, step: Step) {
        self.with(|state| state.step = step);
        ctx.request_repaint();
    }

    /// Runs `work` off the UI thread.
    fn spawn(&self, ctx: &egui::Context, work: impl FnOnce(&Self, &egui::Context) + Send + 'static) {
        let (shared, thread_ctx) = (self.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("docker-restart".into())
            .spawn(move || work(&shared, &thread_ctx));
        if let Err(error) = spawned {
            self.set(
                ctx,
                Step::Failed {
                    error: format!("Horizon could not start the Docker restart: {error}."),
                    command: None,
                },
            );
        }
    }
}

fn inspect() -> (Target, Plan) {
    let path = horizon_core::HorizonHome::resolve()
        .root()
        .join("cloud")
        .join("settings.json");
    let target = Settings::load(&path).map_or_else(|_| Target::default(), |settings| Target::from_settings(&settings));
    let plan = docker_daemon::plan(&target.facts());
    (target, plan)
}

/// Whether `failure` came from a stuck or stopped Docker.
pub(super) fn offered(failure: &Failure) -> bool {
    failure.meaning.is_some_and(diagnosis::restarts_docker)
}

/// The offer in a failure's row of actions, until a restart is under way.
pub(super) fn button(ui: &mut egui::Ui, failure: &Failure) {
    if !offered(failure) {
        return;
    }
    let shared = Shared::of(ui.ctx());
    if !matches!(shared.with(|state| state.step.clone()), Step::Idle) {
        return;
    }
    if ui
        .add(action_button("Restart Docker…"))
        .on_hover_text("Find out how Docker runs on this computer and ask before restarting it")
        .clicked()
    {
        shared.set(ui.ctx(), Step::Checking);
        shared.spawn(ui.ctx(), |shared, ctx| {
            let (target, plan) = (shared.with(|state| state.inspect))();
            shared.set(ctx, Step::Confirm { target, plan });
        });
    }
}

/// The check, question, progress or result of a restart, under a failure's actions.
/// `retry` labels the cloud's own retry; true when the person chose it.
pub(super) fn status(ui: &mut egui::Ui, failure: &Failure, retry: Option<&str>) -> bool {
    if !offered(failure) {
        return false;
    }
    let shared = Shared::of(ui.ctx());
    let step = shared.with(|state| state.step.clone());
    let mut retried = false;
    match step {
        Step::Idle => return false,
        Step::Checking => busy(ui, "Checking how Docker runs on this computer…"),
        Step::Confirm {
            target,
            plan: plan @ Plan::Restart { .. },
        } => confirmation_card(ui, &question(&plan), |ui| {
            if ui.add(danger_button("Restart Docker")).clicked() {
                start(ui.ctx(), &shared, target, plan);
            }
            if ui.add(action_button("Cancel")).clicked() {
                shared.set(ui.ctx(), Step::Idle);
            }
        }),
        Step::Confirm {
            plan: Plan::Manual {
                instructions, command, ..
            },
            ..
        } => {
            ui.add_space(6.0);
            ui.label(RichText::new(instructions).size(13.0).color(theme::FG()));
            command_and_close(ui, &shared, command.as_deref());
        }
        Step::Restarting { doing, since } => {
            busy(ui, &format!("{doing} ({} s)", since.elapsed().as_secs()));
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        Step::Answered(version) => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Docker {version} answers again."))
                    .size(13.0)
                    .color(theme::PALETTE_GREEN()),
            );
            ui.horizontal(|ui| {
                if let Some(retry) = retry
                    && ui.add(action_button(retry)).clicked()
                {
                    retried = true;
                    shared.set(ui.ctx(), Step::Idle);
                }
                if ui.add(action_button("Close")).clicked() {
                    shared.set(ui.ctx(), Step::Idle);
                }
            });
        }
        Step::Failed { error, command } => {
            ui.add_space(6.0);
            ui.label(RichText::new(error).size(13.0).color(theme::PALETTE_RED()));
            if command.is_some() {
                ui.label(
                    RichText::new("Run this in a terminal, then retry:")
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
            }
            command_and_close(ui, &shared, command.as_deref());
        }
    }
    retried
}

fn start(ctx: &egui::Context, shared: &Shared, target: Target, plan: Plan) {
    let Plan::Restart { argv, shown, .. } = &plan else {
        return;
    };
    let argv = argv.clone();
    shared.set(
        ctx,
        Step::Restarting {
            doing: format!("Restarting Docker: {shown}"),
            since: Instant::now(),
        },
    );
    shared.spawn(ctx, move |shared, ctx| {
        let restart = shared.with(|state| state.restart);
        let waiting = || {
            shared.with(|state| {
                if let Step::Restarting { doing, .. } = &mut state.step {
                    "Waiting for Docker to answer…".clone_into(doing);
                }
            });
            ctx.request_repaint();
        };
        let step = match restart(&target, &argv, &waiting) {
            Ok(version) => Step::Answered(version),
            Err(error) => Step::Failed {
                error: format!("Docker did not come back: {error}."),
                command: plan.command().map(str::to_owned),
            },
        };
        shared.set(ctx, step);
    });
}

/// What restarting does, asked before it happens.
fn question(plan: &Plan) -> String {
    let Plan::Restart { shown, .. } = plan else {
        return String::new();
    };
    let rights = if plan.asks_password() {
        " Your computer asks for an administrator password."
    } else {
        ""
    };
    format!(
        "Restart Docker? This stops every container that runs in this Docker, also containers that \
         other programs started, then starts Docker again. Horizon runs: {shown}.{rights} When Docker \
         answers again, retry the cloud."
    )
}

fn busy(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0));
        ui.label(RichText::new(text).size(13.0).color(theme::FG_SOFT()));
    });
}

fn command_and_close(ui: &mut egui::Ui, shared: &Shared, command: Option<&str>) {
    if let Some(command) = command {
        ui.label(RichText::new(command).monospace().size(13.0).color(theme::FG()));
    }
    ui.horizontal(|ui| {
        if let Some(command) = command
            && ui.add(action_button("Copy command")).clicked()
        {
            ui.ctx().copy_text(command.to_owned());
        }
        if ui.add(action_button("Close")).clicked() {
            shared.set(ui.ctx(), Step::Idle);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use egui::{Event, PointerButton, Pos2, RawInput};

    const ROOTLESS: &str = "systemctl --user restart docker";

    fn failure(line: &str) -> Failure {
        Failure {
            summary: "worker image contract creation failed; inspect deployment output".into(),
            cause: Some(line.into()),
            meaning: diagnosis::meaning(line),
        }
    }

    fn stuck() -> Failure {
        failure("Docker did not answer docker version within 5 s")
    }

    fn rootless() -> (Target, Plan) {
        let argv = ["systemctl", "--user", "restart", "docker.service"]
            .map(str::to_owned)
            .to_vec();
        let (shown, command) = (ROOTLESS.into(), Some(ROOTLESS.into()));
        let plan = Plan::Restart { argv, shown, command };
        (Target::default(), plan)
    }

    /// One pass of a failure box: its texts with their centers, and whether Retry was chosen.
    fn frame(ctx: &egui::Context, failure: &Failure, events: Vec<Event>) -> (Vec<(String, Pos2)>, bool) {
        let mut retried = false;
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(720.0, 600.0));
        let input = RawInput {
            events,
            screen_rect: Some(screen),
            ..RawInput::default()
        };
        let output = ctx
            .run_ui(input, |ui| {
                ui.horizontal(|ui| button(ui, failure));
                retried = status(ui, failure, Some("Retry deploy"));
            })
            .discard_textures();
        let texts = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.visual_bounding_rect().center())),
            _ => None,
        });
        (texts.collect(), retried)
    }

    fn texts(ctx: &egui::Context, failure: &Failure) -> Vec<String> {
        frame(ctx, failure, Vec::new())
            .0
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    fn click(ctx: &egui::Context, failure: &Failure, label: &str) -> bool {
        let (shown, _) = frame(ctx, failure, Vec::new());
        let Some(&(_, at)) = shown.iter().find(|(text, _)| text == label) else {
            panic!("no {label} in {shown:?}");
        };
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(ctx, failure, vec![Event::PointerMoved(at)]);
        let _ = frame(ctx, failure, vec![press(true)]);
        frame(ctx, failure, vec![press(false)]).1
    }

    /// Passes until a text containing `wanted` shows, as a background job finishes.
    fn until(ctx: &egui::Context, failure: &Failure, wanted: &str) -> Vec<String> {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let shown = texts(ctx, failure);
            if shown.iter().any(|text| text.contains(wanted)) {
                return shown;
            }
            assert!(Instant::now() < deadline, "no {wanted} in {shown:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A window whose restart finds `inspect` and restarts with `restart`, then asks.
    fn asked(inspect: Inspect, restart: Restart) -> (egui::Context, Failure, Vec<String>) {
        let (ctx, failure) = (egui::Context::default(), stuck());
        Shared::of(&ctx).with(|state| (state.inspect, state.restart) = (inspect, restart));
        assert!(!click(&ctx, &failure, "Restart Docker…"));
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while matches!(Shared::of(&ctx).with(|state| state.step.clone()), Step::Checking) {
            assert!(Instant::now() < deadline, "the check never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shown = texts(&ctx, &failure);
        assert!(
            !shown.iter().any(|text| text == "Restart Docker…"),
            "one restart at a time"
        );
        (ctx, failure, shown)
    }

    #[test]
    fn only_a_failure_that_docker_caused_offers_a_restart() {
        for docker in [
            "Docker did not answer docker version within 5 s",
            "Cannot connect to the Docker daemon at unix:///run/user/1000/docker.sock",
            "Error response from daemon: Conflict. The container name \"/horizon-contract-1\" is already in use by container \"0123\".",
        ] {
            assert!(offered(&failure(docker)), "{docker}");
            assert_eq!(texts(&egui::Context::default(), &failure(docker)), ["Restart Docker…"]);
        }
        let registry = failure("error from registry: denied");
        assert!(!offered(&registry));
        assert!(texts(&egui::Context::default(), &registry).is_empty());
    }

    #[test]
    fn a_confirmed_restart_reports_docker_answering_and_offers_the_clouds_retry() {
        let (ctx, failure, question) = asked(rootless, |_, argv, waiting| {
            assert_eq!(argv, ["systemctl", "--user", "restart", "docker.service"]);
            waiting();
            Ok("29.8.1".into())
        });
        let asked = question
            .iter()
            .find(|text| text.contains("Restart Docker?"))
            .expect("the question");
        assert!(
            asked.contains("stops every container") && asked.contains(ROOTLESS),
            "{asked}"
        );
        assert!(!click(&ctx, &failure, "Restart Docker"));
        until(&ctx, &failure, "Docker 29.8.1 answers again.");
        assert!(
            click(&ctx, &failure, "Retry deploy"),
            "Retry runs the cloud's own retry"
        );
        assert_eq!(texts(&ctx, &failure), ["Restart Docker…"]);
    }

    #[test]
    fn cancelling_the_question_restarts_nothing() {
        let (ctx, failure, _) = asked(rootless, |_, _, _| panic!("a cancelled question restarts nothing"));
        click(&ctx, &failure, "Cancel");
        assert_eq!(texts(&ctx, &failure), ["Restart Docker…"]);
    }

    #[test]
    fn a_docker_horizon_must_not_restart_shows_what_to_do_instead() {
        let (ctx, failure, shown) = asked(
            || {
                let (instructions, command) = (
                    "Docker runs as a service.".into(),
                    Some("sudo systemctl restart docker".into()),
                );
                (Target::default(), Plan::Manual { instructions, command })
            },
            |_, _, _| panic!("Horizon runs nothing for a manual plan"),
        );
        for wanted in [
            "Docker runs as a service.",
            "sudo systemctl restart docker",
            "Copy command",
        ] {
            assert!(shown.iter().any(|text| text == wanted), "{wanted}: {shown:?}");
        }
        assert!(!shown.iter().any(|text| text == "Restart Docker"), "{shown:?}");
        click(&ctx, &failure, "Close");
        assert_eq!(texts(&ctx, &failure), ["Restart Docker…"]);
    }

    #[test]
    fn a_failed_restart_says_why_and_gives_the_command_to_run() {
        let (ctx, failure, _) = asked(rootless, |_, _, _| {
            let last = docker_daemon::Health::NotResponding;
            Err(RestartError::NotAnswering { last })
        });
        click(&ctx, &failure, "Restart Docker");
        let shown = until(&ctx, &failure, "Docker did not come back");
        let error = "Docker did not come back: Docker restarted but still does not answer.";
        assert!(shown.iter().any(|text| text == error), "{shown:?}");
        assert!(shown.iter().any(|text| text == ROOTLESS), "{shown:?}");
        assert!(!shown.iter().any(|text| text == "Retry deploy"), "{shown:?}");
    }
}
