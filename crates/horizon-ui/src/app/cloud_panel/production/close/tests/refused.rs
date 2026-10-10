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

#[cfg(unix)]
fn current_offer(app: &HorizonApp) -> offer::Offer {
    let close = &app.cloud_prototype.production.close;
    offer::offer(
        &app.cloud_prototype.production.runtimes[&101],
        true,
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

    app.discard_cloud(0, &ctx);
    let close = &app.cloud_prototype.production.close;
    assert!(close.deleting.is_empty() && close.hidden.is_empty());
    app.finish_closing_clouds(&ctx);
    assert!(
        !app.cloud_close_confirmation_open(),
        "no dialog for a cloud that is gone"
    );
    assert!(app.cloud_prototype.production.close.failed.is_empty());
}

#[test]
fn a_busy_cloud_removed_anyway_forgets_the_failure() {
    let (temp, mut app) = test_app();
    let (_panel, _sender) = closing_cloud_with_panel(&mut app, temp.path());
    let close = &mut app.cloud_prototype.production.close;
    close.confirming = Some(101);
    close
        .failed
        .insert(101, Failure::new("Could not delete the cloud resources", true, None));
    app.remove_cloud_anyway(101, &egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert!(!app.cloud_close_confirmation_open());
    assert!(app.cloud_prototype.production.close.failed.is_empty());
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
