//! Masked, zeroizing API credential drafts; saved values never enter this editor.
use super::super::requests::Target;
use super::Reply;
use crate::theme;
use horizon_core::cloud_runtime::tailnet::{self, ApiCredential, ApiCredentialKind};

use zeroize::{Zeroize, Zeroizing};

pub(super) enum Action {
    Save,
    Cancel,
}

pub(super) struct Editor {
    pub(super) target: Target,
    pub(super) request: Option<String>,
    kind: ApiCredentialKind,
    token: Zeroizing<String>,
    client_id: Zeroizing<String>,
    client_secret: Zeroizing<String>,
}

impl Editor {
    pub(super) fn new(target: Target, kind: Option<ApiCredentialKind>) -> Self {
        Self {
            target,
            request: None,
            kind: kind.unwrap_or(ApiCredentialKind::PersonalToken),
            token: Zeroizing::new(String::new()),
            client_id: Zeroizing::new(String::new()),
            client_secret: Zeroizing::new(String::new()),
        }
    }

    pub(super) fn kind(&self) -> ApiCredentialKind {
        self.kind
    }

    fn choose(&mut self, kind: ApiCredentialKind) {
        if self.request.is_some() {
            return;
        }
        if self.kind != kind {
            self.token.zeroize();
            self.client_id.zeroize();
            self.client_secret.zeroize();
            self.kind = kind;
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let id = egui::Id::unique(("tailnet-api", &self.target.id));
        ui.push_id(id, |ui| {
            ui.add_space(8.0);
            ui.add_enabled_ui(self.request.is_none(), |ui| { ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(self.kind == ApiCredentialKind::PersonalToken, "Personal API token").clicked() {
                    self.choose(ApiCredentialKind::PersonalToken);
                }
                if ui.selectable_label(self.kind == ApiCredentialKind::OAuth, "OAuth client").clicked() {
                    self.choose(ApiCredentialKind::OAuth);
                }
            }); });
            ui.add_space(8.0);
            match self.kind {
                ApiCredentialKind::PersonalToken => {
                    password(ui, "Personal API token", "tskey-api-…", &mut self.token);
                }
                ApiCredentialKind::OAuth => {
                    password(ui, "Client ID", "OAuth client ID", &mut self.client_id);
                    ui.add_space(8.0);
                    password(ui, "Client secret", "OAuth client secret", &mut self.client_secret);
                }
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Stored on this computer only. Your auth key still enrolls workers. Saving does not test API access or remove nodes.")
                    .size(12.0)
                    .color(theme::FG_SOFT()),
            );
            if self.kind == ApiCredentialKind::OAuth {
                ui.label(
                    egui::RichText::new("Use devices:core with tag:horizon-worker. This scope can allow removal of other tailnet devices too.")
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
            } else {
                ui.label(
                    egui::RichText::new("Personal API tokens can have broad tailnet access and expire. Replace an expired or revoked token.")
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let complete = match self.kind {
                    ApiCredentialKind::PersonalToken => !self.token.trim().is_empty(),
                    ApiCredentialKind::OAuth => !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty(),
                };
                if ui.add_enabled(complete, egui::Button::new("Save API credential")).clicked() {
                    action = Some(Action::Save);
                }
                if ui.button("Discard credential draft").clicked() {
                    action = Some(Action::Cancel);
                }
            });
        });
        action
    }

    pub(super) fn save(self) -> Reply {
        let credential = match self.kind {
            ApiCredentialKind::PersonalToken => ApiCredential::personal_token(self.token.trim()),
            ApiCredentialKind::OAuth => ApiCredential::oauth(self.client_id.trim(), self.client_secret.trim()),
        }
        .map_err(|error| error.to_string())?;
        let store = super::removal::checked_store(&self.target)?;
        store
            .save_api_credential(&self.target.id, &credential)
            .map_err(|error| error.to_string())?;
        Ok(vec![(
            self.target.id,
            Ok(tailnet::ApiCredentialStatus::Stored(self.kind)),
        )])
    }
}

fn password(ui: &mut egui::Ui, label: &str, hint: &str, value: &mut Zeroizing<String>) -> egui::Response {
    ui.label(egui::RichText::new(label).size(12.0));
    let response = ui.add(
        egui::TextEdit::singleline(&mut **value)
            .id_salt(label)
            .password(true)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .font(egui::FontId::proportional(14.0))
            .margin(egui::Margin::symmetric(10, 8))
            .background_color(theme::PANEL_BG_ALT()),
    );
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
        state.clear_undoer();
        state.store(ui.ctx(), response.id);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(id: &str) -> Target {
        Target {
            root: "synthetic-not-used".into(),
            id: id.into(),
            name: "Synthetic".into(),
        }
    }

    #[test]
    fn root_change_discards_the_secret_draft_without_starting_a_reader_or_losing_write_error() {
        use super::super::{Pending, State, TaskKind};
        use horizon_core::cloud_runtime::tailnet::{Catalog, Tailnet};
        let old_root = std::path::PathBuf::from("synthetic-old-root-not-opened");
        let new_root = std::path::PathBuf::from("synthetic-new-root-not-opened");
        let catalog = Catalog {
            tailnets: vec![Tailnet {
                id: "work".into(),
                name: "Synthetic".into(),
            }],
        };
        for kind in [ApiCredentialKind::PersonalToken, ApiCredentialKind::OAuth] {
            let mut draft = Editor::new(
                Target {
                    root: old_root.clone(),
                    id: "work".into(),
                    name: "Synthetic".into(),
                },
                Some(kind),
            );
            *draft.token = "synthetic-token".into();
            *draft.client_id = "synthetic-id".into();
            *draft.client_secret = "synthetic-secret".into();
            let (_sender, receiver) = std::sync::mpsc::channel();
            let mut state = State {
                editor: Some(draft),
                receiver: Some(Pending {
                    kind: TaskKind::Read,
                    receiver,
                }),
                write_result: Some(Err("Existing write failure".into())),
                ..State::default()
            };
            state.refresh(&egui::Context::default(), &new_root, &catalog);
            assert!(state.editor.is_none());
            assert_eq!(state.write_error(), Some("Existing write failure"));
            assert_eq!(state.revision(), 0, "blocked refresh must not start a worker");
            assert!(state.busy());
            assert!(matches!(
                state.receiver.as_ref().unwrap().receiver.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            assert_eq!(state.refresh.as_ref().unwrap().0, new_root);
        }
    }

    #[test]
    fn scope_suspension_discards_secrets_without_rendering_or_dropping_the_owned_writer() {
        use super::super::{Pending, State, TaskKind};
        use crate::app::tailnets::State as TailnetsState;

        for kind in [ApiCredentialKind::PersonalToken, ApiCredentialKind::OAuth] {
            let mut draft = Editor::new(target("work"), Some(kind));
            *draft.token = "synthetic-token".into();
            *draft.client_id = "synthetic-id".into();
            *draft.client_secret = "synthetic-secret".into();
            let old_root = draft.target.root.clone();
            let (writer, receiver) = std::sync::mpsc::channel();
            let (reader, old_read) = std::sync::mpsc::channel();
            let mut settings = TailnetsState {
                root: old_root.clone(),
                initialized: true,
                api_credentials: State {
                    editor: Some(draft),
                    ids: ["work".into()].into(),
                    receiver: Some(Pending {
                        kind: TaskKind::Write,
                        receiver,
                    }),
                    barriers: vec![Pending {
                        kind: TaskKind::Read,
                        receiver: old_read,
                    }],
                    refresh: Some((old_root, vec!["work".into()])),
                    write_result: Some(Err("Earlier write failure".into())),
                    ..State::default()
                },
                ..TailnetsState::default()
            };
            let ctx = egui::Context::default();
            let mut production = TailnetsState::for_root_fixture("new-root-not-opened".into());
            production.observe_settings(&mut settings, &ctx);
            assert!(settings.settings_scope_changed());
            assert!(settings.api_credentials.editor.is_none());
            assert!(settings.api_credentials.refresh.is_none());
            assert_eq!(settings.api_credentials.write_error(), Some("Earlier write failure"));
            assert!(settings.api_credentials.receiver.is_some());
            assert_eq!(settings.api_credentials.barriers.len(), 1);
            writer.send(Err("Owned writer failed after root drift".into())).unwrap();
            reader.send(Ok(vec![])).unwrap();
            production.observe_settings(&mut settings, &ctx);
            assert!(!settings.api_credentials.busy());
            assert_eq!(
                settings.api_credentials.write_error(),
                Some("Owned writer failed after root drift")
            );
            assert!(settings.receiver.is_none());
            assert!(production.cached_catalog_at(&production.root).is_none());
        }
    }

    #[test]
    fn replacing_either_stored_kind_starts_with_empty_secret_fields() {
        for kind in [ApiCredentialKind::PersonalToken, ApiCredentialKind::OAuth] {
            let editor = Editor::new(target("synthetic-network"), Some(kind));
            assert_eq!(editor.kind, kind);
            assert!(editor.token.is_empty());
            assert!(editor.client_id.is_empty());
            assert!(editor.client_secret.is_empty());
        }
    }

    #[test]
    fn invalid_drafts_fail_before_creating_store_state_and_report_no_value() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("not-created");
        let mut token = Editor::new(target("synthetic-network"), None);
        *token.token = "tskey-auth-synthetic-enrollment-only".into();
        let mut oauth = Editor::new(target("synthetic-network"), Some(ApiCredentialKind::OAuth));
        *oauth.client_id = "synthetic client id".into();
        *oauth.client_secret = "synthetic-client-secret".into();
        let expected = ApiCredential::personal_token("tskey-auth-synthetic-enrollment-only")
            .err()
            .unwrap()
            .to_string();
        for mut draft in [token, oauth] {
            draft.target.root = root.clone();
            assert_eq!(draft.save(), Err(expected.clone()));
            assert!(
                !root.exists(),
                "Invalid drafts must fail before filesystem or OS-store work"
            );
        }
    }

    #[test]
    fn an_owner_request_locks_the_requested_kind_until_the_draft_is_discarded() {
        let mut editor = Editor::new(target("work"), Some(ApiCredentialKind::PersonalToken));
        editor.request = Some("synthetic-request".into());
        editor.choose(ApiCredentialKind::OAuth);
        assert_eq!(editor.kind(), ApiCredentialKind::PersonalToken);
    }

    #[test]
    fn a_valid_draft_cannot_save_into_a_renamed_saved_target() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("tailnets.json");
        let bytes = br#"{"tailnets":[{"id":"work","name":"Renamed"}]}"#;
        std::fs::write(&file, bytes).unwrap();
        let mut draft = Editor::new(
            Target {
                root: directory.path().into(),
                id: "work".into(),
                name: "Previous".into(),
            },
            None,
        );
        *draft.token = "tskey-api-synthetic-owner-only".into();
        assert_eq!(
            draft.save().unwrap_err(),
            "The saved tailnet changed. Refresh and choose it again."
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn switching_credential_sources_clears_every_secret_draft() {
        let mut editor = Editor::new(target("work"), None);
        *editor.token = "synthetic-token".into();
        editor.choose(ApiCredentialKind::OAuth);
        assert!(editor.token.is_empty());
        *editor.client_id = "synthetic-id".into();
        *editor.client_secret = "synthetic-secret".into();
        editor.choose(ApiCredentialKind::PersonalToken);
        assert!(editor.client_id.is_empty());
        assert!(editor.client_secret.is_empty());
    }

    #[test]
    fn clearing_a_password_draft_cannot_restore_it_with_undo() {
        let ctx = egui::Context::default();
        let mut value = Zeroizing::new(String::new());
        let frame = |input, value: &mut Zeroizing<String>, focus| {
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let response = password(ui, "Synthetic token", "", value);
                    if focus {
                        response.request_focus();
                    }
                });
            });
            output.textures_delta.clear();
        };
        frame(egui::RawInput::default(), &mut value, true);
        frame(
            egui::RawInput {
                events: vec![egui::Event::Text("synthetic-secret".into())],
                ..egui::RawInput::default()
            },
            &mut value,
            false,
        );
        assert_eq!(value.as_str(), "synthetic-secret");
        value.zeroize();
        frame(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: Some(egui::Key::Z),
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::CTRL,
                    },
                ],
                ..egui::RawInput::default()
            },
            &mut value,
            false,
        );
        assert!(value.is_empty());
    }
}
