//! Capture launch context and prepare the repository while the user enters a title.
use super::{HorizonApp, cloud_runtime};
use egui::Context;
use horizon_core::{WorkspaceId, cloud_panel::Placement, cloud_runtime::repository::launch::Prepared};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

struct Loaded {
    prepared: Prepared,
    ready_profiles: Vec<String>,
}

#[derive(Default)]
pub(super) struct State {
    pub workspace: Option<String>,
    session: Option<String>,
    receiver: Option<Receiver<cloud_runtime::Result<Loaded>>>,
    cancel: cloud_runtime::Cancellation,
    pub revision: Option<String>,
    pub submitted: bool,
    pub accounts_checked: bool,
    ready_profiles: Vec<String>,
}
impl State {
    pub fn loading(&self) -> bool {
        self.receiver.is_some()
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl HorizonApp {
    pub(in crate::app) fn open_workspace_cloud(&mut self, ctx: &Context, workspace: WorkspaceId) {
        let Some(source) = self.board.workspace(workspace) else {
            return;
        };
        let local = source.local_id.clone();
        let repository = source
            .cwd
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let form = &mut self.cloud_prototype.production;
        form.pending_creation = None;
        form.launch = State::default();
        form.launch.workspace = Some(local);
        form.launch.session = self.active_session.as_ref().map(|session| session.session_id.clone());
        form.title.clear();
        form.repository = repository;
        form.revision.clear();
        form.profiles = None;
        form.selected_profile.clear();
        form.size = None;
        form.placement = Placement::default();
        form.creating = true;
        form.focus_title_on_open = true;
        self.read_cloud_profiles(ctx);
    }

    pub(super) fn read_cloud_profiles(&mut self, ctx: &Context) {
        let form = &mut self.cloud_prototype.production;
        form.launch.cancel.cancel();
        form.launch.cancel = cloud_runtime::Cancellation::default();
        form.launch.revision = None;
        form.launch.accounts_checked = false;
        form.launch.ready_profiles.clear();
        form.profiles = None;
        let root = self
            .cloud_prototype
            .root
            .clone()
            .unwrap_or_else(|| horizon_core::HorizonHome::resolve().root().join("cloud"));
        let repository = form.repository.clone();
        let revision = form.revision.clone();
        let cancel = form.launch.cancel.clone();
        let (sender, receiver) = channel();
        form.launch.receiver = Some(receiver);
        self.cloud_prototype.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let runner = cloud_runtime::command::Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let result = cloud_runtime::repository::launch::prepare(&repository, &revision, &runner).map(|prepared| {
                let ready_profiles =
                    cloud_runtime::repository::launch::ready_profiles(&root, &prepared.config, &cancel);
                Loaded {
                    prepared,
                    ready_profiles,
                }
            });
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_cloud_launch(&mut self, ctx: &Context) {
        let form = &mut self.cloud_prototype.production;
        if form.launch.workspace.is_some()
            && form.launch.session != self.active_session.as_ref().map(|session| session.session_id.clone())
        {
            form.launch = State::default();
            form.creating = false;
            return;
        }
        if let Some(receiver) = &form.launch.receiver {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                Err(TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid(
                    "Repository preparation interrupted. Retry loading the configuration.",
                )),
            };
            form.launch.receiver = None;
            match result {
                Ok(loaded) => {
                    let prepared = loaded.prepared;
                    form.launch.ready_profiles = loaded.ready_profiles;
                    form.repository = prepared.repository.to_string_lossy().into_owned();
                    if !prepared.config.profiles.contains_key(&form.selected_profile) {
                        form.selected_profile.clone_from(&prepared.config.default);
                        form.size = None;
                        form.placement = Placement::default();
                    }
                    // A reread profile keeps a chosen size only while it still offers it.
                    let profile = prepared.config.profiles.get(&form.selected_profile);
                    form.size = form.size.filter(|&size| {
                        profile.is_some_and(|profile| cloud_runtime::flavors::sized(profile, size).is_ok())
                    });
                    form.profiles = Some(prepared.config);
                    form.launch.revision = Some(prepared.revision);
                }
                Err(error) => {
                    form.launch.submitted = false;
                    self.cloud_prototype.error = Some(error.to_string());
                }
            }
        }
        if form.launch.submitted && form.pending_creation.is_none() && form.profiles.is_some() {
            form.launch.submitted = false;
            if !form.launch.accounts_checked && !form.launch.ready_profiles.contains(&form.selected_profile) {
                self.open_cloud_accounts(ctx, true);
                self.cloud_prototype.production.launch.submitted = true;
            } else if let Err(error) = self.create_production_cloud(ctx) {
                self.cloud_prototype.error = Some(error.to_string());
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::{RuntimeState, cloud_panel::CloudConfig};

    fn loaded(path: &std::path::Path) -> Loaded {
        Loaded {
            prepared: Prepared {
                repository: path.into(), revision: "a".repeat(40),
                config: CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap(),
            }, ready_profiles: vec!["dev".into()],
        }
    }
    #[test]
    fn deployment_persists_its_session_reference_before_creating_state() {
        let (temp, mut app) = test_app();
        let session = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .unwrap();
        app.activate_persistent_session(&session);
        let ctx = Context::default();
        app.restore_cloud_state(&ctx);
        let root = temp.path().join("cloud");
        app.cloud_prototype.root = Some(root.clone());
        let workspace = app.board.ensure_workspace();
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            101,
            "Retryable".into(),
            app.board.workspace(workspace).unwrap().local_id.clone(),
            temp.path().into(),
            [0.0, 0.0],
        );
        let cloud_id = cloud_runtime::new_id();
        group.remote = Some(horizon_core::cloud_panel::CloudLaunch {
            deployment_started: false,
            id: cloud_id.clone(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: loaded(temp.path()).prepared.config.profiles["dev"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        });
        app.cloud_prototype.groups.0.push(group);
        let runtime_path = session.runtime_state_path.clone();
        std::fs::remove_file(&runtime_path).unwrap();
        std::fs::create_dir(&runtime_path).unwrap();
        app.start_production_deployment(101, &ctx);
        assert!(!root.exists(), "persistence failure must not create a state directory");
        assert!(
            !app.cloud_prototype.groups.0[0]
                .remote
                .as_ref()
                .unwrap()
                .deployment_started
        );
        assert!(app.cloud_prototype.production.runtimes[&101].receiver.is_none());
        assert!(
            app.cloud_prototype.production.runtimes[&101]
                .error
                .as_ref()
                .unwrap()
                .contains("Could not save")
        );
        std::fs::remove_dir(&runtime_path).unwrap();
        app.start_production_deployment(101, &ctx);
        let saved = RuntimeState::load(&runtime_path).unwrap().unwrap();
        assert_eq!(saved.cloud_groups.0.len(), 1);
        assert_eq!(saved.cloud_groups.0[0].remote.as_ref().unwrap().id, cloud_id);
        assert!(!saved.cloud_groups.0[0].remote.as_ref().unwrap().deployment_started);
        assert!(app.cloud_prototype.production.runtimes[&101].receiver.is_none());
        assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    }

    #[test]
    fn mock_workspace_creation_honors_a_non_active_target() {
        let (temp, mut app) = test_app();
        app.cloud_prototype.root = Some(temp.path().into());
        let target = app.board.ensure_workspace();
        app.board.workspace_mut(target).unwrap().position = [2000.0, 3000.0];
        let local_id = app.board.workspace(target).unwrap().local_id.clone();
        let active = app.board.create_workspace("Other");
        app.board.active_workspace = Some(active);
        assert_eq!(app.board.active_workspace, Some(active));
        app.add_mock_cloud_in_workspace(&Context::default(), target);
        assert_eq!(app.cloud_prototype.groups.0.len(), 1);
        assert_eq!(app.cloud_prototype.groups.0[0].workspace, local_id);
        let position = app.cloud_prototype.groups.0[0].position;
        assert!((position[0] - 2024.0).abs() < f32::EPSILON);
        assert!((position[1] - 3128.0).abs() < f32::EPSILON);
        assert!(!app.cloud_prototype.production.creating);
    }

    #[test]
    fn workspace_entry_waits_for_cloud_state_restoration() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        app.open_cloud_for_workspace(&Context::default(), workspace);
        assert!(!app.cloud_prototype.production.creating);
        assert!(!app.cloud_prototype.production.launch.loading());
        assert!(app.cloud_prototype.groups.0.is_empty());
    }

    #[test]
    fn submission_waits_for_preparation_and_keeps_its_destination() {
        let (temp, mut app) = test_app();
        let session = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .unwrap();
        app.activate_persistent_session(&session);
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        let form = &mut app.cloud_prototype.production;
        form.launch.receiver = Some(receiver);
        form.title = "My cloud".into();
        form.launch.submitted = true;
        app.poll_cloud_launch(&ctx);
        assert!(app.cloud_prototype.production.launch.submitted);
        assert!(app.cloud_prototype.production.pending_creation.is_none());
        let _ = app.board.create_workspace("Other");
        sender.send(Ok(loaded(temp.path()))).unwrap();
        app.poll_cloud_launch(&ctx);
        assert!(app.cloud_prototype.production.pending_creation.is_some());
        assert!(!app.cloud_prototype.production.setup.open);
        assert_eq!(
            app.cloud_prototype.production.launch.workspace.as_deref(),
            Some(app.board.workspace(workspace).unwrap().local_id.as_str())
        );
        let before = std::ptr::from_ref(app.cloud_prototype.production.pending_creation.as_ref().unwrap());
        app.poll_cloud_launch(&ctx);
        assert_eq!(
            before,
            std::ptr::from_ref(app.cloud_prototype.production.pending_creation.as_ref().unwrap())
        );
    }
    #[test]
    fn revision_reload_retains_an_explicit_profile() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        app.cloud_prototype.production.launch.receiver = Some(receiver);
        app.cloud_prototype.production.selected_profile = "other".into();
        let mut result = loaded(temp.path());
        result
            .prepared
            .config
            .profiles
            .insert("other".into(), result.prepared.config.profiles["dev"].clone());
        sender.send(Ok(result)).unwrap();
        app.poll_cloud_launch(&ctx);
        assert_eq!(app.cloud_prototype.production.selected_profile, "other");
    }

    #[test]
    fn rereading_profiles_keeps_only_a_size_the_selected_profile_still_offers() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        let mut reread = |selected: &str, size, container_gb| {
            let (sender, receiver) = channel();
            let form = &mut app.cloud_prototype.production;
            form.launch.receiver = Some(receiver);
            form.selected_profile = selected.into();
            form.size = Some(size);
            let mut result = loaded(temp.path());
            if let Some(profile) = result.prepared.config.profiles.get_mut("dev") {
                profile.storage.container_gb = container_gb;
            }
            sender.send(Ok(result)).unwrap();
            app.poll_cloud_launch(&ctx);
            app.cloud_prototype.production.size
        };
        assert_eq!(reread("dev", (16, 32), 20), Some((16, 32)));
        assert_eq!(reread("dev", (2, 4), 40), None, "the reread disk needs more vCPU");
        assert_eq!(
            reread("removed", (16, 32), 20),
            None,
            "a replaced profile starts at its size"
        );
        app.cloud_prototype.production.size = Some((16, 32));
        app.open_workspace_cloud(&ctx, workspace);
        assert_eq!(app.cloud_prototype.production.size, None);
    }

    #[test]
    fn enter_in_title_submits_without_a_button_click() {
        use crate::test_egui::DiscardTextures;
        let (temp, mut app) = test_app();
        let session = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .unwrap();
        app.activate_persistent_session(&session);
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        app.cloud_prototype.production.launch.receiver = Some(receiver);
        sender.send(Ok(loaded(temp.path()))).unwrap();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx
                .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
                .discard_textures();
        }
        app.cloud_prototype.production.title = "Title only".into();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("cloud-title")));
        let mut event = input();
        event.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx
            .run_ui(event, |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
        assert!(app.cloud_prototype.production.pending_creation.is_some());
        assert!(!app.cloud_prototype.production.setup.open);
    }

    #[test]
    fn enter_after_failed_preparation_leaves_the_form_editable() {
        use crate::test_egui::DiscardTextures;
        let (_temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        app.cloud_prototype.production.launch.receiver = Some(receiver);
        sender
            .send(Err(cloud_runtime::Error::Invalid("Missing configuration")))
            .unwrap();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx
                .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
                .discard_textures();
        }
        app.cloud_prototype.production.title = "Keep editing".into();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("cloud-title")));
        let mut event = input();
        event.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx
            .run_ui(event, |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
        assert!(!app.cloud_prototype.production.launch.submitted);
        assert!(app.cloud_prototype.production.pending_creation.is_none());
        assert!(ctx.read_response(egui::Id::new("cloud-title")).unwrap().enabled());
    }

    #[test]
    fn missing_accounts_keep_title_and_launch_intent_for_repair() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.cloud_prototype.root = Some(temp.path().join("cloud"));
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        app.cloud_prototype.production.launch.receiver = Some(receiver);
        app.cloud_prototype.production.title = "Keep this title".into();
        app.cloud_prototype.production.launch.submitted = true;
        let mut result = loaded(temp.path());
        result.ready_profiles.clear();
        sender.send(Ok(result)).unwrap();
        app.poll_cloud_launch(&ctx);
        assert!(app.cloud_prototype.production.setup.open);
        assert!(app.cloud_prototype.production.launch.submitted);
        assert_eq!(app.cloud_prototype.production.title, "Keep this title");
        assert!(app.cloud_prototype.groups.0.is_empty());
    }
    #[test]
    fn reopening_and_session_switch_discard_stale_preparation() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let cancel = app.cloud_prototype.production.launch.cancel.clone();
        let (sender, receiver) = channel();
        app.cloud_prototype.production.launch.receiver = Some(receiver);
        app.open_workspace_cloud(&ctx, workspace);
        assert!(cancel.is_cancelled());
        assert!(sender.send(Ok(loaded(temp.path()))).is_err());
        app.cloud_prototype.production.launch.session = Some("other session".into());
        app.poll_cloud_launch(&ctx);
        assert!(!app.cloud_prototype.production.creating);
        assert!(!app.cloud_prototype.production.launch.loading());
    }
    #[test]
    fn unsaved_workspace_launch_keeps_title_without_creating_a_cloud() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        let (sender, receiver) = channel();
        let form = &mut app.cloud_prototype.production;
        form.launch.receiver = Some(receiver);
        form.title = "Keep this title".into();
        form.launch.submitted = true;
        sender.send(Ok(loaded(temp.path()))).unwrap();
        app.poll_cloud_launch(&ctx);
        assert!(app.cloud_prototype.error.as_ref().unwrap().contains("saved session"));
        assert!(app.cloud_prototype.production.creating);
        assert_eq!(app.cloud_prototype.production.title, "Keep this title");
        assert!(app.cloud_prototype.production.pending_creation.is_none());
        assert!(app.cloud_prototype.groups.0.is_empty());
        assert!(app.cloud_prototype.production.runtimes.is_empty());
    }

    #[test]
    fn reloading_configuration_revalidates_accounts_before_submission() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.ensure_workspace();
        app.open_workspace_cloud(&ctx, workspace);
        app.cloud_prototype.production.launch.accounts_checked = true;
        app.cloud_prototype.production.launch.ready_profiles.push("dev".into());
        app.cloud_prototype.production.revision = "different-commit".into();
        app.read_cloud_profiles(&ctx);
        assert!(!app.cloud_prototype.production.launch.accounts_checked);
        assert!(app.cloud_prototype.production.launch.ready_profiles.is_empty());
        let (sender, receiver) = channel();
        let form = &mut app.cloud_prototype.production;
        form.launch.receiver = Some(receiver);
        form.launch.submitted = true;
        form.title = "Keep this title".into();
        let mut result = loaded(temp.path());
        result.ready_profiles.clear();
        sender.send(Ok(result)).unwrap();
        app.poll_cloud_launch(&ctx);
        assert!(app.cloud_prototype.production.setup.open);
        assert!(app.cloud_prototype.production.pending_creation.is_none());
        assert!(app.cloud_prototype.groups.0.is_empty());
    }
}
