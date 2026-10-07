//! Presentation of the existing repository-backed cloud creation flow.
use super::{HorizonApp, Production};
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use crate::theme;
use egui::{Align, Button, Context, Frame, Id, Key, LayerId, Layout, Order, RichText, Stroke, TextEdit, Ui, Vec2};
use horizon_core::{
    ShortcutBinding, ShortcutKey, ShortcutModifiers, cloud_panel::Placement,
    cloud_runtime::repository::launch::Configuration,
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
/// Room for the heading and the action bar around the body.
const CHROME_HEIGHT: f32 = 270.0;
const BODY_MIN_HEIGHT: f32 = 120.0;

/// The dialog body's height, in one column or two, taken from the window alone.
pub(super) fn body_height(viewport: egui::Rect) -> f32 {
    (viewport.height() - CHROME_HEIGHT).max(BODY_MIN_HEIGHT)
}

/// Whether the dialog `id` is measured before it is drawn: when it opens in a window of another
/// size than the one it was last shown in, so the size egui remembers cannot place the first
/// visible frame.
fn measure_on_open(ctx: &Context, id: Id) -> bool {
    let window = ctx.content_rect().size();
    let key = id.with("shown-in");
    let shown_in = ctx.data(|data| data.get_temp::<Vec2>(key));
    ctx.data_mut(|data| data.insert_temp(key, window));
    let opening = !ctx.memory(|memory| memory.areas().visible_last_frame(&LayerId::new(Order::Tooltip, id)));
    opening && shown_in != Some(window)
}

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
        let body_height = body_height(viewport);
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
        let sizing_pass = measure_on_open(ctx, id);
        // Root chrome uses Tooltip order; raise this modal last to contain its input too.
        let response = egui::Modal::new(id)
            .area(
                egui::Modal::default_area(id)
                    .order(Order::Tooltip)
                    .sizing_pass(sizing_pass),
            )
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
                self.cloud_creation_columns(ui, width, body_height, &mut actions, refocus_repository);
                ui.add_space(12.0);
                checks::footer(ui, &mut self.cloud_prototype.production, &mut actions);
            });
        ctx.move_to_top(response.response.layer_id);
        let dismissed = self.cloud_creation_dismissed(ctx, &response, picking, escape);
        if dismissed || actions.cancel {
            self.close_cloud_creation();
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

    /// The worker fields and the summary: side by side when the dialog is wide enough, else
    /// stacked in one scroll area. Either way the body is `body_height` tall.
    fn cloud_creation_columns(
        &mut self,
        ui: &mut Ui,
        width: f32,
        body_height: f32,
        actions: &mut Actions,
        refocus_repository: bool,
    ) {
        if width >= TWO_COLUMNS {
            let left = width - SUMMARY_WIDTH - GUTTER;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GUTTER;
                column(ui, left, body_height, |ui| {
                    super::super::runtime::solid_scroll_area(ui)
                        .id_salt("cloud-creation-body")
                        .max_height(body_height)
                        .show(ui, |ui| self.cloud_creation_body(ui, actions, refocus_repository));
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
            // No column holds this height, so the scroll area fills it itself instead of
            // keeping what the dialog's previous size left it, such as egui's sizing pass.
            super::super::runtime::solid_scroll_area(ui)
                .id_salt("cloud-creation-body")
                .auto_shrink(false)
                .min_scrolled_height(body_height)
                .max_height(body_height)
                .show(ui, |ui| {
                    self.cloud_creation_body(ui, actions, refocus_repository);
                    ui.add_space(12.0);
                    checks::with_summary(
                        ui,
                        &mut self.cloud_prototype.production,
                        self.cloud_prototype.root.as_deref(),
                    );
                });
        }
    }

    /// Closes the dialog. A clone still running stops, and a token or key that was typed but not
    /// used is forgotten.
    pub(in crate::app::cloud_panel::production) fn close_cloud_creation(&mut self) {
        let form = &mut self.cloud_prototype.production;
        form.creating = false;
        form.pending_creation = None;
        form.launch = super::launch::State::default();
        form.source = source::State::default();
        form.checks = checks::State::default();
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
        // A picker closed without a repository has nothing to read.
        if refocus_repository
            && self.cloud_prototype.production.profiles.is_none()
            && !self.cloud_prototype.production.repository.trim().is_empty()
        {
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
                if !form.launch.loading() && form.profiles.is_none() && !checks::source_step(form) {
                    match super::repository_setup::render(ui, form) {
                        super::repository_setup::Choice::None => {}
                        super::repository_setup::Choice::SetupAgent => actions.repository = RepositoryAction::Setup,
                        super::repository_setup::Choice::QuickStart => {
                            super::repository_setup::set_configuration(form, Configuration::QuickStart);
                            actions.repository = RepositoryAction::Load;
                        }
                    }
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
        form.source.show_chosen_again();
        let repository = path.to_string_lossy();
        if form.repository != repository {
            form.repository = repository.into_owned();
            form.profiles = None;
            // Quick start is chosen for one repository; another one is read as committed.
            if form.launch.configuration == Configuration::QuickStart {
                form.launch.configuration = Configuration::Committed;
            }
            form.launch.unconfigured = false;
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

/// Drops what egui remembers of the edits made in a field. It records every one, in plaintext even
/// for a password field, so a secret typed or pasted there would outlive the string that held it.
pub(super) fn forget_undo(ctx: &Context, id: Id) {
    if let Some(mut state) = TextEdit::load_state(ctx, id) {
        state.clear_undoer();
        state.store(ctx, id);
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
        super::repository_setup::quick_start_note(ui, form);
        ui.add_space(12.0);
        form.tailnets.choice(ui, &mut form.tailnet);
        ui.add_space(12.0);
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
/// the summary leaves the container disk editor to More options.
fn machine(ui: &mut Ui, form: &mut Production) -> bool {
    if let Some(profile) = selector::profile(form) {
        let gpu = profile.gpu;
        if form
            .provider
            .is_some_and(|chosen| !provider::choices(&form.prices, profile).contains(&chosen))
        {
            form.provider = None;
            form.placement = Placement::default();
            form.size = None;
        }
        form.placement = form.placement.for_profile(gpu);
    }
    selector::section(ui, form);
    selector::profile(form).is_some_and(|profile| {
        provider::current(form.provider, profile).placement
            == horizon_core::cloud_runtime::provider::Placement::DataCenters
    })
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
    let mut local = form.launch.configuration == Configuration::LocalImageOnly;
    if ui.checkbox(&mut local, "Use local image-only settings").changed() {
        let configuration = if local {
            Configuration::LocalImageOnly
        } else {
            Configuration::Committed
        };
        super::repository_setup::set_configuration(form, configuration);
        changed = true;
    }
    if local {
        ui.label(
            RichText::new("Reads the working copy of .horizon/cloud.yml. Only profiles without build are offered; settings are saved locally for this cloud.")
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
    }
    let load = ui
        .add(
            Button::new(RichText::new("Read .horizon/cloud.yml").size(13.0))
                .min_size(Vec2::new(0.0, 32.0))
                .corner_radius(8),
        )
        .clicked();
    // Reading the file leaves quick start for the repository's own settings.
    if load && form.launch.configuration == Configuration::QuickStart {
        super::repository_setup::set_configuration(form, Configuration::Committed);
    }
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
        (false, false) if blocked.is_none() && !form.tailnets.ready() => Some("Load tailnet settings before starting."),
        (false, false) => blocked,
        (true, false) => Some("Enter a cloud title to start this cloud."),
        (false, true) => Some("Read the repository profile before starting."),
        (true, true) => Some("Enter a cloud title and read the repository profile."),
    }
}

pub(super) fn can_submit(form: &Production) -> bool {
    can_submit_given(form, storage::launch_reason(form))
}

/// [`can_submit`] with the chosen worker's launch reason already worked out.
fn can_submit_given(form: &Production, blocked: Option<&'static str>) -> bool {
    // While another repository is being asked for, the one loaded is not what would start.
    !form.source.editing()
        && !form.title.trim().is_empty()
        && (form.profiles.is_some() || form.launch.loading())
        && form.pending_creation.is_none()
        && !form.launch.submitted
        && form.launch.watch.is_none()
        && !form.launch.siblings.blocks_launch()
        && blocked.is_none()
        && form.tailnets.ready()
}
