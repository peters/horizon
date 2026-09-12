use super::super::{InventoryPage, RemoteEnvironments, paint::InventoryRow};
use super::*;
use crate::{app::test_support::raw_input, test_egui::DiscardTextures};
use horizon_core::cloud_run::{CloudProvider, WorkerLifetime};
use horizon_core::remote_github_credential::RemoteCredentialInstallation::{self, Installed, Present};

fn scope() -> Scope {
    Scope {
        expected: RemoteEnvironmentSummary {
            workspace_local_id: "synthetic".into(),
            owning_session_id: "owner".into(),
            revision: 0,
            repository: "fixture/project".into(),
            provider: CloudProvider::LocalDocker,
            profile: "test".into(),
            lifetime: WorkerLifetime::Persistent,
            generation: 0,
            saved_phase: None,
            workflow_id: None,
            job_id: None,
            worker_identity: None,
            checkpoint: None,
            panel_count: 0,
        },
        config: RemoteProviderConfig::default(),
        owner: "owner".into(),
    }
}
fn pending(state: &mut RepositoryState, scope: &Scope, mutating: bool) -> mpsc::SyncSender<Result<Completion, String>> {
    let (tx, receiver) = mpsc::sync_channel(1);
    state.pending = Some(Pending {
        receiver,
        scope: scope.clone(),
        mutating,
        discard: false,
    });
    tx
}
fn current(scope: &Scope) -> Current<'_> {
    (&scope.expected, &scope.config, &scope.owner)
}
fn submitted(credential: Option<RemoteCredentialInstallation>) -> Completion {
    Completion::Submitted(git::ConfiguredRemoteGitSubmission {
        credential,
        submission: git::RemoteGitSubmission::Submitted,
    })
}

#[test]
fn cancel_and_context_invalidation_clear_transient_pat() {
    let mut state = RepositoryState {
        token: "synthetic_repository_pat".into(),
        consent: true,
        ..Default::default()
    };
    state.cancel();
    assert!(state.token.is_empty() && !state.consent && state.confirmation.is_none());
    state.token = "another_synthetic_pat".into();
    state.consent = true;
    state.invalidate();
    assert!(state.token.is_empty() && !state.consent);
}

#[test]
fn changed_selection_or_lost_reply_preserves_unattributed_unknown() {
    for lost in [false, true] {
        let mut state = RepositoryState::default();
        let scope = scope();
        let tx = pending(&mut state, &scope, true);
        if !lost {
            assert!(tx.send(Err("Synthetic Git refusal".into())).is_ok());
        }
        drop(tx);
        state.drain(if lost { Some(current(&scope)) } else { None });
        assert!(state.unknown && !state.is_pending());
    }
}

#[test]
fn single_flight_selection_and_late_mutation_results_never_replay() {
    let home = HorizonHome::from_root("/never-opened".into());
    let ctx = Context::default();
    for index in [None, Some(0), Some(2), Some(1)] {
        let scope = scope();
        let mut other = scope.expected.clone();
        other.workspace_local_id = "other".into();
        let mut view = RemoteEnvironments {
            selected: Some(0),
            page: Some(InventoryPage {
                rows: [scope.expected.clone(), other]
                    .into_iter()
                    .map(InventoryRow::new)
                    .collect(),
                next_cursor: None,
            }),
            ..Default::default()
        };
        let tx = pending(&mut view.repository, &scope, true);
        view.repository
            .action(InventoryAction::InspectRepository, &home, &ctx, scope.clone());
        assert!(view.repository.is_pending());
        match index {
            Some(index) => view.apply(InventoryAction::Select(index), &home, &ctx),
            None => view.repository.invalidate(),
        }
        assert_eq!(view.selected, Some(usize::from(index == Some(1))));
        assert!(tx.send(Ok(submitted(None))).is_ok());
        view.repository.drain(Some(current(&scope)));
        let discarded = index.is_none() || index == Some(1);
        assert_eq!(view.repository.unknown, discarded);
        assert_eq!(view.repository.notice.is_none(), discarded);
        assert!(!view.repository.is_pending());
        view.repository.invalidate();
        view.repository.drain(Some(current(&scope)));
        assert_eq!(view.repository.unknown, discarded);
        assert!(!view.repository.is_pending());
    }
}

#[test]
fn detached_submission_is_not_completed_and_degraded_receipt_is_not_ready() {
    let mut state = RepositoryState::default();
    let scope = scope();
    for (credential, text) in [
        (None, ""),
        (Some(Installed), "Credential installed"),
        (Some(Present), "already present and unchanged"),
    ] {
        let tx = pending(&mut state, &scope, true);
        assert!(tx.send(Ok(submitted(credential))).is_ok());
        state.drain(Some(current(&scope)));
        let notice = state.notice.as_deref().unwrap();
        assert!(notice.contains(text) && notice.contains("not yet known"));
        assert!(!state.is_pending());
        if credential.is_some() {
            assert!(notice.contains("permissions are not verified"));
        }
    }
    let mut observation = git::RemoteGitObservation {
        state: git::RemoteGitState::Complete,
        reason: None,
    };
    assert!(observation_text(observation).contains("Original preparation complete"));
    observation.reason = Some(git::RemoteGitReason::Conflict);
    assert!(!observation_text(observation).contains("preparation complete"));
}

#[test]
fn password_widget_clears_plaintext_undo_and_masks_output() {
    let ctx = Context::default();
    let mut token = "synthetic_repository_pat".to_owned();
    for _ in 0..2 {
        let output = ctx.run_ui(raw_input([800.0, 600.0], None), |ui| {
            let id = paint::password_field(ui, &mut token);
            let state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap();
            assert!(
                !state
                    .undoer()
                    .has_undo(&(egui::text::CCursorRange::default(), String::new()))
            );
        });
        assert!(!format!("{:?}", output.shapes).contains(&token));
        let _ = output.discard_textures();
    }
    token.clear();
}

#[test]
fn unsupported_repository_actions_never_dispatch() {
    let ctx = Context::default();
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("never-opened"));
    for provider in [CloudProvider::LocalDocker, CloudProvider::RunPod, CloudProvider::Azure] {
        assert_eq!(
            supported(provider),
            cfg!(target_os = "linux") && provider != CloudProvider::Azure
        );
        if supported(provider) {
            continue;
        }
        let mut scope = scope();
        scope.expected.provider = provider;
        for action in [
            InventoryAction::PrepareRepository,
            InventoryAction::ConfirmRepository,
            InventoryAction::InspectRepository,
        ] {
            let mut state = RepositoryState::default();
            state.action(action, &home, &ctx, scope.clone());
            assert!(!state.is_pending() && state.notice.is_none());
        }
    }
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn start_actions_clear_repository_pat_without_forgetting_pending_mutation() {
    for action in [
        InventoryAction::PrepareTaskStart(0),
        InventoryAction::ConfirmTaskStart,
        InventoryAction::CancelTaskStart,
    ] {
        let mut view = super::super::RemoteEnvironments::default();
        view.repository.token = "synthetic_repository_pat".into();
        view.repository.consent = true;
        let tx = pending(&mut view.repository, &scope(), true);
        view.apply(
            action,
            &HorizonHome::from_root("/never-opened".into()),
            &Context::default(),
        );
        assert!(view.repository.token.is_empty() && !view.repository.consent);
        assert!(view.repository.is_pending());
        drop(tx);
        view.repository.drain(None);
        assert!(view.repository.unknown && !view.repository.is_pending());
    }
}
