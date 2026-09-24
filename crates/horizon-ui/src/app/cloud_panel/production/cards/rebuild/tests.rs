use super::super::super::{
    Runtime,
    rebuild::{
        Attempt,
        tests::{Phase, deployment},
    },
};
use super::super::runtime_actions;
use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_runtime::{Cancellation, Event, progress::Progress, state::Deployment};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn runtime(state: Deployment) -> Runtime {
    Runtime {
        stage: Some(state.stage),
        state: Some(state),
        ..Default::default()
    }
}

fn ready(build: bool, phase: Phase) -> Runtime {
    runtime(deployment(Path::new("/synthetic"), build, phase))
}

fn render(
    ctx: &egui::Context,
    runtime: &mut Runtime,
    input: egui::RawInput,
) -> (Option<Action>, Vec<egui::epaint::TextShape>) {
    let mut action = None;
    let output = ctx
        .run_ui(input, |ui| {
            action = runtime_actions(ui, 1, runtime);
        })
        .discard_textures();
    let texts = output
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            egui::Shape::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    (action, texts)
}

/// Texts drawn by one frame of the runtime actions.
fn texts(ctx: &egui::Context, runtime: &mut Runtime) -> Vec<String> {
    render(ctx, runtime, egui::RawInput::default())
        .1
        .iter()
        .map(|text| text.galley.text().to_owned())
        .collect()
}

fn has(texts: &[String], wanted: &str) -> bool {
    texts.iter().any(|text| text.starts_with(wanted))
}

/// Clicks the text starting with `label` and returns the action it produced.
fn click(ctx: &egui::Context, runtime: &mut Runtime, label: &str) -> Option<Action> {
    let mut point = None;
    for _ in 0..2 {
        point = render(ctx, runtime, egui::RawInput::default())
            .1
            .iter()
            .find(|text| text.galley.text().starts_with(label))
            .map(|text| text.pos + text.galley.size() * 0.5);
    }
    let point = point.unwrap_or_else(|| panic!("{label} is drawn"));
    let mut action = None;
    for pressed in [true, false] {
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        action = render(ctx, runtime, input).0.or(action);
    }
    action
}

const OFFER: &str = "Rebuild image & restart…";

/// Turns the offered Ready cloud into a case that hides the offer.
type Change = fn(&mut Runtime);

#[test]
fn rebuild_is_offered_only_on_a_ready_bound_cloud_with_a_recipe_and_nothing_pending() {
    let ctx = egui::Context::default();
    let offered = texts(&ctx, &mut ready(true, Phase::None));
    assert!(has(&offered, OFFER));
    assert!(has(&offered, "Stop worker…"));

    let hidden: [(&str, Change); 8] = [
        ("image-only profile", |runtime| {
            *runtime = ready(false, Phase::None);
        }),
        ("stop requested", |runtime| {
            runtime.state.as_mut().unwrap().stop_requested = true;
        }),
        ("stopped", |runtime| {
            runtime.stage = Some(Stage::Stopped);
            runtime.state.as_mut().unwrap().stage = Stage::Stopped;
        }),
        ("reconnect failed", |runtime| {
            runtime.stage = Some(Stage::Readiness);
            runtime.state.as_mut().unwrap().stage = Stage::Readiness;
            runtime.error = Some("Worker readiness timed out".into());
        }),
        ("saved record not ready", |runtime| {
            runtime.state.as_mut().unwrap().stage = Stage::Readiness;
        }),
        ("not bound", |runtime| {
            runtime.state.as_mut().unwrap().operation = CreateState::Prepared;
        }),
        ("sessions still relaunching", |runtime| {
            runtime.state.as_mut().unwrap().session_restart =
                Some(horizon_core::cloud_runtime::state::OperationId::generate());
        }),
        ("replacement pending", |runtime| {
            *runtime = ready(true, Phase::Built);
        }),
    ];
    for (case, change) in hidden {
        let mut runtime = ready(true, Phase::None);
        change(&mut runtime);
        assert!(!has(&texts(&ctx, &mut runtime), OFFER), "{case}");
    }
}

#[test]
fn rebuild_confirmation_names_its_consequences_before_starting() {
    let ctx = egui::Context::default();
    let mut runtime = ready(true, Phase::None);
    assert!(click(&ctx, &mut runtime, OFFER).is_none());
    assert!(runtime.confirmation == Confirmation::Rebuild);
    let confirming = texts(&ctx, &mut runtime);
    let text = confirming
        .iter()
        .find(|text| text.starts_with("Rebuild the image"))
        .expect("the confirmation is drawn");
    for consequence in [
        "latest committed .horizon recipe",
        "newest agent CLIs",
        "restart the worker",
        "Running agent processes restart",
        "Files under /workspace, including worktrees and agent logins, are kept",
        "Uncommitted changes are not used",
        "cannot change the cloud's size or capabilities",
    ] {
        assert!(text.contains(consequence), "{consequence}");
    }
    assert!(!has(&confirming, OFFER));

    assert!(click(&ctx, &mut runtime, "Keep current image").is_none());
    assert!(runtime.confirmation == Confirmation::None);
    assert!(has(&texts(&ctx, &mut runtime), OFFER));

    runtime.confirmation = Confirmation::Rebuild;
    assert!(matches!(
        click(&ctx, &mut runtime, "Rebuild and restart"),
        Some(Action::Rebuild)
    ));
}

fn rebuilding(kind: Kind, stage: Stage) -> (Runtime, std::sync::mpsc::Sender<Event>) {
    let mut runtime = ready(true, Phase::None);
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    runtime.cancel = Some(Cancellation::default());
    runtime.rebuild = Some(Attempt::new(kind));
    let start = Instant::now().checked_sub(Duration::from_secs(90)).unwrap();
    runtime.progress.stage(Stage::Validate, start);
    if stage != Stage::Validate {
        runtime.progress.stage(stage, start + Duration::from_secs(2));
    }
    runtime
        .progress
        .update(Progress::activity("Building the committed recipe"));
    runtime.stage = Some(stage);
    (runtime, sender)
}

#[test]
fn a_running_rebuild_lists_its_steps_and_cancels_only_before_the_switch() {
    let ctx = egui::Context::default();
    let (mut runtime, _sender) = rebuilding(Kind::Rebuild, Stage::Build);
    let running = texts(&ctx, &mut runtime);
    for shown in [
        "Rebuilding image · 0m",
        "Validate · 0m 02s",
        "Build locally · 1m",
        "Push image",
        "Replace image",
        "Check readiness",
        "Start sessions",
        "Ready",
        "Building the committed recipe",
        "Cancel rebuild",
        "Verbose output",
    ] {
        assert!(has(&running, shown), "{shown} is shown while rebuilding");
    }
    for hidden in [
        "Provision worker",
        "Prepare worktrees",
        "Cancel operation",
        "Reconnect cloud",
        "Stop worker…",
        OFFER,
        "Delete cloud resources…",
        "Check provider",
        "Image rebuild pending",
    ] {
        assert!(!has(&running, hidden), "{hidden} is hidden while rebuilding");
    }

    assert!(click(&ctx, &mut runtime, "Cancel rebuild").is_none());
    assert!(runtime.cancel.as_ref().unwrap().is_cancelled());
    let cancelling = texts(&ctx, &mut runtime);
    assert!(has(&cancelling, "Cancelling…"));
    assert!(!has(&cancelling, "Cancel rebuild"));

    for stage in [Stage::Replace, Stage::Provision, Stage::Readiness, Stage::Sessions] {
        let (mut runtime, _sender) = rebuilding(Kind::Continue, stage);
        let switching = texts(&ctx, &mut runtime);
        assert!(has(&switching, "Continuing image rebuild · "));
        assert!(!has(&switching, "Cancel rebuild"), "{stage:?}");
        assert!(has(&switching, "The worker is switching images"), "{stage:?}");
    }
    // Continuing a switch that was already requested cannot stop before it.
    for (phase, cancellable) in [(Phase::Built, true), (Phase::Requested, false)] {
        let (mut runtime, _sender) = rebuilding(Kind::Continue, Stage::Validate);
        runtime.state = Some(deployment(Path::new("/synthetic"), true, phase));
        let continuing = texts(&ctx, &mut runtime);
        assert_eq!(has(&continuing, "Cancel rebuild"), cancellable, "{phase:?}");
        assert_eq!(
            has(&continuing, "The worker is switching images"),
            !cancellable,
            "{phase:?}"
        );
    }
    let (mut runtime, _sender) = rebuilding(Kind::Cancel, Stage::Validate);
    let reverting = texts(&ctx, &mut runtime);
    assert!(has(&reverting, "Cancelling image rebuild · "));
    assert!(!has(&reverting, "Cancel rebuild"));

    // Ready ends the running view; the rebuild's steps stay listed.
    runtime.stage = Some(Stage::Ready);
    runtime.progress.stage(Stage::Replace, Instant::now());
    runtime.progress.stage(Stage::Ready, Instant::now());
    let finished = texts(&ctx, &mut runtime);
    assert!(has(&finished, "Replace image · 0m"));
    assert!(!has(&finished, "Provision worker"));
    assert!(!has(&finished, "Cancelling image rebuild"));
    assert!(has(&finished, OFFER));
}

#[test]
fn a_pending_replacement_offers_continue_and_cancel_by_phase() {
    let ctx = egui::Context::default();
    for (phase, explanation) in [(Phase::Prepared, PREPARED), (Phase::Built, BUILT)] {
        let mut runtime = ready(true, phase);
        let pending = texts(&ctx, &mut runtime);
        for shown in [
            "Image rebuild pending",
            explanation,
            "Continue rebuild",
            "Cancel rebuild",
            "Reconnect cloud",
            "Delete cloud resources…",
        ] {
            assert!(has(&pending, shown), "{phase:?}: {shown}");
        }
        assert!(!has(&pending, "Cancel rebuild…"), "{phase:?}: nothing to switch back");
        assert!(!has(&pending, "Stop worker…"), "{phase:?}: stop would be refused");
        assert!(!has(&pending, OFFER), "{phase:?}");
        assert!(matches!(
            click(&ctx, &mut runtime, "Continue rebuild"),
            Some(Action::ContinueRebuild)
        ));
        assert!(matches!(
            click(&ctx, &mut runtime, "Cancel rebuild"),
            Some(Action::CancelRebuild)
        ));

        // Nothing was sent, so there is nothing to switch back from.
        runtime.confirmation = Confirmation::CancelRebuild;
        let unsent = texts(&ctx, &mut runtime);
        assert!(!has(&unsent, "Switch back and restart"), "{phase:?}");
        assert!(has(&unsent, "Continue rebuild"), "{phase:?}");
        runtime.confirmation = Confirmation::None;

        // While another operation runs, the notice stays without its actions.
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        runtime.stage = Some(Stage::Readiness);
        let busy = texts(&ctx, &mut runtime);
        assert!(has(&busy, explanation));
        assert!(!has(&busy, "Continue rebuild"));
    }
    assert!(PREPARED.contains("did not finish") && PREPARED.contains("keeps its current image"));
    assert!(BUILT.contains("rebuilt image is ready") && BUILT.contains("has not switched"));
}

#[test]
fn cancelling_a_requested_switch_needs_a_confirmation() {
    let ctx = egui::Context::default();
    let mut runtime = ready(true, Phase::Requested);
    assert_eq!(runtime.stage, Some(Stage::Replace));
    let pending = texts(&ctx, &mut runtime);
    for shown in [
        "Image rebuild pending",
        REQUESTED,
        "Continue rebuild",
        "Cancel rebuild…",
        "Replace image",
        "Worker status needs confirmation",
        "Check provider",
        "Delete cloud resources…",
    ] {
        assert!(has(&pending, shown), "{shown}");
    }
    for hidden in ["Reconnect cloud", "Stop worker…", OFFER, "Provision worker"] {
        assert!(!has(&pending, hidden), "{hidden}");
    }
    assert!(REQUESTED.contains("may be in progress"));

    assert!(click(&ctx, &mut runtime, "Cancel rebuild…").is_none());
    assert!(runtime.confirmation == Confirmation::CancelRebuild);
    let confirming = texts(&ctx, &mut runtime);
    assert!(has(&confirming, CANCEL_REQUESTED));
    assert!(CANCEL_REQUESTED.contains("switches back to its previous image"));
    assert!(CANCEL_REQUESTED.contains("restarts it again"));
    assert!(!has(&confirming, "Continue rebuild"));
    assert!(click(&ctx, &mut runtime, "Keep the new image").is_none());
    assert!(runtime.confirmation == Confirmation::None);

    runtime.confirmation = Confirmation::CancelRebuild;
    assert!(matches!(
        click(&ctx, &mut runtime, "Switch back and restart"),
        Some(Action::CancelRebuild)
    ));
    runtime.confirmation = Confirmation::None;
    assert!(matches!(
        click(&ctx, &mut runtime, "Continue rebuild"),
        Some(Action::ContinueRebuild)
    ));
}

#[test]
fn rebuild_outcomes_stay_on_the_card() {
    let ctx = egui::Context::default();
    let mut runtime = ready(true, Phase::None);
    runtime.rebuild = Some(Attempt::new(Kind::Rebuild));
    for line in [
        "Agent CLI releases: claude 2.0.1",
        "Image unchanged; nothing to restart",
        "Session panel-1 (claude) lost its process in the container reset and was not relaunched",
    ] {
        runtime.observe_rebuild(&Event::Output(line.into()));
    }
    let finished = texts(&ctx, &mut runtime);
    for shown in [
        "Agent CLI releases: claude 2.0.1",
        "Image unchanged; nothing to restart",
        "Session panel-1 (claude) lost its process",
        OFFER,
    ] {
        assert!(has(&finished, shown), "{shown}");
    }

    // A failed attempt that left its journal shows the outcome with the notice.
    *runtime.state.as_mut().unwrap() = deployment(Path::new("/synthetic"), true, Phase::Built);
    runtime.error = Some("Registry push failed".into());
    let failed = texts(&ctx, &mut runtime);
    assert!(has(&failed, "Registry push failed"));
    assert_eq!(
        failed.iter().filter(|text| text.starts_with("Image unchanged")).count(),
        1,
        "notes are drawn once"
    );

    // A reconnect that fails after the switch keeps the outcome beside its error.
    let mut state = deployment(Path::new("/synthetic"), true, Phase::None);
    state.stage = Stage::Readiness;
    runtime.state = Some(state);
    runtime.stage = Some(Stage::Readiness);
    runtime.error = Some("Worker readiness timed out".into());
    let reconnect_failed = texts(&ctx, &mut runtime);
    assert!(has(&reconnect_failed, "Worker readiness timed out"));
    assert_eq!(
        reconnect_failed
            .iter()
            .filter(|text| text.starts_with("Session panel-1 (claude) lost its process"))
            .count(),
        1
    );
}

#[test]
fn remote_device_release_waits_for_a_pending_replacement() {
    for (phase, offered) in [
        (Phase::None, true),
        (Phase::Prepared, false),
        (Phase::Built, false),
        (Phase::Requested, false),
    ] {
        let mut state = deployment(Path::new("/synthetic"), true, phase);
        if offered {
            let mut switching = state.clone();
            switching.stage = Stage::Replace;
            switching.profile.capabilities.browserstack =
                Some(serde_json::from_value(serde_json::json!({"provider": "account"})).unwrap());
            assert!(
                !runtime(switching).can_release_remote_devices(),
                "a switch recorded without its journal still waits"
            );
        }
        state.profile.capabilities.browserstack =
            Some(serde_json::from_value(serde_json::json!({"provider": "account"})).unwrap());
        assert_eq!(runtime(state).can_release_remote_devices(), offered, "{phase:?}");
    }
}
