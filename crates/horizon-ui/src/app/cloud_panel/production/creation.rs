//! Presentation of the existing repository-backed cloud creation flow.
use super::{CloudConfig, HorizonApp, PathBuf, Production};
use crate::theme;
use egui::{Align, Button, Context, Frame, Id, Layout, RichText, Stroke, TextEdit, Ui, Vec2};

#[derive(Default)]
struct Actions {
    repository: RepositoryAction,
    create: bool,
    cancel: bool,
}

#[derive(Default)]
enum RepositoryAction {
    #[default]
    None,
    Load,
    Setup,
}

impl HorizonApp {
    pub(in crate::app::cloud_panel) fn render_cloud_creation(&mut self, ctx: &Context) {
        if !self.cloud_prototype.production.creating {
            return;
        }
        let viewport = ctx.content_rect();
        let width = (viewport.width() - 64.0).clamp(240.0, 640.0);
        let body_height = (viewport.height() - 240.0).max(100.0);
        let mut actions = Actions::default();
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let id = Id::new("cloud-creation");
        // Root chrome uses Tooltip order; raise this modal last to contain its input too.
        let response = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
            .frame(
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(16)
                    .inner_margin(24),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                heading(ui);
                ui.add_space(16.0);
                egui::ScrollArea::vertical()
                    .id_salt("cloud-creation-body")
                    .max_height(body_height)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(self.cloud_prototype.production.pending_creation.is_none(), |ui| {
                            if fields(ui, &mut self.cloud_prototype.production) {
                                actions.repository = RepositoryAction::Load;
                            }
                            if self.cloud_prototype.production.profiles.is_none()
                                && super::repository_setup::render(ui, &mut self.cloud_prototype.production)
                            {
                                actions.repository = RepositoryAction::Setup;
                            }
                        });
                        if let Some(error) = &self.cloud_prototype.error {
                            ui.add_space(8.0);
                            ui.colored_label(theme::PALETTE_RED(), error);
                        }
                    });
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                footer(ui, &self.cloud_prototype.production, &mut actions);
            });
        ctx.move_to_top(response.response.layer_id);
        let dismissed = response.should_close();
        if dismissed && escape {
            self.consume_navigation_key(
                ctx,
                horizon_core::ShortcutBinding::new(
                    horizon_core::ShortcutModifiers::NONE,
                    horizon_core::ShortcutKey::Escape,
                ),
            );
        }
        if dismissed || actions.cancel {
            self.cloud_prototype.production.creating = false;
            self.cloud_prototype.production.pending_creation = None;
            return;
        }
        match actions.repository {
            RepositoryAction::Load => self.read_cloud_profiles(),
            RepositoryAction::Setup => self.start_cloud_repository_setup(ctx),
            RepositoryAction::None => {}
        }
        if actions.create
            && let Err(error) = self.create_production_cloud(ctx)
        {
            self.cloud_prototype.error = Some(error.to_string());
        }
        self.poll_cloud_creation(ctx);
    }

    pub(super) fn read_cloud_profiles(&mut self) {
        let form = &mut self.cloud_prototype.production;
        let result = std::fs::read_to_string(PathBuf::from(&form.repository).join(".horizon/cloud.yml"))
            .map_err(|_| "Cannot read .horizon/cloud.yml".to_owned())
            .and_then(|yaml| CloudConfig::parse(&yaml).map_err(|error| error.to_string()));
        match result {
            Ok(config) => {
                form.selected_profile.clone_from(&config.default);
                form.profiles = Some(config);
                self.cloud_prototype.error = None;
            }
            Err(error) => {
                form.profiles = None;
                form.selected_profile.clear();
                self.cloud_prototype.error = Some(error);
            }
        }
    }
}

fn heading(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("New cloud").size(26.0).strong().color(theme::FG()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            Frame::new()
                .fill(theme::PANEL_BG_ALT())
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new("RunPod").size(13.0).color(theme::FG_SOFT()));
                });
        });
    });
    ui.label(
        RichText::new("A remote workspace for your repository and agents.")
            .size(14.0)
            .color(theme::FG_SOFT()),
    );
}

fn field(ui: &mut Ui, label: &str, id: &str, value: &mut String, hint: &str) -> egui::Response {
    ui.label(RichText::new(label).size(14.0).strong().color(theme::FG()));
    ui.add_sized(
        [ui.available_width(), 38.0],
        TextEdit::singleline(value)
            .id(Id::new(id))
            .font(egui::FontId::proportional(15.0))
            .margin(Vec2::new(12.0, 10.0))
            .hint_text(hint),
    )
}

fn fields(ui: &mut Ui, form: &mut Production) -> bool {
    let title = field(
        ui,
        "Cloud title",
        "cloud-title",
        &mut form.title,
        "e.g. Feature development",
    );
    if std::mem::take(&mut form.focus_title_on_open) {
        title.request_focus();
    }
    ui.add_space(8.0);
    if field(
        ui,
        "Repository",
        "cloud-repository",
        &mut form.repository,
        "/path/to/repository",
    )
    .changed()
    {
        form.profiles = None;
    }
    ui.add_space(8.0);
    field(
        ui,
        "Committed base revision",
        "cloud-revision",
        &mut form.revision,
        "HEAD",
    );
    ui.label(
        RichText::new("Only committed files are transferred. Local changes stay on this computer.")
            .size(12.0)
            .color(theme::FG_SOFT()),
    );
    let load = ui
        .add(
            Button::new(RichText::new("Read .horizon/cloud.yml").size(13.0))
                .min_size(Vec2::new(0.0, 32.0))
                .corner_radius(8),
        )
        .clicked();
    ui.add_space(8.0);
    ui.label(RichText::new("Profile").size(14.0).strong().color(theme::FG()));
    if let Some(config) = &form.profiles {
        ui.horizontal_wrapped(|ui| {
            for name in config.profiles.keys() {
                if ui
                    .add(
                        Button::new(RichText::new(name).size(14.0))
                            .selected(form.selected_profile == *name)
                            .min_size(Vec2::new(0.0, 34.0))
                            .corner_radius(8),
                    )
                    .clicked()
                {
                    form.selected_profile.clone_from(name);
                }
            }
        });
        if let Some(profile) = config.profiles.get(&form.selected_profile) {
            ui.label(
                RichText::new(format!(
                    "{} vCPU · {} GB memory · {}",
                    profile.cpu,
                    profile.memory_gb,
                    if profile.gpu { "GPU" } else { "CPU only" }
                ))
                .size(13.0)
                .color(theme::FG_SOFT()),
            );
        }
    } else {
        ui.label(
            RichText::new("Read the repository settings to choose a profile.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
    }
    load
}

fn footer(ui: &mut Ui, form: &Production, actions: &mut Actions) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 40.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            actions.create = ui
                .add_enabled(
                    !form.title.trim().is_empty() && form.profiles.is_some() && form.pending_creation.is_none(),
                    Button::new(
                        RichText::new(if form.pending_creation.is_some() {
                            "Checking repository…"
                        } else {
                            "Create cloud"
                        })
                        .size(14.0)
                        .strong(),
                    )
                    .min_size(Vec2::new(136.0, 40.0))
                    .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.35))
                    .stroke(Stroke::new(1.0, theme::ACCENT()))
                    .corner_radius(10),
                )
                .clicked();
            actions.cancel = ui
                .add(
                    Button::new(RichText::new("Cancel").size(14.0))
                        .min_size(Vec2::new(88.0, 40.0))
                        .corner_radius(10),
                )
                .clicked();
        },
    );
}
