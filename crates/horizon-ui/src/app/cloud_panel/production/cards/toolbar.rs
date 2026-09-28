//! Always-visible production summary and ordinary workspace controls.
use super::{Stage, cloud_runtime, theme};
use egui::RichText;
use horizon_core::{Board, WorkspaceLayout, cloud_panel::CloudGroup};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum View {
    #[default]
    Closed,
    Configuration,
    Activity,
    Management,
}

impl View {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Closed => "Cloud",
            Self::Configuration => "Configuration",
            Self::Activity => "Activity",
            Self::Management => "Manage",
        }
    }
}

#[derive(Default)]
pub(super) enum LayoutChange {
    #[default]
    Unchanged,
    Set(Option<WorkspaceLayout>),
}

#[derive(Default)]
pub(super) struct Response {
    pub layout: LayoutChange,
    pub fullscreen: bool,
    pub toggle_controls: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    group: &CloudGroup,
    runtime: &mut super::super::Runtime,
    board: &Board,
    fullscreen: bool,
) -> Response {
    for (text, size) in [
        (egui::TextStyle::Body, 14.0),
        (egui::TextStyle::Button, 14.0),
        (egui::TextStyle::Small, 12.0),
    ] {
        if let Some(font) = ui.style_mut().text_styles.get_mut(&text) {
            font.size = size;
        }
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
    ui.spacing_mut().interact_size.y = 28.0;
    let mut response = Response::default();
    let state = if runtime.state_unavailable {
        "Unknown (record unavailable)"
    } else {
        runtime.stage.map_or("Not deployed", Stage::label)
    };
    let error = runtime.error.as_ref().or(runtime.remote_release_error.as_ref());
    let deployment = if error.is_some() {
        RichText::new(format!("Deployment: {state} · Needs attention — open Activity")).color(theme::PALETTE_RED())
    } else {
        RichText::new(format!("Deployment: {state}"))
    };
    ui.add(egui::Label::new(deployment).truncate())
        .on_hover_text(error.map_or(state, String::as_str));
    ui.add(egui::Label::new(attachment_summary(group, runtime, board)).truncate())
        .on_hover_text("Local terminal processes are counted separately from deployment. A running process alone does not confirm the remote connection; check the terminal output. Browser and desktop connections are shown on their panels.");
    let worker = runtime.state.as_ref().and_then(|state| state.worker.as_ref());
    let label = worker.map_or_else(
        || "Worker: awaiting details".to_owned(),
        |worker| format!("Worker: {} (last observed)", worker.desired_status),
    );
    let profile = runtime
        .state
        .as_ref()
        .map(|state| &state.profile)
        .or_else(|| group.remote.as_ref().map(|launch| &launch.profile));
    let label = profile.map_or(label.clone(), |profile| {
        format!("{} vCPU · {} GB · {label}", profile.cpu, profile.memory_gb)
    });
    ui.add(egui::Label::new(&label).truncate()).on_hover_text(label);
    ui.add_space(4.0);
    spending(ui, runtime);
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new("Disconnecting does not stop compute or storage charges.")
                .small()
                .color(theme::FG_DIM()),
        )
        .truncate(),
    );
    ui.separator();
    response.toggle_controls = ui
        .button(if group.toolbar_expanded {
            "▾  Hide controls"
        } else {
            "▸  Show controls"
        })
        .clicked();
    if !group.toolbar_expanded {
        return response;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let mut selected = group.layout;
        if crate::app::workspace::workspace_layout_buttons(
            ui,
            &mut selected,
            theme::workspace_accent(group.issue.saturating_sub(101) as usize),
        ) {
            response.layout = LayoutChange::Set(selected);
        }
        response.fullscreen = ui
            .button(if fullscreen { "Exit full screen" } else { "Full screen" })
            .clicked();
    });
    ui.horizontal(|ui| {
        for item in [View::Configuration, View::Activity, View::Management] {
            if ui.selectable_label(runtime.detail_view == item, item.label()).clicked() {
                runtime.detail_view = item;
            }
        }
    });
    response
}

pub(super) fn attachment_summary(group: &CloudGroup, runtime: &super::super::Runtime, board: &Board) -> String {
    if runtime.needs_attach
        || !runtime.pending_session_attachments.is_empty()
        || !runtime.pending_member_attachments.is_empty()
        || !runtime.pending_browser_attachments.is_empty()
    {
        return "Sessions: attaching…".into();
    }
    let (total, running) = board
        .panels
        .iter()
        .filter(|panel| group.panels.contains(&panel.local_id) && panel.terminal().is_some())
        .fold((0, 0), |(total, running), panel| {
            (total + 1, running + usize::from(!panel.child_exited()))
        });
    if total == 0 {
        return "Terminals: none attached".into();
    }
    format!("Terminals: {running}/{total} running")
}

fn spending(ui: &mut egui::Ui, runtime: &super::super::Runtime) {
    let now = std::time::SystemTime::now();
    let worker = runtime.state.as_ref().and_then(|state| state.worker.as_ref());
    let run = runtime.current_run_cost(now);
    let total = runtime.total_cost(now);
    let rate = worker.and_then(cloud_runtime::cost::hourly_rate);
    ui.columns(3, |columns| {
        let (title, amount, note) = if let Some(total) = total {
            (if total.excludes_before.is_some() { "Past 12 months" } else { "Since creation" },
             horizon_core::format_cost(total.total()),
             runtime.billing.explanation(&total, std::time::Instant::now()))
        } else if runtime.billing.sample().is_some() && runtime.billing.error().is_none() {
            ("Since creation", "Awaiting billing".into(),
             "No billed periods have been reported. The current-run estimate does not establish a lifetime total.".into())
        } else {
            ("Since creation", "Unavailable".into(), runtime.billing.error().map_or_else(
                || "Provider billing has not been read yet.".into(), |error| format!("Provider billing unavailable: {error}")))
        };
        metric(&mut columns[0], title, &amount, &note);
        let billing = if runtime.billing.error().is_some() { "Billing stale/unavailable" }
            else if runtime.billing.refreshing() { "Refreshing billing…" }
            else if total.is_some() { "Billed + estimated" }
            else if runtime.billing.sample().is_some() { "Awaiting provider billing" }
            else { "Billing not available" };
        columns[0].add(egui::Label::new(RichText::new(billing).small()).truncate());
        metric(&mut columns[1], "This run (estimate)", &run.map_or_else(|| "Unavailable".into(), |run| horizon_core::format_cost(run.amount)),
            "Estimate from the last observed worker start and rate. Reconnect or check the provider to refresh worker state.");
        metric(&mut columns[2], "Worker rate", &rate.map_or_else(|| "Unavailable".into(), cloud_runtime::cost::format_rate),
            "Last reported worker hourly rate. Storage and other provider charges may be additional.");
    });
}

fn metric(ui: &mut egui::Ui, title: &str, value: &str, note: &str) {
    ui.add(egui::Label::new(RichText::new(title).small().color(theme::FG_DIM())).truncate());
    ui.add(egui::Label::new(RichText::new(value).strong().size(18.0)).truncate())
        .on_hover_text(note);
}

#[cfg(test)]
mod tests {
    use super::super::super::Runtime;
    use super::*;
    #[test]
    fn unreported_billing_does_not_become_a_zero_lifetime_total_after_stop() {
        use cloud_runtime::billing::{BillingBucket, BucketSize, History};
        use std::time::{Instant, SystemTime};
        let mut runtime = Runtime {
            stage: Some(Stage::Ready),
            state: Some(
                serde_json::from_value(serde_json::json!({
                    "version":1,"cloud_id":"billing-fixture","repository":"/synthetic","revision":"a",
                    "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
                    "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
                    "spec":null,"sessions":[],"worker":{"id":"worker1","name":"billing fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING",
                        "costPerHr":0.69,"lastStartedAt":"2024-07-12T19:14:40Z"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        runtime.billing.record(
            "worker1",
            Ok(History {
                buckets: Vec::new(),
                from: SystemTime::UNIX_EPOCH,
            }),
            Instant::now(),
        );
        let now = SystemTime::now();
        assert!(runtime.total_cost(now).is_none());
        assert!(runtime.current_run_cost(now).is_some());
        assert!(!runtime.cost_badge(now).unwrap().contains("total"));
        runtime.stage = Some(Stage::Validate);
        runtime.error = Some("Synthetic reconnect failure".into());
        let group = CloudGroup::new(101, "Fixture".into(), "fixture".into(), "/synthetic".into(), [0.0, 0.0]);
        let context = egui::Context::default();
        let board = Board::new();
        for _ in 0..2 {
            use crate::test_egui::DiscardTextures;
            let output = context
                .run_ui(egui::RawInput::default(), |ui| {
                    show(ui, &group, &mut runtime, &board, false);
                })
                .discard_textures();
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            assert!(texts.iter().any(|text| text.contains("Needs attention")));
            assert!(
                texts
                    .iter()
                    .any(|text| text.contains("4 vCPU · 8 GB · Worker: RUNNING"))
            );
            assert!(texts.iter().any(|text| text == "Awaiting provider billing"));
            assert!(!texts.iter().any(|text| text == "Billing not available"));
            assert!(texts.iter().any(|text| text == "Unavailable"));
        }
        assert!(runtime.current_run_cost(now).is_none());
        runtime.stage = Some(Stage::Stopped);
        let state = runtime.state.as_mut().unwrap();
        state.stage = Stage::Stopped;
        state.worker.as_mut().unwrap().desired_status = "EXITED".into();
        assert!(runtime.total_cost(now).is_none());
        assert!(runtime.cost_badge(now).is_none());
        runtime.billing.record(
            "worker1",
            Ok(History {
                buckets: vec![BillingBucket {
                    time: "2024-07-12T19:00:00Z".into(),
                    size: BucketSize::Hour,
                    amount: 0.0,
                    time_billed_ms: 0,
                }],
                from: SystemTime::UNIX_EPOCH,
            }),
            Instant::now(),
        );
        assert_eq!(runtime.cost_badge(now).as_deref(), Some("$0.00 total"));
    }

    #[test]
    #[cfg(unix)] // Exercises the actual PTY lifecycle with a disposable Unix shell.
    fn cloud_terminal_summary_distinguishes_process_liveness_from_ssh_panel_status() {
        let mut board = horizon_core::Board::new();
        let workspace = board.create_workspace("cloud fixture");
        let id = board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: horizon_core::PanelKind::Shell,
                    command: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "read line".into()],
                    ..Default::default()
                },
                workspace,
            )
            .unwrap();
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            101,
            "Cloud".into(),
            board.workspace(workspace).unwrap().local_id.clone(),
            "/synthetic".into(),
            [0.0, 0.0],
        );
        group.panels.push(board.panel(id).unwrap().local_id.clone());
        let mut runtime = Runtime::default();
        assert!(board.panel(id).unwrap().ssh_status().is_none());
        assert_eq!(attachment_summary(&group, &runtime, &board), "Terminals: 1/1 running");
        board.panel(id).unwrap().terminal().unwrap().write_input(b"done\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !board.panel(id).unwrap().child_exited() && std::time::Instant::now() < deadline {
            board.panel_mut(id).unwrap().process_output();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(attachment_summary(&group, &runtime, &board), "Terminals: 0/1 running");
        runtime.needs_attach = true;
        assert_eq!(attachment_summary(&group, &runtime, &board), "Sessions: attaching…");
    }
}
