//! Cloud-only named networks. Settings never reads a saved key back into the form.
use crate::theme;
use horizon_core::cloud_runtime::tailnet::{self, Catalog, Selection};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
};
use zeroize::{Zeroize, Zeroizing};

pub(super) struct State {
    root: PathBuf,
    catalog: Catalog,
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
}
impl Default for State {
    fn default() -> Self {
        Self {
            root: horizon_core::HorizonHome::resolve().root().join("cloud"),
            catalog: Catalog::default(),
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
        }
    }
}
impl State {
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
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(catalog) => {
                            self.catalog = catalog;
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
        if !self.initialized && self.receiver.is_none() && self.reload_barriers.is_empty() {
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
    pub(super) fn settings(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        ui.label(egui::RichText::new("Tailnets").size(24.0).strong().color(theme::FG()));
        ui.label(egui::RichText::new("Private networks for your clouds").color(theme::FG_SOFT()));
        ui.add_space(20.0);
        let networks = self.catalog.tailnets.clone();
        ui.add_enabled_ui(self.receiver.is_none(), |ui| {
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
    pub(super) fn loaded_fixture() -> Self {
        Self {
            initialized: true,
            ..Self::default()
        }
    }
    fn status(&self, ui: &mut egui::Ui) {
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
        self.reload();
        self.settings_error = if editor.failed { editor.message.take() } else { None };
        if let Some(receiver) = editor.receiver.take() {
            self.reload_barriers.push(receiver);
        }
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
            assert!(state.receiver.is_some());
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
            state.poll(&ctx);
            assert!(state.initialized);
            assert!(state.receiver.is_some());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while state.receiver.is_some() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
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
}
