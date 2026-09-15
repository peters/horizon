use std::collections::BTreeMap;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, DeviceRequirement, RemoteAdapterKind,
    RemoteAuthentication, RemoteBrowserConfig, RemoteConfigError, RemoteProviderProfile, RemoteSessionLimits,
    RemoteTargetProfile,
};

use super::{
    MAX_PORTABLE_PROFILE_BYTES, RemoteProfileError, export_portable, export_portable_file_from_config, import_portable,
    import_portable_file_into_config, parse_portable, read_portable_profile, refuse_config_path, summary_line,
    write_portable_profile,
};
use crate::config::Config;
use crate::config_migration::CURRENT_CONFIG_VERSION;

fn provider(endpoint: &str) -> RemoteProviderProfile {
    let mut bindings = BTreeMap::new();
    bindings.insert(
        CredentialReference::from("key"),
        CredentialBinding {
            store: CredentialStoreKind::OsKeychain,
            slot: Some("remote-browser/grid/key".to_string()),
        },
    );
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: ControlEndpoint::parse(endpoint).expect("endpoint"),
        authentication: RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from("key"),
        },
        credential_bindings: bindings,
        limits: RemoteSessionLimits::default(),
    }
}

fn local() -> RemoteBrowserConfig {
    let mut providers = BTreeMap::new();
    providers.insert("grid".to_string(), provider("https://grid.example.net/wd/hub"));
    let mut targets = BTreeMap::new();
    targets.insert(
        "ios_phone".to_string(),
        RemoteTargetProfile {
            provider: "grid".to_string(),
            browser_name: "safari".to_string(),
            platform_name: "iOS".to_string(),
            device: DeviceRequirement::default(),
            capability_extensions: BTreeMap::new(),
        },
    );
    RemoteBrowserConfig { providers, targets }
}

fn redirected() -> RemoteBrowserConfig {
    let mut redirected = local();
    redirected.providers.get_mut("grid").expect("grid").endpoint =
        ControlEndpoint::parse("https://elsewhere.example.net/wd/hub").expect("endpoint");
    redirected
}

#[test]
fn export_strips_bindings_and_import_restores_the_definition_elsewhere() {
    let document = export_portable(&local()).expect("export");
    assert!(
        document.starts_with("horizon_remote_browser_profile: 1\n"),
        "{document}"
    );
    assert!(!document.contains("credential_bindings"), "{document}");
    assert!(!document.contains("remote-browser/grid/key"), "{document}");

    let mut second_computer = RemoteBrowserConfig::default();
    let summary = import_portable(&mut second_computer, &document).expect("import");
    assert_eq!(summary.providers_added, vec!["grid".to_string()]);
    assert_eq!(summary.targets_added, vec!["ios_phone".to_string()]);
    assert_eq!(
        summary_line(&summary),
        "added 1 provider(s) and 1 target(s), updated 0 provider(s) and 0 target(s)"
    );
    let imported = &second_computer.providers["grid"];
    assert!(
        imported.credential_bindings.is_empty(),
        "bindings are entered on the second computer"
    );
    assert_eq!(imported.endpoint, local().providers["grid"].endpoint);
    assert_eq!(
        imported.authentication.references(),
        local().providers["grid"].authentication.references()
    );
}

#[test]
fn an_empty_definition_has_nothing_to_export() {
    assert!(matches!(
        export_portable(&RemoteBrowserConfig::default()),
        Err(RemoteProfileError::NothingToExport)
    ));
}

#[test]
fn documents_that_are_not_portable_profiles_are_refused_without_touching_the_target() {
    let mut target = local();
    let before = target.clone();
    let refused = [
        ("horizon_remote_browser_profile: 2\nremote: {}\n", "format"),
        ("remote: {}\n", "parse"),
        ("horizon_remote_browser_profile: 1\nremote: {}\nsecret: x\n", "parse"),
        ("browser:\n  remote: {}\n", "parse"),
    ];
    for (document, expected) in refused {
        let error = import_portable(&mut target, document).expect_err(document);
        let actual = match error {
            RemoteProfileError::UnsupportedFormat { found: 2 } => "format",
            RemoteProfileError::Parse(_) => "parse",
            _ => "other",
        };
        assert_eq!(actual, expected, "{document}: {error}");
    }
    assert_eq!(target, before);
}

#[test]
fn a_document_that_carries_bindings_or_redirects_an_endpoint_is_refused() {
    let with_bindings = "horizon_remote_browser_profile: 1\nremote:\n  providers:\n    grid:\n      adapter: webdriver\n      endpoint: https://grid.example.net/wd/hub\n      authentication: { kind: bearer, token_ref: key }\n      credential_bindings:\n        key: { store: session }\n";
    let mut target = RemoteBrowserConfig::default();
    let error = import_portable(&mut target, with_bindings).expect_err("bindings");
    assert!(
        matches!(
            error,
            RemoteProfileError::Config(RemoteConfigError::ImportCarriesBindings { .. })
        ),
        "{error}"
    );
    assert!(target.is_empty());

    let mut trusted = local();
    let document = export_portable(&redirected()).expect("export");
    let error = import_portable(&mut trusted, &document).expect_err("endpoint conflict");
    assert!(
        matches!(
            error,
            RemoteProfileError::Config(RemoteConfigError::ImportEndpointConflict { .. })
        ),
        "{error}"
    );
    assert_eq!(trusted, local());
}

#[test]
fn oversized_documents_are_refused_before_parsing() {
    let padding = "#".repeat(usize::try_from(MAX_PORTABLE_PROFILE_BYTES).expect("usize") + 1);
    assert!(matches!(
        parse_portable(&padding),
        Err(RemoteProfileError::TooLarge { .. })
    ));
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("big.yaml");
    std::fs::write(&path, padding).expect("write");
    assert!(matches!(
        read_portable_profile(&path),
        Err(RemoteProfileError::TooLarge { .. })
    ));
}

#[test]
fn profile_files_round_trip_through_a_second_configuration_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile_path = dir.path().join("profiles").join("remote-browser-profile.yaml");
    write_portable_profile(&profile_path, &local()).expect("write");
    let staged_left_behind = std::fs::read_dir(profile_path.parent().expect("parent"))
        .expect("dir")
        .any(|entry| entry.expect("entry").file_name().to_string_lossy().ends_with(".tmp"));
    assert!(!staged_left_behind);

    let config_path = dir.path().join("second").join("config.yaml");
    let summary = import_portable_file_into_config(&config_path, &profile_path).expect("import");
    assert_eq!(summary.providers_added, vec!["grid".to_string()]);
    let written = std::fs::read_to_string(&config_path).expect("config");
    assert!(!written.contains("credential_bindings"), "{written}");
    let loaded = Config::load(Some(&config_path)).expect("load");
    assert_eq!(loaded.browser.remote.targets["ios_phone"].provider, "grid");

    // Importing again updates rather than duplicates, and the rest of the
    // file survives the rewrite.
    let again = import_portable_file_into_config(&config_path, &profile_path).expect("re-import");
    assert_eq!(again.providers_updated, vec!["grid".to_string()]);
    assert_eq!(again.targets_updated, vec!["ios_phone".to_string()]);
    let reloaded = Config::load(Some(&config_path)).expect("reload");
    assert!((reloaded.window.width - Config::default().window.width).abs() < f32::EPSILON);

    let exported_path = dir.path().join("second").join("out.yaml");
    let exported = export_portable_file_from_config(&config_path, &exported_path).expect("export");
    assert!(exported.providers["grid"].credential_bindings.is_empty());
    assert_eq!(
        read_portable_profile(&exported_path).expect("read"),
        export_portable(&loaded.browser.remote).expect("document")
    );
}

#[test]
fn an_older_configuration_file_is_migrated_in_memory_and_only_rewritten_by_a_successful_import() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.yaml");
    let mut config = Config {
        version: CURRENT_CONFIG_VERSION - 1,
        ..Config::default()
    };
    config.browser.remote = local();
    let old_text = config.to_yaml().expect("yaml");
    std::fs::write(&config_path, &old_text).expect("write");

    // A conflicting profile fails without the migration reaching the disk.
    let conflict = dir.path().join("conflict.yaml");
    write_portable_profile(&conflict, &redirected()).expect("profile");
    let error = import_portable_file_into_config(&config_path, &conflict).expect_err("conflict");
    assert!(matches!(error, RemoteProfileError::Config(_)), "{error}");
    assert_eq!(std::fs::read_to_string(&config_path).expect("read"), old_text);

    // A compatible one rewrites the file once, migrated and merged.
    let profile = dir.path().join("profile.yaml");
    write_portable_profile(&profile, &local()).expect("profile");
    import_portable_file_into_config(&config_path, &profile).expect("import");
    let loaded = Config::load(Some(&config_path)).expect("load");
    assert_eq!(loaded.version, CURRENT_CONFIG_VERSION);
    assert!(
        loaded.browser.remote.providers["grid"]
            .credential_bindings
            .contains_key(&CredentialReference::from("key"))
    );
}

#[test]
fn the_configuration_file_is_never_a_profile_destination_or_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.yaml");
    let mut config = Config::default();
    config.browser.remote = local();
    let text = config.to_yaml().expect("yaml");
    std::fs::write(&config_path, &text).expect("write");

    let error = export_portable_file_from_config(&config_path, &config_path).expect_err("same file");
    assert!(matches!(error, RemoteProfileError::IsConfigPath { .. }), "{error}");
    let error = export_portable_file_from_config(&config_path, &dir.path().join(".").join("config.yaml"))
        .expect_err("same file spelled differently");
    assert!(matches!(error, RemoteProfileError::IsConfigPath { .. }), "{error}");
    let error = import_portable_file_into_config(&config_path, &config_path).expect_err("same file");
    assert!(matches!(error, RemoteProfileError::IsConfigPath { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(&config_path).expect("read"), text);

    // Paths that do not exist yet are normalised before the comparison, so
    // no spelling of the configuration path slips through as a destination.
    let absent = dir.path().join("absent").join("config.yaml");
    for alias in [
        absent.clone(),
        dir.path().join("absent").join(".").join("config.yaml"),
        dir.path().join("absent").join("sub").join("..").join("config.yaml"),
        dir.path().join(".").join("absent").join("config.yaml"),
    ] {
        assert!(
            matches!(
                refuse_config_path(&absent, &alias),
                Err(RemoteProfileError::IsConfigPath { .. })
            ),
            "{}",
            alias.display()
        );
    }
    refuse_config_path(&absent, &dir.path().join("absent").join("profile.yaml")).expect("different files");
    refuse_config_path(&absent, &dir.path().join("absent2").join("config.yaml")).expect("different directories");
}

#[test]
fn staging_never_follows_a_planted_name_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("profile.yaml");
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "keep").expect("victim");
    // Names of the shape an older staging scheme used, pointing at the victim.
    #[cfg(unix)]
    for planted in [
        format!("profile.yaml.{}.tmp", std::process::id()),
        ".remote-profile-x.tmp".to_string(),
    ] {
        std::os::unix::fs::symlink(&victim, dir.path().join(planted)).expect("symlink");
    }
    write_portable_profile(&target, &local()).expect("write");
    assert_eq!(std::fs::read_to_string(&victim).expect("victim"), "keep");
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|name| name.to_ascii_lowercase().ends_with(".tmp") && !name.starts_with(".remote-profile-x"))
        .filter(|name| !name.starts_with("profile.yaml."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        read_portable_profile(&target).expect("read"),
        export_portable(&local()).expect("document")
    );
}

#[test]
fn a_failed_import_leaves_the_configuration_file_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.yaml");
    let mut config = Config::default();
    config.browser.remote = local();
    std::fs::write(&config_path, config.to_yaml().expect("yaml")).expect("write");
    let before = std::fs::read_to_string(&config_path).expect("read");

    let profile_path = dir.path().join("redirect.yaml");
    write_portable_profile(&profile_path, &redirected()).expect("write profile");
    let error = import_portable_file_into_config(&config_path, &profile_path).expect_err("conflict");
    assert!(matches!(error, RemoteProfileError::Config(_)), "{error}");
    assert_eq!(std::fs::read_to_string(&config_path).expect("read"), before);

    let missing = import_portable_file_into_config(&config_path, &dir.path().join("absent.yaml")).expect_err("io");
    assert!(matches!(missing, RemoteProfileError::Io { .. }), "{missing}");
    assert_eq!(std::fs::read_to_string(&config_path).expect("read"), before);
}
