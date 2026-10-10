//! Two synthetic failed clouds for a debug build, so the layout of a long failure line
//! can be seen without a provider: one with a member panel shows the failure in its
//! Status tab, one without panels shows it in its body. It never runs in a release
//! build, only on an ephemeral session without a cloud, and it asks no provider: no
//! record binds a worker.
use super::Stage;
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use std::time::Instant;

const WITH_PANEL: u32 = 8_800_003;
const WITHOUT_PANELS: u32 = 8_800_004;

const CAUSE: &str = "Error response from daemon: Conflict. The container name \"/horizon-contract-00000000-1111-2222-3333-444444444444\" is already in use by container \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\". You have to remove (or rename) that container to be able to reuse that name.";

pub(super) fn seed(app: &mut HorizonApp) {
    if std::env::var_os("HORIZON_CLOUD_FAILURE_PREVIEW").is_some() {
        seed_failures(app);
    }
}

fn seed_failures(app: &mut HorizonApp) -> bool {
    if !super::preview::accepts(app) {
        return false;
    }
    let Some(launch) = preview_launch() else {
        return false;
    };
    let workspace = app.board.create_workspace("Synthetic failed clouds");
    let Some(workspace_local) = app.board.workspace(workspace).map(|item| item.local_id.clone()) else {
        return false;
    };
    let Some(member) = super::preview::waiting_member(app, workspace, "Agent") else {
        return false;
    };
    for (issue, origin, panels) in [
        (WITH_PANEL, [40.0, 40.0], vec![member]),
        (WITHOUT_PANELS, [40.0, 760.0], Vec::new()),
    ] {
        let mut group = CloudGroup::new(
            issue,
            "Synthetic failed cloud".into(),
            workspace_local.clone(),
            "/synthetic".into(),
            origin,
        );
        // The workspace's grid gives each cloud its slot, so no size is set here.
        group.remote = Some(launch.clone());
        group.panels = panels;
        app.cloud_prototype.groups.0.push(group);
        let runtime = app.cloud_prototype.production.runtimes.entry(issue).or_default();
        let now = Instant::now();
        for stage in [Stage::Validate, Stage::Provision, Stage::Readiness] {
            runtime.progress.stage(stage, now);
            runtime.stage = Some(stage);
            // Enough lines that the output under the cause has rows to keep.
            for line in 1..=3 {
                runtime.push_log(format!("synthetic {} output {line}", stage.label().to_lowercase()));
            }
        }
        runtime.push_log("starting the synthetic contract container".into());
        runtime.push_log(CAUSE.into());
        runtime.progress.finish(now);
        runtime.error = Some("Readiness check failed; inspect deployment output".into());
        // The cloud with a panel opens on its Status tab, where the failure is read.
        if issue == WITH_PANEL {
            runtime.drawer = Some(super::cards::Tab::Status);
        }
    }
    true
}

fn preview_launch() -> Option<CloudLaunch> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: failed\nprofiles:\n  failed:\n    provider: hetzner\n    image: example.invalid/worker\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n",
    )
    .ok()?;
    Some(CloudLaunch {
        deployment_started: true,
        id: "synthetic-failed".into(),
        revision: "a".repeat(40),
        profile_name: "failed".into(),
        profile: config.profiles.get("failed")?.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{test_app, test_app_with_startup};
    use crate::app::view::canvas_scene_transform;
    use crate::test_egui::DiscardTextures;
    use horizon_core::{CanvasViewState, RuntimeState, StartupDecision};

    #[test]
    fn the_preview_seeds_two_failed_clouds_once() {
        let (_temp, mut app) = test_app();
        assert!(seed_failures(&mut app));
        assert!(!seed_failures(&mut app), "one preview only");
        let groups = &app.cloud_prototype.groups.0;
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].panels.len(), 1);
        assert_eq!(groups[1].panels, Vec::<String>::new());
        for group in groups {
            let runtime = &app.cloud_prototype.production.runtimes[&group.issue];
            assert_eq!(runtime.stage, Some(Stage::Readiness));
            assert!(
                runtime.receiver.is_none() && runtime.state.is_none(),
                "nothing reaches a provider"
            );
            assert_eq!(runtime.logs.back().map(|line| line.text.as_str()), Some(CAUSE));
        }
    }

    /// The preview as a session shows it: seeded on restore, then placed by its workspace's grid.
    fn arranged() -> (tempfile::TempDir, egui::Context, HorizonApp) {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.prepare_cloud_prototype(&ctx);
        assert!(seed_failures(&mut app));
        app.prepare_cloud_prototype(&ctx);
        app.cloud_prototype.ready = true;
        app.canvas_view = CanvasViewState::new([0.0, 0.0], 1.0);
        (temp, ctx, app)
    }

    /// A text the cards draw: where, its rows, and whether it was cut.
    type Drawn = (String, egui::Rect, Vec<String>, bool);

    /// Draws the cards for a few frames, the first with `events` at `pointer`, and
    /// returns the texts of the last.
    fn frames(ctx: &egui::Context, app: &mut HorizonApp, pointer: egui::Pos2, events: &[egui::Event]) -> Vec<Drawn> {
        let mut output = None;
        // The cards settle their measured heights, and a wheel its scroll, over a few frames.
        for frame in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(3000.0, 2200.0))),
                time: Some(ctx.input(|input| input.time) + 0.5),
                ..egui::RawInput::default()
            };
            input.events.push(egui::Event::PointerMoved(pointer));
            if frame == 1 {
                input.events.extend_from_slice(events);
            }
            output = Some(
                ctx.run_ui(input, |ui| app.render_production_runtimes(ui.ctx()))
                    .discard_textures(),
            );
        }
        output
            .map(|output| output.shapes)
            .unwrap_or_default()
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                    text.galley.rows.iter().map(|row| row.text()).collect(),
                    text.galley.elided,
                )),
                _ => None,
            })
            .collect()
    }

    /// Draws the cards with the steps of the cloud without panels scrolled to the
    /// failed step, as F02 of the procedure does, and returns every drawn `CAUSE`.
    fn causes(ctx: &egui::Context, app: &mut HorizonApp, narrow: egui::Rect) -> Vec<Drawn> {
        let away = egui::Pos2::new(2900.0, 2100.0);
        let wheel = |y| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, y),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let drawn = frames(ctx, app, away, &[]);
        let step = |drawn: &[Drawn], label: &str| {
            drawn
                .iter()
                .find(|(text, rect, ..)| text == label && narrow.contains_rect(*rect))
                .map(|(_, rect, ..)| *rect)
        };
        let Some(first) = step(&drawn, "Validate") else {
            return Vec::new();
        };
        // Back to the top first, so each call scrolls the same distance.
        let _ = frames(ctx, app, first.center(), &[wheel(10_000.0)]);
        let drawn = frames(ctx, app, away, &[]);
        let (Some(first), Some(failed)) = (step(&drawn, "Validate"), step(&drawn, "Check readiness")) else {
            return Vec::new();
        };
        // The failed step at the top of the steps leaves its cause and actions in view.
        let drawn = frames(ctx, app, first.center(), &[wheel(first.top() - failed.bottom())]);
        drawn.into_iter().filter(|(text, ..)| text == CAUSE).collect()
    }

    #[test]
    fn the_preview_cause_is_cut_at_the_width_each_cloud_really_gets() {
        let (_temp, ctx, mut app) = arranged();
        let canvas = app.canvas_rect(&ctx);
        let transform = canvas_scene_transform(canvas, app.canvas_view);
        let bounds: Vec<_> = app
            .cloud_prototype
            .groups
            .0
            .iter()
            .map(|group| {
                let (min, max) = group.bounds();
                (
                    group.issue,
                    transform * egui::Rect::from_min_max(min.into(), max.into()),
                )
            })
            .collect();
        let without_panels = app
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == WITHOUT_PANELS);
        assert!(
            without_panels.is_some_and(|group| group.size[0] < 760.0),
            "the cloud without panels shows its steps above its output"
        );
        let narrow = bounds
            .iter()
            .find(|(issue, _)| *issue == WITHOUT_PANELS)
            .map(|(_, rect)| *rect);
        let narrow = narrow.unwrap_or(egui::Rect::NOTHING);
        let shown = causes(&ctx, &mut app, narrow);
        for (issue, cloud) in &bounds {
            let cut: Vec<_> = shown
                .iter()
                .filter(|(_, rect, _, elided)| *elided && cloud.contains_rect(*rect))
                .collect();
            assert_eq!(
                cut.len(),
                1,
                "{issue}: one cut cause inside the cloud {cloud:?}: {shown:?}"
            );
            let (_, _, rows, _) = cut[0];
            assert_eq!(rows.len(), super::super::cards::CAUSE_ROWS, "{issue}: {rows:?}");
            assert!(rows.last().is_some_and(|row| row.ends_with('…')), "{issue}: {rows:?}");
        }

        // Show more in both places: the 64-character id breaks inside each width.
        for (place, issue) in [("drawer", WITH_PANEL), ("steps", WITHOUT_PANELS)] {
            let attempt = app.cloud_prototype.production.runtimes[&issue].progress.attempt();
            super::super::cards::expand_cause(&ctx, (place, issue), attempt, CAUSE);
        }
        let expanded = causes(&ctx, &mut app, narrow);
        assert!(expanded.iter().all(|(.., elided)| !elided), "{expanded:?}");
        assert_eq!(
            expanded.len(),
            shown.len(),
            "both causes are drawn in full: {expanded:?}"
        );
        let id = "0123456789abcdef".repeat(4);
        for (_, rect, rows, _) in &expanded {
            assert!(
                !rows.iter().any(|row| row.contains(&id)),
                "the id breaks across rows: {rows:?}"
            );
            assert!(
                bounds
                    .iter()
                    .any(|(_, cloud)| cloud.left() <= rect.left() && rect.right() <= cloud.right()),
                "{rect:?} stays inside its cloud"
            );
        }
    }
}
