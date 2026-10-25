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
        exposure_ingress: false,
        platform: Platform::Prepared,
        purpose: Purpose::Worker,
    }
}

fn parse(plan: &Plan) -> Value {
    let document = plan.cloud_config().unwrap();
    assert!(document.starts_with("#cloud-config\n"));
    serde_yaml::from_str(&document).unwrap()
}

#[test]
fn protected_start_is_explicit_and_the_default_keeps_its_original_bootstrap_files() {
    let mut candidate = plan();
    let ordinary = parse(&candidate);
    assert_eq!(content(&ordinary, START), start_script(2, Platform::Prepared));
    assert!(
        ordinary["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .all(|f| f["path"] != INGRESS)
    );
    candidate.exposure_ingress = true;
    let protected = parse(&candidate);
    assert_eq!(
        content(&protected, START),
        format!("#!/bin/sh\nset -eu\nexec /usr/bin/python3 -I {INGRESS} 2 {WORKSPACE}\n")
    );
    assert_eq!(content(&protected, INGRESS), include_str!("ingress.py"));
    assert_eq!(file(&protected, INGRESS)["permissions"], "0700");
    for path in [ENVIRONMENT_FILE, IMAGE_FILE, SETUP, GUARD, UNIT, REGISTRY_FILE] {
        assert_eq!(file(&protected, path), file(&ordinary, path));
    }
    assert_eq!(protected["mounts"], ordinary["mounts"]);
    assert_eq!(protected["runcmd"], ordinary["runcmd"]);
    candidate.environment.remove("HORIZON_CLOUD_OPERATION");
    assert!(candidate.cloud_config().is_err());
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

fn azure_plan() -> Plan {
    Plan {
        workspace_device: AZURE_DATA_DISK.into(),
        platform: Platform::Azure,
        ..plan()
    }
}

/// The guard every host rendered before Azure existed, byte for byte.
const PREPARED_GUARD: &str = "#!/bin/sh
set -eu
command -v docker >/dev/null || { echo 'horizon-worker: Docker is not installed on this host' >&2; exit 1; }
mountpoint -q /mnt/horizon-volume || { echo 'horizon-worker: the workspace volume is not mounted' >&2; exit 1; }
mkdir -p /mnt/horizon-volume/workspace
iptables -C DOCKER-USER -d 169.254.0.0/16 -j DROP 2>/dev/null || iptables -I DOCKER-USER -d 169.254.0.0/16 -j DROP
";

#[test]
fn a_prepared_host_keeps_its_user_data() {
    let document = plan().cloud_config().unwrap();
    let config: Value = serde_yaml::from_str(&document).unwrap();
    let keys: Vec<&str> = config
        .as_mapping()
        .unwrap()
        .keys()
        .map(|key| key.as_str().unwrap())
        .collect();
    assert_eq!(keys, ["ssh_pwauth", "disable_root", "mounts", "runcmd", "write_files"]);
    assert_eq!(content(&config, GUARD), PREPARED_GUARD);
    for absent in [
        "package_update",
        "packages",
        "fs_setup",
        "168.63",
        "ip6tables",
        "docker.io",
    ] {
        assert!(!document.contains(absent), "{absent}");
    }
}

fn browser_vm_plan() -> Plan {
    Plan {
        environment: BTreeMap::new(),
        shm_gb: 1,
        purpose: Purpose::BrowserVm,
        ..plan()
    }
}

#[test]
fn a_browser_vm_host_mounts_its_state_volume_and_keeps_its_ssh_route() {
    let document = browser_vm_plan().cloud_config().unwrap();
    let config: Value = serde_yaml::from_str(&document).unwrap();
    // The volume is the state directory of the image, so the profiles and their
    // sign-ins live on it.
    assert_eq!(config["mounts"][0][1].as_str(), Some(BROWSER_VM_STATE));
    assert_eq!(config["mounts"][0][0].as_str(), Some(plan().workspace_device.as_str()));
    // The person signs in on the display through the SSH route of this host, so root
    // logs in with the key registered for the server; only the password is refused.
    assert_eq!(config["disable_root"], Value::Bool(false));
    assert_eq!(config["ssh_pwauth"], Value::Bool(false));
    let setup = content(&config, BROWSER_VM_SETUP);
    assert!(
        !setup.contains("mask"),
        "the Device panel and the bridge reach the machine over ssh"
    );
    assert!(setup.contains("systemctl enable --now horizon-browser-vm.service"));
    let start = content(&config, BROWSER_VM_START);
    assert!(start.contains("-p 127.0.0.1:5900:5900"), "{start}");
    assert!(start.contains("-v /state:/state"), "{start}");
    assert!(
        !start.contains("22:22"),
        "the worker contract's port is not this host's"
    );
    assert!(content(&config, BROWSER_VM_GUARD).contains("mountpoint -q /state"));
    let unit = content(&config, BROWSER_VM_UNIT);
    assert!(unit.contains("RequiresMountsFor=/state") && unit.contains("docker stop horizon-browser-vm"));
    // Nothing of the worker contract is rendered: no environment file, no worker port.
    assert!(
        config["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .all(|file| file["path"] != ENVIRONMENT_FILE)
    );
    assert_eq!(config["runcmd"], serde_yaml::to_value([[BROWSER_VM_SETUP]]).unwrap());
}

#[test]
fn an_azure_host_leaves_every_filesystem_to_its_own_script() {
    let document = azure_plan().cloud_config().unwrap();
    let config: Value = serde_yaml::from_str(&document).unwrap();
    assert_eq!(config["package_update"], Value::Bool(true));
    assert_eq!(config["packages"], serde_yaml::to_value(["docker.io"]).unwrap());
    assert!(
        config.get("fs_setup").is_none(),
        "cloud-init must not format the workspace: it ran mkfs on a device that already held one"
    );
    let setup = content(&config, SETUP);
    assert!(setup.contains(&format!("blkid -o value -s TYPE {AZURE_DATA_DISK}")));
    assert!(setup.contains("mkfs.ext4 -L horizon-ws"));
    assert!(setup.contains(&format!(
        "mountpoint -q {AZURE_VOLUME_MOUNT} || mount {AZURE_VOLUME_MOUNT}"
    )));
    assert_eq!(config["mounts"][0][0].as_str(), Some(AZURE_DATA_DISK));
    assert_eq!(config["mounts"][0][1].as_str(), Some(AZURE_VOLUME_MOUNT));
    let prepared = parse(&plan());
    for path in [ENVIRONMENT_FILE, IMAGE_FILE, REGISTRY_FILE] {
        assert_eq!(file(&config, path), file(&prepared, path), "{path}");
    }
    assert_eq!(config["runcmd"], prepared["runcmd"]);
    let start = content(&config, START);
    assert!(start.contains(&format!("-v {AZURE_VOLUME_MOUNT}/workspace:/workspace")));
    let unit = content(&config, UNIT);
    assert!(unit.contains(&format!("RequiresMountsFor={AZURE_VOLUME_MOUNT}")));
    assert!(!unit.contains(VOLUME_MOUNT));
}

#[test]
fn a_rebuilt_host_never_reformats_the_workspace_disk() {
    // Asserted from the rendered document, never executed: a test must not run host
    // bootstrap on the machine that runs `cargo test`. The behaviour itself is proven on an
    // owned VM by task AZV-L3, which keeps the filesystem UUID and its creation time across
    // a rebuild.
    let config = parse(&azure_plan());
    let setup = content(&config, SETUP);
    let format = format!("mkfs.ext4 -L {WORKSPACE_LABEL} {AZURE_DATA_DISK}");
    assert_eq!(setup.matches("mkfs.ext4").count(), 1, "one format step: {setup}");
    let guarded =
        format!("if [ -z \"$(blkid -o value -s TYPE {AZURE_DATA_DISK} 2>/dev/null)\" ]; then\n  {format}\nfi\n");
    assert!(
        setup.contains(&guarded),
        "the only format step runs inside the empty-device guard: {setup}"
    );
    let mount = format!("mountpoint -q {AZURE_VOLUME_MOUNT} || mount {AZURE_VOLUME_MOUNT}");
    assert!(
        setup.find(&guarded) < setup.find(&mount),
        "the mount follows the guard, so a device that already holds a filesystem is mounted as it is: {setup}"
    );
}

#[test]
fn an_azure_guard_closes_both_metadata_addresses_and_ipv6_before_every_start() {
    let config = parse(&azure_plan());
    let guard = content(&config, GUARD);
    for line in PREPARED_GUARD.lines().filter(|line| !line.contains("horizon-volume")) {
        assert!(guard.lines().any(|guard| guard == line), "{line}");
    }
    assert!(
        guard.contains(&format!("mountpoint -q {AZURE_VOLUME_MOUNT} || {{")),
        "an Azure host checks the mount point the temporary disk cannot shadow"
    );
    assert!(guard.contains(&format!("mkdir -p {AZURE_VOLUME_MOUNT}/workspace")));
    for rule in [
        "iptables -C DOCKER-USER -d 168.63.129.16/32 -p tcp -m multiport --dports 80,32526 -j DROP 2>/dev/null || iptables -I DOCKER-USER -d 168.63.129.16/32 -p tcp -m multiport --dports 80,32526 -j DROP",
        "ip6tables -C FORWARD -i docker0 -j DROP 2>/dev/null || ip6tables -I FORWARD -i docker0 -j DROP",
    ] {
        assert!(guard.lines().any(|line| line == rule), "{rule}");
    }
    assert!(
        !guard.contains("-d 168.63.129.16/32 -j DROP"),
        "the platform address also serves DNS, so only its own ports are closed"
    );
    let unit = content(&config, UNIT);
    assert!(unit.find(&format!("ExecStartPre={GUARD}")).unwrap() < unit.find(&format!("ExecStart={START}")).unwrap());
    let start = content(&config, START);
    for address in ["169.254", "168.63", "ip6tables"] {
        assert!(!start.contains(address), "{address}");
    }
}

#[test]
fn an_azure_host_formats_no_device_but_its_data_disk() {
    for device in [
        "/dev/disk/by-id/scsi-0HC_Volume_101",
        "/dev/sdc",
        "/dev/disk/azure/scsi1/lun1",
        "/dev/disk/azure/scsi1/lun0/../../root",
        "",
    ] {
        let plan = Plan {
            workspace_device: device.into(),
            ..azure_plan()
        };
        assert!(matches!(plan.cloud_config(), Err(CloudError::Invalid(_))), "{device}");
    }
    // A prepared host never takes the Azure path, which it would not format.
    let plan = Plan {
        workspace_device: AZURE_DATA_DISK.into(),
        ..plan()
    };
    assert!(plan.cloud_config().is_err());
}
