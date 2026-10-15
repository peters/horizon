use super::*;
use crate::app::tailnets::State as TailnetsState;
use crate::test_egui::DiscardTextures;

fn catalog() -> Catalog {
    Catalog {
        tailnets: vec![tailnet::Tailnet {
            id: "work".into(),
            name: "Synthetic".into(),
        }],
    }
}

fn pending_settings(write: Receiver<Reply>, stale_catalog: Receiver<Result<Catalog, String>>) -> TailnetsState {
    TailnetsState {
        root: "synthetic-root-not-opened".into(),
        catalog: catalog(),
        catalog_loaded: true,
        initialized: true,
        receiver: Some(stale_catalog),
        api_credentials: State {
            ids: ["work".into()].into(),
            statuses: [(
                "work".into(),
                Ok(ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)),
            )]
            .into(),
            receiver: Some(Pending {
                kind: TaskKind::Write,
                receiver: write,
            }),
            message: Some("Independent reader failure".into()),
            failed: true,
            ..State::default()
        },
        ..TailnetsState::default()
    }
}

#[test]
fn close_reopen_keeps_the_writer_and_its_result_authoritative_until_fresh_status() {
    for succeeds in [false, true] {
        for complete_while_closed in [false, true] {
            let ctx = egui::Context::default();
            let (write, pending_write) = channel();
            let (stale_catalog, pending_catalog) = channel();
            let mut editor = pending_settings(pending_write, pending_catalog);
            let root = editor.root.clone();
            let mut production = TailnetsState::for_root_fixture(root.clone());

            for _ in 0..2 {
                production.observe_settings(&mut editor, &ctx);
                production.reload_after(editor);
                let mut reopened = TailnetsState::for_root_fixture(root.clone());
                production.reopen_settings(&mut reopened);
                assert!(reopened.api_credentials.busy());
                assert_eq!(reopened.api_credentials.barriers.len(), 1);
                assert_eq!(reopened.reload_barriers.len(), 1);
                assert_eq!(production.catalog.tailnets[0].id, "work");
                assert!(production.catalog_loaded);
                assert!(!production.ready(), "The passive view cannot bypass an owner barrier");
                assert!(reopened.selections.is_empty());
                production.poll(&ctx);
                production.refresh(&ctx);
                assert!(production.receiver.is_none());
                assert!(production.api_credentials.receiver.is_none());
                assert_eq!(production.api_credential_observation("work"), Observation::Pending);
                assert!(reopened.receiver.is_none(), "No reader can pass the pending writer");
                editor = reopened;
            }

            let final_status = if succeeds {
                ApiCredentialStatus::Missing
            } else {
                ApiCredentialStatus::Stored(ApiCredentialKind::PersonalToken)
            };
            let reply = if succeeds {
                Ok(vec![("work".into(), Ok(final_status))])
            } else {
                Err("Synthetic removal failed".into())
            };
            let mut editor = if complete_while_closed {
                production.reload_after(editor);
                write.send(reply).unwrap();
                production.poll(&ctx);
                let mut reopened = TailnetsState::for_root_fixture(root.clone());
                production.reopen_settings(&mut reopened);
                reopened
            } else {
                write.send(reply).unwrap();
                editor
            };
            production.observe_settings(&mut editor, &ctx);
            assert!(!editor.api_credentials.busy());
            assert_eq!(production.api_credential_observation("work"), Observation::Pending);
            assert_eq!(
                editor.api_credentials.write_error(),
                (!succeeds).then_some("Synthetic removal failed")
            );
            assert_eq!(editor.api_credentials.read_error(), Some("Independent reader failure"));

            // Supply the post-barrier read result without starting filesystem/keychain IO.
            let (read, pending_read) = queued(TaskKind::Read);
            editor.initialized = true;
            editor.api_credentials.ids = ["work".into()].into();
            editor.api_credentials.receiver = Some(pending_read);
            stale_catalog.send(Ok(Catalog::default())).unwrap();
            production.observe_settings(&mut editor, &ctx);
            assert!(editor.reload_barriers.is_empty());
            assert_eq!(production.cached_catalog_at(&root).unwrap().tailnets.len(), 1);
            assert_eq!(production.api_credential_observation("work"), Observation::Pending);
            read.send(Ok(vec![("work".into(), Ok(final_status))])).unwrap();
            production.observe_settings(&mut editor, &ctx);
            assert_eq!(
                production.api_credential_observation("work"),
                Observation::Ready(final_status)
            );
            assert!(editor.api_credentials.read_error().is_none());
            assert_eq!(
                editor.api_credentials.write_error(),
                (!succeeds).then_some("Synthetic removal failed"),
                "Successful read recovery cannot hide a write failure"
            );
            production.reload_after(editor);
            let mut reopened = TailnetsState::for_root_fixture(root);
            production.reopen_settings(&mut reopened);
            assert_eq!(
                reopened.api_credentials.write_error(),
                (!succeeds).then_some("Synthetic removal failed")
            );
        }
    }
}

#[test]
fn reopen_keeps_the_production_catalog_and_selected_tailnet_available_without_io() {
    let ctx = egui::Context::default();
    let root = PathBuf::from("synthetic-root-not-opened");
    let mut production = TailnetsState {
        root: root.clone(),
        catalog: catalog(),
        catalog_loaded: true,
        initialized: true,
        settings_error: Some("Independent settings failure".into()),
        ..TailnetsState::default()
    };
    production.selections.insert("cloud".into(), Some("work".into()));
    let mut editor = TailnetsState::for_root_fixture(root.clone());
    production.reopen_settings(&mut editor);
    assert_eq!(production.catalog.tailnets[0].name, "Synthetic");
    assert!(production.catalog_loaded);
    assert!(production.initialized);
    assert!(production.ready());
    assert_eq!(
        production.settings_error.as_deref(),
        Some("Independent settings failure")
    );
    assert_eq!(production.selections["cloud"].as_deref(), Some("work"));
    let mut selected = production.selections["cloud"].clone();
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            assert!(!production.choice(ui, &mut selected));
        })
        .discard_textures();
    assert_eq!(selected.as_deref(), Some("work"));
    production.refresh(&ctx);
    assert!(production.receiver.is_none());
    assert!(production.api_credentials.receiver.is_none());
    assert_eq!(production.cached_catalog_at(&root).unwrap().tailnets[0].id, "work");
    editor.catalog.tailnets.clear();
    editor.catalog_revision += 1;
    production.observe_settings(&mut editor, &ctx);
    assert!(
        production.catalog.tailnets.is_empty(),
        "An explicit empty catalog removes old rows"
    );
}

#[test]
fn cloud_selection_buttons_cannot_start_a_save_on_the_passive_production_state() {
    let root = PathBuf::from("synthetic-root-not-opened");
    let mut production = TailnetsState {
        root: root.clone(),
        catalog: catalog(),
        catalog_loaded: true,
        initialized: true,
        ..TailnetsState::default()
    };
    production.selections.insert("cloud".into(), Some("work".into()));
    let buttons = |state: &mut TailnetsState| {
        crate::test_egui::accesskit_labels(|ui| state.cloud(ui, &root, "cloud", false))
            .into_iter()
            .filter(|(label, _)| matches!(label.as_str(), "None" | "Synthetic"))
            .collect::<Vec<_>>()
    };
    let active = buttons(&mut production);
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|(_, disabled)| !disabled));

    let mut editor = TailnetsState::for_root_fixture(root.clone());
    production.reopen_settings(&mut editor);
    assert!(production.ready(), "The catalog remains available to read");
    let passive = buttons(&mut production);
    assert_eq!(passive.len(), 2);
    assert!(passive.iter().all(|(_, disabled)| *disabled));
    assert_eq!(production.selections["cloud"].as_deref(), Some("work"));
    assert!(production.receiver.is_none(), "No competing selection writer starts");
    assert!(editor.receiver.is_none());
}

#[test]
fn root_drift_refuses_the_mirror_without_adopting_or_dropping_the_old_writer() {
    let ctx = egui::Context::default();
    let (write, pending_write) = channel();
    let (_catalog, pending_catalog) = channel();
    let editor = pending_settings(pending_write, pending_catalog);
    let old_root = editor.root.clone();
    let mut production = TailnetsState::for_root_fixture(old_root.clone());
    production.reload_after(editor);
    let new_root = PathBuf::from("different-root-not-opened");
    let mut other = TailnetsState::for_root_fixture(new_root.clone());
    production.reopen_settings(&mut other);
    assert!(production.api_credentials.busy());
    assert!(!other.api_credentials.busy());

    let mut reopened = TailnetsState::for_root_fixture(old_root.clone());
    production.reopen_settings(&mut reopened);
    reopened.api_credentials.refresh = Some((old_root.clone(), vec!["work".into()]));
    production.root = new_root.clone();
    assert!(production.cached_catalog_at(&new_root).is_none());
    assert!(production.cached_catalog_at(&old_root).is_none());
    assert_eq!(production.api_credential_observation("work"), Observation::Unavailable);
    production.refresh(&ctx);
    production.poll(&ctx);
    assert!(production.receiver.is_none());
    production.observe_settings(&mut reopened, &ctx);
    assert!(reopened.api_credentials.busy());
    assert!(reopened.settings_scope_changed());
    assert_eq!(reopened.root, old_root);
    assert!(production.cached_catalog_at(&new_root).is_none());
    reopened.refresh(&ctx);
    reopened.poll(&ctx);
    assert!(reopened.receiver.is_none());
    assert!(reopened.api_credentials.refresh.is_none());
    production.root = old_root.clone();
    production.observe_settings(&mut reopened, &ctx);
    assert!(production.cached_catalog_at(&old_root).is_none());
    assert_eq!(production.api_credential_observation("work"), Observation::Unavailable);
    assert!(!production.ready());
    write.send(Err("Original root removal failed".into())).unwrap();
    production.observe_settings(&mut reopened, &ctx);
    assert_eq!(
        reopened.api_credentials.write_error(),
        Some("Original root removal failed")
    );
    production.root = new_root.clone();
    production.reload_after(reopened);
    assert_eq!(production.root, new_root);
    assert!(!production.initialized, "Only the current root may reload after close");
    assert!(production.api_credentials.write_error().is_none());
    assert!(production.cached_catalog_at(&old_root).is_none());
}
