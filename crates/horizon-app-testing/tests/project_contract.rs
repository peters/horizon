use horizon_app_testing::Error;
use horizon_app_testing::catalog::{decode, resolve};
use horizon_app_testing::contract::{Contract, Form, MatrixEntry, Platform, project_path};
use horizon_app_testing::recipe::Recipe;
use std::path::Path;

const AGENTS: &str = r#"# Example project

## Remote device testing

```yaml
remote-device-testing:
  version: 1
  provider: browserstack
  apps:
    ios:
      build: ["./scripts/build-ios-test.sh"]
      artifact: build/ios/App.ipa
      bundle_id: com.example.app
    android:
      build: ["./gradlew", assembleDebug]
      artifact: build/android/App.apk
      package: com.example.app
  launch_arguments:
    BASE_URL: "http://localhost:{tunnel.port.backend}"
  tunnel:
    ports: {backend: 8080}
  matrix:
    - {platform: ios, form: phone, os: latest}
    - {platform: ios, form: tablet, os: latest}
    - {platform: android, form: phone, os: latest-1}
  recipes: [docs/features/native-smoke.md]
```
"#;

const RECIPE: &str = r"# Smoke

Human-readable steps and evidence boundaries go here.

```yaml
device-recipe:
  version: 1
  id: smoke
  steps:
    - id: home-visible
      action: wait
      target: {by: identifier, value: home.title}
      state: visible
      timeout_millis: 10000
    - id: open-menu
      action: tap
      target: {by: identifier, value: menu.open}
```
";

#[test]
fn reads_both_platforms_and_resolves_only_declared_ports() -> Result<(), Error> {
    let contract = Contract::from_agents(AGENTS)?;
    assert_eq!(contract.apps.len(), 2);
    assert_eq!(
        contract.resolve_value(&contract.launch_arguments["BASE_URL"])?,
        "http://localhost:8080"
    );
    assert_eq!(contract.max_parallel, 16);
    assert!(contract.evidence.video);
    Ok(())
}

#[test]
fn distinguishes_missing_repeated_and_malformed_contracts() {
    assert_eq!(
        Contract::from_agents("# No contract").err(),
        Some(Error::ContractMissing)
    );
    assert_eq!(
        Contract::from_agents(&format!("{AGENTS}\n{AGENTS}")).err(),
        Some(Error::ContractInvalid)
    );
    assert_eq!(
        Contract::from_agents(&AGENTS.replace("version: 1", "version: 99")).err(),
        Some(Error::ContractInvalid)
    );
    assert_eq!(
        Contract::from_agents(&AGENTS.replace("    BASE_URL:", "    PASSWORD:")).err(),
        Some(Error::ContractInvalid)
    );
    assert_eq!(
        Contract::from_agents(&AGENTS.replace(
            "provider: browserstack",
            "provider: browserstack\n  credentials: secret-value"
        ))
        .err(),
        Some(Error::ContractInvalid)
    );
}

#[test]
fn contract_failures_never_echo_values() {
    let invalid = AGENTS.replace("provider: browserstack", "provider: super-secret-value!");
    let error = Contract::from_agents(&invalid).err();
    assert_eq!(error, Some(Error::ContractInvalid));
    assert!(!format!("{error:?}").contains("super-secret"));
}

#[test]
fn rejects_traversal_and_inconsistent_platform_artifacts() {
    for (before, after, expected) in [
        ("build/ios/App.ipa", "../App.ipa", Error::PathRejected),
        ("build/ios/App.ipa", "/tmp/App.ipa", Error::PathRejected),
        ("build/ios/App.ipa", "build/ios/App.apk", Error::ContractInvalid),
        (
            "docs/features/native-smoke.md",
            "docs/features/*.md",
            Error::PathRejected,
        ),
        ("package: com.example.app", "package: invalid", Error::ContractInvalid),
        (
            "build: [\"./gradlew\", assembleDebug]",
            "build: []",
            Error::ContractInvalid,
        ),
    ] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace(before, after)).err(),
            Some(expected)
        );
    }
}

#[test]
fn refuses_external_endpoints_undeclared_ports_and_unresolved_templates() -> Result<(), Error> {
    let contract = Contract::from_agents(AGENTS)?;
    for value in [
        "http://192.168.1.1:8080",
        "https://localhost:8443",
        "http://user:password@localhost:8080",
        "http://localhost:{tunnel.port.other}",
        "{env.TOKEN}",
        "http://localhost:0",
    ] {
        assert_eq!(contract.resolve_value(value).err(), Some(Error::ContractInvalid));
    }
    assert_eq!(
        contract.resolve_value("http://[::1]:8080/path")?,
        "http://[::1]:8080/path"
    );
    Ok(())
}

#[test]
fn missing_artifact_is_allowed_before_build_but_path_stays_in_root() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = project_path(root.path(), Path::new("build/ios/App.ipa"))?;
    assert!(path.starts_with(root.path().canonicalize()?));
    assert_eq!(
        project_path(root.path(), Path::new("../escape.ipa")).err(),
        Some(Error::PathRejected)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn rejects_existing_and_dangling_symlink_escapes() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("build"))?;
    assert_eq!(
        project_path(root.path(), Path::new("build/App.ipa")).err(),
        Some(Error::PathRejected)
    );
    std::os::unix::fs::symlink(outside.path().join("missing"), root.path().join("dangling"))?;
    assert_eq!(
        project_path(root.path(), Path::new("dangling/App.ipa")).err(),
        Some(Error::PathRejected)
    );
    Ok(())
}

#[test]
fn structured_recipe_preserves_actions_and_step_ids() -> Result<(), Error> {
    let recipe = Recipe::from_markdown(RECIPE)?;
    assert_eq!(recipe.steps.len(), 2);
    assert_eq!(recipe.steps[0].id, "home-visible");
    Ok(())
}

#[test]
fn prose_and_invalid_steps_cannot_report_success() {
    for source in [
        "# Tap the button",
        &RECIPE.replace("action: tap", "action: guess"),
        &RECIPE.replace("id: open-menu", "id: home-visible"),
        &RECIPE.replace("timeout_millis: 10000", "timeout_millis: 60001"),
        &RECIPE.replace("action: tap", "action: tap\n      endpoint: https://example.com"),
    ] {
        assert_eq!(Recipe::from_markdown(source).err(), Some(Error::RecipeInvalid));
    }
}

const DEVICES: &[u8] = br#"[
 {"realMobile":true,"os":"ios","os_version":"18.0","device":"iPhone 16"},
 {"realMobile":true,"os":"ios","os_version":"17.0","device":"iPhone 15"},
 {"realMobile":true,"os":"ios","os_version":"18.1","device":"iPad Pro"},
 {"realMobile":true,"os":"android","os_version":"15.0","device":"Google Pixel 9"},
 {"realMobile":true,"os":"android","os_version":"14.0","device":"Google Pixel 8"},
 {"realMobile":true,"os":"android","os_version":"15.0","device":"Samsung Galaxy Tab S9"}
]"#;

#[test]
fn resolves_native_phone_and_tablet_matrix_without_browser_names() -> Result<(), Error> {
    let contract = Contract::from_agents(AGENTS)?;
    let devices = decode(DEVICES)?;
    let selected = resolve(&contract.matrix, &devices)?;
    assert_eq!(selected.len(), 3);
    assert_eq!(selected[0].device.model, "iPhone 16");
    assert_eq!(selected[1].device.model, "iPad Pro");
    assert_eq!(selected[2].device.model, "Google Pixel 8");
    Ok(())
}

#[test]
fn unavailable_os_is_a_failure_instead_of_a_smaller_matrix() -> Result<(), Error> {
    let contract = Contract::from_agents(&AGENTS.replace("latest-1", "latest-2"))?;
    assert_eq!(
        resolve(&contract.matrix, &decode(DEVICES)?).err(),
        Some(Error::MatrixUnavailable)
    );
    Ok(())
}

#[test]
fn exact_model_and_minor_os_are_resolved_deterministically() -> Result<(), Error> {
    let devices = decode(DEVICES)?;
    let entry = MatrixEntry {
        platform: Platform::Ios,
        form: Form::Tablet,
        os: "18.1".into(),
        device: Some("ipad pro".into()),
    };
    assert_eq!(resolve(&[entry], &devices)?[0].device.os_version, "18.1");
    assert_eq!(
        decode(br#"[{"realMobile":true,"os":"desktop","os_version":"1","device":"Virtual"}]"#).err(),
        Some(Error::CatalogInvalid)
    );
    Ok(())
}

#[test]
fn symbolic_offsets_follow_offered_generations_across_numbering_jumps() -> Result<(), Error> {
    let devices = decode(
        br#"[
      {"realMobile":true,"os":"ios","os_version":"27","device":"iPhone 15"},
      {"realMobile":true,"os":"ios","os_version":"26","device":"iPhone 15"},
      {"realMobile":true,"os":"ios","os_version":"18","device":"iPhone 14"},
      {"realMobile":true,"os":"ios","os_version":"17","device":"iPhone 14"}
    ]"#,
    )?;
    let entry = MatrixEntry {
        platform: Platform::Ios,
        form: Form::Phone,
        os: "latest-2".into(),
        device: None,
    };
    assert_eq!(resolve(&[entry], &devices)?[0].device.os_version, "18");
    Ok(())
}

#[test]
fn rejects_uppercase_remote_urls_and_nonportable_paths() {
    for url in [
        "HTTP://example.com:8080",
        "https://user@localhost:8080",
        " https://example.com:8080",
        " https://user@localhost:8080",
    ] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace("http://localhost:{tunnel.port.backend}", url)).err(),
            Some(Error::ContractInvalid)
        );
    }
    for path in ["C:/App.ipa", "build/alternate:stream.ipa"] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace("build/ios/App.ipa", path)).err(),
            Some(Error::PathRejected)
        );
    }
}

#[test]
fn validates_platform_specific_application_ids() {
    for id in ["com.example.bad-app", "1.example.app", "com.example.123"] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace("package: com.example.app", &format!("package: {id}"))).err(),
            Some(Error::ContractInvalid)
        );
    }
    assert!(
        Contract::from_agents(&AGENTS.replace("bundle_id: com.example.app", "bundle_id: com.example.test-app")).is_ok()
    );
    assert!(
        Contract::from_agents(&AGENTS.replace("package: com.example.app", "package: com.example.test_app")).is_ok()
    );
}

#[test]
fn fieldless_actions_reject_unknown_fields() {
    for action in ["back", "home", "launch", "terminate", "reset", "screenshot"] {
        let valid = format!(
            "```yaml\ndevice-recipe:\n  version: 1\n  id: smoke\n  steps:\n    - id: step\n      action: {action}\n```\n"
        );
        assert!(Recipe::from_markdown(&valid).is_ok(), "{action}");
        let invalid = valid.replace("\n```", "\n      extra: ignored\n```");
        assert_eq!(
            Recipe::from_markdown(&invalid).err(),
            Some(Error::RecipeInvalid),
            "{action}"
        );
    }
}

#[test]
fn all_launch_urls_obey_the_loopback_policy_and_deep_links_must_parse() {
    for url in [
        "ws://example.com:8080",
        "ws:example.com:8080",
        "wss:example.com:8080",
        "ftp:example.com:8080",
        "custom://external-host:8080",
        " http:/example.com:8080",
    ] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace("http://localhost:{tunnel.port.backend}", url)).err(),
            Some(Error::ContractInvalid)
        );
    }
    let invalid = "```yaml\ndevice-recipe:\n  version: 1\n  id: smoke\n  steps:\n    - id: step\n      action: deep_link\n      url: 'not-a-url://['\n```\n";
    assert_eq!(Recipe::from_markdown(invalid).err(), Some(Error::RecipeInvalid));
}

#[test]
fn virtual_and_unverified_devices_cannot_qualify_a_real_device_matrix() {
    for row in [
        r#"[{"os":"ios","os_version":"27","device":"Virtual"}]"#,
        r#"[{"os":"ios","os_version":"27","device":"Virtual","realMobile":false}]"#,
    ] {
        assert_eq!(decode(row.as_bytes()).err(), Some(Error::CatalogInvalid));
    }
    let rows=br#"[{"os":"ios","os_version":"27","device":"Virtual","realMobile":false},{"os":"ios","os_version":"27","device":"iPhone 15","realMobile":true}]"#;
    let offered = decode(rows).unwrap();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].model, "iPhone 15");
}
