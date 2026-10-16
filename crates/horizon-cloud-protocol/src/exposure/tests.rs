use super::*;

fn request(mode: ExposureMode, sources: Vec<SourceRule>) -> ExposureRequest {
    ExposureRequest {
        mode,
        target_port: 8080,
        sources,
        limits: ConnectionLimits::default(),
    }
}
fn cidr(value: &str) -> SourceRule {
    SourceRule::Cidr(value.into())
}

#[test]
fn absent_mode_is_off_and_never_inherits_public_admission() {
    let decoded = ExposureRequest::from_json(br#"{"target_port":8080}"#).unwrap();
    assert_eq!(decoded.mode, ExposureMode::Off);
    assert!(!decoded.validate().unwrap().allows("192.0.2.1".parse().unwrap()));
    assert!(
        request(ExposureMode::Public, vec![])
            .validate()
            .unwrap()
            .allows("192.0.2.1".parse().unwrap())
    );
}

#[test]
fn empty_restricted_and_mixed_invalid_rules_cannot_become_public() {
    assert!(matches!(
        request(ExposureMode::Restricted, vec![]).validate(),
        Err(Error::EmptySources)
    ));
    assert!(matches!(
        request(ExposureMode::Restricted, vec![cidr("0.0.0.0/0"), cidr("invalid")]).validate(),
        Err(Error::Cidr)
    ));
    for mode in [ExposureMode::Off, ExposureMode::Public] {
        assert!(matches!(
            request(mode, vec![cidr("0.0.0.0/0")]).validate(),
            Err(Error::UnexpectedSources)
        ));
    }
}

#[test]
fn named_rules_and_tailnet_mode_are_facts_without_direct_identity_authority() {
    for source in [
        SourceRule::DnsName("HOST.Test.TS.Net.".into()),
        SourceRule::Tag("tag:prod".into()),
    ] {
        let policy = request(ExposureMode::Restricted, vec![source]).validate().unwrap();
        assert!(policy.has_named_sources());
        assert!(!policy.allows("100.64.0.1".parse().unwrap()));
        assert!(policy.matches_tailnet("100.64.0.1".parse().unwrap(), "host.test.ts.net", &["tag:prod".into()]));
    }
    let policy = request(ExposureMode::TailnetOnly, vec![]).validate().unwrap();
    assert!(!policy.allows("100.64.0.1".parse().unwrap()));
    assert!(!policy.matches_tailnet("100.64.0.1".parse().unwrap(), "host.test.ts.net", &["tag:prod".into()]));
    assert!(matches!(
        request(ExposureMode::TailnetOnly, vec![cidr("0.0.0.0/0")]).validate(),
        Err(Error::UnexpectedSources)
    ));
}

#[test]
fn dns_only_rules_compare_normalized_complete_names_without_other_source_matches() {
    let policy = request(
        ExposureMode::Restricted,
        vec![SourceRule::DnsName("host.test.ts.net".into())],
    )
    .validate()
    .unwrap();
    let source_ip = "192.0.2.1".parse().unwrap();
    for name in [
        "host.test.ts.net",
        "HOST.Test.TS.Net",
        "host.test.ts.net.",
        "HOST.Test.TS.Net.",
    ] {
        assert!(policy.matches_tailnet(source_ip, name, &[]), "{name}");
    }
    for name in [
        "host",
        "test.ts.net",
        "prefix.host.test.ts.net",
        "host.test.ts.net.evil",
        "host.test.ts.net..",
        "host..test.ts.net",
        "*.test.ts.net",
        "host.test.ts.net/",
        " host.test.ts.net",
        "-host.test.ts.net",
    ] {
        assert!(!policy.matches_tailnet(source_ip, name, &[]), "{name}");
    }
    assert!(!policy.allows(source_ip));
}

#[test]
fn mixed_cidr_full_dns_and_tag_rules_have_exact_or_semantics() {
    let policy = request(
        ExposureMode::Restricted,
        vec![
            cidr("100.64.0.0/10"),
            SourceRule::DnsName("Host.Test.TS.Net.".into()),
            SourceRule::Tag("tag:prod".into()),
        ],
    )
    .validate()
    .unwrap();
    for (ip, name, tags) in [
        ("100.64.0.1", "other.test.ts.net", vec![]),
        ("192.0.2.1", "host.test.ts.net", vec![]),
        ("192.0.2.1", "other.test.ts.net", vec!["tag:prod".into()]),
    ] {
        assert!(policy.matches_tailnet(ip.parse().unwrap(), name, &tags));
    }
    for (name, tags) in [
        ("host", vec![]),
        ("prefix.host.test.ts.net", vec![]),
        ("host.test.ts.net.evil", vec![]),
        ("other.test.ts.net", vec!["tag:production".into()]),
        ("other.test.ts.net", vec!["tag:Prod".into()]),
    ] {
        assert!(!policy.matches_tailnet("192.0.2.1".parse().unwrap(), name, &tags));
    }
    assert!(policy.allows("100.64.0.1".parse().unwrap()));
    assert!(!policy.allows("127.0.0.1".parse().unwrap()));
}

#[test]
fn malformed_identity_rule_never_falls_back_to_a_valid_cidr() {
    for name in [
        "host",
        "*.test.ts.net",
        ".test.ts.net",
        "host.test.ts.net..",
        "host.test.ts.net/",
        "-host.test.ts.net",
        "host test.ts.net",
    ] {
        assert!(matches!(
            request(
                ExposureMode::Restricted,
                vec![cidr("0.0.0.0/0"), SourceRule::DnsName(name.into())]
            )
            .validate(),
            Err(Error::DnsName)
        ));
    }
    for value in [
        "prod",
        "tag:",
        "tag:*",
        "tag:1prod",
        "tag:prod/other",
        "tag:prod_name",
        "tag:prod ",
    ] {
        assert!(matches!(
            request(
                ExposureMode::Restricted,
                vec![cidr("0.0.0.0/0"), SourceRule::Tag(value.into())]
            )
            .validate(),
            Err(Error::Tag)
        ));
    }
    for rule in [
        SourceRule::DnsName(format!("{}.test.ts.net", "x".repeat(64))),
        SourceRule::Tag(format!("tag:{}", "x".repeat(64))),
    ] {
        assert!(request(ExposureMode::Restricted, vec![rule]).validate().is_err());
    }
}

#[test]
fn management_control_proxy_and_ingress_ports_are_not_targets() {
    for port in BLOCKED_PORTS
        .into_iter()
        .chain(RESERVED_INGRESS_START..=RESERVED_BACKEND_END)
        .chain([0])
    {
        let mut input = request(ExposureMode::Public, vec![]);
        input.target_port = port;
        assert!(matches!(input.validate(), Err(Error::TargetPort)), "{port}");
    }
    for port in [80, 443, 8080, 65535] {
        let mut input = request(ExposureMode::Public, vec![]);
        input.target_port = port;
        assert_eq!(input.validate().unwrap().target_port(), port);
    }
}

#[test]
fn malformed_fields_counts_limits_and_encoded_bytes_are_bounded() {
    for value in [
        br#"{"target_port":8080,"target_port":80}"#.as_slice(),
        br#"{"target_port":8080,"peer":"192.0.2.1"}"#,
        br#"{"target_port":8080,"mode":"anyone"}"#,
        br#"{"target_port":8080,"limits":{"total":1,"per_source":1,"unlimited":true}}"#,
        br#"{"target_port":8080,"sources":[{"kind":"cidr","value":"0.0.0.0/0","trusted":true}]}"#,
    ] {
        assert_eq!(ExposureRequest::from_json(value), Err(Error::Encoding));
    }
    assert_eq!(
        ExposureRequest::from_json(&vec![b' '; MAX_REQUEST_BYTES + 1]),
        Err(Error::RequestSize)
    );
    let mut input = request(ExposureMode::Restricted, vec![cidr("0.0.0.0/0"); MAX_SOURCE_RULES]);
    assert!(input.validate().is_ok());
    input.sources.push(cidr("0.0.0.0/0"));
    assert!(matches!(input.validate(), Err(Error::SourceCount)));
    for limits in [
        ConnectionLimits {
            total: 0,
            per_source: 1,
        },
        ConnectionLimits {
            total: 129,
            per_source: 1,
        },
        ConnectionLimits {
            total: 1,
            per_source: 0,
        },
        ConnectionLimits {
            total: 1,
            per_source: 2,
        },
    ] {
        let mut input = request(ExposureMode::Public, vec![]);
        input.limits = limits;
        assert!(matches!(input.validate(), Err(Error::ConnectionLimits)));
    }
}

#[test]
fn ipv4_and_ipv6_prefixes_match_exact_families_and_boundaries() {
    for (network, allowed, denied) in [
        (
            "192.0.2.129/24",
            vec!["192.0.2.0", "192.0.2.255"],
            vec!["192.0.1.255", "192.0.3.0", "2001:db8::1"],
        ),
        (
            "2001:db8:1::abcd/64",
            vec!["2001:db8:1::", "2001:db8:1:0:ffff:ffff:ffff:ffff"],
            vec!["2001:db8:2::", "192.0.2.1"],
        ),
        ("192.0.2.1/32", vec!["192.0.2.1"], vec!["192.0.2.0", "192.0.2.2"]),
        (
            "2001:db8::1/128",
            vec!["2001:db8::1"],
            vec!["2001:db8::", "2001:db8::2"],
        ),
    ] {
        let policy = request(ExposureMode::Restricted, vec![cidr(network)])
            .validate()
            .unwrap();
        for ip in allowed {
            assert!(policy.allows(ip.parse().unwrap()), "{network} {ip}");
        }
        for ip in denied {
            assert!(!policy.allows(ip.parse().unwrap()), "{network} {ip}");
        }
    }
}

#[test]
fn mapped_ipv6_cannot_cross_an_ipv6_allowlist() {
    let policy = request(ExposureMode::Restricted, vec![cidr("::ffff:192.0.2.129/120")])
        .validate()
        .unwrap();
    for ip in ["192.0.2.1", "::ffff:192.0.2.1"] {
        assert!(policy.allows(ip.parse().unwrap()));
    }
    let all_v6 = request(ExposureMode::Restricted, vec![cidr("::/0")])
        .validate()
        .unwrap();
    assert!(!all_v6.allows("::ffff:192.0.2.1".parse().unwrap()));
    assert!(all_v6.allows("2001:db8::1".parse().unwrap()));
    assert!(matches!(
        request(ExposureMode::Restricted, vec![cidr("::ffff:192.0.2.1/95")]).validate(),
        Err(Error::Cidr)
    ));
    let all_v4 = request(ExposureMode::Restricted, vec![cidr("0.0.0.0/0")])
        .validate()
        .unwrap();
    assert!(all_v4.allows("255.255.255.255".parse().unwrap()));
}

#[test]
fn malformed_prefixes_and_oversized_sources_fail_closed() {
    for value in [
        "192.0.2.1",
        "192.0.2.0/33",
        "::/129",
        "192.0.2.0/-1",
        "192.0.2.0/+1",
        "192.0.2.0/01",
        "192.0.2.0/24 ",
        " 192.0.2.0/24",
        "example.com/24",
        "fe80::1%lo/64",
        "0.0.0.0/0/0",
    ] {
        assert!(
            matches!(
                request(ExposureMode::Restricted, vec![cidr(value)]).validate(),
                Err(Error::Cidr)
            ),
            "{value}"
        );
    }
    assert!(matches!(
        request(ExposureMode::Restricted, vec![cidr(&"x".repeat(255))]).validate(),
        Err(Error::Cidr)
    ));
}

#[test]
fn backend_ports_are_derived_facts_not_accepted_service_targets() {
    for virtual_port in RESERVED_INGRESS_START..=RESERVED_INGRESS_END {
        let backend = producer_backend_port(virtual_port).unwrap();
        assert!((RESERVED_BACKEND_START..=RESERVED_BACKEND_END).contains(&backend));
        let mut input = request(ExposureMode::Public, vec![]);
        input.target_port = backend;
        assert!(matches!(input.validate(), Err(Error::TargetPort)));
    }
    for port in [0, 19999, 20032, 65535] {
        assert_eq!(producer_backend_port(port), None);
    }
}

#[test]
fn https_routes_require_explicit_public_policy_and_a_supported_port() {
    assert_eq!(Route::default(), Route::Gate {});
    for https in [443, 8443, 10000] {
        let route = Route::FunnelHttps { https };
        assert!(route.validate_for(&request(ExposureMode::Public, vec![])).is_ok());
        for mode in [ExposureMode::Off, ExposureMode::TailnetOnly, ExposureMode::Restricted] {
            let sources = if mode == ExposureMode::Restricted {
                vec![cidr("192.0.2.0/24")]
            } else {
                vec![]
            };
            assert!(route.validate_for(&request(mode, sources)).is_err());
        }
        assert!(route.accepts_ingress(https));
        assert!(!route.accepts_ingress(RESERVED_INGRESS_START));
    }
    for https in [0, 80, 8080, 65535] {
        let route = Route::FunnelHttps { https };
        assert!(route.validate_for(&request(ExposureMode::Public, vec![])).is_err());
        assert!(route.url("host.test.ts.net").is_err());
        assert!(!route.accepts_ingress(https));
    }
    for bytes in [
        br#"{"kind":"gate","https":443}"#.as_slice(),
        br#"{"kind":"funnel_https","https":443,"https":8443}"#,
        br#"{"kind":"funnel_https","https":443,"approved":true}"#,
        br#"{"kind":"public"}"#,
    ] {
        assert!(serde_json::from_slice::<Route>(bytes).is_err());
    }
}

#[test]
fn endpoint_facts_use_exact_canonical_dns_and_never_select_a_raw_fallback() {
    let gate = Route::Gate {};
    assert_eq!(gate.observed_url("host.test.ts.net", None).unwrap(), None);
    assert_eq!(
        gate.observed_url("host.test.ts.net", Some(20000)).unwrap(),
        Some("tcp://host.test.ts.net:20000".into())
    );
    for port in [0, 19999, 20032, 65535] {
        assert!(gate.observed_url("host.test.ts.net", Some(port)).is_err());
    }
    for name in ["host", "Host.Test.TS.Net", "host.test.ts.net.", "host.test.ts.net/path"] {
        assert!(gate.observed_url(name, Some(20000)).is_err());
        assert!(Route::FunnelHttps { https: 443 }.url(name).is_err());
    }
    for (https, expected) in [
        (443, "https://host.test.ts.net/"),
        (8443, "https://host.test.ts.net:8443/"),
        (10000, "https://host.test.ts.net:10000/"),
    ] {
        let route = Route::FunnelHttps { https };
        assert_eq!(
            route.observed_url("host.test.ts.net", None).unwrap(),
            Some(expected.into())
        );
        assert!(route.observed_url("host.test.ts.net", Some(20000)).is_err());
    }
    assert!(gate.accepts_ingress(20000));
    assert!(gate.accepts_ingress(20031));
    assert!(!gate.accepts_ingress(20032));
}
