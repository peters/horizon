use super::*;
use crate::{app::test_support::raw_input, test_egui::DiscardTextures};
use horizon_core::cloud_run::{CloudProvider, WorkerLifetime};
use horizon_core::remote_github_credential::RemoteCredentialInstallation::{Installed, Present};

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
fn single_flight_and_late_mutation_result_never_replay() {
    let scope = scope();
    let ctx = Context::default();
    let mut state = RepositoryState::default();
    let tx = pending(&mut state, &scope, true);
    state.action(
        InventoryAction::InspectRepository,
        &HorizonHome::from_root("/not-opened".into()),
        &ctx,
        scope.clone(),
    );
    assert!(state.is_pending());
    state.invalidate();
    assert!(
        tx.send(Ok(Completion::Submitted(git::ConfiguredRemoteGitSubmission {
            credential: None,
            submission: git::RemoteGitSubmission::Submitted,
        })))
        .is_ok()
    );
    state.drain(Some(current(&scope)));
    assert!(state.unknown && state.notice.is_none() && !state.is_pending());
    state.invalidate();
    state.drain(Some(current(&scope)));
    assert!(state.unknown && !state.is_pending());
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
fn detached_submission_is_not_completed_and_degraded_receipt_is_not_ready() {
    let mut state = RepositoryState::default();
    let scope = scope();
    let tx = pending(&mut state, &scope, true);
    assert!(
        tx.send(Ok(Completion::Submitted(git::ConfiguredRemoteGitSubmission {
            credential: None,
            submission: git::RemoteGitSubmission::Submitted,
        })))
        .is_ok()
    );
    state.drain(Some(current(&scope)));
    assert!(state.notice.as_deref().unwrap().contains("not yet known"));
    assert!(!state.is_pending());
    for (credential, text) in [
        (Installed, "Credential installed"),
        (Present, "already present and unchanged"),
    ] {
        let tx = pending(&mut state, &scope, true);
        assert!(
            tx.send(Ok(Completion::Submitted(git::ConfiguredRemoteGitSubmission {
                credential: Some(credential),
                submission: git::RemoteGitSubmission::Submitted,
            })))
            .is_ok()
        );
        state.drain(Some(current(&scope)));
        let notice = state.notice.as_deref().unwrap();
        assert!(
            notice.contains(text)
                && notice.contains("permissions are not verified")
                && notice.contains("not yet known")
        );
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
