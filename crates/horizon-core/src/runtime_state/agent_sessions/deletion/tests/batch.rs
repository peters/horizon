use super::*;

fn binding(id: u128) -> AgentSessionBinding {
    let mut binding = super::binding(id);
    binding.kind = PanelKind::Codex;
    binding
}

#[test]
fn batch_preserves_protected_sessions_deduplicates_and_reports_partial_failure() {
    let a = binding(1);
    let b = binding(2);
    let c = binding(3);
    let mut catalog = AgentSessionCatalog {
        sessions: [&a, &b, &c]
            .into_iter()
            .map(|binding| AgentSessionRecord {
                kind: binding.kind,
                session_id: binding.session_id.clone(),
                cwd: binding.cwd.clone(),
                label: None,
                updated_at: 0,
                interactive: true,
            })
            .collect(),
    };
    let protected = HashSet::from([AgentSessionKey::new(b.kind, &b.session_id)]);
    let report = catalog.delete_with(&[a.clone(), a.clone(), b, c.clone()], &protected, |session| {
        if session.session_id == c.session_id {
            Err(Error::State("synthetic provider failure".into()))
        } else {
            Ok(DeletionOutcome::Removed)
        }
    });
    assert_eq!(report.deleted.len(), 1);
    assert_eq!(report.failures.len(), 2);
    catalog.remove_deleted_sessions(&report);
    assert_eq!(catalog.sessions.len(), 2);
    assert!(
        !catalog
            .sessions
            .iter()
            .any(|session| session.session_id == a.session_id)
    );
}

#[test]
fn invalid_or_unknown_selection_never_reaches_provider() {
    let catalog = AgentSessionCatalog::default();
    let mut invalid = binding(1);
    invalid.session_id = "../outside".into();
    let report = catalog.delete_with(&[invalid, binding(2)], &HashSet::new(), |_| panic!("must not execute"));
    assert_eq!(report.failures.len(), 2);
    assert!(report.deleted.is_empty());
}

#[test]
fn reservations_exclude_queued_sessions_from_all_catalogs_and_panel_launches() {
    let session = binding(0xabc_987);
    let catalog = AgentSessionCatalog {
        sessions: vec![AgentSessionRecord {
            kind: session.kind,
            session_id: session.session_id.clone(),
            cwd: normalize_cwd(session.cwd.as_deref()),
            label: None,
            updated_at: 0,
            interactive: true,
        }],
    };
    let guard = reserve_saved_session_deletions(std::slice::from_ref(&session)).expect("reserve");
    assert!(saved_session_deletion_pending(session.kind, &session.session_id));
    assert!(
        catalog
            .clone()
            .recent_for(session.kind, session.cwd.as_deref())
            .is_empty()
    );
    assert!(reserve_saved_session_deletions(std::slice::from_ref(&session)).is_err());
    let mut board = crate::Board::new();
    let workspace = board.create_workspace("Synthetic protection test");
    assert!(
        board
            .create_panel(
                crate::PanelOptions {
                    kind: session.kind,
                    resume: crate::PanelResume::Session {
                        session_id: session.session_id.clone()
                    },
                    ..Default::default()
                },
                workspace
            )
            .is_err()
    );
    assert!(
        board
            .create_panel(
                crate::PanelOptions {
                    kind: session.kind,
                    resume: crate::PanelResume::Fresh,
                    session_binding: Some(session.clone()),
                    ..Default::default()
                },
                workspace
            )
            .is_err()
    );
    drop(guard);
    assert!(!saved_session_deletion_pending(session.kind, &session.session_id));
    assert_eq!(catalog.recent_for(session.kind, session.cwd.as_deref()).len(), 1);
}

#[test]
fn recovery_required_is_removed_from_catalog_without_claiming_deletion() {
    let session = binding(7);
    let key = AgentSessionKey::new(session.kind, &session.session_id);
    let mut catalog = AgentSessionCatalog {
        sessions: vec![AgentSessionRecord {
            kind: session.kind,
            session_id: session.session_id.clone(),
            cwd: session.cwd.clone(),
            label: None,
            updated_at: 0,
            interactive: true,
        }],
    };
    let report = catalog.delete_with(std::slice::from_ref(&session), &HashSet::new(), |_| {
        Ok(DeletionOutcome::RecoveryRequired {
            directory: "/sample/recovery".into(),
            message: "Restore transcript first".into(),
        })
    });
    assert!(report.deleted.is_empty());
    assert!(report.failures.is_empty());
    assert_eq!(report.recoveries.len(), 1);
    assert_eq!(report.recoveries[0].key, key);
    catalog.remove_deleted_sessions(&report);
    assert!(catalog.recent_for(session.kind, session.cwd.as_deref()).is_empty());
}
