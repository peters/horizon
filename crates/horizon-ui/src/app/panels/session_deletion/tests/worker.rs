use super::*;

#[test]
fn frame_lifecycle_finishes_deletion_without_renderable_panels() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    assert!(app.board.panels.is_empty());
    let binding = AgentSessionBinding::new(
        horizon_core::PanelKind::Codex,
        "01a0f8a8-d7e2-7810-9e6b-999999999999".into(),
        None,
        None,
        None,
    );
    let key = AgentSessionKey::new(binding.kind, &binding.session_id);
    let reservation = horizon_core::reserve_saved_session_deletions(std::slice::from_ref(&binding)).expect("reserve");
    ctx.data_mut(|data| {
        data.insert_temp(
            job_id(),
            DeletionJob {
                owner: PanelId(42),
                viewport: egui::ViewportId::ROOT,
                reservation: Arc::new(reservation),
                state: Arc::new(Mutex::new(DeletionProgress {
                    finished: true,
                    done: 1,
                    total: 1,
                    report: AgentSessionDeletionReport {
                        deleted: vec![key.clone()],
                        ..Default::default()
                    },
                })),
            },
        )
    });
    assert!(horizon_core::saved_session_deletion_pending(
        binding.kind,
        &binding.session_id
    ));
    let _ = ctx
        .run_ui(egui::RawInput::default(), |_| {
            app.process_frame_inputs(&ctx);
        })
        .discard_textures();
    assert!(deletion_progress(&ctx).is_none());
    assert!(!horizon_core::saved_session_deletion_pending(
        binding.kind,
        &binding.session_id
    ));
    let report = ctx
        .data(|data| data.get_temp::<Arc<AgentSessionDeletionReport>>(receipt_id()))
        .expect("receipt");
    assert_eq!(report.deleted, vec![key]);
    assert!(app.board.panels.is_empty());
}
