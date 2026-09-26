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
        (GUARD, "0700"),
        (START, "0700"),
        (UNIT, "0644"),
    ] {
        assert_eq!(file(&config, path)["permissions"], mode, "{path}");
        assert_eq!(file(&config, path)["owner"], "root:root", "{path}");
    }
    let registry: serde_json::Value = serde_json::from_str(content(&config, REGISTRY_FILE)).unwrap();
    let auth = registry["auths"]["registry.example"]["auth"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(auth).unwrap(),
        b"pull-token:pull-secret-value"
    );
    assert_eq!(
        serde_yaml::to_string(&config["mounts"]).unwrap(),
        "- - /dev/disk/by-id/scsi-0HC_Volume_101\n  - /mnt/horizon-volume\n  - ext4\n  - defaults,nofail,discard\n  - '0'\n  - '2'\n"
    );
    let commands: Vec<Vec<&str>> = config["runcmd"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|command| {
            command
                .as_sequence()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(
        commands[0],
        ["systemctl", "disable", "--now", "ssh.socket", "ssh.service"]
    );
    assert_eq!(commands[1], ["passwd", "--lock", "root"]);
    assert_eq!(
        commands.last().unwrap(),
        &["systemctl", "enable", "--now", "horizon-worker.service"],
        "host sshd is off before the container claims port 22"
    );
    let start = content(&config, START);
    assert!(start.contains("--shm-size 2g"));
    assert!(start.contains("-p 22:22"));
    assert!(start.contains(&format!("-v {WORKSPACE}:/workspace")));
    assert!(start.contains("--pull never"));
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
    assert!(guard.contains("iptables -I DOCKER-USER -d 169.254.169.254/32 -j DROP"));
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
    for script in [GUARD, START, UNIT] {
        assert!(!content(&config, script).contains("rm -rf"), "{script}");
    }
    assert_eq!(config["write_files"].as_sequence().unwrap().len(), 6);
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
