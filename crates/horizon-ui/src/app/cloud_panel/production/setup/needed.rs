//! The image repository a cloud's `.horizon/cloud.yml` names, shown in Container registry
//! when Cloud settings open from that cloud and no binding covers the repository yet.
use super::HorizonApp;
use super::dashboard::{Readiness, Tone, caption, chip, label};
use crate::{app::util::primary_button, theme};
use egui::{Align, Layout, RichText, Ui, vec2};
use horizon_core::cloud_runtime::registry::{self, Publishing, draft::Draft};

/// The repository, and whether the card still has to bring it into view.
#[derive(Default)]
pub(super) struct Needed {
    repository: Option<registry::Needed>,
    reveal: bool,
}

impl Needed {
    pub(super) fn new(repository: Option<registry::Needed>) -> Self {
        Self {
            reveal: repository.is_some(),
            repository,
        }
    }

    /// The repository while no binding or new entry in `drafts` covers it.
    pub(super) fn pending(&self, drafts: &[Draft]) -> Option<&registry::Needed> {
        self.repository
            .as_ref()
            .filter(|needed| !drafts.iter().any(|draft| draft.repository == needed.repository))
    }

    /// Whether `draft` is the new entry for this repository, whose publishing fields open.
    pub(super) fn added(&self, draft: &Draft) -> bool {
        draft.original.is_none()
            && self
                .repository
                .as_ref()
                .is_some_and(|needed| needed.repository == draft.repository)
    }
}

impl HorizonApp {
    /// Cloud settings for the image repository of cloud `id`: the one its record names,
    /// else the one it was created with.
    pub(in crate::app::cloud_panel) fn open_cloud_registry(&mut self, ctx: &egui::Context, id: u32) {
        let production = &self.cloud_prototype.production;
        let image = production
            .runtimes
            .get(&id)
            .and_then(|runtime| runtime.state.as_ref())
            .map(|state| state.profile.image.clone())
            .or_else(|| {
                self.cloud_prototype
                    .groups
                    .0
                    .iter()
                    .find(|group| group.issue == id)
                    .and_then(|group| group.remote.as_ref())
                    .map(|launch| launch.profile.image.clone())
            });
        self.open_cloud_settings(ctx, false, image);
    }
}

impl Readiness {
    /// Settings otherwise complete still need the cloud's repository set up.
    pub(super) fn needing(self, pending: Option<&registry::Needed>) -> Self {
        match pending {
            Some(needed) if self.tone == Tone::Ready => Self {
                tone: Tone::Attention,
                title: "Almost ready to launch",
                cause: format!(
                    "Add credentials for {}, which this cloud's .horizon/cloud.yml names.",
                    needed.repository
                ),
            },
            _ => self,
        }
    }
}

/// The repository before it is bound: its state and one action, which adds an entry
/// with the repository filled in.
pub(super) fn block(ui: &mut Ui, needed: &mut Needed, drafts: &mut Vec<Draft>) {
    let Some(pending) = needed.pending(drafts).cloned() else {
        return;
    };
    ui.separator();
    let response = ui
        .scope(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&pending.repository)
                        .size(13.0)
                        .strong()
                        .color(theme::FG()),
                );
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    chip(ui, Tone::Attention, "Not set up");
                });
            });
            caption(ui, "This cloud's .horizon/cloud.yml names this image repository.");
            label(ui, &format!("Publishing: {}", publishing(&pending.publishing)));
            label(
                ui,
                "Worker pull: no pull credential yet. Workers can pull only a public image.",
            );
            if ui
                .add(primary_button("Add credentials").min_size(vec2(148.0, 32.0)))
                .clicked()
            {
                drafts.push(Draft {
                    repository: pending.repository.clone(),
                    ..Draft::default()
                });
            }
        })
        .response;
    if std::mem::take(&mut needed.reveal) {
        response.scroll_to_me(Some(Align::Center));
    }
}

fn publishing(publishing: &Publishing) -> String {
    match publishing {
        Publishing::GitHub(login) => format!("Horizon publishes as {login} on GitHub."),
        Publishing::AskOnCard => "the cloud card asks GitHub once, at the first push.".into(),
        Publishing::Credential => "needs a publishing credential for this repository.".into(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{State, registry as card};
    use super::*;
    use crate::{app::test_support::test_app_with_startup, test_egui::DiscardTextures};
    use horizon_core::{
        RuntimeState, StartupDecision,
        cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
    };
    use std::time::{Duration, Instant};

    const CONFIG: &str = "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: registry.example/team/app\n    build:\n      context: .horizon\n      dockerfile: .horizon/Dockerfile\n    min_cpu: 4\n    min_memory_gb: 8\n";

    fn opened_for_a_cloud() -> (tempfile::TempDir, State) {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.cloud_prototype.root = Some(temp.path().join("cloud"));
        let mut group = CloudGroup::new(7, "App".into(), "w".into(), temp.path().into(), [0.0, 0.0]);
        group.remote = Some(CloudLaunch {
            deployment_started: true,
            id: "cloud-7".into(),
            revision: "9f3c2a1".into(),
            profile_name: "cpu".into(),
            profile: CloudConfig::parse(CONFIG).unwrap().profiles["cpu"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        });
        app.cloud_prototype.groups.0.push(group);
        app.open_cloud_registry(&ctx, 7);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.cloud_prototype.production.setup.receiver.is_some() {
            assert!(Instant::now() < deadline, "the settings did not load");
            app.poll_cloud_accounts(&ctx);
            std::thread::yield_now();
        }
        (temp, std::mem::take(&mut app.cloud_prototype.production.setup))
    }

    fn texts(state: &mut State) -> Vec<String> {
        let State {
            draft,
            verified,
            needed,
            ..
        } = state;
        let draft = draft.as_deref_mut().unwrap();
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                card::card(ui, draft, verified, needed);
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_cloud_repository_shows_filled_in_with_its_state_and_one_action() {
        let (_temp, mut state) = opened_for_a_cloud();
        let shown = texts(&mut state);
        for wanted in [
            "registry.example/team/app",
            "Not set up",
            "Publishing: needs a publishing credential for this repository.",
            "Worker pull: no pull credential yet. Workers can pull only a public image.",
            "Add credentials",
            "Add another",
        ] {
            assert!(shown.iter().any(|text| text == wanted), "{wanted}: {shown:?}");
        }
        for hidden in [
            "New image repository",
            "Add image repository",
            "Read-only pull credential",
        ] {
            assert!(!shown.iter().any(|text| text == hidden), "{hidden}: {shown:?}");
        }
        let draft = state.draft.as_deref_mut().unwrap();
        let pending = state.needed.pending(&draft.registries).cloned().unwrap();
        let ready = Readiness {
            tone: Tone::Ready,
            title: "Settings complete",
            cause: String::new(),
        };
        let banner = ready.needing(Some(&pending));
        assert_eq!(banner.tone, Tone::Attention);
        assert!(banner.cause.contains("registry.example/team/app"), "{}", banner.cause);
        draft.registries.push(Draft {
            repository: pending.repository.clone(),
            ..Draft::default()
        });
        assert!(state.needed.added(&draft.registries[0]), "its publishing fields open");
        let shown = texts(&mut state);
        assert!(
            !shown.iter().any(|text| text == "Not set up"),
            "the entry replaces the block"
        );
        assert!(shown.iter().any(|text| text == "Publishing credential"), "{shown:?}");
    }

    #[test]
    fn settings_opened_from_the_menu_keep_the_empty_form_behind_its_button() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State {
            draft: Some(Box::new(
                horizon_core::cloud_runtime::setup::Draft::load(temp.path()).unwrap(),
            )),
            ..State::default()
        };
        let shown = texts(&mut state);
        assert!(shown.iter().any(|text| text == "Add image repository"), "{shown:?}");
        assert!(!shown.iter().any(|text| text == "New image repository"), "{shown:?}");
    }
}
