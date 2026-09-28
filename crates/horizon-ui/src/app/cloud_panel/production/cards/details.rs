//! Secondary views keep long-running activity separate from everyday layout controls.
use super::{Action, danger_button, profile_details, runtime_actions, toolbar};
use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};

#[derive(Default)]
pub(super) struct Response {
    pub action: Option<Action>,
    pub resize: Option<(u16, u16)>,
}

pub(super) fn show(
    ctx: &egui::Context,
    group: &CloudGroup,
    launch: &CloudLaunch,
    runtime: &mut super::super::Runtime,
    companions: &mut super::super::companions::State,
    region_of: &dyn Fn(&str) -> Option<String>,
) -> Response {
    let selected = runtime.detail_view;
    let mut response = Response::default();
    if selected == toolbar::View::Closed {
        return response;
    }
    let mut open = true;
    egui::Window::new(format!("{} — {}", group.title, selected.label()))
        .id(egui::Id::new(("cloud-details", group.issue)))
        .order(egui::Order::Foreground)
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(16.0))
        .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 220.0))
        .constrain_to(ctx.content_rect().shrink(24.0))
        .open(&mut open)
        .collapsible(false)
        .default_width(560.0)
        .default_height(440.0)
        .show(ctx, |ui| {
            crate::app::cloud_panel::runtime::readable_runtime_style(ui);
            crate::app::cloud_panel::runtime::solid_scroll_area(ui)
                .id_salt(("cloud-details-body", group.issue, selected as u8))
                .max_height((ctx.content_rect().height() - 140.0).clamp(160.0, 520.0))
                .show(ui, |ui| match selected {
                    toolbar::View::Configuration => {
                        super::sizing::profile_summary(ui, launch, runtime, region_of);
                        ui.separator();
                        companions.render(ui, &launch.id);
                    }
                    toolbar::View::Activity => super::progress_output(ui, group.issue, runtime),
                    toolbar::View::Management => {
                        response.resize = profile_details(ui, group.issue, launch, runtime, region_of);
                        if runtime.can_release_remote_devices()
                            && ui
                                .add(danger_button("Release devices and remove remote credentials"))
                                .clicked()
                        {
                            response.action = Some(Action::RevokeBrowserstack);
                        }
                        response.action = runtime_actions(ui, group.issue, runtime).or(response.action);
                    }
                    toolbar::View::Closed => {}
                });
        });
    if !open {
        runtime.detail_view = toolbar::View::Closed;
    }
    response
}
