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

#[test]
fn per_device_managed_ports_require_complete_host_bindings() -> Result<(), Error> {
    use std::collections::BTreeMap;
    let text = AGENTS.replace(
        "ports: {backend: 8080}",
        "ports: {backend: {start: [python3, backend.py], timeout_seconds: 900}, metrics: 9000}",
    );
    let contract = Contract::from_agents(&text)?;
    let value = "http://localhost:{tunnel.port.backend}";
    assert_eq!(contract.resolve_value(value).err(), Some(Error::ContractInvalid));
    for (device_port, expected) in [(41935, "http://localhost:41935"), (40303, "http://localhost:40303")] {
        let ports = BTreeMap::from([("backend".into(), device_port), ("metrics".into(), 9000)]);
        assert_eq!(contract.resolve_value_with_ports(value, &ports)?, expected);
    }
    for scheme in ["http", "https", "ws", "wss", "youpark"] {
        let value = format!("{scheme}://localhost:{{tunnel.port.backend}}/native");
        let declaration = Contract::from_agents(&text.replace("http://localhost:{tunnel.port.backend}", &value))?;
        let ports = BTreeMap::from([("backend".into(), 41935), ("metrics".into(), 9000)]);
        assert_eq!(
            declaration.resolve_value_with_ports(&value, &ports)?,
            format!("{scheme}://localhost:41935/native")
        );
    }
    for ports in [
        BTreeMap::from([("backend".into(), 41935)]),
        BTreeMap::from([("backend".into(), 0), ("metrics".into(), 9000)]),
        BTreeMap::from([("backend".into(), 9000), ("metrics".into(), 9000)]),
        BTreeMap::from([("backend".into(), 41935), ("metrics".into(), 9001)]),
        BTreeMap::from([("backend".into(), 41935), ("unexpected".into(), 9000)]),
    ] {
        assert_eq!(
            contract.resolve_value_with_ports(value, &ports).err(),
            Some(Error::ContractInvalid)
        );
    }
    let ports = BTreeMap::from([("backend".into(), 41935), ("metrics".into(), 9000)]);
    assert_eq!(
        contract
            .resolve_value_with_ports("http://localhost:41935", &ports)
            .err(),
        Some(Error::ContractInvalid)
    );
    for value in [
        "http://localhost:9000/{tunnel.port.backend}",
        "http://localhost:{tunnel.port.backend}/{tunnel.port.backend}",
        "backend={tunnel.port.backend}",
    ] {
        assert_eq!(
            contract.resolve_value_with_ports(value, &ports).err(),
            Some(Error::ContractInvalid)
        );
        assert_eq!(
            Contract::from_agents(&text.replace("http://localhost:{tunnel.port.backend}", value)).err(),
            Some(Error::ContractInvalid)
        );
    }
    Ok(())
}

#[test]
fn malformed_managed_backend_declarations_fail_before_any_child_starts() {
    for spec in [
        "{start: [], timeout_seconds: 900}",
        "{start: [python3, backend.py], timeout_seconds: 0}",
        "{start: [python3, backend.py], timeout_seconds: 1201}",
        "{start: [python3, backend.py], timeout_seconds: 900, host: example.com}",
    ] {
        assert_eq!(
            Contract::from_agents(&AGENTS.replace("ports: {backend: 8080}", &format!("ports: {{backend: {spec}}}")))
                .err(),
            Some(Error::ContractInvalid)
        );
    }
    let text = AGENTS.replace(
        "ports: {backend: 8080}",
        "ports: {backend: {start: [python3, backend.py], timeout_seconds: 900}}",
    );
    for value in [
        "http://localhost:1",
        "http://localhost:1/{tunnel.port.backend}",
        "http://example.com:{tunnel.port.backend}",
    ] {
        assert_eq!(
            Contract::from_agents(&text.replace("http://localhost:{tunnel.port.backend}", value)).err(),
            Some(Error::ContractInvalid)
        );
    }
}

#[test]
fn backend_readiness_rejects_extraneous_or_sensitive_fields() -> Result<(), Error> {
    use horizon_app_testing::backend::ready_port;
    assert_eq!(ready_port(br#"{"native_backend_ready":1,"port":41935}"#)?, 41935);
    for input in [
        br#"{"native_backend_ready":1,"port":0}"#.as_slice(),
        br#"{"native_backend_ready":2,"port":41935}"#,
        br#"{"native_backend_ready":1,"port":41935,"host":"example.com"}"#,
        br#"{"native_backend_ready":1,"port":41935,"key":"synthetic"}"#,
        br#"{"native_backend_ready":1,"port":65536}"#,
        br#"{"native_backend_ready":1,"port":41935,"port":41935}"#,
        b"private child diagnostic",
    ] {
        assert_eq!(ready_port(input).err(), Some(Error::BackendReadyInvalid));
    }
    assert_eq!(ready_port(&[b' '; 257]).err(), Some(Error::BackendReadyInvalid));
    Ok(())
}

#[test]
fn published_schema_matches_the_project_contract_wire_types() {
    let published: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/architecture/remote-device-testing.schema.json"
    ))
    .unwrap();
    let actual = serde_json::to_value(horizon_app_testing::contract::schema()).unwrap();
    assert_eq!(
        published, actual,
        "regenerate the documented project schema after wire changes"
    );
}

#[test]
fn endpoint_references_require_an_explicit_validated_scheme() {
    for endpoint in [
        "//example.com:8080",
        " //example.com:8080",
        "192.168.1.1:8080",
        "192.168.1.1:8080/path",
        "127.0.0.1:8080",
        "[::1]:8080",
        "\\\\example.com:8080",
        "/\\example.com:8080",
        "\\/example.com:8080",
    ] {
        let input = AGENTS.replace(
            "\"http://localhost:{tunnel.port.backend}\"",
            &serde_json::to_string(endpoint).unwrap(),
        );
        assert_eq!(Contract::from_agents(&input).err(), Some(Error::ContractInvalid));
    }
    assert!(Contract::from_agents(AGENTS).is_ok());
}

#[test]
fn persisted_recipes_reject_snapshot_refs_but_interactive_actions_accept_them() {
    use horizon_app_testing::recipe::{Action, Target};
    let action = Action::Tap {
        target: Target::Ref("n1".into()),
    };
    assert!(action.validate().is_ok());
    for action in ["tap", "long_press", "type", "clear", "wait", "assert"] {
        let extra = match action {
            "long_press" => "      duration_millis: 500\n",
            "type" => "      text: synthetic\n",
            "wait" => "      state: visible\n      timeout_millis: 1000\n",
            "assert" => "      state: visible\n",
            _ => "",
        };
        let recipe = format!(
            "```yaml\ndevice-recipe:\n  version: 1\n  id: smoke\n  steps:\n    - id: step\n      action: {action}\n      target: {{by: ref, value: n1}}\n{extra}```\n"
        );
        assert_eq!(Recipe::from_markdown(&recipe).err(), Some(Error::RecipeInvalid));
    }
}

#[test]
fn unknown_catalog_families_cannot_satisfy_phone_entries() {
    let catalog = decode(
        br#"[
      {"realMobile":true,"os":"android","os_version":"99","device":"NewVendor Tablet 1"},
      {"realMobile":true,"os":"android","os_version":"99","device":"OnePlus Pad 2"},
      {"realMobile":true,"os":"android","os_version":"99","device":"Google Pixel Watch 4"},
      {"realMobile":true,"os":"ios","os_version":"99","device":"New Apple Tablet"},
      {"realMobile":true,"os":"android","os_version":"15","device":"Google Pixel 9"},
      {"realMobile":true,"os":"ios","os_version":"27","device":"iPhone 17"}
    ]"#,
    )
    .unwrap();
    assert_eq!(catalog.len(), 2);
    assert!(
        catalog
            .iter()
            .all(|device| device.form == horizon_app_testing::contract::Form::Phone)
    );
    let contract = Contract::from_agents(AGENTS).unwrap();
    assert_eq!(
        resolve(&contract.matrix, &catalog).err(),
        Some(Error::MatrixUnavailable)
    );
    assert_eq!(
        decode(br#"[{"realMobile":true,"os":"android","os_version":"99","device":"Unknown Tablet"}]"#).err(),
        Some(Error::CatalogInvalid)
    );
}

#[test]
fn harmless_feature_options_remain_valid() {
    for name in ["AUTH_ENABLED", "OAUTH_ENABLED", "COOKIE_POLICY", "KEYBOARD_MODE"] {
        let agents = AGENTS.replace(
            "    BASE_URL: \"http://localhost:{tunnel.port.backend}\"",
            &format!("    {name}: enabled"),
        );
        Contract::from_agents(&agents).unwrap();
    }
}
