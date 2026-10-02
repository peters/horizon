use super::*;
use crate::test_egui::DiscardTextures;

#[test]
fn conversation_checkboxes_expose_distinct_full_identity_labels() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let bindings: Vec<_> = (0..2)
        .map(|index| {
            AgentSessionBinding::new(
                horizon_core::PanelKind::Codex,
                format!("01a0f8a8-d7e2-7810-9e6b-{index:012}"),
                None,
                Some("Same title".into()),
                None,
            )
        })
        .collect();
    let mut state = SessionDeletionUi {
        managing: true,
        ..Default::default()
    };
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            for binding in &bindings {
                ui.push_id(&binding.session_id, |ui| state.render_row_controls(ui, binding));
            }
        })
        .discard_textures();
    let update = output.platform_output.accesskit_update.expect("accessibility update");
    for binding in bindings {
        let label = format!("Select conversation {}", binding.session_id);
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some(label.as_str()))
        );
    }
}

#[test]
fn picker_catalog_cache_tracks_panel_identity_folder_and_membership_without_cloning_options() {
    let cache = PickerCatalogCache {
        owner: PanelId(1),
        kind: horizon_core::PanelKind::Codex,
        revision: (None, None, 0),
        panels: vec![PickerPanelScope {
            id: PanelId(1),
            session: Some("exact".into()),
            cwd: Some("/sample/project".into()),
        }]
        .into(),
    };
    let scope = (PanelId(1), Some("exact"), Some(Path::new("/sample/project")));
    assert!(cache.matches_panels([scope].into_iter()));
    for changed in [
        (PanelId(2), scope.1, scope.2),
        (scope.0, Some("other"), scope.2),
        (scope.0, None, scope.2),
        (scope.0, scope.1, Some(Path::new("/sample/other"))),
    ] {
        assert!(!cache.matches_panels([changed].into_iter()));
    }
    assert!(!cache.matches_panels([scope, scope].into_iter()));
    assert!(!cache.matches_panels(std::iter::empty()));
}

#[test]
fn batch_start_failure_counts_every_conversation_and_preserves_preflight_failures() {
    let sessions: Vec<_> = (0..10)
        .map(|index| {
            AgentSessionBinding::new(
                horizon_core::PanelKind::Codex,
                format!("session-{index}"),
                None,
                None,
                None,
            )
        })
        .collect();
    let unavailable = vec![horizon_core::AgentSessionDeletionFailure {
        session_id: "already-attached".into(),
        message: "Attached to an open panel".into(),
    }];
    let mut duplicated = sessions.clone();
    duplicated.push(sessions[0].clone());
    for error in ["Reservation conflict", "Worker could not start"] {
        let report = failed_deletion_start(&duplicated, unavailable.clone(), error);
        assert!(report.deleted.is_empty());
        assert_eq!(report.failures.len(), 11);
        assert_eq!(report.failures[0].session_id, "already-attached");
        assert_eq!(report.failures[0].message, "Attached to an open panel");
        for session in &sessions {
            assert!(
                report
                    .failures
                    .iter()
                    .any(|failure| failure.session_id == session.session_id && failure.message == error)
            );
        }
        let mut ui = SessionDeletionUi::default();
        ui.finish(&report);
        assert_eq!(ui.message.as_deref(), Some("Deleted 0 conversations. 11 failed."));
    }
}

#[test]
fn recovery_receipt_does_not_claim_success_or_resumable_failure() {
    let ctx = egui::Context::default();
    let report = AgentSessionDeletionReport {
        recoveries: vec![horizon_core::AgentSessionDeletionRecovery {
            key: AgentSessionKey::new(horizon_core::PanelKind::Claude, "synthetic-id"),
            session_id: "synthetic-id".into(),
            directory: "/sample/recovery".into(),
            message: "Restore transcript first".into(),
        }],
        ..Default::default()
    };
    ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
    let restored = SessionDeletionUi::restored(&ctx);
    assert_eq!(
        restored.message.as_deref(),
        Some("Deleted 0 conversations. 0 failed. Recovery needed for 1 conversation.")
    );
    assert!(restored.details.is_empty());
    assert!(restored.recovery_details[0].contains("/sample/recovery"));
}

#[test]
fn scope_changes_cancel_confirmation_without_silently_reducing_it() {
    let a = AgentSessionBinding::new(
        horizon_core::PanelKind::Claude,
        "a".into(),
        Some("/sample/a".into()),
        None,
        None,
    );
    let b = AgentSessionBinding::new(
        horizon_core::PanelKind::Claude,
        "b".into(),
        Some("/sample/b".into()),
        None,
        None,
    );
    let mut state = SessionDeletionUi {
        selected: Arc::new(HashSet::from(["a".into(), "b".into()])),
        confirmation: Some(vec![a.clone(), b.clone()].into()),
        ..Default::default()
    };
    state.reconcile_options(std::slice::from_ref(&a));
    assert!(!state.confirming());
    assert_eq!(state.selected.as_ref(), &HashSet::from(["a".into()]));
    state.confirmation = Some(vec![b.clone()].into());
    let changed = AgentSessionBinding {
        cwd: Some("/sample/changed".into()),
        ..b
    };
    state.reconcile_options(&[a, changed]);
    assert!(!state.confirming());
}

#[test]
fn delete_all_confirmation_cancels_on_scope_growth_but_single_selection_remains_exact() {
    let a = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "a".into(), None, None, None);
    let b = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "b".into(), None, None, None);
    let mut state = SessionDeletionUi {
        confirmation: Some(vec![a.clone()].into()),
        confirmation_all: true,
        ..Default::default()
    };
    state.reconcile_options(std::slice::from_ref(&a));
    assert!(state.confirming());
    state.reconcile_options(&[a.clone(), b.clone()]);
    assert!(!state.confirming());
    state.confirmation_all = false;
    state.confirmation = Some(vec![a.clone()].into());
    state.reconcile_options(&[a.clone(), b]);
    assert_eq!(state.confirmation.as_deref(), Some(std::slice::from_ref(&a)));
}

#[test]
fn delete_all_confirmation_detects_new_identity_replacing_a_duplicate() {
    let a = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "a".into(), None, None, None);
    let b = AgentSessionBinding::new(horizon_core::PanelKind::Claude, "b".into(), None, None, None);
    let mut state = SessionDeletionUi {
        confirmation: Some(vec![a.clone(), a.clone()].into()),
        confirmation_all: true,
        ..Default::default()
    };
    state.reconcile_options(&[a, b]);
    assert!(!state.confirming());
}

#[test]
fn cleanup_warning_is_retained_separately_from_failed_deletions() {
    let ctx = egui::Context::default();
    let report = AgentSessionDeletionReport {
        deleted: vec![AgentSessionKey::new(horizon_core::PanelKind::Claude, "synthetic-id")],
        cleanup_warnings: vec![horizon_core::AgentSessionDeletionCleanupWarning {
            session_id: "synthetic-id".into(),
            directory: "/sample/recovery-bundle".into(),
            message: "Synthetic cleanup error".into(),
        }],
        ..Default::default()
    };
    ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
    let restored = SessionDeletionUi::restored(&ctx);
    assert_eq!(
        restored.message.as_deref(),
        Some("Deleted 1 conversation. 0 failed. 1 awaiting file cleanup.")
    );
    assert!(restored.details.is_empty());
    assert_eq!(restored.cleanup_details.len(), 1);
    assert!(restored.cleanup_details[0].contains("/sample/recovery-bundle"));
    assert!(restored.cleanup_details[0].contains("Synthetic cleanup error"));
}

#[test]
fn completion_report_survives_dismissal_and_reopening() {
    let ctx = egui::Context::default();
    let report = AgentSessionDeletionReport {
        failures: vec![horizon_core::AgentSessionDeletionFailure {
            session_id: "synthetic-id".into(),
            message: "Provider unavailable".into(),
        }],
        ..Default::default()
    };
    ctx.data_mut(|data| data.insert_temp(receipt_id(), Arc::new(report)));
    let restored = SessionDeletionUi::restored(&ctx);
    assert!(
        restored
            .message
            .as_deref()
            .is_some_and(|message| message.contains("1 failed"))
    );
    assert!(restored.details[0].contains("Provider unavailable"));
}

#[test]
fn management_controls_wrap_and_bulk_errors_remain_bounded() {
    let ctx = egui::Context::default();
    let options: Vec<_> = (0..16)
        .map(|index| {
            AgentSessionBinding::new(
                horizon_core::PanelKind::Codex,
                format!("synthetic-{index}"),
                None,
                None,
                None,
            )
        })
        .collect();
    let mut state = SessionDeletionUi {
        managing: true,
        selected: Arc::new(options.iter().map(|binding| binding.session_id.clone()).collect()),
        ..Default::default()
    };
    state.finish(&AgentSessionDeletionReport {
        failures: (0..500)
            .map(|index| horizon_core::AgentSessionDeletionFailure {
                session_id: format!("synthetic-{index}"),
                message: "Synthetic provider failure".into(),
            })
            .collect(),
        ..Default::default()
    });
    state.selected = Arc::new(options.iter().map(|binding| binding.session_id.clone()).collect());
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 700.0))),
                ..Default::default()
            },
            |ui| {
                ui.set_width(280.0);
                state.render_toolbar(ui, &options);
                assert!(ui.cursor().top() < 180.0, "failure summary stays bounded");
            },
        )
        .discard_textures();
    let text = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Delete selected (16)" => Some(text),
            _ => None,
        })
        .expect("delete action visible");
    assert!(
        text.pos.x + text.galley.size().x <= 310.0,
        "destructive action remains inside narrow viewport"
    );
}

#[test]
fn confirmation_cancel_does_not_enqueue_deletion() {
    let ctx = egui::Context::default();
    let mut state = SessionDeletionUi {
        confirmation: Some(
            vec![AgentSessionBinding::new(
                horizon_core::PanelKind::Codex,
                "synthetic-id".into(),
                None,
                None,
                None,
            )]
            .into(),
        ),
        ..Default::default()
    };
    let run = |state: &mut SessionDeletionUi, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))),
                events,
                ..Default::default()
            },
            |ui| {
                assert!(state.render_confirmation(ui).is_none());
            },
        )
        .discard_textures()
    };
    let output = run(&mut state, Vec::new());
    let at = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Cancel" => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("cancel visible");
    for pressed in [true, false] {
        run(
            &mut state,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(!state.confirming());
    assert!(take_deletion_request(&ctx).is_none());
}

#[test]
fn bulk_confirmation_names_every_folder_and_unknown_scope() {
    let ctx = egui::Context::default();
    let mut sessions: Vec<_> = [Some("/sample/a"), Some("/sample/b"), None]
        .into_iter()
        .enumerate()
        .map(|(i, cwd)| {
            AgentSessionBinding::new(
                horizon_core::PanelKind::Codex,
                format!("synthetic-{i}"),
                cwd.map(str::to_owned),
                None,
                None,
            )
        })
        .collect();
    let a = sessions[0].cwd.clone().expect("folder a");
    let b = sessions[1].cwd.clone().expect("folder b");
    let mut state = SessionDeletionUi {
        confirmation: Some(std::mem::take(&mut sessions).into()),
        ..Default::default()
    };
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 700.0))),
                ..Default::default()
            },
            |ui| {
                assert!(state.render_confirmation(ui).is_none());
            },
        )
        .discard_textures();
    let text: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains(&"Folders: 2 recorded folders"));
    assert!(text.contains(&a.as_str()));
    assert!(text.contains(&b.as_str()));
    assert!(text.contains(&"Conversations with no recorded folder are also included."));
    assert!(!text.iter().any(|text| text.starts_with("Folder: ")));
}
