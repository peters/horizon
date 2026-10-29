//! Presentation of the existing repository-backed cloud creation flow.
use super::{HorizonApp, Production};
use crate::app::view::canvas_scene_transform;
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use crate::theme;
use egui::{Align, Context, Frame, Id, Key, Layout, Order, Pos2, Rect, RichText, Stroke, TextEdit, Ui, Vec2};
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
pub(super) mod publishing;
pub(super) mod selector;
pub(super) mod siblings;
pub(super) mod source;
mod storage;
pub(super) mod uncommitted;
pub(in crate::app::cloud_panel::production) mod watch;

/// The configuration summary's width beside the catalog.
const SUMMARY_WIDTH: f32 = 330.0;
const GUTTER: f32 = 24.0;
/// The narrowest form that still shows the summary beside the catalog.
const TWO_COLUMNS: f32 = 800.0;
/// Room for the heading and the action bar around the body.
pub(super) const CHROME_HEIGHT: f32 = 270.0;
/// The narrowest form that stays usable, also in a workspace the person made small.
pub(super) const FORM_MIN_WIDTH: f32 = 640.0;
/// The lowest body, so that the fields never collapse to a sliver.
const BODY_MIN_HEIGHT: f32 = 320.0;
/// Room for the form's own margins beside its content, both sides together.
const SIDE_MARGIN: f32 = 64.0;

/// The height the body takes in a window of `window`, which is also the tallest body.
fn body_ceiling(window: egui::Rect) -> f32 {
    (window.height() - CHROME_HEIGHT).max(BODY_MIN_HEIGHT)
}

/// The form on `surface`: the width and body height it takes there, and where its frame starts. It
/// fills the workspace that holds it between its own floor and the size the dialog had as a modal,
/// and it stays on `canvas`, where an edge moves it back inside.
fn form_layout(surface: Rect, canvas: Rect, ceiling: f32, widest: f32) -> (f32, f32, Pos2) {
    let width = (surface.width() - SIDE_MARGIN)
        .clamp(FORM_MIN_WIDTH, widest)
        .min(canvas.width() - SIDE_MARGIN);
    let body = (surface.height() - CHROME_HEIGHT).clamp(BODY_MIN_HEIGHT, ceiling);
    // The form draws its own chrome, so the room kept below the workspace's top is the body plus
    // that reserve, which also covers a heading and an action bar taller than this dialog's.
    let origin = Pos2::new(
        surface
            .min
            .x
            .min(canvas.right() - (width + SIDE_MARGIN))
            .max(canvas.left()),
        surface
            .min
            .y
            .min(canvas.bottom() - (body + CHROME_HEIGHT))
            .max(canvas.top()),
    );
    (width, body, origin)
}

#[derive(Default)]
struct Actions {
    repository: RepositoryAction,
    create: bool,
    /// Ends the draft: its workspace goes when it held nothing, and what was typed is lost.
    cancel: bool,
    /// Puts the form away: the workspace keeps the draft, and its card brings the form back.
    hide: bool,
    /// The repository's `cloud.yml` asks for This PC, and the person chose it.
    this_pc: bool,
}

#[derive(Default)]
enum RepositoryAction {
    #[default]
    None,
    Load,
    Setup,
    /// Something in the running setup agent's progress view.
    SetupRun(super::repository_setup::Action),
    /// A folder or fresh clone to make the repository.
    Adopt(PathBuf),
    /// Choose the folder clones go into.
    CloneFolder,
    /// The checkout of the named same-worker sibling.
    ChooseSibling(String),
}

impl HorizonApp {
    /// Closes the dialog. A setup of its repository goes on after it, so the setup
    /// board shows it with the others.
    pub(in crate::app) fn close_cloud_creation_to_board(&mut self, ctx: &Context) {
        let setup = super::repository_setup::running_here(&self.cloud_prototype.production);
        self.close_cloud_creation();
        if setup {
            self.open_setup_board(ctx, true);
        }
    }

    /// Draws the New cloud form on the workspace that holds the draft, or parks the draft.
    ///
    /// The form is a screen-space area of `Order::Middle`, not a modal: it has no backdrop, no
    /// top layer and no dismissal of its own, so a click on the board or Escape leaves it and its
    /// choices as they are. Its rect is the on-screen frame of the draft workspace, so a pan moves
    /// it with that workspace. While that workspace cannot show it, the draft waits in the
    /// workspace and its card is the way back.
    pub(in crate::app::cloud_panel) fn render_cloud_creation(&mut self, ctx: &Context) {
        if !self.cloud_prototype.production.creation_dialog_open() {
            return;
        }
        let Some(surface) = self.cloud_creation_surface(ctx) else {
            if Self::cloud_creation_surface_was_shown(ctx) {
                // The workspace that holds the form left the canvas, so the form cannot follow
                // it: the draft parks in the workspace, and its card brings the form back.
                self.hide_cloud_creation(ctx);
            } else {
                // The form opened on a workspace that is off the canvas, so that workspace comes
                // into view: the form opens on it and follows the pan.
                self.reveal_cloud_creation_workspace(ctx);
            }
            return;
        };
        // A setup agent of this repository shows its output; large surfaces give it room.
        let widest = if super::repository_setup::running_here(&self.cloud_prototype.production) {
            1480.0
        } else {
            1180.0
        };
        let (width, body_height, origin) =
            form_layout(surface, self.canvas_rect(ctx), body_ceiling(ctx.content_rect()), widest);
        let mut actions = Actions::default();
        let picking = self.dir_picker.is_some();
        let refocus_repository = self.prepare_cloud_creation(ctx, picking);
        self.sync_cloud_creation_choices(ctx);
        // A refused Start shows in the footer that holds the Start button. In the body it
        // landed under the fields, below the fold of a scrolling column, which reads as no
        // answer at all.
        let refused = self.cloud_prototype.error.clone();
        egui::Area::new(Id::unique("cloud-creation"))
            .order(Order::Middle)
            .fixed_pos(origin)
            .constrain(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(16)
                    .inner_margin(24)
                    .show(ui, |ui| {
                        ui.set_width(width);
                        if picking {
                            ui.disable();
                        }
                        ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                        // While a setup agent prepares the repository, nothing is rented yet: the
                        // form is a preparation view with its own heading and footer.
                        let preparing = super::repository_setup::running_here(&self.cloud_prototype.production);
                        if preparing {
                            super::repository_setup::heading(ui, &self.cloud_prototype.production);
                        } else {
                            heading(ui, &mut self.cloud_prototype.production);
                        }
                        ui.add_space(16.0);
                        self.cloud_creation_columns(ui, width, body_height, &mut actions, refocus_repository);
                        ui.add_space(12.0);
                        self.cloud_creation_footer(ui, &mut actions, preparing, refused.as_deref());
                        // The setup agent keeps an interrupt of its own, in its panel, and Close
                        // parks the draft: nothing here needs Escape.
                        if preparing && super::repository_setup::working_here(&self.cloud_prototype.production) {
                            ui.label(
                                RichText::new(
                                    "Close returns to the workspace. Show agent keeps Escape for an interrupt.",
                                )
                                .size(11.5)
                                .color(theme::FG_DIM()),
                            );
                        }
                    });
            });
        if actions.hide {
            self.hide_cloud_creation(ctx);
            return;
        }
        if actions.cancel {
            self.close_cloud_creation_to_board(ctx);
            return;
        }
        if let Some(target) = self.cloud_prototype.production.publishing.take_setup() {
            self.edit_creation_registry(ctx, (!target.is_empty()).then_some(target));
            return;
        }
        if actions.this_pc {
            self.open_repository_on_this_pc(ctx);
            return;
        }
        match actions.repository {
            RepositoryAction::Load => self.read_cloud_profiles(ctx),
            RepositoryAction::Setup => self.start_cloud_repository_setup(ctx),
            RepositoryAction::SetupRun(action) => {
                // The dialog acts on the run of the repository it shows.
                if let Some(run) = self.cloud_prototype.production.run_here_key() {
                    self.cloud_repository_setup_action(ctx, run, action);
                }
            }
            RepositoryAction::ChooseSibling(alias) => self.choose_cloud_sibling(ctx, alias),
            RepositoryAction::Adopt(path) => self.adopt_cloud_source(ctx, &path),
            RepositoryAction::CloneFolder => self.choose_clone_folder(ctx),
            RepositoryAction::None => {}
        }
        // The Start button and Enter in the title take the same path: with "Start new
        // cloud once available" checked for a sold-out worker, both arm the watch.
        if actions.create
            && !self.cloud_prototype.production.title.trim().is_empty()
            && self.keep_new_workspace_choice(horizon_core::cloud_panel::WorkspacePlacement::Cloud)
        {
            selector::summary::start(&mut self.cloud_prototype.production);
        }
        watch::poll(&mut self.cloud_prototype.production);
        self.poll_cloud_launch(ctx);
        self.poll_cloud_creation(ctx);
    }

    fn sync_cloud_creation_choices(&mut self, ctx: &Context) {
        let form = &mut self.cloud_prototype.production;
        publishing::update(form, self.cloud_prototype.root.as_deref(), ctx);
        form.launch.siblings.sync(
            ctx,
            &form.repository,
            form.launch.revision.as_deref(),
            form.profiles.as_ref(),
            &form.selected_profile,
        );
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
        // While a setup agent works, its live output takes the summary's place and half the width.
        let setup_output = super::repository_setup::running_here(&self.cloud_prototype.production);
        if width >= TWO_COLUMNS {
            let right = if setup_output {
                (width * 0.5).max(SUMMARY_WIDTH)
            } else {
                SUMMARY_WIDTH
            };
            let left = width - right - GUTTER;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GUTTER;
                column(ui, left, body_height, |ui| {
                    super::super::runtime::solid_scroll_area(ui)
                        .id_salt("cloud-creation-body")
                        .max_height(body_height)
                        .show(ui, |ui| {
                            self.cloud_creation_body(ui, actions, refocus_repository, setup_output);
                        });
                });
                column(ui, right, body_height, |ui| {
                    if setup_output {
                        self.show_cloud_setup_terminal(ui, body_height);
                        return;
                    }
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
                    self.cloud_creation_body(ui, actions, refocus_repository, false);
                    ui.add_space(12.0);
                    checks::with_summary(
                        ui,
                        &mut self.cloud_prototype.production,
                        self.cloud_prototype.root.as_deref(),
                    );
                });
        }
    }

    /// The dialog's footer: the setup agent's own controls, or the Start button with the reason
    /// a refused Start gave.
    fn cloud_creation_footer(&mut self, ui: &mut Ui, actions: &mut Actions, preparing: bool, refused: Option<&str>) {
        if preparing {
            match preparing_footer(ui, &self.cloud_prototype.production, refused) {
                super::repository_setup::Footer::None => {}
                // Close parks the draft, as Escape does: the setup agent works on either way.
                super::repository_setup::Footer::Close => actions.hide = true,
                super::repository_setup::Footer::Run(action) => {
                    actions.repository = RepositoryAction::SetupRun(action);
                }
            }
        } else {
            checks::footer(ui, &mut self.cloud_prototype.production, actions, refused);
        }
    }

    /// Ends the draft: a clone still running stops, a token or key that was typed but not
    /// used is forgotten, the saved choices of a parked draft go, and the workspace goes when
    /// it held nothing.
    pub(in crate::app) fn close_cloud_creation(&mut self) {
        // The record is what a later start would show as a card, and this draft is over.
        self.forget_cloud_draft_record();
        self.close_cloud_form();
    }

    /// Ends what the dialog holds and keeps the saved choices of its parked draft: a session
    /// switch saves the board that shows the card, and the record brings the card back with
    /// that session.
    pub(in crate::app) fn close_parked_cloud_draft(&mut self) {
        // The empty workspace stays with the card. Its Intent names the board that goes, so the
        // record restores it instead.
        self.cloud_prototype.production.new_workspace = None;
        self.close_cloud_form();
    }

    /// Ends what the dialog holds, and the workspace New workspace made for it when that
    /// workspace held nothing.
    fn close_cloud_form(&mut self) {
        // A workspace New workspace made for a cloud that never came goes with the dialog.
        self.discard_new_cloud_workspace();
        let form = &mut self.cloud_prototype.production;
        form.draft = None;
        form.creating = false;
        form.pending_creation = None;
        form.launch = super::launch::State::default();
        form.source = source::State::default();
        form.checks = checks::State::default();
        form.publishing = publishing::State::default();
        // A Start that waits for the Docker hosts' read goes with the dialog.
        form.setup_build_on.read_again();
    }

    /// What the dialog settles before it draws: the pickers that closed, the prices and the
    /// checks it keeps current, and a clone that finished. True when focus returns to the field.
    fn prepare_cloud_creation(&mut self, ctx: &Context, picking: bool) -> bool {
        // Focus returns to the field when its picker closes, whether or not a directory was chosen.
        let refocus_repository = !picking && std::mem::take(&mut self.cloud_prototype.production.picking_for_source);
        if !picking {
            self.cloud_prototype.production.launch.siblings.stop_browsing();
            self.cloud_prototype.production.source.take_choosing_parent();
        }
        self.request_cloud_prices(ctx);
        // Start needs the tailnet settings, and their picker now waits in More options.
        self.cloud_prototype.production.tailnets.poll(ctx);
        self.offer_setup_presets();
        let root = self.cloud_prototype.root.clone();
        self.cloud_prototype
            .production
            .setup_build_on
            .refresh(root.as_deref(), ctx);
        match self.cloud_prototype.production.setup_build_on.deferred() {
            super::repository_setup::Deferred::Waiting => {}
            super::repository_setup::Deferred::Start(submitted) => {
                self.start_submitted_setup(ctx, *submitted);
                // Another waiting Start goes on in the next frame.
                ctx.request_repaint();
            }
            super::repository_setup::Deferred::Gone(reason) => self.cloud_prototype.error = Some(reason),
        }
        checks::update(
            &mut self.cloud_prototype.production,
            self.cloud_prototype.root.as_deref(),
            ctx,
        );
        self.cloud_prototype
            .production
            .source
            .connect_github(self.cloud_prototype.root.as_deref());
        if let Some(path) = self.cloud_prototype.production.source.poll(ctx) {
            self.adopt_cloud_source(ctx, &path);
        }
        refocus_repository
    }

    /// Everything left of the summary: title, profile, worker, place and more options.
    /// `setup_output_beside` is true when a setup agent's output has its own column.
    fn cloud_creation_body(
        &mut self,
        ui: &mut Ui,
        actions: &mut Actions,
        refocus_repository: bool,
        setup_output_beside: bool,
    ) {
        // A read failure of the settings, and nothing else, is what a fixing agent starts from.
        let error = self.cloud_prototype.production.launch.read_error.clone();
        let form = &mut self.cloud_prototype.production;
        ui.add_enabled_ui(
            form.pending_creation.is_none() && !form.launch.submitted && form.launch.watch.is_none(),
            |ui| {
                // A setup run needs none of the cloud's fields yet: beside its own output, or
                // inline with the output below it in a narrow dialog.
                if setup_output_beside || super::repository_setup::running_here(form) {
                    if let super::repository_setup::Choice::Run(action) =
                        super::repository_setup::render(ui, form, setup_output_beside, None)
                    {
                        actions.repository = RepositoryAction::SetupRun(action);
                    }
                    return;
                }
                actions.this_pc = super::new_workspace::notes(ui, form);
                actions.repository = fields(ui, form, &mut actions.create, refocus_repository);
                if !form.launch.loading() && form.profiles.is_none() && !checks::source_step(form) {
                    match super::repository_setup::render(ui, form, setup_output_beside, error.as_deref()) {
                        super::repository_setup::Choice::None => {}
                        super::repository_setup::Choice::SetupAgent => actions.repository = RepositoryAction::Setup,
                        super::repository_setup::Choice::Run(action) => {
                            actions.repository = RepositoryAction::SetupRun(action);
                        }
                        super::repository_setup::Choice::QuickStart => {
                            super::repository_setup::set_configuration(form, Configuration::QuickStart);
                            actions.repository = RepositoryAction::Load;
                        }
                    }
                }
            },
        );
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

    /// The screen-space rect the form sits on: the on-screen frame of the workspace that holds
    /// the draft, so the form moves with a pan. `None` while that frame is off the canvas or the
    /// workspace is detached. A draft that no workspace holds sits on the canvas.
    fn cloud_creation_surface(&self, ctx: &Context) -> Option<Rect> {
        let canvas = self.canvas_rect(ctx);
        let Some(local) = self.cloud_prototype.production.creation_workspace() else {
            return Some(canvas);
        };
        let workspace = self.board.workspace_id_by_local_id(&local)?;
        if self.workspace_is_detached(workspace) {
            return None;
        }
        let (position, size) = self.workspace_focus_frame(workspace)?;
        let frame = canvas_scene_transform(canvas, self.canvas_view) * Rect::from_min_size(position, size);
        // The frame starts above the workspace's name badge, and the form must not cover the
        // workspace it sits on, so the form's part starts below that badge.
        let surface = Rect::from_min_max(
            Pos2::new(
                frame.min.x,
                frame.min.y + crate::app::WS_TITLE_HEIGHT + crate::app::WS_BG_PAD,
            ),
            frame.max,
        );
        surface.intersects(canvas).then_some(surface)
    }

    /// Whether the form drew itself in the previous frame. A form that did and can no longer sit
    /// on its workspace is parked; one that did not is still opening, and its workspace comes
    /// into view instead.
    fn cloud_creation_surface_was_shown(ctx: &Context) -> bool {
        let layer = egui::LayerId::new(Order::Middle, Id::unique("cloud-creation"));
        ctx.memory(|memory| memory.areas().visible_last_frame(&layer))
    }

    /// Brings the workspace that holds the draft into view, so the form opens on it.
    fn reveal_cloud_creation_workspace(&mut self, ctx: &Context) {
        let Some(workspace) = self
            .cloud_prototype
            .production
            .creation_workspace()
            .as_deref()
            .and_then(|local| self.board.workspace_id_by_local_id(local))
        else {
            return;
        };
        self.focus_workspace_visible(ctx, workspace, false);
    }

    /// Makes `path` the repository and reads its profiles: a folder that was typed, or a fresh clone.
    fn adopt_cloud_source(&mut self, ctx: &Context, path: &Path) {
        self.set_cloud_repository(path);
        self.read_cloud_profiles(ctx);
    }

    /// Lets the person pick the folder clones go into, starting from the nearest one that exists.
    fn choose_clone_folder(&mut self, ctx: &Context) {
        self.consume_picker_activation(ctx);
        self.cloud_prototype.production.picking_for_source = true;
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
            // A read of the previous repository, quiet or not, must not bring it back.
            form.launch.switch_repository();
            // Quick start is chosen for one repository; another one reads its own settings.
            if form.launch.configuration == Configuration::QuickStart {
                form.launch.configuration = Configuration::WorkingCopy;
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

/// The title, and one chip per provider the dialog lists: its mark and its name. A click
/// moves this cloud to that provider; a provider this machine is not ready for says below
/// the title what it needs.
fn heading(ui: &mut Ui, form: &mut Production) {
    let entries = provider::entries(form);
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("New cloud").size(26.0).strong().color(theme::FG()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for entry in entries.iter().rev() {
                if provider::choice(ui, entry) {
                    chosen = Some(entry.provider);
                }
            }
        });
    });
    ui.label(
        RichText::new("A remote workspace for your repository and agents.")
            .size(14.0)
            .color(theme::FG_SOFT()),
    );
    // What a selected provider needs is drawn under the title, once that provider is the
    // cloud's own, rather than shown on hover:
    if let Some(reason) = entries
        .iter()
        .find(|entry| entry.selected)
        .and_then(|entry| entry.unavailable.as_deref())
    {
        ui.label(RichText::new(reason).size(13.0).color(theme::PALETTE_YELLOW()));
    }
    // Before the repository's profiles are read there is no cloud to move to a provider.
    if let Some(provider) = chosen.filter(|_| form.profiles.is_some()) {
        provider::choose(form, provider);
    }
    provider::settle(form);
}

fn field(ui: &mut Ui, label: &str, id: &str, value: &mut String, hint: &str) -> egui::Response {
    ui.label(RichText::new(label).size(14.0).strong().color(theme::FG()));
    ui.add_sized(
        [ui.available_width(), 38.0],
        TextEdit::singleline(value)
            .id(Id::unique(id))
            .font(egui::FontId::proportional(15.0))
            .margin(Vec2::new(12.0, 10.0))
            .hint_text(hint),
    )
}

/// The cloud title, focused when the form opens; Enter submits a complete form.
fn title_field(ui: &mut Ui, form: &mut Production, submit: &mut bool) {
    if form.focus_title_on_open
        && ui.is_enabled()
        && !ui.input(|input| input.pointer.any_down() || input.pointer.any_released())
    {
        ui.memory_mut(|memory| memory.request_focus(Id::unique("cloud-title")));
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
    // The hint sits with the field it asks for, not in the footer.
    if form.title.trim().is_empty() && !checks::source_step(form) {
        ui.label(
            RichText::new("Name the cloud to start it.")
                .size(12.5)
                .color(theme::FG_SOFT()),
        );
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
        profiles::notes(ui, form);
        if uncommitted::section(ui, form) {
            action = RepositoryAction::Load;
        }
        match super::repository_setup::quick_start_note(ui, form) {
            super::repository_setup::Note::None => {}
            super::repository_setup::Note::UpdateFromCi => action = RepositoryAction::Setup,
            super::repository_setup::Note::LeaveQuickStart => {
                super::repository_setup::set_configuration(form, Configuration::WorkingCopy);
                action = RepositoryAction::Load;
            }
        }
        ui.add_space(12.0);
        machine(ui, form)
    } else {
        // A read that failed for a reason outside the file, such as a Git lock, is retried
        // here: the checkout may not change again to start one.
        let failed =
            !form.launch.unconfigured && form.launch.read_error.is_none() && !form.repository.trim().is_empty();
        if failed && ui.button("Try again").clicked() {
            action = RepositoryAction::Load;
        }
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
            // A tailnet is chosen rarely, so it waits here instead of above the machine.
            if form.profiles.is_some() && !form.launch.loading() {
                form.tailnets.choice(ui, &mut form.tailnet);
                ui.add_space(8.0);
            }
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
        // A chosen Docker host, or a provider whose VM sizes and regions are a settings
        // section this machine has not read, stays chosen while no choice lists it: Start
        // then refuses the cloud and says what to add, and the person picks another
        // worker. Resetting would let New cloud fall back to another provider.
        if form.provider.is_some_and(|chosen| {
            !provider::keeps_its_choice(chosen, profile) && !provider::choices(&form.prices, profile).contains(&chosen)
        }) {
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
    ui.add_space(16.0);
    let changed = field(
        ui,
        "Committed base revision",
        "cloud-revision",
        &mut form.revision,
        "HEAD",
    )
    .changed();
    ui.label(RichText::new(REVISION_NOTE).size(12.0).color(theme::FG_SOFT()));
    if changed {
        RepositoryAction::Load
    } else {
        RepositoryAction::None
    }
}

/// What the revision decides, and what the working copy still decides on the checked-out commit.
const REVISION_NOTE: &str = "Only committed files are transferred. On the checked-out commit, the settings come from the working copy of .horizon/cloud.yml and update as you save it, and the image builds with the uncommitted files of its recipe, listed under the profile. Another revision uses that commit alone.";

/// The footer of a dialog that prepares its repository: the setup agent's controls, with the
/// reason a refused Start gave above them. That view replaces the Start button, so the reason
/// must not go with it.
pub(super) fn preparing_footer(
    ui: &mut Ui,
    form: &Production,
    refused: Option<&str>,
) -> super::repository_setup::Footer {
    checks::refusal(ui, refused);
    super::repository_setup::footer(ui, form)
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
        // A missing title is asked for under its field.
        (true, false) => None,
        (_, true) => Some("The repository has no usable cloud settings yet."),
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
