//! Presentation of the existing repository-backed cloud creation flow.
use super::{HorizonApp, Production};
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use crate::theme;
use egui::{Align, Button, Context, Frame, Id, Key, Layout, RichText, Stroke, TextEdit, Ui, Vec2};
use horizon_core::{
    ShortcutBinding, ShortcutKey, ShortcutModifiers, cloud_panel::Placement,
    cloud_runtime::provider::Placement as ProviderPlacement,
};
use std::path::{Path, PathBuf};

pub(super) mod checks;
mod costs;
mod placement;
mod pricing;
mod profiles;
pub(super) mod provider;
pub(super) mod selector;
pub(super) mod siblings;
pub(super) mod source;
mod storage;
mod watch;

/// The configuration summary's width beside the catalog.
const SUMMARY_WIDTH: f32 = 330.0;
const GUTTER: f32 = 24.0;
/// The narrowest dialog that still shows the summary beside the catalog.
const TWO_COLUMNS: f32 = 800.0;

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
    /// A folder or fresh clone to make the repository.
    Adopt(PathBuf),
    /// Choose the folder clones go into.
    CloneFolder,
    /// The checkout of the named same-worker sibling.
    ChooseSibling(String),
}

impl HorizonApp {
    pub(in crate::app::cloud_panel) fn render_cloud_creation(&mut self, ctx: &Context) {
        if !self.cloud_prototype.production.creating {
            return;
        }
        let viewport = ctx.content_rect();
        let width = (viewport.width() - 64.0).clamp(240.0, 1180.0);
        // Room for the heading and the action bar under the columns.
        let body_height = (viewport.height() - 270.0).max(120.0);
        let mut actions = Actions::default();
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let picking = self.dir_picker.is_some();
        let refocus_repository = self.prepare_cloud_creation(ctx, picking);
        let form = &mut self.cloud_prototype.production;
        form.launch.siblings.sync(
            ctx,
            &form.repository,
            form.launch.revision.as_deref(),
            form.profiles.as_ref(),
            &form.selected_profile,
        );
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
                heading(ui, provider::label(&self.cloud_prototype.production));
                ui.add_space(16.0);
                if width >= TWO_COLUMNS {
                    let left = width - SUMMARY_WIDTH - GUTTER;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = GUTTER;
                        column(ui, left, body_height, |ui| {
                            super::super::runtime::solid_scroll_area(ui)
                                .id_salt("cloud-creation-body")
                                .max_height(body_height)
                                .show(ui, |ui| self.cloud_creation_body(ui, &mut actions, refocus_repository));
                        });
                        column(ui, SUMMARY_WIDTH, body_height, |ui| {
                            super::super::runtime::solid_scroll_area(ui)
                                .id_salt("cloud-creation-summary")
                                .max_height(body_height)
                                .show(ui, |ui| {
                                    checks::with_summary(
                                        ui,
                                        &mut self.cloud_prototype.production,
                                        self.cloud_prototype.root.as_deref(),
                                    );
                                });
                        });
                    });
                } else {
                    super::super::runtime::solid_scroll_area(ui)
                        .id_salt("cloud-creation-body")
                        .max_height(body_height)
                        .show(ui, |ui| {
                            self.cloud_creation_body(ui, &mut actions, refocus_repository);
                            ui.add_space(12.0);
                            checks::with_summary(
                                ui,
                                &mut self.cloud_prototype.production,
                                self.cloud_prototype.root.as_deref(),
                            );
                        });
                }
                ui.add_space(12.0);
                checks::footer(ui, &mut self.cloud_prototype.production, &mut actions);
            });
        ctx.move_to_top(response.response.layer_id);
        let dismissed = self.cloud_creation_dismissed(ctx, &response, picking, escape);
        if dismissed || actions.cancel {
            self.cloud_prototype.production.creating = false;
            self.cloud_prototype.production.pending_creation = None;
            self.cloud_prototype.production.launch = super::launch::State::default();
            // A clone still running stops, and a token that was not used is forgotten.
            self.cloud_prototype.production.source = source::State::default();
            return;
        }
        match actions.repository {
            RepositoryAction::Choose => self.choose_cloud_repository(ctx),
            RepositoryAction::Load => self.read_cloud_profiles(ctx),
            RepositoryAction::Setup => self.start_cloud_repository_setup(ctx),
            RepositoryAction::ChooseSibling(alias) => self.choose_cloud_sibling(ctx, alias),
            RepositoryAction::Adopt(path) => self.adopt_cloud_source(ctx, &path),
            RepositoryAction::CloneFolder => self.choose_clone_folder(ctx),
            RepositoryAction::None => {}
        }
        // The Start button and Enter in the title take the same path: with "Start new
        // cloud once available" checked for a sold-out worker, both arm the watch.
        if actions.create && !self.cloud_prototype.production.title.trim().is_empty() {
            selector::summary::start(&mut self.cloud_prototype.production);
        }
        watch::poll(&mut self.cloud_prototype.production);
        self.poll_cloud_launch(ctx);
        self.poll_cloud_creation(ctx);
    }

    /// What the dialog settles before it draws: the pickers that closed, the prices and the
    /// checks it keeps current, and a clone that finished. True when focus returns to the field.
    fn prepare_cloud_creation(&mut self, ctx: &Context, picking: bool) -> bool {
        // Focus returns to the field when its picker closes, whether or not a directory was chosen.
        let refocus_repository = !picking && std::mem::take(&mut self.cloud_prototype.production.choosing_repository);
        if !picking {
            self.cloud_prototype.production.launch.siblings.stop_browsing();
            self.cloud_prototype.production.source.take_choosing_parent();
        }
        if refocus_repository && self.cloud_prototype.production.profiles.is_none() {
            self.read_cloud_profiles(ctx);
        }
        self.request_cloud_prices(ctx);
        checks::update(
            &mut self.cloud_prototype.production,
            self.cloud_prototype.root.as_deref(),
            ctx,
        );
        if let Some(path) = self.cloud_prototype.production.source.poll(ctx) {
            self.adopt_cloud_source(ctx, &path);
        }
        refocus_repository
    }

    /// Everything left of the summary: title, profile, worker, place and more options.
    fn cloud_creation_body(&mut self, ui: &mut Ui, actions: &mut Actions, refocus_repository: bool) {
        let form = &mut self.cloud_prototype.production;
        ui.add_enabled_ui(
            form.pending_creation.is_none() && !form.launch.submitted && form.launch.watch.is_none(),
            |ui| {
                actions.repository = fields(ui, form, &mut actions.create, refocus_repository);
                if !form.launch.loading()
                    && form.profiles.is_none()
                    && !checks::source_step(form)
                    && super::repository_setup::render(ui, form)
                {
                    actions.repository = RepositoryAction::Setup;
                }
            },
        );
        if let Some(error) = &self.cloud_prototype.error {
            ui.add_space(8.0);
            ui.colored_label(theme::PALETTE_RED(), error);
        }
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
        self.consume_picker_activation(ctx);
        let form = &mut self.cloud_prototype.production;
        form.choosing_repository = true;
        let current = (!form.repository.trim().is_empty()).then(|| Path::new(&form.repository));
        self.dir_picker = Some(DirPicker::with_seed(DirPickerPurpose::CloudRepository, current));
    }

    /// Makes `path` the repository and reads its profiles: a folder that was typed, or a fresh clone.
    fn adopt_cloud_source(&mut self, ctx: &Context, path: &Path) {
        self.set_cloud_repository(path);
        self.read_cloud_profiles(ctx);
    }

    /// Lets the person pick the folder clones go into, starting from the nearest one that exists.
    fn choose_clone_folder(&mut self, ctx: &Context) {
        self.consume_picker_activation(ctx);
        let mut parent = self.cloud_prototype.production.source.clone_parent();
        while !parent.is_dir() && parent.pop() {}
        self.dir_picker = Some(DirPicker::with_seed(DirPickerPurpose::CloudRepository, Some(&parent)));
    }

    /// The repository picker also chooses a sibling's checkout, which it then returns.
    fn choose_cloud_sibling(&mut self, ctx: &Context, alias: String) {
        self.consume_picker_activation(ctx);
        let siblings = &mut self.cloud_prototype.production.launch.siblings;
        let seed = siblings.seed(&alias);
        siblings.browse(alias);
        self.dir_picker = Some(DirPicker::with_seed(DirPickerPurpose::CloudRepository, seed.as_deref()));
    }

    fn consume_picker_activation(&mut self, ctx: &Context) {
        // Keyboard activation (Enter with any modifiers) must neither confirm the picker drawn later this
        // frame nor, held, confirm it on a repeat.
        if ctx.input(|input| input.key_pressed(Key::Enter)) {
            self.consume_navigation_key(ctx, navigation_key(ShortcutKey::Enter));
        }
    }

    pub(in crate::app) fn set_cloud_repository(&mut self, path: &Path) {
        let form = &mut self.cloud_prototype.production;
        if form.source.take_choosing_parent() {
            form.source.set_parent(path);
            return;
        }
        if form.launch.siblings.choose_checkout(path) {
            return;
        }
        let repository = path.to_string_lossy();
        if form.repository != repository {
            form.repository = repository.into_owned();
            form.profiles = None;
            form.selected_profile.clear();
            form.size = None;
            form.placement = Placement::default();
            form.provider = None;
            form.launch.accounts_checked = false;
            if form.title.trim().is_empty() {
                form.title = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            self.cloud_prototype.error = None;
        }
    }
}

fn navigation_key(key: ShortcutKey) -> ShortcutBinding {
    ShortcutBinding::new(ShortcutModifiers::NONE, key)
}

fn heading(ui: &mut Ui, provider: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("New cloud").size(26.0).strong().color(theme::FG()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            Frame::new()
                .fill(theme::PANEL_BG_ALT())
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.label(RichText::new(provider).size(13.0).color(theme::FG_SOFT()));
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

/// The cloud title, focused when the form opens; Enter submits a complete form.
fn title_field(ui: &mut Ui, form: &mut Production, submit: &mut bool) {
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
    title_requirement(ui, form);
    if title.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && can_submit(form) {
        *submit = true;
    }
}

fn column(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(Vec2::new(width, height), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.set_max_width(width);
        // A steady height keeps the dialog from jumping as choices change.
        ui.set_min_height(height);
        add(ui);
    });
}

fn fields(ui: &mut Ui, form: &mut Production, submit: &mut bool, refocus_repository: bool) -> RepositoryAction {
    let step = source::step(ui, form, refocus_repository);
    let mut action = match step {
        source::Step { adopt: Some(path), .. } => RepositoryAction::Adopt(path),
        source::Step { browse: true, .. } => RepositoryAction::Choose,
        source::Step {
            choose_folder: true, ..
        } => RepositoryAction::CloneFolder,
        _ => RepositoryAction::None,
    };
    ui.add_space(8.0);
    title_field(ui, form, submit);
    if checks::source_step(form) {
        return action;
    }
    let listed = if form.launch.loading() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Preparing cloud…");
        });
        false
    } else if form.profiles.is_some() {
        profiles::field(ui, form);
        ui.add_space(8.0);
        machine(ui, form)
    } else {
        false
    };
    // Siblings can hold back Start, so they stay in view.
    if form.profiles.is_some()
        && !form.launch.loading()
        && let Some(alias) = siblings::section(ui, &mut form.launch.siblings)
    {
        action = RepositoryAction::ChooseSibling(alias);
    }
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("More options").size(14.0))
        .id_salt("cloud-more-options")
        .default_open(form.profiles.is_none() && !form.launch.loading())
        .show(ui, |ui| {
            if listed
                && let Some(profile) = form
                    .profiles
                    .as_mut()
                    .and_then(|config| config.profiles.get_mut(&form.selected_profile))
            {
                storage::container_field(ui, profile, provider::current(form.provider, profile));
            }
            match advanced_fields(ui, form) {
                RepositoryAction::None => {}
                chosen => action = chosen,
            }
        });
    action
}

/// The worker for the selected profile: the wide selector where the provider lists its
/// offers, and the provider's own size and location fields otherwise. Returns whether
/// the selector was shown.
fn machine(ui: &mut Ui, form: &mut Production) -> bool {
    let Some(config) = &mut form.profiles else {
        return false;
    };
    let Some(profile) = config.profiles.get_mut(&form.selected_profile) else {
        return false;
    };
    // A profile reread as CPU only drops a GPU type chosen while it was a GPU profile.
    if !profile.gpu {
        form.placement.gpu_types.clear();
    }
    let choices = provider::choices(&form.prices, profile);
    // A provider chosen before the profile or the configured providers changed is
    // dropped once it is no longer a choice, with the place it named.
    if form.provider.is_some_and(|chosen| !choices.contains(&chosen)) {
        form.provider = None;
        form.placement = Placement::default();
    }
    let provider = provider::current(form.provider, profile);
    // A profile naming a provider this machine cannot use is never moved on its
    // own: the person picks one it can, even when there is only one.
    let unusable = !choices.is_empty() && !choices.contains(&provider);
    if unusable {
        ui.small(format!(
            "This profile names {}, which this machine has no credentials for. Choose a provider it can use.",
            provider.label
        ));
    }
    if (choices.len() > 1 || unusable)
        && let Some(chosen) = provider::choice(ui, &choices, provider)
    {
        form.provider = Some(chosen);
        // Each provider names places and sizes its own way.
        form.placement = Placement::default();
        form.size = None;
        return false;
    }
    if provider.placement == ProviderPlacement::DataCenters {
        selector::section(ui, form);
        return true;
    }
    let (cpu, memory_gb) = form.size.unwrap_or((profile.cpu, profile.memory_gb));
    storage::field(ui, profile, provider);
    if let Some(size) = provider::size_field(ui, &form.prices, profile, (cpu, memory_gb)) {
        form.size = Some(size);
    }
    let sized = horizon_core::cloud_runtime::prices::Profile {
        cpu,
        memory_gb,
        ..profile.clone()
    };
    ui.add_space(4.0);
    if let Some(placement) = provider::card(ui, &form.prices, (provider, &sized), &form.placement) {
        form.placement = placement;
    }
    false
}

fn advanced_fields(ui: &mut Ui, form: &mut Production) -> RepositoryAction {
    let mut changed = false;
    ui.add_space(8.0);
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
    if form.profiles.is_none() {
        ui.label(
            RichText::new("Read the repository settings to choose a profile.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
    }
    if load || changed {
        RepositoryAction::Load
    } else {
        RepositoryAction::None
    }
}

fn title_requirement(ui: &mut Ui, form: &Production) {
    if form.title.trim().is_empty() && !checks::source_step(form) {
        ui.label(
            RichText::new("A cloud title is required before Start cloud can be used.")
                .size(14.0)
                .color(theme::FG()),
        );
    }
}

/// Why Start is unavailable, for the selector tests; the dialog works it out once per
/// frame with [`submit_reason_given`].
#[cfg(all(test, unix))]
fn submit_reason(form: &Production) -> Option<&'static str> {
    submit_reason_given(form, storage::launch_reason(form))
}

/// [`submit_reason`] with the chosen worker's launch reason already worked out.
fn submit_reason_given(form: &Production, blocked: Option<&'static str>) -> Option<&'static str> {
    if form.pending_creation.is_some() || form.launch.submitted {
        return None;
    }
    match (
        form.title.trim().is_empty(),
        form.profiles.is_none() && !form.launch.loading(),
    ) {
        (false, false) => blocked,
        (true, false) => Some("Enter a cloud title to start this cloud."),
        (false, true) => Some("Read the repository profile before starting."),
        (true, true) => Some("Enter a cloud title and read the repository profile."),
    }
}

fn can_submit(form: &Production) -> bool {
    can_submit_given(form, storage::launch_reason(form))
}

/// [`can_submit`] with the chosen worker's launch reason already worked out.
fn can_submit_given(form: &Production, blocked: Option<&'static str>) -> bool {
    !form.title.trim().is_empty()
        && (form.profiles.is_some() || form.launch.loading())
        && form.pending_creation.is_none()
        && !form.launch.submitted
        && form.launch.watch.is_none()
        && !form.launch.siblings.blocks_launch()
        && blocked.is_none()
}
