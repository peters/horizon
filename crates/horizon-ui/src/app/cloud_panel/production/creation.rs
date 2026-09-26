//! Presentation of the existing repository-backed cloud creation flow.
use super::{HorizonApp, Production};
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use crate::theme;
use egui::{Align, Button, Context, Frame, Id, Key, Layout, RichText, Stroke, TextEdit, Ui, Vec2};
use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers, cloud_panel::Placement, dir_search};
use std::path::Path;

mod costs;
mod placement;
mod pricing;

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
    Choose,
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
        let picking = self.dir_picker.is_some();
        // Focus returns to the field when its picker closes, whether or not a directory was chosen.
        let refocus_repository = !picking && std::mem::take(&mut self.cloud_prototype.production.choosing_repository);
        if refocus_repository && self.cloud_prototype.production.profiles.is_none() {
            self.read_cloud_profiles(ctx);
        }
        self.request_cloud_prices(ctx);
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
                if picking {
                    ui.disable();
                }
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                heading(ui);
                ui.add_space(16.0);
                egui::ScrollArea::vertical()
                    .id_salt("cloud-creation-body")
                    .max_height(body_height)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(
                            self.cloud_prototype.production.pending_creation.is_none()
                                && !self.cloud_prototype.production.launch.submitted,
                            |ui| {
                                actions.repository = fields(
                                    ui,
                                    &mut self.cloud_prototype.production,
                                    &mut actions.create,
                                    refocus_repository,
                                );
                                if !self.cloud_prototype.production.launch.loading()
                                    && self.cloud_prototype.production.profiles.is_none()
                                    && super::repository_setup::render(ui, &mut self.cloud_prototype.production)
                                {
                                    actions.repository = RepositoryAction::Setup;
                                }
                            },
                        );
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
        let dismissed = self.cloud_creation_dismissed(ctx, &response, picking, escape);
        if dismissed || actions.cancel {
            self.cloud_prototype.production.creating = false;
            self.cloud_prototype.production.pending_creation = None;
            self.cloud_prototype.production.launch = super::launch::State::default();
            return;
        }
        match actions.repository {
            RepositoryAction::Choose => self.choose_cloud_repository(ctx),
            RepositoryAction::Load => self.read_cloud_profiles(ctx),
            RepositoryAction::Setup => self.start_cloud_repository_setup(ctx),
            RepositoryAction::None => {}
        }
        if actions.create && !self.cloud_prototype.production.title.trim().is_empty() {
            self.cloud_prototype.production.launch.submitted = true;
        }
        self.poll_cloud_launch(ctx);
        self.poll_cloud_creation(ctx);
    }

    /// Keeps prices and stock current for the size being chosen.
    fn request_cloud_prices(&mut self, ctx: &Context) {
        let production = &mut self.cloud_prototype.production;
        production.prices.poll();
        let Some(root) = self.cloud_prototype.root.as_deref() else {
            return;
        };
        let Some(profile) = production
            .profiles
            .as_ref()
            .and_then(|config| config.profiles.get(&production.selected_profile))
        else {
            return;
        };
        let (cpu, memory_gb) = production.size.unwrap_or((profile.cpu, profile.memory_gb));
        let sized = horizon_core::cloud_runtime::prices::Profile {
            cpu,
            memory_gb,
            ..profile.clone()
        };
        production.prices.request(root, &sized, ctx);
    }

    /// The directory picker drawn above this dialog owns Escape and outside clicks until it closes.
    fn cloud_creation_dismissed(
        &mut self,
        ctx: &Context,
        response: &egui::ModalResponse<()>,
        picking: bool,
        escape: bool,
    ) -> bool {
        if !picking {
            let dismissed = response.should_close();
            if dismissed && escape {
                self.consume_navigation_key(ctx, navigation_key(ShortcutKey::Escape));
            }
            return dismissed;
        }
        if escape {
            // The picker, drawn later this frame, still cancels on this press; held repeats must not
            // reach the dialog once the picker has closed.
            self.hold_navigation_key(navigation_key(ShortcutKey::Escape));
        }
        let clicked_body = ctx
            .input(|input| input.pointer.primary_clicked().then(|| input.pointer.interact_pos()))
            .flatten()
            .is_some_and(|position| ctx.layer_id_at(position) == Some(response.response.layer_id));
        if response.backdrop_response.clicked() || clicked_body {
            self.dir_picker = None;
        }
        false
    }

    fn choose_cloud_repository(&mut self, ctx: &Context) {
        // Keyboard activation (Enter with any modifiers) must neither confirm the picker drawn later this
        // frame nor, held, confirm it on a repeat.
        if ctx.input(|input| input.key_pressed(Key::Enter)) {
            self.consume_navigation_key(ctx, navigation_key(ShortcutKey::Enter));
        }
        let form = &mut self.cloud_prototype.production;
        form.choosing_repository = true;
        let current = (!form.repository.trim().is_empty()).then(|| Path::new(&form.repository));
        self.dir_picker = Some(DirPicker::with_seed(DirPickerPurpose::CloudRepository, current));
    }

    pub(in crate::app) fn set_cloud_repository(&mut self, path: &Path) {
        let form = &mut self.cloud_prototype.production;
        let repository = path.to_string_lossy();
        if form.repository != repository {
            form.repository = repository.into_owned();
            form.profiles = None;
            form.selected_profile.clear();
            form.size = None;
            form.placement = Placement::default();
            form.launch.accounts_checked = false;
            self.cloud_prototype.error = None;
        }
    }
}

fn navigation_key(key: ShortcutKey) -> ShortcutBinding {
    ShortcutBinding::new(ShortcutModifiers::NONE, key)
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

fn repository_field(ui: &mut Ui, repository: &str) -> egui::Response {
    ui.label(RichText::new("Repository").size(14.0).strong().color(theme::FG()));
    let path = if repository.trim().is_empty() {
        RichText::new("Choose a local repository").color(theme::FG_DIM())
    } else {
        RichText::new(dir_search::abbreviate_home(Path::new(repository))).color(theme::FG())
    };
    ui.scope_builder(egui::UiBuilder::new().id(Id::new("cloud-repository")), |ui| {
        ui.spacing_mut().button_padding = Vec2::new(12.0, 10.0);
        ui.add(
            Button::new(path.size(15.0))
                .right_text(RichText::new("Browse…").size(13.0).color(theme::FG_SOFT()))
                .truncate()
                .fill(ui.visuals().text_edit_bg_color())
                .min_size(Vec2::new(ui.available_width(), 38.0)),
        )
    })
    .inner
}

fn fields(ui: &mut Ui, form: &mut Production, submit: &mut bool, refocus_repository: bool) -> RepositoryAction {
    if form.focus_title_on_open
        && !ui.is_sizing_pass()
        && ui.is_enabled()
        && !ui.input(|input| input.pointer.any_down() || input.pointer.any_released())
    {
        ui.memory_mut(|memory| memory.request_focus(Id::new("cloud-title")));
        form.focus_title_on_open = false;
    }
    let title = field(
        ui,
        "Cloud title",
        "cloud-title",
        &mut form.title,
        "e.g. Feature development",
    );
    if title.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && can_submit(form) {
        *submit = true;
    }
    if form.launch.loading() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Preparing cloud…");
        });
    } else if let Some(config) = &form.profiles {
        ui.small(&form.repository);
        if let Some(profile) = config.profiles.get(&form.selected_profile) {
            let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
            ui.small(format!("{} · {cpu} vCPU · {memory_gb} GB", form.selected_profile));
            if let Some(size) = pricing::size_field(ui, &form.prices, profile, (cpu, memory_gb)) {
                form.size = Some(size);
            }
            let sized = horizon_core::cloud_runtime::prices::Profile {
                cpu,
                memory_gb,
                ..profile.clone()
            };
            if let Some(placement) = placement::region_field(ui, &form.prices, &sized, &form.placement) {
                form.placement = placement;
            }
            ui.add_space(4.0);
            if pricing::card(ui, &form.prices, &sized, &form.placement) {
                form.prices.refresh();
            }
        }
    }
    ui.small("Only committed files are transferred. Local changes stay on this computer.");
    let mut action = RepositoryAction::None;
    egui::CollapsingHeader::new("Advanced")
        .default_open(form.profiles.is_none() && !form.launch.loading())
        .show(ui, |ui| {
            action = advanced_fields(ui, form, refocus_repository);
        });
    action
}

fn advanced_fields(ui: &mut Ui, form: &mut Production, refocus_repository: bool) -> RepositoryAction {
    let mut changed = false;
    ui.add_space(8.0);
    let repository = repository_field(ui, &form.repository);
    if refocus_repository {
        repository.request_focus();
    }
    let choose = repository.clicked();
    ui.add_space(8.0);
    changed |= field(
        ui,
        "Committed base revision",
        "cloud-revision",
        &mut form.revision,
        "HEAD",
    )
    .changed();
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
                    if form.selected_profile != *name {
                        form.size = None;
                        form.placement = Placement::default();
                    }
                    form.selected_profile.clone_from(name);
                    form.launch.accounts_checked = false;
                }
            }
        });
        if let Some(profile) = config.profiles.get(&form.selected_profile) {
            let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
            ui.label(
                RichText::new(format!(
                    "{cpu} vCPU · {memory_gb} GB memory · {}",
                    if profile.gpu { "GPU" } else { "CPU only" }
                ))
                .size(13.0)
                .color(theme::FG_SOFT()),
            );
            let sized = horizon_core::cloud_runtime::prices::Profile {
                cpu,
                memory_gb,
                ..profile.clone()
            };
            if let Some(placement) = placement::data_center_field(ui, &form.prices, &sized, &form.placement) {
                form.placement = placement;
            }
        }
    } else {
        ui.label(
            RichText::new("Read the repository settings to choose a profile.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
    }
    if choose {
        RepositoryAction::Choose
    } else if load || changed {
        RepositoryAction::Load
    } else {
        RepositoryAction::None
    }
}

/// Offered CPU worker sizes; a GPU profile's size is fixed. Buttons and inline notes rather
/// than drop-downs and tooltips, which would draw below this Tooltip-order modal.
fn footer(ui: &mut Ui, form: &Production, actions: &mut Actions) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 40.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            actions.create |= ui
                .add_enabled(
                    can_submit(form),
                    Button::new(
                        RichText::new(if form.pending_creation.is_some() || form.launch.submitted {
                            "Starting cloud…"
                        } else {
                            "Start cloud"
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

fn can_submit(form: &Production) -> bool {
    !form.title.trim().is_empty()
        && (form.profiles.is_some() || form.launch.loading())
        && form.pending_creation.is_none()
        && !form.launch.submitted
}
