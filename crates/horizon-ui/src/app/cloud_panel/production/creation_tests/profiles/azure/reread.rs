use super::*;
use crate::app::cloud_panel::production::creation_tests::local_profiles::finish_read;

const AZURE_PROFILE: &str = "version: 1\ndefault: development\nprofiles:\n  development:\n    provider: azure\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 16\n";

#[test]
fn a_profile_provider_edit_replaces_automatic_azure_placement() {
    reread_provider(false);
}

#[test]
fn a_profile_provider_edit_keeps_an_explicit_azure_choice() {
    reread_provider(true);
}

fn reread_provider(explicit: bool) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    prepare(&mut app, &ctx, temp.path());
    let config_path = temp.path().join(".horizon/cloud.yml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, AZURE_PROFILE).unwrap();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    azure_binding(&mut app);
    let output = tall_frame(&ctx, &mut app);
    if explicit {
        click(&ctx, &mut app, azure_entry(&output).center());
    }
    assert_eq!(app.cloud_prototype.production.placement.cpu_types, ["Standard_D4s_v3"]);

    // The file watcher applies this change quietly while the same dialog remains open.
    std::fs::write(
        &config_path,
        AZURE_PROFILE.replace("provider: azure", "provider: runpod"),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_cloud_launch(&ctx);
        let form = &app.cloud_prototype.production;
        if form
            .profiles
            .as_ref()
            .and_then(|config| config.profiles.get(&form.selected_profile))
            .is_some_and(|profile| profile.provider == "runpod")
        {
            break;
        }
        assert!(Instant::now() < deadline, "the dialog did not read the provider edit");
        std::thread::sleep(Duration::from_millis(20));
    }
    let form = &app.cloud_prototype.production;
    let expected = if explicit { "azure" } else { "runpod" };
    assert_eq!(
        provider::current(
            form.provider,
            form.profiles
                .as_ref()
                .and_then(|config| config.profiles.get(&form.selected_profile))
                .unwrap()
        )
        .id,
        expected
    );
    if explicit {
        assert_eq!(form.provider, Some(&horizon_core::cloud_runtime::provider::AZURE));
        assert_eq!(form.placement.cpu_types, ["Standard_D4s_v3"]);
        assert_eq!(form.placement.data_centers, ["northeurope"]);
    } else {
        assert!(
            form.provider.is_none(),
            "automatic placement is not a provider override"
        );
        assert!(
            form.placement.cpu_types.is_empty(),
            "the old provider's VM size is cleared"
        );
        assert!(
            form.placement.data_centers.is_empty(),
            "the old provider's region is cleared"
        );
    }

    app.cloud_prototype.production.launch.accounts_checked = true;
    let output = tall_frame(&ctx, &mut app);
    click(&ctx, &mut app, label_rect(&output, "Start cloud").center());
    finish_creation(&ctx, &mut app);
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile.provider, expected);
    if explicit {
        assert_eq!(launch.placement.cpu_types, ["Standard_D4s_v3"]);
        assert_eq!(launch.placement.data_centers, ["northeurope"]);
    } else {
        assert!(launch.placement.cpu_types.is_empty());
        assert!(launch.placement.data_centers.is_empty());
    }
}
