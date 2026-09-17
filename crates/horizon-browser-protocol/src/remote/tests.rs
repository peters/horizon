use super::*;

const SAMPLE: &str = r#"{
  "providers": {
    "device_cloud": {
      "adapter": "webdriver",
      "endpoint": "https://grid.example.net/wd/hub/",
      "authentication": {"kind": "basic", "username_ref": "cloud-user", "password_ref": "cloud-key"},
      "credential_bindings": {
        "cloud-user": {"store": "os_keychain", "slot": "remote-browser/device-cloud/username"},
        "cloud-key": {"store": "session"}
      },
      "limits": {"max_sessions": 1, "allocation_timeout_seconds": 120, "idle_release_seconds": 180, "max_session_seconds": 1800}
    }
  },
  "targets": {
    "ios_phone": {
      "provider": "device_cloud",
      "browser_name": "safari",
      "platform_name": "iOS",
      "device": {"kind": "physical", "model": "iPhone 16", "os_version": "18"},
      "capability_extensions": {"appium:automationName": "XCUITest", "bstack:options": {"local": "false"}}
    }
  }
}"#;

fn sample() -> RemoteBrowserConfig {
    let config: RemoteBrowserConfig = serde_json::from_str(SAMPLE).expect("sample parses");
    config.validate().expect("sample validates");
    config
}

fn provider(config: &RemoteBrowserConfig) -> &RemoteProviderProfile {
    config.providers.get("device_cloud").expect("provider exists")
}

#[test]
fn sample_round_trips_and_canonicalizes_the_endpoint() {
    let config = sample();
    assert_eq!(provider(&config).endpoint.as_str(), "https://grid.example.net/wd/hub");
    assert_eq!(provider(&config).endpoint.origin(), "https://grid.example.net");
    let encoded = serde_json::to_string(&config).expect("serializes");
    let decoded: RemoteBrowserConfig = serde_json::from_str(&encoded).expect("re-parses");
    assert_eq!(decoded, config);
    assert!(!encoded.contains("secret"));
}

#[test]
fn empty_config_is_default_and_serializes_to_nothing() {
    let empty = RemoteBrowserConfig::default();
    assert!(empty.is_empty());
    assert_eq!(serde_json::to_string(&empty).expect("serializes"), "{}");
    empty.validate().expect("empty is valid");
}

#[test]
fn unknown_keys_are_rejected_everywhere() {
    let cases = [
        r#"{"providers": {}, "targets": {}, "tunnels": {}}"#,
        r#"{"providers": {"p": {"endpoint": "https://a.test", "token": "x"}}}"#,
        r#"{"providers": {"p": {"endpoint": "https://a.test", "limits": {"minutes": 1}}}}"#,
        r#"{"providers": {"p": {"endpoint": "https://a.test"}}, "targets": {"t": {"provider": "p", "browser_name": "a", "platform_name": "b", "endpoint": "x"}}}"#,
    ];
    for case in cases {
        assert!(serde_json::from_str::<RemoteBrowserConfig>(case).is_err(), "{case}");
    }
}

#[test]
fn authentication_variants_reject_cross_variant_fields() {
    let cases = [
        r#"{"kind": "none", "token_ref": "x"}"#,
        r#"{"kind": "basic", "username_ref": "u"}"#,
        r#"{"kind": "basic", "username_ref": "u", "password_ref": "p", "token_ref": "t"}"#,
        r#"{"kind": "bearer"}"#,
        r#"{"kind": "bearer", "token_ref": "t", "password_ref": "p"}"#,
        r#"{"kind": "digest", "username_ref": "u"}"#,
    ];
    for case in cases {
        assert!(serde_json::from_str::<RemoteAuthentication>(case).is_err(), "{case}");
    }
    let bearer: RemoteAuthentication =
        serde_json::from_str(r#"{"kind": "bearer", "token_ref": "grid-token"}"#).expect("bearer");
    assert_eq!(bearer.references(), vec![&CredentialReference::from("grid-token")]);
    assert_eq!(
        serde_json::from_str::<RemoteAuthentication>(r#"{"kind": "none"}"#).expect("none"),
        RemoteAuthentication::None {}
    );
}

#[test]
fn endpoint_rules_reject_credentials_in_urls_and_plain_http() {
    let problems = [
        ("http://grid.example.net/wd/hub", EndpointProblem::NotHttps),
        ("https://user:key@grid.example.net/wd/hub", EndpointProblem::Userinfo),
        ("https://grid.example.net/wd/hub?key=1", EndpointProblem::QueryString),
        ("https://grid.example.net/wd/hub#x", EndpointProblem::Fragment),
        ("not a url", EndpointProblem::Unparseable),
        ("ftp://grid.example.net", EndpointProblem::NotHttps),
    ];
    for (input, expected) in problems {
        assert_eq!(ControlEndpoint::parse(input).expect_err(input), expected, "{input}");
    }
    assert_eq!(
        ControlEndpoint::parse("https://@grid.example.net/wd/hub").expect_err("empty userinfo"),
        EndpointProblem::Userinfo
    );
    assert_eq!(
        ControlEndpoint::parse("https://:@grid.example.net/wd/hub").expect_err("empty userinfo pair"),
        EndpointProblem::Userinfo
    );
    ControlEndpoint::parse("https://grid.example.net/wd/hub@path").expect("an @ in the path is not userinfo");
    let loopback = ControlEndpoint::parse("http://127.0.0.1:4723/").expect("loopback http grid");
    assert!(loopback.is_loopback_http());
    assert_eq!(loopback.as_str(), "http://127.0.0.1:4723");
    let ipv6 = ControlEndpoint::parse("http://[::1]:4723/wd/hub").expect("loopback ipv6");
    assert!(ipv6.is_loopback_http());
    assert!(
        !ControlEndpoint::parse("https://grid.example.net")
            .expect("https")
            .is_loopback_http()
    );
}

#[test]
fn endpoint_parse_error_never_echoes_the_value() {
    let error = serde_json::from_str::<RemoteBrowserConfig>(
        r#"{"providers": {"p": {"endpoint": "https://alice:hunter2@grid.example.net"}}}"#,
    )
    .expect_err("userinfo rejected");
    let message = error.to_string();
    assert!(!message.contains("hunter2"), "{message}");
    assert!(message.contains("username or password"), "{message}");
}

#[test]
fn authentication_references_must_be_bound_exactly() {
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .remove(&CredentialReference::from("cloud-key"));
    assert_eq!(
        config.validate().expect_err("missing binding"),
        RemoteConfigError::InvalidCredential {
            provider: "device_cloud".into(),
            reference: "cloud-key".into(),
            problem: CredentialReferenceProblem::MissingBinding,
        }
    );
    config.validate_definition().expect("definition alone is fine");

    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("stale"),
            CredentialBinding {
                store: CredentialStoreKind::Session,
                slot: None,
            },
        );
    assert!(matches!(
        config.validate().expect_err("unused binding"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::UnusedBinding,
            ..
        }
    ));

    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("cloud-user"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: None,
            },
        );
    assert!(matches!(
        config.validate().expect_err("keychain needs a slot"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::SlotRequired,
            ..
        }
    ));
}

#[test]
fn malformed_present_bindings_fail_at_definition_time() {
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("cloud-user"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: None,
            },
        );
    assert!(matches!(
        config
            .validate_definition()
            .expect_err("slot required is a definition error"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::SlotRequired,
            ..
        }
    ));
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .get_mut(&CredentialReference::from("cloud-user"))
        .expect("binding")
        .slot = Some("bad slot with spaces".into());
    assert!(matches!(
        config.validate_definition().expect_err("malformed slot"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::MalformedSlot,
            ..
        }
    ));
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .remove(&CredentialReference::from("cloud-key"));
    config
        .validate_definition()
        .expect("a missing binding is readiness, not a definition error");
}

#[test]
fn limits_are_bounded_per_field() {
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .limits
        .max_sessions = 0;
    assert_eq!(
        config.validate().expect_err("zero sessions"),
        RemoteConfigError::InvalidLimit {
            provider: "device_cloud".into(),
            field: "max_sessions",
            min: 1,
            max: 8
        }
    );
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .limits
        .max_session_seconds = 100_000;
    assert!(matches!(
        config.validate().expect_err("too long"),
        RemoteConfigError::InvalidLimit {
            field: "max_session_seconds",
            ..
        }
    ));
}

#[test]
fn targets_need_a_configured_provider_and_valid_names() {
    let mut config = sample();
    config.targets.get_mut("ios_phone").expect("target").provider = "elsewhere".into();
    assert_eq!(
        config.validate().expect_err("unknown provider"),
        RemoteConfigError::UnknownProvider {
            target: "ios_phone".into(),
            provider: "elsewhere".into()
        }
    );
    let mut config = sample();
    let target = config.targets.remove("ios_phone").expect("target");
    config.targets.insert("ios phone".into(), target);
    assert_eq!(
        config.validate().expect_err("space in name"),
        RemoteConfigError::InvalidTargetName {
            target: "ios phone".into()
        }
    );
    let mut config = sample();
    config.targets.get_mut("ios_phone").expect("target").browser_name = " safari".into();
    assert!(matches!(
        config.validate().expect_err("padded"),
        RemoteConfigError::InvalidBrowserName { .. }
    ));
}

#[test]
fn capability_extensions_must_be_namespaced_secret_free_and_non_conflicting() {
    let cases = [
        (
            "browserName",
            serde_json::json!("chrome"),
            ExtensionProblem::NotNamespaced,
        ),
        (
            "appium:deviceName",
            serde_json::json!("iPhone 16"),
            ExtensionProblem::ConflictsWithNormalizedField,
        ),
        (
            "appium:platformVersion",
            serde_json::json!("18"),
            ExtensionProblem::ConflictsWithNormalizedField,
        ),
        (
            "bstack:options",
            serde_json::json!({"realMobile": "true"}),
            ExtensionProblem::ConflictsWithNormalizedField,
        ),
        (
            "bstack:options",
            serde_json::json!({"userName": "u"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "bstack:options",
            serde_json::json!({"accessKey": "k"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "sauce:accessKey",
            serde_json::json!("k"),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:clientSecret",
            serde_json::json!("s"),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:api_token",
            serde_json::json!("t"),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"apiToken": "t"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"private-key": "t"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"authCode": "c"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"licenseKey": "k"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"oauthToken": "t"}),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:passphrase",
            serde_json::json!("p"),
            ExtensionProblem::CarriesCredential,
        ),
        (
            "vendor:options",
            serde_json::json!({"os_version": "18"}),
            ExtensionProblem::ConflictsWithNormalizedField,
        ),
    ];
    for (key, value, expected) in cases {
        let mut config = sample();
        let target = config.targets.get_mut("ios_phone").expect("target");
        target.capability_extensions.clear();
        target.capability_extensions.insert(key.into(), value);
        assert_eq!(
            config.validate().expect_err(key),
            RemoteConfigError::InvalidCapabilityExtension {
                target: "ios_phone".into(),
                key: key.into(),
                problem: expected
            },
            "{key}"
        );
    }
}

#[test]
fn capability_extension_keys_follow_the_vendor_name_grammar() {
    let cases = [
        (":name", serde_json::json!(1), ExtensionProblem::NotNamespaced),
        (
            "vendor:foo:username",
            serde_json::json!("u"),
            ExtensionProblem::NotNamespaced,
        ),
        (
            "vendor:user name",
            serde_json::json!("u"),
            ExtensionProblem::NotNamespaced,
        ),
        (
            "vendor:options",
            serde_json::json!({"user name": "u"}),
            ExtensionProblem::InvalidOptionKey,
        ),
        (
            "vendor:options",
            serde_json::json!({"username ": "u"}),
            ExtensionProblem::InvalidOptionKey,
        ),
        (
            "vendor:options",
            serde_json::json!({"nested:username": "u"}),
            ExtensionProblem::InvalidOptionKey,
        ),
    ];
    for (key, value, expected) in cases {
        let mut config = sample();
        let target = config.targets.get_mut("ios_phone").expect("target");
        target.capability_extensions.clear();
        target.capability_extensions.insert(key.into(), value);
        assert_eq!(
            config.validate().expect_err(key),
            RemoteConfigError::InvalidCapabilityExtension {
                target: "ios_phone".into(),
                key: key.into(),
                problem: expected
            },
            "{key}"
        );
    }
}

#[test]
fn benign_namespaced_extensions_are_accepted() {
    let mut config = sample();
    let target = config.targets.get_mut("ios_phone").expect("target");
    target.capability_extensions.clear();
    for (key, value) in [
        ("appium:automationName", serde_json::json!("XCUITest")),
        ("appium:hideKeyboard", serde_json::json!(true)),
        ("appium:newCommandTimeout", serde_json::json!(60)),
        (
            "bstack:options",
            serde_json::json!({"local": "false", "idleTimeout": 60, "projectName": "horizon"}),
        ),
    ] {
        target.capability_extensions.insert(key.into(), value);
    }
    config.validate().expect("benign extensions validate");
}

#[test]
fn portable_export_strips_bindings_and_reports_presence() {
    let config = sample();
    let portable = config.export_portable();
    assert!(provider(&portable).credential_bindings.is_empty());
    assert_eq!(portable.targets, config.targets);
    portable.validate_definition().expect("portable definition is valid");
    assert!(portable.validate().is_err(), "portable form has no bindings");
    let encoded = serde_json::to_string(&portable).expect("serializes");
    assert!(!encoded.contains("credential_bindings"));
    assert!(!encoded.contains("os_keychain"));
    assert!(
        encoded.contains("cloud-user"),
        "authentication references stay in the portable form"
    );

    let presence = config.binding_presence();
    let cloud = presence.get("device_cloud").expect("provider presence");
    assert_eq!(cloud.get(&CredentialReference::from("cloud-user")), Some(&true));
    assert!(!portable.binding_presence()["device_cloud"][&CredentialReference::from("cloud-key")]);
}

#[test]
fn import_adds_new_definitions_and_keeps_local_bindings() {
    let mut local = sample();
    let mut incoming = sample().export_portable();
    incoming
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .limits
        .max_sessions = 2;
    incoming.targets.insert(
        "android_phone".into(),
        RemoteTargetProfile {
            provider: "device_cloud".into(),
            browser_name: "chrome".into(),
            platform_name: "Android".into(),
            device: DeviceRequirement {
                kind: DeviceKind::Physical,
                model: Some("Google Pixel 9".into()),
                os_version: None,
            },
            capability_extensions: BTreeMap::new(),
        },
    );
    let summary = local.import_portable(&incoming).expect("import");
    assert_eq!(summary.providers_updated, vec!["device_cloud".to_string()]);
    assert_eq!(summary.targets_added, vec!["android_phone".to_string()]);
    assert_eq!(summary.targets_updated, vec!["ios_phone".to_string()]);
    assert_eq!(provider(&local).limits.max_sessions, 2);
    assert_eq!(provider(&local).credential_bindings.len(), 2, "local bindings survive");
    local.validate().expect("merged config is fully valid");
}

#[test]
fn import_refuses_bindings_and_endpoint_changes_without_touching_local_state() {
    let mut local = sample();
    let before = local.clone();
    let with_bindings = sample();
    assert_eq!(
        local.import_portable(&with_bindings).expect_err("bindings rejected"),
        RemoteConfigError::ImportCarriesBindings {
            provider: "device_cloud".into()
        }
    );
    assert_eq!(local, before);

    let mut redirected = sample().export_portable();
    redirected.providers.get_mut("device_cloud").expect("provider").endpoint =
        ControlEndpoint::parse("https://attacker.example.net/wd/hub").expect("parses");
    assert_eq!(
        local.import_portable(&redirected).expect_err("endpoint conflict"),
        RemoteConfigError::ImportEndpointConflict {
            provider: "device_cloud".into()
        }
    );
    assert_eq!(local, before);

    let mut broken = sample().export_portable();
    broken.targets.get_mut("ios_phone").expect("target").provider = "missing".into();
    assert!(local.import_portable(&broken).is_err());
    assert_eq!(local, before);

    let mut reshaped = sample().export_portable();
    reshaped
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .authentication = RemoteAuthentication::Bearer {
        token_ref: CredentialReference::from("cloud-token"),
    };
    assert_eq!(
        local
            .import_portable(&reshaped)
            .expect_err("authentication shape changed under local bindings"),
        RemoteConfigError::ImportAuthenticationConflict {
            provider: "device_cloud".into()
        }
    );
    assert_eq!(local, before);

    let mut unbound = sample();
    unbound
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .clear();
    let summary = unbound
        .import_portable(&reshaped)
        .expect("without local bindings the shape may change");
    assert_eq!(summary.providers_updated, vec!["device_cloud".to_string()]);
    assert!(matches!(
        unbound.providers["device_cloud"].authentication,
        RemoteAuthentication::Bearer { .. }
    ));
}

#[test]
fn error_messages_name_identifiers_only() {
    let error = RemoteConfigError::InvalidCredential {
        provider: "device_cloud".into(),
        reference: "cloud-key".into(),
        problem: CredentialReferenceProblem::MissingBinding,
    };
    assert_eq!(
        error.to_string(),
        "provider `device_cloud`: credential `cloud-key` is referenced by the authentication block but has no credential binding"
    );
    let endpoint = RemoteConfigError::InvalidEndpoint {
        provider: "p".into(),
        problem: EndpointProblem::NotHttps,
    };
    assert!(endpoint.to_string().contains("must use https"));
}

#[test]
fn duplicate_os_slots_on_one_origin_are_rejected_at_definition_time() {
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("cloud-key"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: Some("remote-browser/device-cloud/username".into()),
            },
        );
    assert!(matches!(
        config
            .validate_definition()
            .expect_err("two bindings on one origin and slot would share one OS item"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::DuplicateSlot,
            ..
        }
    ));

    // The same slot under another endpoint origin is a different OS item.
    let mut config = sample();
    let mut mirror = provider(&config).clone();
    mirror.endpoint = ControlEndpoint::parse("https://mirror.example.net/wd/hub").expect("endpoint");
    config.providers.insert("mirror".into(), mirror);
    config
        .validate_definition()
        .expect("distinct origins may reuse a slot name");
}

#[test]
fn capability_extension_values_are_checked_at_every_depth() {
    let mut config = sample();
    let target = config.targets.get_mut("ios_phone").expect("target");
    target.capability_extensions.insert(
        "vendor:options".into(),
        serde_json::json!({"safe": {"accessKey": "hidden"}}),
    );
    assert!(matches!(
        config.validate_definition().expect_err("credential two levels down"),
        RemoteConfigError::InvalidCapabilityExtension {
            problem: ExtensionProblem::CarriesCredential,
            ..
        }
    ));

    let target = config.targets.get_mut("ios_phone").expect("target");
    target.capability_extensions.insert(
        "vendor:options".into(),
        serde_json::json!({"list": [{"deviceName": "iPhone"}]}),
    );
    assert!(matches!(
        config
            .validate_definition()
            .expect_err("normalized field inside an array"),
        RemoteConfigError::InvalidCapabilityExtension {
            problem: ExtensionProblem::ConflictsWithNormalizedField,
            ..
        }
    ));

    let target = config.targets.get_mut("ios_phone").expect("target");
    target.capability_extensions.insert(
        "vendor:options".into(),
        serde_json::json!({"nested": {"projectName": "horizon", "tags": ["a", {"note": "b"}]}}),
    );
    config.validate_definition().expect("benign nested values pass");
}

#[test]
fn environment_bindings_name_variables_not_values() {
    let mut config = sample();
    let bindings = &mut config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings;
    bindings.insert(
        CredentialReference::from("cloud-user"),
        CredentialBinding {
            store: CredentialStoreKind::Environment,
            slot: Some("REMOTE_BROWSER_USERNAME".into()),
        },
    );
    bindings.insert(
        CredentialReference::from("cloud-key"),
        CredentialBinding {
            store: CredentialStoreKind::Environment,
            slot: Some("REMOTE_BROWSER_ACCESS_KEY".into()),
        },
    );
    config.validate().expect("environment bindings validate");
    let names = config.environment_variable_names();
    assert!(names.contains("REMOTE_BROWSER_USERNAME"));
    assert!(names.contains("REMOTE_BROWSER_ACCESS_KEY"));
    let encoded = serde_json::to_string(&config).expect("serializes");
    assert!(encoded.contains("environment"));
    assert!(encoded.contains("REMOTE_BROWSER_USERNAME"));
    assert!(!encoded.contains("secret"));
    let portable = config.export_portable();
    assert!(portable.environment_variable_names().is_empty());
    assert!(
        !serde_json::to_string(&portable)
            .expect("portable")
            .contains("REMOTE_BROWSER")
    );
}

#[test]
fn environment_bindings_reject_missing_and_malformed_names() {
    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("cloud-user"),
            CredentialBinding {
                store: CredentialStoreKind::Environment,
                slot: None,
            },
        );
    assert!(matches!(
        config.validate_definition().expect_err("variable name required"),
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::SlotRequired,
            ..
        }
    ));

    let mut config = sample();
    config
        .providers
        .get_mut("device_cloud")
        .expect("provider")
        .credential_bindings
        .insert(
            CredentialReference::from("cloud-user"),
            CredentialBinding {
                store: CredentialStoreKind::Environment,
                slot: Some("REMOTE-BROWSER-USER".into()),
            },
        );
    let error = config.validate_definition().expect_err("hyphens are not POSIX names");
    assert!(matches!(
        error,
        RemoteConfigError::InvalidCredential {
            problem: CredentialReferenceProblem::MalformedVariable,
            ..
        }
    ));
    assert!(error.to_string().contains("cloud-user"), "{error}");
    assert!(!error.to_string().contains("secret"), "{error}");
}
