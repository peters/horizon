//! The Connections tab: what is attached to the cloud and the ways into it. Rarely used
//! controls wait under a disclosure so the attached sessions stay the first thing read.
use super::super::super::Runtime;
use super::super::{Action, danger_button, section};
use super::{Context, Response};
use crate::theme;
use egui::RichText;

pub(super) fn show(ui: &mut egui::Ui, runtime: &mut Runtime, context: &mut Context<'_>, response: &mut Response) {
    section::show(ui, "Sessions", |ui| sessions(ui, runtime, context));
    section::show(ui, "Access", |ui| {
        if let Some(action) = access(ui, runtime) {
            response.action = Some(action);
        }
    });
    if let Some(root) = context.root {
        // The tailnet control names itself.
        section::frame(ui, |ui| {
            context.tailnets.cloud(
                ui,
                root,
                &context.launch.id,
                runtime.receiver.is_some() || runtime.state.as_ref().is_some_and(|s| s.spec.is_some()),
            );
        });
    }
    let releasable = runtime.can_release_remote_devices();
    egui::CollapsingHeader::new(
        RichText::new(if releasable {
            "Companion clouds and remote devices"
        } else {
            "Companion clouds"
        })
        .size(13.0)
        .color(theme::FG_SOFT()),
    )
    .id_salt(("cloud-connections-more", context.group.issue))
    .default_open(false)
    .show(ui, |ui| {
        context.companions.render(ui, &context.launch.id);
        if releasable
            && ui
                .add(danger_button("Release devices and remove remote credentials"))
                .on_hover_text("Stops this cloud’s hosted browser sessions and private tunnel, then deletes its copied credentials. Reconnect transfers them again only while the local grant remains configured.")
                .clicked()
        {
            response.action = Some(Action::RevokeBrowserstack);
        }
    });
}

fn sessions(ui: &mut egui::Ui, runtime: &Runtime, context: &Context<'_>) {
    ui.label(
        RichText::new(super::super::attachment_summary(context.group, runtime, context.board))
            .size(14.0)
            .color(theme::FG()),
    )
    .on_hover_text("Local terminal processes are counted separately from deployment. A running process alone does not confirm the remote connection; check the terminal output.");
    let listed = runtime
        .state
        .as_ref()
        .map_or(&[][..], |state| state.sessions.as_slice());
    if !listed.is_empty() {
        ui.add_space(6.0);
        // Branch and worktree names have no bound: each column is capped, and a cut value
        // shows in full on hover.
        const GAP: f32 = 16.0;
        let column = ((ui.available_width() - 2.0 * GAP) / 3.0).max(80.0);
        egui::Grid::new(("cloud-sessions", context.group.issue))
            .num_columns(3)
            .spacing([GAP, 6.0])
            .min_col_width(48.0)
            .max_col_width(column)
            .show(ui, |ui| {
                for session in listed {
                    // The drawer wraps text; wrapped grid cells shrink to a letter wide.
                    let cell = |ui: &mut egui::Ui, text: RichText| {
                        ui.add(egui::Label::new(text).truncate());
                    };
                    cell(ui, RichText::new(&session.agent).size(14.0).color(theme::FG()));
                    cell(
                        ui,
                        RichText::new(
                            [session.branch.as_str(), session.worktree.as_str()]
                                .into_iter()
                                .filter(|part| !part.trim().is_empty())
                                .collect::<Vec<_>>()
                                .join(" · "),
                        )
                        .size(13.0)
                        .color(theme::FG_SOFT()),
                    );
                    cell(
                        ui,
                        RichText::new(format!("tmux {}", session.tmux))
                            .monospace()
                            .size(12.5)
                            .color(theme::FG_DIM()),
                    );
                    ui.end_row();
                }
            });
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new("Sessions continue while disconnected.")
            .size(12.5)
            .color(theme::FG_DIM()),
    );
}

/// The desktop viewer and local network sharing once the cloud is connected; also while
/// a rebuild or resize runs, so a paused share can still be switched off.
pub(in crate::app::cloud_panel::production::cards) fn access(
    ui: &mut egui::Ui,
    runtime: &mut Runtime,
) -> Option<Action> {
    if !runtime.connected_and_ready() && !runtime.sharing.held() {
        ui.label(
            RichText::new("Connect the cloud to open its desktop or share this computer's network with it.")
                .size(13.0)
                .color(theme::FG_DIM()),
        );
        return None;
    }
    // A held share shows while disconnected; the desktop viewer needs the connection.
    let desktop = (runtime.connected_and_ready()
        && section::row(ui, "Desktop", "Open the worker's desktop in a viewer panel.", |ui| {
            super::super::desktop_button(ui, runtime)
        }))
    .then_some(Action::Desktop);
    ui.add_space(4.0);
    super::super::super::local_network::show(ui, runtime).or(desktop)
}
