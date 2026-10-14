//! Cloud-only named networks. Settings never reads a saved key back into the form.
mod api_credentials;
pub(in crate::app) mod requests;
pub(in crate::app) use api_credentials::Observation as ApiCredentialObservation;

use crate::theme;
use horizon_core::cloud_runtime::tailnet::{self, Catalog, Selection};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(PartialEq, Eq)]
struct DiscoveryVersion {
    catalog: u64,
    api: u64,
    pending: bool,
    failed: bool,
}

struct SettingsDiscovery {
    version: DiscoveryVersion,
    catalog: Option<Catalog>,
    observations: BTreeMap<String, ApiCredentialObservation>,
    fallback: ApiCredentialObservation,
}

pub(super) struct State {
    root: PathBuf,
    catalog: Catalog,
    catalog_loaded: bool,
    catalog_revision: u64,
    settings_discovery: Option<SettingsDiscovery>,
    selections: BTreeMap<String, Option<String>>,
    receiver: Option<Receiver<Result<Catalog, String>>>,
    initialized: bool,
    reload_barriers: Vec<Receiver<Result<Catalog, String>>>,
    edit: Option<String>,
    name: String,
    key: Zeroizing<String>,
    message: Option<String>,
    settings_error: Option<String>,
    failed: bool,
    api_credentials: api_credentials::State,
}
impl Default for State {
    fn default() -> Self {
        Self {
            root: horizon_core::HorizonHome::resolve().root().join("cloud"),
            catalog: Catalog::default(),
            catalog_loaded: false,
            catalog_revision: 0,
            settings_discovery: None,
            selections: BTreeMap::new(),
            receiver: None,
            initialized: false,
            reload_barriers: Vec::new(),
            edit: None,
            name: String::new(),
            key: Zeroizing::new(String::new()),
            message: None,
            settings_error: None,
            failed: false,
            api_credentials: api_credentials::State::default(),
        }
    }
}
impl State {
    pub(in crate::app) fn api_credential_observation(&self, id: &str) -> ApiCredentialObservation {
        if let Some(discovery) = &self.settings_discovery {
            return discovery.observations.get(id).copied().unwrap_or(discovery.fallback);
        }
        if self.api_credentials.row_error(id).is_some() {
            return ApiCredentialObservation::Unavailable;
        }
        if !self.initialized || self.receiver.is_some() || !self.reload_barriers.is_empty() {
            ApiCredentialObservation::Pending
        } else {
            self.api_credentials.observation(id)
        }
    }

    pub(in crate::app) fn cached_catalog_at(&self, root: &Path) -> Option<&Catalog> {
        if self.root != root {
            return None;
        }
        self.settings_discovery
            .as_ref()
            .and_then(|discovery| discovery.catalog.as_ref())
            .or_else(|| self.catalog_loaded.then_some(&self.catalog))
    }

    pub(in crate::app) fn observe_settings(&mut self, editor: &mut Self, ctx: &egui::Context) {
        if self.root != editor.root {
            self.settings_discovery = None;
            return;
        }
        editor.poll(ctx);
        let pending = !editor.initialized
            || editor.receiver.is_some()
            || !editor.reload_barriers.is_empty()
            || editor.api_credentials.busy();
        let version = DiscoveryVersion {
            catalog: editor.catalog_revision,
            api: editor.api_credentials.revision(),
            pending,
            failed: editor.failed,
        };
        if self
            .settings_discovery
            .as_ref()
            .is_some_and(|discovery| discovery.version == version)
        {
            return;
        }
        let catalog = editor.cached_catalog_at(&editor.root).cloned();
        let observations = catalog.as_ref().map_or_else(BTreeMap::new, |catalog| {
            catalog
                .tailnets
                .iter()
                .map(|network| (network.id.clone(), editor.api_credential_observation(&network.id)))
                .collect()
        });
        self.settings_discovery = Some(SettingsDiscovery {
            version,
            catalog,
            observations,
            fallback: if pending {
                ApiCredentialObservation::Pending
            } else {
                ApiCredentialObservation::Unavailable
            },
        });
    }

    fn start(&mut self, ctx: &egui::Context, task: impl FnOnce() -> Result<Catalog, String> + Send + 'static) {
        let (sender, receiver) = channel();
        self.receiver = Some(receiver);
        self.message = None;
        self.failed = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(task());
            ctx.request_repaint();
        });
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        self.api_credentials.poll(ctx);
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(catalog) => {
                            self.catalog = catalog;
                            self.catalog_loaded = true;
                            self.catalog_revision = self.catalog_revision.wrapping_add(1);
                            self.api_credentials.refresh(ctx, &self.root, &self.catalog);
                            if self
                                .edit
                                .as_ref()
                                .is_some_and(|id| !self.catalog.tailnets.iter().any(|network| &network.id == id))
                            {
                                self.edit = None;
                                self.name.clear();
                                self.key.zeroize();
                            }
                            self.message = None;
                        }
                        Err(error) => {
                            self.message = Some(error);
                            self.failed = true;
                            self.selections.clear();
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.failed = true;
                    self.message = Some("Operation interrupted. Try again.".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        self.reload_barriers.retain(|receiver| match receiver.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Ok(Ok(_)) => false,
            result => {
                self.settings_error = Some(match result {
                    Ok(Err(error)) => error,
                    _ => "Tailnet settings operation interrupted. Try again.".into(),
                });
                false
            }
        });
        if !self.reload_barriers.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !self.initialized
            && self.receiver.is_none()
            && self.reload_barriers.is_empty()
            && !self.api_credentials.busy()
        {
            self.initialized = true;
            self.refresh(ctx);
        }
    }
    pub(super) fn refresh(&mut self, ctx: &egui::Context) {
        if self.receiver.is_some() || !self.reload_barriers.is_empty() {
            self.initialized = false;
            return;
        }
        let root = self.root.clone();
        self.start(ctx, move || {
            let store = tailnet::store(&root);
            store.recover().and_then(|()| store.load()).map_err(|e| e.to_string())
        });
    }
    pub(super) fn settings(&mut self, ui: &mut egui::Ui, requests: &mut requests::Requests) {
        self.poll(ui.ctx());
        ui.label(egui::RichText::new("Tailnets").size(24.0).strong().color(theme::FG()));
        ui.label(egui::RichText::new("Private networks for your clouds").color(theme::FG_SOFT()));
        ui.add_space(20.0);
        let networks = self
            .cached_catalog_at(&self.root)
            .map_or_else(Vec::new, |catalog| catalog.tailnets.clone());
        ui.add_enabled_ui(self.receiver.is_none() && !self.api_credentials.busy(), |ui| {
            for network in networks {
                egui::Frame::new()
                    .fill(theme::PANEL_BG())
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
                    .corner_radius(10)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2((ui.available_width() - 180.0).max(60.0), 24.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&network.name).strong().color(theme::FG()),
                                        )
                                        .truncate(),
                                    );
                                },
                            )
                            .response
                            .on_hover_text(&network.name);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Remove").clicked() {
                                    let root = self.root.clone();
                                    let id = network.id.clone();
                                    self.start(ui.ctx(), move || {
                                        tailnet::store(&root).delete(&id).map_err(|e| e.to_string())
                                    });
                                }
                                if ui.button("Replace key").clicked() {
                                    self.edit = Some(network.id.clone());
                                    self.name.clone_from(&network.name);
                                    self.key.zeroize();
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Auth key protected in OS keychain")
                                .size(12.0)
                                .color(theme::FG_SOFT()),
                        );
                        let observation = self.api_credential_observation(&network.id);
                        self.api_credentials.row(ui, &network.id, observation);
                        self.api_credentials.manage(
                            ui,
                            &requests::Target {
                                root: self.root.clone(),
                                id: network.id.clone(),
                                name: network.name.clone(),
                            },
                            requests,
                        );
                    });
                ui.add_space(8.0);
            }
            if self.catalog.tailnets.is_empty() {
                ui.label("Add a tailnet, then choose it when starting a cloud.");
                ui.add_space(12.0);
            }
            ui.scope(|ui| {
                let widgets = &mut ui.visuals_mut().widgets;
                for visual in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
                    visual.corner_radius = egui::CornerRadius::same(8);
                }
                self.form(ui);
            });
        });
        self.status(ui);
        self.api_credentials.status(ui);
    }
    fn form(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add_space(20.0);
        ui.label(
            egui::RichText::new(if self.edit.is_some() {
                "Replace auth key"
            } else {
                "Add tailnet"
            })
            .size(18.0)
            .strong()
            .color(theme::FG()),
        );
        ui.add_space(18.0);
        ui.label(egui::RichText::new("Name").size(13.0).strong());
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.name)
                .hint_text("Work network")
                .desired_width(f32::INFINITY)
                .font(egui::FontId::proportional(14.0))
                .margin(egui::Margin::symmetric(12, 12))
                .background_color(theme::PANEL_BG_ALT()),
        );
        ui.add_space(18.0);
        ui.label(egui::RichText::new("Auth key").size(13.0).strong());
        ui.add_space(4.0);
        let password = ui.add(
            egui::TextEdit::singleline(&mut *self.key)
                .password(true)
                .hint_text("tskey-auth-…")
                .desired_width(f32::INFINITY)
                .font(egui::FontId::proportional(14.0))
                .margin(egui::Margin::symmetric(12, 12))
                .background_color(theme::PANEL_BG_ALT()),
        );
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), password.id) {
            state.clear_undoer();
            state.store(ui.ctx(), password.id);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Use a preauthorized, non-ephemeral auth key. A reusable key can enroll multiple clouds.",
            )
            .size(12.0)
            .color(theme::FG_SOFT()),
        );
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let valid = !self.name.trim().is_empty() && tailnet::valid_key(self.key.trim());
            if ui
                .add_enabled(
                    valid,
                    egui::Button::new(egui::RichText::new("Save tailnet").strong()).min_size(egui::vec2(112.0, 36.0)),
                )
                .clicked()
            {
                let root = self.root.clone();
                let id = self.edit.take();
                let name = std::mem::take(&mut self.name);
                let key = std::mem::replace(&mut self.key, Zeroizing::new(String::new()));
                self.start(ui.ctx(), move || {
                    tailnet::store(&root)
                        .save(id.as_deref(), &name, key.trim())
                        .map_err(|e| e.to_string())
                });
            }
            if self.edit.is_some()
                && ui
                    .add(egui::Button::new("Cancel").min_size(egui::vec2(80.0, 36.0)))
                    .clicked()
            {
                self.edit = None;
                self.name.clear();
                self.key.zeroize();
            }
            if ui
                .add(egui::Button::new("Refresh").min_size(egui::vec2(80.0, 36.0)))
                .clicked()
            {
                self.refresh(ui.ctx());
            }
        });
    }
    pub(super) fn ready(&self) -> bool {
        self.initialized && self.receiver.is_none() && self.reload_barriers.is_empty() && !self.failed
    }
    #[cfg(test)]
    pub(in crate::app) fn for_root_fixture(root: PathBuf) -> Self {
        Self {
            root,
            ..Self::default()
        }
    }
    #[cfg(test)]
    pub(in crate::app) fn api_reply_fixture(
        root: PathBuf,
        catalog: Catalog,
        receiver: Receiver<api_credentials::Reply>,
    ) -> Self {
        let ids = catalog.tailnets.iter().map(|network| network.id.clone()).collect();
        Self {
            root,
            catalog,
            initialized: true,
            catalog_loaded: true,
            api_credentials: api_credentials::State::reply_fixture(ids, receiver),
            ..Self::default()
        }
    }
    #[cfg(all(test, unix))]
    pub(super) fn loaded_fixture() -> Self {
        Self {
            initialized: true,
            catalog_loaded: true,
            ..Self::default()
        }
    }
    fn status(&self, ui: &mut egui::Ui) {
        if let Some(error) = self.api_credentials.write_error() {
            ui.colored_label(theme::PALETTE_RED(), error);
        }
        if let Some(error) = self.api_credentials.read_error() {
            ui.colored_label(theme::PALETTE_RED(), error);
        }
        if let Some(error) = &self.settings_error {
            ui.colored_label(theme::PALETTE_RED(), error);
        }
        if self.receiver.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Saving tailnet settings…");
            });
        }
        if let Some(message) = &self.message
            && self.settings_error.as_ref() != Some(message)
        {
            ui.colored_label(
                if self.failed {
                    theme::PALETTE_RED()
                } else {
                    theme::FG_SOFT()
                },
                message,
            );
        }
    }
    pub(super) fn choice(&mut self, ui: &mut egui::Ui, selected: &mut Option<String>) -> bool {
        self.poll(ui.ctx());
        let before = selected.clone();
        self.status(ui);
        ui.label(egui::RichText::new("Tailnet").size(14.0).strong().color(theme::FG()));
        ui.add_enabled_ui(self.ready(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(egui::Button::selectable(selected.is_none(), "None").min_size(egui::vec2(64.0, 32.0)))
                    .clicked()
                {
                    *selected = None;
                }
                for tailnet in &self.catalog.tailnets {
                    if ui
                        .add(
                            egui::Button::selectable(selected.as_ref() == Some(&tailnet.id), &tailnet.name)
                                .min_size(egui::vec2(96.0, 32.0)),
                        )
                        .clicked()
                    {
                        *selected = Some(tailnet.id.clone());
                    }
                }
            })
        });
        if selected
            .as_ref()
            .is_some_and(|id| !self.catalog.tailnets.iter().any(|t| &t.id == id))
        {
            ui.colored_label(theme::PALETTE_RED(), "Selected tailnet is unavailable.");
        }
        ui.label(
            egui::RichText::new("Choose once, when provisioning. Manage saved networks in Settings → Tailnets.")
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
        before != *selected
    }
    pub(super) fn reload(&mut self) {
        self.initialized = false;
        self.selections.clear();
    }
    pub(super) fn reload_after(&mut self, mut editor: Self) {
        self.settings_discovery = None;
        if self.root != editor.root {
            return;
        }
        let catalog = editor
            .cached_catalog_at(&self.root)
            .cloned()
            .or_else(|| self.catalog_loaded.then(|| self.catalog.clone()));
        self.catalog_loaded = catalog.is_some();
        self.catalog = catalog.unwrap_or_default();
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        self.reload();
        self.settings_error = if editor.failed { editor.message.take() } else { None };
        // Every pre-close reader is a barrier, never a current catalog answer.
        // The latest same-root Settings catalog remains visible while they settle.
        if let Some(receiver) = self.receiver.take() {
            self.reload_barriers.push(receiver);
        }
        if let Some(receiver) = editor.receiver.take() {
            self.reload_barriers.push(receiver);
        }
        self.reload_barriers.append(&mut editor.reload_barriers);
        self.api_credentials
            .reload_after(editor.api_credentials, self.catalog_loaded.then_some(&self.catalog));
    }
    pub(super) fn invalidate(&mut self, id: &str) {
        self.selections.remove(id);
    }
    pub(super) fn cloud(&mut self, ui: &mut egui::Ui, root: &std::path::Path, id: &str, busy: bool) {
        self.poll(ui.ctx());
        if !self.selections.contains_key(id) {
            let cloud = root.join(id);
            // Loaded once per opened card, never from a per-panel redraw loop.
            match Selection::load(&cloud) {
                Ok(selection) => {
                    self.selections.insert(id.into(), selection.tailnet);
                }
                Err(error) => {
                    ui.colored_label(theme::PALETTE_RED(), error.to_string());
                    return;
                }
            }
        }
        let mut selected = self.selections.get(id).cloned().flatten();
        if busy {
            let name = selected.as_ref().map_or("None", |id| {
                self.catalog
                    .tailnets
                    .iter()
                    .find(|t| &t.id == id)
                    .map_or("Unavailable tailnet", |t| t.name.as_str())
            });
            ui.label(egui::RichText::new("Tailnet").size(12.0).color(theme::FG_DIM()));
            ui.label(egui::RichText::new(name).size(16.0).strong().color(theme::FG()));
            ui.label(
                egui::RichText::new("Selected at provisioning")
                    .size(12.0)
                    .color(theme::FG_SOFT()),
            );
            self.status(ui);
            return;
        }
        let changed = ui
            .add_enabled_ui(!busy && self.receiver.is_none(), |ui| self.choice(ui, &mut selected))
            .inner;
        if changed {
            let (root, id) = (root.to_path_buf(), id.to_owned());
            self.selections.insert(id.clone(), selected.clone());
            self.start(ui.ctx(), move || {
                tailnet::change(&root, &id, selected.as_deref()).map_err(|e| e.to_string())?;
                tailnet::store(&root).load().map_err(|e| e.to_string())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_discovery_keeps_the_last_catalog_scoped_to_its_storage_root() {
        let mut state = State::for_root_fixture(PathBuf::from("synthetic-cloud-root"));
        assert!(state.cached_catalog_at(&state.root).is_none());
        state.catalog_loaded = true;
        assert!(state.cached_catalog_at(&state.root).is_some());
        assert!(state.cached_catalog_at(Path::new("another-cloud-root")).is_none());
        assert_eq!(
            state.api_credential_observation("work"),
            ApiCredentialObservation::Pending
        );
        state.initialized = true;
        state.failed = true;
        assert!(state.cached_catalog_at(&state.root).is_some());
        assert_eq!(
            state.api_credential_observation("work"),
            ApiCredentialObservation::Unavailable
        );
        let (_sender, receiver) = channel();
        state.receiver = Some(receiver);
        assert_eq!(
            state.api_credential_observation("work"),
            ApiCredentialObservation::Pending
        );
    }

    #[test]
    fn reload_waits_for_the_current_operation_then_loads_fresh_metadata() {
        for result in [Ok(Catalog::default()), Err("Synthetic operation failure".into())] {
            let root = tempfile::tempdir().unwrap();
            let (saved, settings_receiver) = channel();
            let editor = State {
                root: root.path().into(),
                receiver: Some(settings_receiver),
                ..State::default()
            };
            let (sender, receiver) = channel();
            let mut state = State {
                root: root.path().into(),
                initialized: true,
                receiver: Some(receiver),
                ..State::default()
            };
            let ctx = egui::Context::default();
            state.reload_after(editor);
            state.poll(&ctx);
            assert!(!state.initialized);
            assert!(state.receiver.is_none());
            assert_eq!(state.reload_barriers.len(), 2);
            sender.send(result).unwrap();
            state.poll(&ctx);
            assert!(!state.initialized);
            assert!(state.receiver.is_none());
            std::fs::write(
                root.path().join("tailnets.json"),
                r#"{"tailnets":[{"id":"fresh","name":"New network"}]}"#,
            )
            .unwrap();
            saved.send(Ok(Catalog::default())).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !state.initialized {
                assert!(std::time::Instant::now() < deadline);
                state.poll(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(state.receiver.is_some());
            while state.receiver.is_some() || state.api_credentials.busy() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
                state.poll(&ctx);
            }
            assert!(state.receiver.is_none());
            assert_eq!(state.catalog.tailnets.len(), 1);
            assert_eq!(state.catalog.tailnets[0].id, "fresh");
        }
    }

    #[test]
    fn closed_settings_failures_remain_visible_after_catalog_reload() {
        assert!(!State::default().ready());
        let mut completed = State::default();
        completed.reload_after(State {
            failed: true,
            message: Some("Completed save failure".into()),
            ..State::default()
        });
        assert_eq!(completed.settings_error.as_deref(), Some("Completed save failure"));
        for disconnected in [false, true] {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("tailnets.json"), r#"{"tailnets":[]}"#).unwrap();
            let (sender, receiver) = channel();
            let mut state = State {
                root: root.path().into(),
                initialized: true,
                ..State::default()
            };
            state.reload_after(State {
                root: root.path().into(),
                receiver: Some(receiver),
                ..State::default()
            });
            assert!(!state.ready());
            if !disconnected {
                sender.send(Err("Synthetic save failure".into())).unwrap();
            }
            drop(sender);
            let ctx = egui::Context::default();
            state.poll(&ctx);
            assert!(state.settings_error.is_some());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while state.receiver.is_some() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
                state.poll(&ctx);
            }
            assert!(state.ready());
            assert!(state.settings_error.is_some());
        }
    }

    #[test]
    fn a_closed_api_reader_recovers_without_overwriting_save_or_catalog_errors() {
        for unrelated in [None, Some("Synthetic save failure"), Some("Synthetic catalog failure")] {
            for disconnected in [false, true] {
                let root = tempfile::tempdir().unwrap();
                std::fs::write(root.path().join("tailnets.json"), r#"{"tailnets":[]}"#).unwrap();
                let (sender, receiver) = channel();
                let mut editor = State::api_reply_fixture(root.path().into(), Catalog::default(), receiver);
                editor.failed = unrelated.is_some();
                editor.message = unrelated.map(String::from);
                let mut state = State::for_root_fixture(root.path().into());
                state.reload_after(editor);
                if !disconnected {
                    sender.send(Err("Synthetic API read failure".into())).unwrap();
                }
                drop(sender);
                let ctx = egui::Context::default();
                state.poll(&ctx);
                assert!(state.api_credentials.read_error().is_some());
                assert_eq!(state.settings_error.as_deref(), unrelated);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while (state.receiver.is_some() || state.api_credentials.busy()) && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    state.poll(&ctx);
                }
                assert!(state.ready());
                assert!(!state.api_credentials.busy());
                assert!(state.api_credentials.read_error().is_none());
                assert_eq!(state.settings_error.as_deref(), unrelated);
            }
        }
    }

    #[test]
    fn a_known_row_failure_stays_unavailable_during_close_and_catalog_reload() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog {
            tailnets: ["locked", "healthy"]
                .into_iter()
                .map(|id| tailnet::Tailnet {
                    id: id.into(),
                    name: format!("Synthetic {id}"),
                })
                .collect(),
        };
        let (sender, receiver) = channel();
        let mut editor = State::api_reply_fixture(root.path().into(), catalog, receiver);
        sender
            .send(Ok(vec![
                ("locked".into(), Err("Synthetic unavailable store".into())),
                ("healthy".into(), Ok(tailnet::ApiCredentialStatus::Missing)),
            ]))
            .unwrap();
        let ctx = egui::Context::default();
        let mut production = State::for_root_fixture(root.path().into());
        production.observe_settings(&mut editor, &ctx);
        assert_eq!(
            production.api_credential_observation("locked"),
            ApiCredentialObservation::Unavailable
        );
        production.reload_after(editor);
        assert_eq!(production.cached_catalog_at(root.path()).unwrap().tailnets.len(), 2);
        assert_eq!(
            production.api_credential_observation("locked"),
            ApiCredentialObservation::Unavailable
        );
        assert_eq!(
            production.api_credential_observation("healthy"),
            ApiCredentialObservation::Pending
        );
        assert_eq!(
            production.api_credentials.row_error("locked"),
            Some("Synthetic unavailable store")
        );
    }

    #[test]
    fn an_auth_key_or_catalog_failure_does_not_replace_a_successful_api_observation() {
        for message in ["Synthetic auth-key save failure", "Synthetic catalog refresh failure"] {
            let root = tempfile::tempdir().unwrap();
            let catalog = Catalog {
                tailnets: vec![tailnet::Tailnet {
                    id: "saved".into(),
                    name: "Synthetic saved network".into(),
                }],
            };
            let (sender, receiver) = channel();
            let mut editor = State::api_reply_fixture(root.path().into(), catalog, receiver);
            sender
                .send(Ok(vec![(
                    "saved".into(),
                    Ok(tailnet::ApiCredentialStatus::Stored(tailnet::ApiCredentialKind::OAuth)),
                )]))
                .unwrap();
            let ctx = egui::Context::default();
            editor.poll(&ctx);
            editor.failed = true;
            editor.message = Some(message.into());
            let expected = ApiCredentialObservation::Ready(tailnet::ApiCredentialStatus::Stored(
                tailnet::ApiCredentialKind::OAuth,
            ));
            assert_eq!(editor.api_credential_observation("saved"), expected);
            let mut production = State::for_root_fixture(root.path().into());
            production.observe_settings(&mut editor, &ctx);
            assert_eq!(production.api_credential_observation("saved"), expected);
            assert_eq!(editor.message.as_deref(), Some(message));
            let (_pending, receiver) = channel();
            editor.receiver = Some(receiver);
            assert_eq!(
                editor.api_credential_observation("saved"),
                ApiCredentialObservation::Pending
            );
        }
    }

    #[test]
    fn closing_before_settings_catalog_load_preserves_known_catalog_and_failure() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog {
            tailnets: vec![tailnet::Tailnet {
                id: "locked".into(),
                name: "Synthetic locked network".into(),
            }],
        };
        let (sender, receiver) = channel();
        let mut production = State::api_reply_fixture(root.path().into(), catalog, receiver);
        sender
            .send(Ok(vec![("locked".into(), Err("Known unavailable storage".into()))]))
            .unwrap();
        production.poll(&egui::Context::default());
        production.reload_after(State::for_root_fixture(root.path().into()));
        assert_eq!(production.cached_catalog_at(root.path()).unwrap().tailnets.len(), 1);
        assert_eq!(
            production.api_credential_observation("locked"),
            ApiCredentialObservation::Unavailable
        );
        let (sender, receiver) = channel();
        let mut empty_editor = State::api_reply_fixture(root.path().into(), Catalog::default(), receiver);
        sender.send(Ok(vec![])).unwrap();
        empty_editor.poll(&egui::Context::default());
        production.reload_after(empty_editor);
        assert!(production.cached_catalog_at(root.path()).unwrap().tailnets.is_empty());
        assert!(production.api_credentials.row_error("locked").is_none());
    }

    #[test]
    fn removing_the_edited_binding_clears_the_form_only_after_success() {
        for success in [false, true] {
            let (sender, receiver) = channel();
            let mut state = State {
                initialized: true,
                edit: Some("removed".into()),
                name: "Work network".into(),
                key: Zeroizing::new("synthetic-input".into()),
                receiver: Some(receiver),
                ..State::default()
            };
            sender
                .send(if success {
                    Ok(Catalog::default())
                } else {
                    Err("Synthetic deletion failure".into())
                })
                .unwrap();
            state.poll(&egui::Context::default());
            assert_eq!(state.edit.is_none(), success);
            assert_eq!(state.name.is_empty(), success);
            assert_eq!(state.key.is_empty(), success);
        }
    }

    #[test]
    fn removed_catalog_rows_never_return_on_close_or_from_a_pre_close_reader() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        std::fs::write(root.join("tailnets.json"), br#"{"tailnets":[]}"#).unwrap();
        let previous = Catalog {
            tailnets: vec![tailnet::Tailnet {
                id: "removed".into(),
                name: "Removed".into(),
            }],
        };
        let (api_sender, api_receiver) = channel();
        let mut editor = State::api_reply_fixture(root.clone(), previous.clone(), api_receiver);
        let (saved, saved_receiver) = channel();
        editor.receiver = Some(saved_receiver);
        let (stale, stale_receiver) = channel();
        let mut production = State {
            root: root.clone(),
            catalog: previous.clone(),
            catalog_loaded: true,
            initialized: true,
            receiver: Some(stale_receiver),
            ..State::default()
        };
        let ctx = egui::Context::default();
        saved.send(Ok(Catalog::default())).unwrap();
        production.observe_settings(&mut editor, &ctx);
        assert!(production.cached_catalog_at(&root).unwrap().tailnets.is_empty());
        production.reload_after(editor);
        assert!(production.cached_catalog_at(&root).unwrap().tailnets.is_empty());
        assert!(production.cached_catalog_at(Path::new("another-root")).is_none());
        stale.send(Ok(previous)).unwrap();
        production.poll(&ctx);
        assert!(
            production.receiver.is_none(),
            "Fresh reads wait for the closed API reader"
        );
        assert!(production.cached_catalog_at(&root).unwrap().tailnets.is_empty());
        api_sender
            .send(Ok(vec![("removed".into(), Ok(tailnet::ApiCredentialStatus::Missing))]))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !production.ready() || production.api_credentials.busy() {
            assert!(std::time::Instant::now() < deadline);
            production.poll(&ctx);
            assert!(production.cached_catalog_at(&root).unwrap().tailnets.is_empty());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            production.api_credential_observation("removed"),
            ApiCredentialObservation::Unavailable
        );
    }

    #[test]
    fn closing_another_roots_editor_cannot_adopt_its_catalog_or_barriers() {
        let original = PathBuf::from("original-root");
        let foreign = PathBuf::from("foreign-root");
        let current = Catalog {
            tailnets: vec![tailnet::Tailnet {
                id: "original".into(),
                name: "Original".into(),
            }],
        };
        let (_sender, receiver) = channel();
        let mut production = State {
            root: original.clone(),
            catalog: current,
            catalog_loaded: true,
            initialized: true,
            receiver: Some(receiver),
            ..State::default()
        };
        let (_foreign_sender, foreign_receiver) = channel();
        production.reload_after(State {
            root: foreign.clone(),
            catalog: Catalog::default(),
            catalog_loaded: true,
            receiver: Some(foreign_receiver),
            ..State::default()
        });
        assert_eq!(
            production.cached_catalog_at(&original).unwrap().tailnets[0].id,
            "original"
        );
        assert!(production.cached_catalog_at(&foreign).is_none());
        assert!(production.receiver.is_some());
        assert!(production.reload_barriers.is_empty());
        assert!(production.initialized);
    }

    #[test]
    fn settings_close_transfers_api_barriers_before_fresh_catalog_read() {
        for failed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            std::fs::write(root.join("tailnets.json"), br#"{"tailnets":[]}"#).unwrap();
            let (sender, receiver) = channel();
            let catalog = Catalog {
                tailnets: vec![tailnet::Tailnet {
                    id: "synthetic".into(),
                    name: "Synthetic".into(),
                }],
            };
            let mut editor = State::api_reply_fixture(root.clone(), catalog, receiver);
            let mut production = State::for_root_fixture(root);
            let ctx = egui::Context::default();
            production.observe_settings(&mut editor, &ctx);
            assert!(production.settings_discovery.is_some());
            production.reload_after(editor);
            assert!(production.settings_discovery.is_none());
            production.poll(&ctx);
            assert!(
                production.receiver.is_none(),
                "fresh catalog read waits for the API operation"
            );
            assert_eq!(
                production.api_credential_observation("synthetic"),
                ApiCredentialObservation::Pending
            );
            if failed {
                sender.send(Err("Synthetic API read failed".into())).unwrap();
            } else {
                sender.send(Ok(vec![])).unwrap();
            }
            production.poll(&ctx);
            assert!(production.receiver.is_some());
            assert!(production.settings_error.is_none());
            if failed {
                assert_eq!(
                    production.api_credentials.read_error(),
                    Some("Synthetic API read failed")
                );
            } else {
                assert!(production.api_credentials.read_error().is_none());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while production.receiver.is_some() || production.api_credentials.busy() {
                assert!(std::time::Instant::now() < deadline);
                production.poll(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(
                production
                    .cached_catalog_at(&production.root)
                    .unwrap()
                    .tailnets
                    .is_empty()
            );
            assert_eq!(
                production.api_credential_observation("synthetic"),
                ApiCredentialObservation::Unavailable
            );
            assert!(production.api_credentials.read_error().is_none());
        }
    }
}
