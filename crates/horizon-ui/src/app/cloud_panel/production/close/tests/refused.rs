//! A close that stopped short asks again with what the attempt itself reported, and
//! leaves nothing behind once its cloud is gone.
use super::*;

/// A cloud whose image push failed before any worker was requested.
#[cfg(unix)]
fn unrequested_cloud(app: &mut HorizonApp, root: &std::path::Path) -> std::path::PathBuf {
    let mut state = deployment();
    state.stage = Stage::Push;
    state.operation = CreateState::Prepared;
    state.spec = None;
    let path = root.join("fixture");
    Store::lock(&path).unwrap().save(&state).unwrap();
    add_cloud(app, root, Some(state));
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().error = Some("error from registry: denied".into());
    path
}

/// What the dialog of cloud 101 offers now, as it renders it.
fn current_offer(app: &HorizonApp) -> offer::Offer {
    let close = &app.cloud_prototype.production.close;
    let launch = app.cloud_prototype.groups.0[0].remote.as_ref().unwrap();
    offer::offer(
        &app.cloud_prototype.production.runtimes[&101],
        launch.deployment_started,
        close.records.get(&101).unwrap_or(&offer::Record::Holds),
        close.failed.get(&101),
    )
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn storage_on_disk_offers_its_deletion_before_removing_anyway() {
    let (temp, mut app) = test_app();
    let path = unrequested_cloud(&mut app, temp.path());
    std::fs::write(path.join("workspace-volume.required"), "").unwrap();
    app.request_cloud_close(101);
    assert_eq!(
        current_offer(&app),
        offer::Offer {
            primary: Some(Primary::Delete),
            remove_anyway: false,
            reason: None,
        },
        "the dialog must not claim nothing is at the provider"
    );

    // No settings.json: the deletion cannot start, and only now may the cloud leave anyway.
    let ctx = egui::Context::default();
    app.delete_for_close(101, &ctx);
    let offer = current_offer(&app);
    assert!(offer.remove_anyway);
    assert!(
        offer
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("Could not delete the cloud resources")),
        "{offer:?}"
    );
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn a_refused_removal_offers_the_deletion_first() {
    let (temp, mut app) = test_app();
    let path = unrequested_cloud(&mut app, temp.path());
    app.request_cloud_close(101);
    assert_eq!(current_offer(&app).primary, Some(Primary::Remove));

    // Storage recorded after the dialog opened refuses the removal.
    std::fs::write(path.join("workspace-volume.required"), "").unwrap();
    app.remove_for_close(101, &egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert_eq!(
        current_offer(&app),
        offer::Offer {
            primary: Some(Primary::Delete),
            remove_anyway: false,
            reason: Some(
                "Could not remove the cloud: Delete the worker and workspace storage before removing this cloud".into()
            ),
        },
        "no deletion was tried, so it is offered before removing anyway"
    );
}

#[test]
fn a_close_step_that_reports_nothing_does_not_show_an_earlier_error() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.cloud_prototype.production.runtimes.get_mut(&101).unwrap().error = Some("an earlier failure".into());
    app.request_cloud_close(101);
    // Without a cloud root the deletion stops before it starts, reporting nothing.
    app.cloud_prototype.root = None;
    app.delete_for_close(101, &egui::Context::default());
    assert_eq!(
        app.cloud_prototype.production.close.failed[&101].reason,
        "Could not delete the cloud resources."
    );
    assert_eq!(
        app.cloud_prototype.production.runtimes[&101].error.as_deref(),
        Some("an earlier failure"),
        "the card keeps showing it"
    );
}

#[test]
fn a_discarded_cloud_leaves_no_close_behind() {
    let (temp, mut app) = test_app();
    let (_panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let ctx = egui::Context::default();
    app.finish_closing_clouds(&ctx);
    assert!(!app.cloud_prototype.production.close.hidden.is_empty());

    app.request_cloud_close(101);
    app.discard_cloud(0, &ctx);
    let close = &app.cloud_prototype.production.close;
    assert!(close.deleting.is_empty() && close.hidden.is_empty());
    assert!(close.confirming.is_none() && close.records.is_empty());
    app.finish_closing_clouds(&ctx);
    assert!(
        !app.cloud_close_confirmation_open(),
        "no dialog for a cloud that is gone"
    );
    assert!(app.cloud_prototype.production.close.failed.is_empty());
}

#[test]
fn an_operation_started_while_the_dialog_was_open_keeps_it_open() {
    let (temp, mut app) = test_app();
    let (_panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let close = &mut app.cloud_prototype.production.close;
    close.confirming = Some(101);
    close
        .failed
        .insert(101, Failure::new("Could not delete the cloud resources", true, None));
    let ctx = egui::Context::default();
    for action in [
        HorizonApp::remove_cloud_anyway,
        HorizonApp::remove_for_close,
        HorizonApp::delete_for_close,
    ] {
        action(&mut app, 101, &ctx);
        assert!(app.cloud_close_confirmation_open(), "the dialog shows the operation");
        assert!(app.cloud_prototype.production.close.failed[&101].deletion_tried);
    }
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert_eq!(
        current_offer(&app),
        offer::Offer {
            primary: None,
            remove_anyway: false,
            reason: Some(offer::BUSY.into()),
        }
    );
}

#[test]
fn a_cloud_without_a_launch_ends_its_close_dialog() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.cloud_prototype.groups.0[0].remote = None;
    app.request_cloud_close(101);
    let ctx = egui::Context::default();
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            app.render_cloud_close_confirmation(ui.ctx());
        })
        .discard_textures();
    assert!(!app.cloud_close_confirmation_open(), "no dialog is left open unseen");
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn an_unreadable_record_offers_only_remove_anyway() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    Store::lock(&path).unwrap().save(&deployment()).unwrap();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    std::fs::write(path.join("deployment.json"), "not a record").unwrap();
    app.request_cloud_close(101);
    let offer = current_offer(&app);
    assert_eq!((offer.primary, offer.remove_anyway), (None, true), "{offer:?}");
    assert!(
        offer
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(offer::STATE_UNKNOWN)),
        "{offer:?}"
    );
}

#[test]
fn a_cloud_that_never_deployed_is_removed_on_every_host() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), None);
    app.request_cloud_close(101);
    assert_eq!(current_offer(&app).primary, Some(Primary::Remove));

    // Also where Horizon keeps no cloud records, so no lock can be taken.
    app.remove_for_close(101, &egui::Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.production.close.confirming.is_none());
}

#[test]
fn a_cloud_whose_identity_names_no_record_can_be_removed_anyway() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.cloud_prototype.groups.0[0].remote.as_mut().unwrap().id = "../outside".into();
    app.request_cloud_close(101);
    let offer = current_offer(&app);
    assert_eq!((offer.primary, offer.remove_anyway), (None, true), "{offer:?}");

    app.remove_cloud_anyway(101, &egui::Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty(), "no record can hold its lock");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn another_controller_keeps_the_cloud_from_being_removed_anyway() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    let other = Store::lock(&path).unwrap();
    other.save(&deployment()).unwrap();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.request_cloud_close(101);
    let busy = offer::Offer {
        primary: None,
        remove_anyway: false,
        reason: Some(offer::BUSY.into()),
    };
    assert_eq!(
        current_offer(&app),
        busy,
        "an operation runs in another Horizon instance"
    );
    let ctx = egui::Context::default();
    let render = |app: &mut HorizonApp| {
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                app.render_cloud_close_confirmation(ui.ctx());
            })
            .discard_textures();
    };
    render(&mut app);
    assert_eq!(current_offer(&app), busy, "still running");
    drop(other);
    render(&mut app);
    assert_eq!(current_offer(&app), busy, "read again at most once a second");
    app.cloud_prototype.production.close.record_read = None;
    render(&mut app);
    assert_eq!(
        current_offer(&app).primary,
        Some(Primary::Delete),
        "the open dialog offers the deletion once that operation ended"
    );

    // An operation starts after the dialog opened.
    let other = Store::lock(&path).unwrap();
    app.delete_for_close(101, &ctx);
    assert_eq!(current_offer(&app), busy, "the deletion found another controller");

    app.remove_cloud_anyway(101, &ctx);
    assert_eq!(app.cloud_prototype.groups.0.len(), 1, "its operation still runs");
    assert!(app.cloud_close_confirmation_open(), "the dialog asks again");
    let failure = &app.cloud_prototype.production.close.failed[&101];
    assert!(failure.deletion_tried, "the deletion attempt stays recorded");
    assert_eq!(
        failure.reason,
        "Could not remove the cloud: Another controller owns this cloud operation"
    );
    assert_eq!(current_offer(&app), busy, "the refused removal preserves Cancel-only");

    drop(other);
    app.cloud_prototype.production.close.record_read = None;
    render(&mut app);
    assert!(current_offer(&app).remove_anyway, "the operation ended");
    app.remove_cloud_anyway(101, &ctx);
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(path.exists(), "its record directory stays");
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn an_operation_that_another_instance_starts_while_the_dialog_is_open_makes_it_wait() {
    let (temp, mut app) = test_app();
    Store::lock(&temp.path().join("fixture"))
        .unwrap()
        .save(&deployment())
        .unwrap();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.request_cloud_close(101);
    assert_eq!(current_offer(&app).primary, Some(Primary::Delete));
    let ctx = egui::Context::default();
    let render = |app: &mut HorizonApp| {
        app.cloud_prototype.production.close.record_read = None; // A second later.
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                app.render_cloud_close_confirmation(ui.ctx());
            })
            .discard_textures();
    };

    let other = Store::lock(&temp.path().join("fixture")).unwrap();
    render(&mut app);
    assert_eq!(current_offer(&app).reason.as_deref(), Some(offer::BUSY));
    assert_eq!(
        (current_offer(&app).primary, current_offer(&app).remove_anyway),
        (None, false)
    );
    drop(other);
    render(&mut app);
    assert_eq!(
        current_offer(&app).primary,
        Some(Primary::Delete),
        "the operation ended"
    );
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn removing_anyway_preserves_the_operation_lock_identity() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    app.request_cloud_close(101);
    app.remove_cloud_anyway(101, &egui::Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty());
    let path = temp.path().join("fixture");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join("operation.lock"))
        .unwrap();
    let next = Store::lock(&path).unwrap();
    assert!(lock.try_lock().is_err(), "all controllers must use the same lock file");
    drop(next);
    assert!(lock.try_lock().is_ok());
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn a_record_that_cannot_be_locked_keeps_the_cloud() {
    let (temp, mut app) = test_app();
    // A directory where the lock file goes: the record cannot be locked.
    let lock = temp.path().join("fixture").join("operation.lock");
    std::fs::create_dir_all(&lock).unwrap();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    let close = &mut app.cloud_prototype.production.close;
    close.confirming = Some(101);
    close
        .failed
        .insert(101, Failure::new("Could not delete the cloud resources", true, None));
    app.remove_cloud_anyway(101, &egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1, "the cloud stays");
    assert!(app.cloud_close_confirmation_open(), "the dialog asks again");
    let failure = &app.cloud_prototype.production.close.failed[&101];
    assert!(failure.deletion_tried, "removing anyway stays offered");
    assert!(
        failure
            .reason
            .starts_with("Could not remove the cloud: Cloud state I/O failed"),
        "{failure:?}"
    );
    assert!(lock.is_dir(), "the record stays as it was");
}

#[test]
#[cfg(unix)] // Durable cloud records require Unix directory durability.
fn a_storage_journal_without_its_deployment_offers_the_deletion() {
    let (temp, mut app) = test_app();
    let path = temp.path().join("fixture");
    drop(Store::lock(&path).unwrap());
    std::fs::write(path.join("workspace-volume.required"), "").unwrap();
    add_cloud(&mut app, temp.path(), None);
    app.request_cloud_close(101);
    assert_eq!(
        current_offer(&app),
        offer::Offer {
            primary: Some(Primary::Delete),
            remove_anyway: false,
            reason: None,
        },
        "the dialog must not claim nothing is at the provider"
    );

    app.remove_for_close(101, &egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1, "the storage may remain");
}

#[test]
fn a_never_deployed_cloud_without_a_state_root_is_removed_directly() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), None);
    app.cloud_prototype.root = None;
    app.request_cloud_close(101);
    assert_eq!(current_offer(&app).primary, Some(Primary::Remove));
    app.remove_for_close(101, &egui::Context::default());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn a_deployed_cloud_without_a_record_is_unknown_and_retained() {
    let (temp, mut app) = test_app();
    add_cloud(&mut app, temp.path(), Some(deployment()));
    for (missing_root, directory_exists) in [(true, false), (false, false), (false, true)] {
        if directory_exists {
            std::fs::create_dir_all(temp.path().join("fixture")).unwrap();
        }
        app.cloud_prototype.root = (!missing_root).then(|| temp.path().into());
        app.request_cloud_close(101);
        assert!(matches!(
            app.cloud_prototype.production.close.records[&101],
            Record::Unreadable(_)
        ));
        let offered = current_offer(&app);
        assert_eq!(offered.primary, None);
        assert!(offered.remove_anyway);
        assert!(!app.remove_deleted_cloud(101, &egui::Context::default()));
        assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    }
}
