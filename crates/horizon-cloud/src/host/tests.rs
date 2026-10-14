use super::*;
use serde_yaml::Value;

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

fn plan() -> Plan {
    Plan {
        image: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
        environment: BTreeMap::from([
            ("PUBLIC_KEY".into(), KEY.into()),
            ("HORIZON_CLOUD_OPERATION".into(), "op-1".into()),
            ("HORIZON_WORKER_CAPABILITIES".into(), r#"{"agents":["codex"]}"#.into()),
        ]),
        registry: Some(RegistryLogin {
            server: "registry.example".into(),
            username: "pull-token".into(),
            password: Credential::new("pull-secret-value".into()).unwrap(),
        }),
        workspace_device: "/dev/disk/by-id/scsi-0HC_Volume_101".into(),
        shm_gb: 2,
        egress: None,
    }
}

fn parse(plan: &Plan) -> Value {
    let document = plan.cloud_config().unwrap();
    assert!(document.starts_with("#cloud-config\n"));
    serde_yaml::from_str(&document).unwrap()
}

fn file<'a>(config: &'a Value, path: &str) -> &'a Value {
    config["write_files"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} is missing"))
}

fn content<'a>(config: &'a Value, path: &str) -> &'a str {
    file(config, path)["content"].as_str().unwrap()
}

/// Docker's login file, which the configuration carries base64-encoded.
fn registry_json(config: &Value) -> serde_json::Value {
    let entry = file(config, REGISTRY_FILE);
    assert_eq!(entry["encoding"], "b64", "the login never passes through YAML as text");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(entry["content"].as_str().unwrap())
        .unwrap();
    serde_json::from_slice(&decoded).unwrap()
}

#[test]
fn the_plan_becomes_root_only_files_a_volume_mount_and_one_service() {
    let config = parse(&plan());
    assert_eq!(
        content(&config, ENVIRONMENT_FILE),
        format!(
            "HORIZON_CLOUD_OPERATION=op-1\nHORIZON_WORKER_CAPABILITIES={{\"agents\":[\"codex\"]}}\nPUBLIC_KEY={KEY}\n"
        )
    );
    assert_eq!(content(&config, IMAGE_FILE), format!("{}\n", plan().image));
    for (path, mode) in [
        (ENVIRONMENT_FILE, "0600"),
        (IMAGE_FILE, "0600"),
        (REGISTRY_FILE, "0600"),
        (SETUP, "0700"),
        (GUARD, "0700"),
        (START, "0700"),
        (UNIT, "0644"),
    ] {
        assert_eq!(file(&config, path)["permissions"], mode, "{path}");
        assert_eq!(file(&config, path)["owner"], "root:root", "{path}");
    }
    let registry: serde_json::Value = registry_json(&config);
    let auth = registry["auths"]["registry.example"]["auth"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(auth).unwrap(),
        b"pull-token:pull-secret-value"
    );
    assert_eq!(
        serde_yaml::to_string(&config["mounts"]).unwrap(),
        "- - /dev/disk/by-id/scsi-0HC_Volume_101\n  - /mnt/horizon-volume\n  - ext4\n  - defaults,nofail,discard\n  - '0'\n  - '2'\n"
    );
    assert_eq!(config["runcmd"], serde_yaml::to_value([[SETUP]]).unwrap());
    assert_eq!(config["ssh_pwauth"], Value::Bool(false));
    assert_eq!(config["disable_root"], Value::Bool(true));
    let setup = content(&config, SETUP);
    let steps = [
        "set -eu",
        "systemctl mask --now ssh.socket ssh.service",
        "passwd --lock root",
        "systemctl enable --now horizon-worker.service",
    ];
    let positions: Vec<usize> = steps.iter().map(|step| setup.find(step).unwrap()).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the worker starts only after host SSH is masked and root is locked, and any failure stops setup"
    );
    assert!(content(&config, UNIT).contains("Environment=DOCKER_CONFIG=/root/.docker"));
    let start = content(&config, START);
    assert!(start.contains("--shm-size 2g"));
    assert!(start.contains("-p 22:22"));
    assert!(start.contains(&format!("-v {WORKSPACE}:/workspace")));
    assert!(start.contains("--pull never"));
    assert!(start.contains("--log-driver local --log-opt max-size=10m --log-opt max-file=5"));
}

#[test]
fn the_metadata_block_and_volume_check_run_before_every_container_start() {
    let config = parse(&plan());
    let unit = content(&config, UNIT);
    let guard_line = unit.find(&format!("ExecStartPre={GUARD}")).unwrap();
    let start_line = unit.find(&format!("ExecStart={START}")).unwrap();
    assert!(guard_line < start_line);
    assert!(unit.contains(&format!("RequiresMountsFor={VOLUME_MOUNT}")));
    let guard = content(&config, GUARD);
    assert!(guard.contains("iptables -I DOCKER-USER -d 169.254.0.0/16 -j DROP"));
    assert!(guard.contains(&format!("mountpoint -q {VOLUME_MOUNT}")));
    assert!(guard.contains("Docker is not installed"));
    assert!(!content(&config, START).contains("169.254"));
}

#[test]
fn values_are_carried_as_data_and_never_reach_a_script() {
    let mut plan = plan();
    let hostile = r#"'; rm -rf / #" $(reboot) `id` {a: b} - [x] &anchor *alias !!tag %"#;
    plan.environment.insert("HOSTILE".into(), hostile.into());
    let config = parse(&plan);
    assert!(content(&config, ENVIRONMENT_FILE).contains(&format!("HOSTILE={hostile}\n")));
    for script in [SETUP, GUARD, START, UNIT] {
        assert!(!content(&config, script).contains("rm -rf"), "{script}");
    }
    assert_eq!(config["write_files"].as_sequence().unwrap().len(), 7);
}

#[test]
fn invalid_plans_are_refused() {
    let cases: [fn(&mut Plan); 13] = [
        |plan| plan.image = "registry.example/worker:latest".into(),
        |plan| plan.image = "registry.example/worker@sha256:$(id)".into(),
        |plan| {
            plan.environment.insert("lower".into(), "x".into());
        },
        |plan| {
            plan.environment.insert("1NAME".into(), "x".into());
        },
        |plan| {
            plan.environment.insert("NAME".into(), "line\nINJECTED=1".into());
        },
        |plan| {
            plan.environment.insert("NAME".into(), "carriage\rreturn".into());
        },
        |plan| plan.workspace_device = "/dev/sdb".into(),
        |plan| plan.workspace_device = "/dev/disk/by-id/../sda".into(),
        |plan| plan.workspace_device = "/dev/disk/by-id/a b".into(),
        |plan| plan.shm_gb = 0,
        |plan| plan.registry.as_mut().unwrap().server = "Registry.example/path".into(),
        |plan| plan.registry.as_mut().unwrap().username = "user:name".into(),
        |plan| {
            plan.environment.insert("LARGE".into(), "x".repeat(USER_DATA_LIMIT));
        },
    ];
    for (index, change) in cases.iter().enumerate() {
        let mut plan = plan();
        change(&mut plan);
        assert!(
            matches!(plan.cloud_config(), Err(CloudError::Invalid(_))),
            "case {index} was accepted"
        );
    }
}

#[test]
fn a_public_image_needs_no_registry_file_and_secrets_stay_out_of_debug() {
    let mut plan = plan();
    let debug = format!("{plan:?}");
    assert!(!debug.contains("pull-secret-value"));
    plan.registry = None;
    let config = parse(&plan);
    assert!(
        config["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .all(|file| file["path"] != REGISTRY_FILE)
    );
}

#[test]
fn registry_servers_are_host_names_with_an_optional_numeric_port() {
    for valid in [
        "registry.example",
        "registry.example:5000",
        "localhost",
        "a-1.b.example:65535",
    ] {
        assert!(valid_authority(valid), "{valid}");
    }
    for invalid in [
        "",
        ":",
        "registry.example:",
        "registry.example:abc",
        "registry.example:+80",
        "registry.example:0",
        "registry.example:70000",
        "registry.example:1:2",
        "-registry.example",
        "registry-.example",
        "registry..example",
        "Registry.example",
        "[::1]:5000",
        "registry.example/path",
    ] {
        assert!(!valid_authority(invalid), "{invalid}");
    }
}

#[test]
fn docker_hub_logins_use_the_legacy_index_key() {
    for hub in ["docker.io", "index.docker.io", "registry-1.docker.io"] {
        let mut plan = plan();
        plan.registry.as_mut().unwrap().server = hub.into();
        let config = parse(&plan);
        let registry: serde_json::Value = registry_json(&config);
        assert!(
            registry["auths"]["https://index.docker.io/v1/"]["auth"].is_string(),
            "{hub}"
        );
    }
    let config = parse(&plan());
    let registry: serde_json::Value = registry_json(&config);
    assert!(registry["auths"]["registry.example"].is_object());
}

#[test]
fn only_well_formed_digest_references_are_accepted() {
    let digest = format!("sha256:{}", "a".repeat(64));
    for valid in [
        format!("registry.example/worker@{digest}"),
        format!("registry.example:5000/team/worker@{digest}"),
        format!("worker@{digest}"),
        format!("library/worker:1.2_rc-3@{digest}"),
        format!("localhost/worker@{digest}"),
        format!("team/a.b_c__d---e@{digest}"),
        format!("team_one/project/worker@{digest}"),
    ] {
        assert!(valid_digest_reference(&valid), "{valid}");
    }
    for invalid in [
        format!("registry.example/worker@@{digest}"),
        format!("registry.example//worker@{digest}"),
        format!("registry.example/worker/@{digest}"),
        format!("registry.example/Worker@{digest}"),
        format!("registry.example/-worker@{digest}"),
        format!("registry.example/wor..ker@{digest}"),
        format!("registry.example/a.-b@{digest}"),
        format!("registry.example/a___b@{digest}"),
        format!("registry.example/a_.b@{digest}"),
        format!("registry.example/worker-@{digest}"),
        format!("registry.example/worker:@{digest}"),
        format!("registry.example/worker:-tag@{digest}"),
        format!("registry.example:x/worker@{digest}"),
        format!("registry.example/worker@sha256:{}", "A".repeat(64)),
        format!("registry.example/worker@sha256:{}", "a".repeat(63)),
        format!("registry.example/worker@sha512:{}", "a".repeat(64)),
        "registry.example/worker".to_owned(),
    ] {
        assert!(!valid_digest_reference(&invalid), "{invalid}");
    }
}

#[test]
fn repository_paths_stop_at_dockers_255_character_limit() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let at_limit = format!("team/{}", "w".repeat(250));
    assert_eq!(at_limit.len(), 255);
    assert!(valid_digest_reference(&format!(
        "registry.example/{at_limit}:tag@{digest}"
    )));
    assert!(!valid_digest_reference(&format!(
        "registry.example/{at_limit}w@{digest}"
    )));
    assert!(!valid_digest_reference(&format!("{}@{digest}", "w".repeat(256))));
}

#[test]
fn json_is_written_into_a_fixed_wiped_buffer_or_refused() {
    let value = serde_json::json!({"secret": "a\nb"});
    let written = super::json_in_wiped(&value, 64).unwrap();
    assert_eq!(&written[..], br#"{"secret":"a\nb"}"#);
    assert!(
        super::json_in_wiped(&value, 8).is_err(),
        "an oversized value fails without growing the buffer"
    );
}

#[test]
fn a_plan_without_egress_renders_no_egress_files_or_rules() {
    let config = parse(&plan());
    for path in [EGRESS, EGRESS_ALLOW, EGRESS_PROXY_UNIT, EGRESS_DNS_UNIT] {
        assert!(
            config["write_files"]
                .as_sequence()
                .unwrap()
                .iter()
                .all(|file| file["path"] != path),
            "{path}"
        );
    }
    for script in [SETUP, GUARD, START, UNIT] {
        assert!(!content(&config, script).contains("egress"), "{script}");
    }
}

#[test]
fn an_allowlisted_worker_starts_only_behind_the_firewall_on_a_network_without_translation() {
    let mut plan = plan();
    plan.egress = Some(crate::Egress {
        allow: ["*.test.example.no".to_owned()].into(),
    });
    let config = parse(&plan);
    for (path, mode) in [
        (EGRESS, "0700"),
        (EGRESS_ALLOW, "0644"),
        (EGRESS_PROXY_UNIT, "0644"),
        (EGRESS_DNS_UNIT, "0644"),
    ] {
        assert_eq!(file(&config, path)["permissions"], mode, "{path}");
        assert_eq!(file(&config, path)["owner"], "root:root", "{path}");
    }
    assert_eq!(content(&config, EGRESS), EGRESS_SCRIPT);
    let allow = content(&config, EGRESS_ALLOW);
    assert!(allow.starts_with("github.com\n") && allow.ends_with("\n*.test.example.no\n"));

    // The egress services start before the worker, and the worker needs them.
    let setup = content(&config, SETUP);
    let services = setup.find("horizon-worker-egress-dns.service").unwrap();
    assert!(services < setup.find("enable --now horizon-worker.service").unwrap());
    let unit = content(&config, UNIT);
    assert!(unit.contains("After=horizon-worker-egress-proxy.service horizon-worker-egress-dns.service"));
    for mode in ["proxy", "dns"] {
        let service = content(
            &config,
            if mode == "proxy" {
                EGRESS_PROXY_UNIT
            } else {
                EGRESS_DNS_UNIT
            },
        );
        assert!(service.contains(&format!("ExecStart=/usr/bin/python3 -I {EGRESS} {mode}\n")));
        assert!(service.contains("Restart=always"));
    }

    // Every rule is in place before the guard ends; the guard stops at the first failure.
    let guard = content(&config, GUARD);
    assert!(guard.starts_with("#!/bin/sh\nset -eu\n"));
    let order = [
        "iptables -I DOCKER-USER -d 169.254.0.0/16 -j DROP",
        "com.docker.network.bridge.enable_ip_masquerade=false horizon-egress",
        "the egress network translates addresses",
        "iptables -A HORIZON-EGRESS -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN",
        "iptables -A HORIZON-EGRESS -j DROP",
        "iptables -I DOCKER-USER -i horizon0 -j HORIZON-EGRESS",
        "iptables -A HORIZON-EGRESS-IN -d 172.30.0.1 -p tcp --dport 3129 -j ACCEPT",
        "iptables -A HORIZON-EGRESS-IN -d 172.30.0.1 -p udp --dport 53 -j ACCEPT",
        "iptables -A HORIZON-EGRESS-IN -j DROP",
        "iptables -I INPUT -i horizon0 -j HORIZON-EGRESS-IN",
        "iptables -t nat -A HORIZON-EGRESS -p tcp --dport 443 -j REDIRECT --to-ports 3129",
        "iptables -t nat -I PREROUTING -i horizon0 -j HORIZON-EGRESS",
        "ip6tables -I FORWARD -i horizon0 -j DROP",
        "systemctl is-active --quiet",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|step| guard.find(step).unwrap_or_else(|| panic!("{step}")))
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{guard}");
    assert!(guard.contains(&format!("log={WORKSPACE}/.horizon-egress\n")));
    // Nothing follows a path on the volume, which the container can swap for a link.
    assert!(guard.contains("rm -f \"$log\" 2>/dev/null || true\nmkdir -m 0755 \"$log\" 2>/dev/null || true\n"));
    for followed in ["chown", "chmod", "mkdir -p \"$log", "install "] {
        assert!(!guard.contains(followed), "{followed}");
    }
    assert!(guard.contains(
        "[ \"$(docker network inspect -f '{{index .Options \"com.docker.network.bridge.enable_ip_masquerade\"}}' horizon-egress)\" = false ]"
    ));

    let start = content(&config, START);
    assert!(start.contains("--network horizon-egress --dns 172.30.0.1 -p 22:22"));

    // The script listens where the firewall sends the container, and logs where the guard
    // made the folder.
    for (constant, value) in [
        ("GATEWAY", format!("'{EGRESS_GATEWAY}'")),
        ("PROXY_PORT", EGRESS_PROXY_PORT.to_string()),
        ("ALLOW", format!("'{EGRESS_ALLOW}'")),
        ("LOG_DIRECTORY", format!("'{WORKSPACE}/.horizon-egress'")),
    ] {
        assert!(
            EGRESS_SCRIPT.contains(&format!("\n{constant} = {value}\n")),
            "{constant}"
        );
    }
    assert!(EGRESS_SUBNET.starts_with("172.30.0."));
}

#[test]
fn the_largest_allowlisted_plan_fits_the_user_data_limit() {
    let mut plan = plan();
    plan.egress = Some(crate::Egress {
        allow: (0..crate::EGRESS_ALLOW_LIMIT)
            .map(|n| format!("*.{0}.{0}.{0}.n{n:0>58}", "a".repeat(63)))
            .collect(),
    });
    assert!(plan.egress.as_ref().unwrap().validate().is_ok());
    let document = plan.cloud_config().unwrap();
    // Room for the scripts to grow before the limit is a concern.
    assert!(document.len() < USER_DATA_LIMIT - 4 * 1024, "{}", document.len());
}

#[test]
fn an_invalid_allowlist_is_refused() {
    let mut plan = plan();
    plan.egress = Some(crate::Egress {
        allow: ["*.com".to_owned()].into(),
    });
    assert!(matches!(plan.cloud_config(), Err(CloudError::Invalid(_))));
}

/// The live check in `docs/testing/procedures/hetzner-egress.md` renders the user data of a
/// real allowlisted server with this test: `HORIZON_TEST_EGRESS_PLAN` names a JSON file with
/// `image`, `device`, `public_key` and `out`.
#[test]
fn a_live_check_can_render_an_allowlisted_plan() {
    let Some(path) = std::env::var_os("HORIZON_TEST_EGRESS_PLAN") else {
        return;
    };
    let input: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let text = |key: &str| input[key].as_str().unwrap().to_owned();
    let plan = Plan {
        image: text("image"),
        environment: BTreeMap::from([
            ("PUBLIC_KEY".into(), text("public_key")),
            ("HORIZON_CLOUD_OPERATION".into(), "egress-check".into()),
        ]),
        registry: None,
        workspace_device: text("device"),
        shm_gb: 1,
        egress: Some(crate::Egress::default()),
    };
    std::fs::write(text("out"), plan.cloud_config().unwrap().as_bytes()).unwrap();
}
