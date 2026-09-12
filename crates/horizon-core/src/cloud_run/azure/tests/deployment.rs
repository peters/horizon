use super::super::{
    AzureDeploymentPlan, AzureError, RESOURCE_GROUP_PREFIX,
    deployment::{
        SSH_PORT, TAG_CLIENT_KEY_DIGEST, TAG_DISK_GIB, TAG_IMAGE_DIGEST, TAG_IMAGE_REF_DIGEST, TAG_JOB, TAG_LIFETIME,
        TAG_PROFILE, TAG_PROTOCOL, TAG_WORKFLOW, client_key_digest, worker_tags,
    },
};
use super::{IMAGE, ed25519_key, profile, target};
use crate::cloud_run::{
    CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudWorkflowId, WorkerLifetime,
    interactive_worker::InteractiveWorkerRequest,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn request(comment: &str) -> InteractiveWorkerRequest {
    InteractiveWorkerRequest {
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        target: target(),
        ssh_public_key: ed25519_key(7, comment),
    }
}

fn decoded_cloud_init(plan: &AzureDeploymentPlan) -> String {
    let encoded = plan.parameters["customData"]["value"].as_str().expect("customData");
    String::from_utf8(STANDARD.decode(encoded).expect("base64")).expect("utf8")
}

#[test]
fn plan_derives_identity_parameters_and_tags_from_validated_inputs() {
    let request = request("operator@example");
    let plan = AzureDeploymentPlan::new(&profile(), &request).expect("plan");
    assert_eq!(
        plan.resource_group,
        format!("{RESOURCE_GROUP_PREFIX}{}-{}", request.workflow_id, request.job_id)
    );
    assert_eq!(plan.location, "northeurope");
    let parameters = &plan.parameters;
    assert_eq!(parameters["vmName"]["value"], "worker");
    assert_eq!(parameters["vmSize"]["value"], "Standard_D4s_v3");
    assert_eq!(parameters["adminPublicKey"]["value"], request.ssh_public_key);
    assert_eq!(parameters["identityId"]["value"], profile().image_pull_identity_id);
    assert_eq!(parameters["dataDiskGib"]["value"], 32);
    assert_eq!(
        parameters["tags"]["value"],
        serde_json::to_value(&plan.tags).expect("tags")
    );
    let expected_tags = worker_tags(&request);
    assert_eq!(plan.tags, expected_tags);
    assert_eq!(plan.tags[TAG_WORKFLOW], request.workflow_id.to_string());
    assert_eq!(plan.tags[TAG_JOB], request.job_id.to_string());
    assert_eq!(plan.tags[TAG_PROTOCOL], CLOUD_RUN_PROTOCOL_VERSION.to_string());
    assert_eq!(plan.tags[TAG_LIFETIME], "persistent");
    assert_eq!(
        plan.tags[TAG_IMAGE_DIGEST],
        IMAGE.rsplit_once("@sha256:").expect("digest").1
    );
    assert_eq!(
        plan.tags[TAG_CLIENT_KEY_DIGEST],
        client_key_digest(&request.ssh_public_key)
    );
    assert_eq!(plan.tags[TAG_CLIENT_KEY_DIGEST].len(), 64);
    assert_ne!(
        client_key_digest(&request.ssh_public_key),
        client_key_digest(&ed25519_key(8, ""))
    );
    assert_eq!(
        client_key_digest("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(plan.tags[TAG_DISK_GIB], "32");
    assert_eq!(
        plan.tags[TAG_PROFILE],
        client_key_digest("cpu-north"),
        "profile name hashed, tag-safe"
    );
    assert_eq!(
        plan.tags[TAG_IMAGE_REF_DIGEST],
        client_key_digest(IMAGE),
        "sha256 of the full reference"
    );
    let mut other_repository = request.clone();
    other_repository.target.image = IMAGE.replace("horizon-remote-worker", "other-worker");
    assert_ne!(
        worker_tags(&other_repository)[TAG_IMAGE_REF_DIGEST],
        plan.tags[TAG_IMAGE_REF_DIGEST]
    );
    assert_eq!(
        worker_tags(&other_repository)[TAG_IMAGE_DIGEST],
        plan.tags[TAG_IMAGE_DIGEST],
        "same manifest digest"
    );
    assert_eq!(plan.tags.len(), 9);
    assert!(
        plan.tags
            .values()
            .all(|value| value.len() <= 256 && !value.chars().any(char::is_control))
    );
}

#[test]
fn template_pins_api_versions_identity_disks_and_closed_network_path() {
    let plan = AzureDeploymentPlan::new(&profile(), &request("")).expect("plan");
    let template = &plan.template;
    let resources = template["resources"].as_array().expect("resources");
    let kinds: Vec<&str> = resources
        .iter()
        .filter_map(|resource| resource["type"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "Microsoft.Network/networkSecurityGroups",
            "Microsoft.Network/virtualNetworks",
            "Microsoft.Network/publicIPAddresses",
            "Microsoft.Compute/disks",
            "Microsoft.Network/networkInterfaces",
            "Microsoft.Compute/virtualMachines",
        ]
    );
    for resource in resources {
        assert_eq!(resource["tags"], "[parameters('tags')]", "{}", resource["type"]);
        assert_eq!(resource["location"], "[parameters('location')]");
        assert!(resource["apiVersion"].as_str().is_some_and(|api| api.starts_with("20")));
    }
    let rule = &resources[0]["properties"]["securityRules"][0]["properties"];
    assert_eq!(rule["destinationPortRange"], SSH_PORT.to_string());
    assert_eq!(rule["access"], "Allow");
    assert_eq!(rule["protocol"], "Tcp");
    assert_eq!(
        resources[0]["properties"]["securityRules"].as_array().map(Vec::len),
        Some(1),
        "only the container SSH port"
    );
    assert_eq!(resources[2]["properties"]["publicIPAllocationMethod"], "Static");
    assert_eq!(resources[2]["sku"]["name"], "Standard");
    let disk = &resources[3];
    assert_eq!(disk["properties"]["diskSizeGB"], "[parameters('dataDiskGib')]");
    assert_eq!(disk["properties"]["creationData"]["createOption"], "Empty");
    assert_eq!(disk["sku"]["name"], "[parameters('diskSku')]");
    assert_eq!(plan.parameters["diskSku"]["value"], "Premium_LRS");
    assert_eq!(
        template["parameters"]["diskSku"]["allowedValues"],
        serde_json::json!(["StandardSSD_LRS", "Premium_LRS"])
    );
    let vm = &resources[5];
    assert_eq!(vm["apiVersion"], "2024-03-01");
    assert_eq!(vm["identity"]["type"], "UserAssigned");
    assert!(vm["identity"]["userAssignedIdentities"]["[parameters('identityId')]"].is_object());
    let storage = &vm["properties"]["storageProfile"];
    assert_eq!(storage["imageReference"]["offer"], "ubuntu-24_04-lts");
    assert_eq!(storage["dataDisks"][0]["createOption"], "Attach");
    assert_eq!(
        storage["dataDisks"][0]["deleteOption"], "Detach",
        "the data disk outlives the VM"
    );
    assert_eq!(
        storage["dataDisks"][0]["managedDisk"]["id"],
        "[resourceId('Microsoft.Compute/disks', variables('data'))]"
    );
    assert_eq!(storage["osDisk"]["deleteOption"], "Delete");
    assert!(
        vm["dependsOn"]
            .as_array()
            .expect("dependsOn")
            .contains(&serde_json::json!(
                "[resourceId('Microsoft.Compute/disks', variables('data'))]"
            ))
    );
    let linux = &vm["properties"]["osProfile"]["linuxConfiguration"];
    assert_eq!(linux["disablePasswordAuthentication"], true);
    assert_eq!(
        linux["ssh"]["publicKeys"][0]["keyData"],
        "[parameters('adminPublicKey')]"
    );
    assert_eq!(
        vm["properties"]["osProfile"]["customData"],
        "[parameters('customData')]"
    );
    assert_eq!(template["parameters"]["adminPublicKey"]["type"], "securestring");
    assert_eq!(template["parameters"]["customData"]["type"], "securestring");
    assert_eq!(template["parameters"]["dataDiskGib"]["maxValue"], 4_095);
    assert_eq!(
        template["outputs"]["publicIp"]["value"],
        "[reference(variables('pip')).ipAddress]"
    );
    assert_eq!(template["variables"]["pip"], "[concat(parameters('vmName'), '-pip')]");
    assert!(
        !template.to_string().contains("Microsoft.DevTestLab"),
        "no schedule for persistent workers"
    );
}

#[test]
fn cloud_init_carries_the_key_as_a_file_and_never_interpolates_operator_text() {
    let hostile = "operator'; rm -rf / #\"$(x)";
    let request = request(hostile);
    let plan = AzureDeploymentPlan::new(&profile(), &request).expect("plan");
    let cloud_init = decoded_cloud_init(&plan);
    assert!(cloud_init.starts_with("#cloud-config\n"));
    assert!(
        !cloud_init.contains(hostile),
        "the raw key text never appears in cloud-init"
    );
    assert!(
        !cloud_init.contains("ssh-ed25519"),
        "the key travels only as a base64 file"
    );
    assert!(cloud_init.contains(&format!("content: {}", STANDARD.encode(&request.ssh_public_key))));
    assert!(cloud_init.contains("path: /etc/horizon-worker/client-key.pub"));
    assert!(cloud_init.contains("permissions: '0600'"));
    assert!(cloud_init.contains(&format!("docker pull -q '{IMAGE}'")));
    assert!(cloud_init.contains("-e \"HORIZON_SSH_PUBLIC_KEY=$(cat /etc/horizon-worker/client-key.pub)\""));
    assert!(cloud_init.contains(&format!("-p {SSH_PORT}:22")));
    assert!(cloud_init.contains("src=/mnt/horizon-workspace,dst=/workspace"));
    assert!(
        cloud_init.contains("wipefs --noheadings --output TYPE \"$DISK\""),
        "every signature is inspected"
    );
    assert!(
        cloud_init.contains("\"\") mkfs.ext4 -q -L horizonws \"$DISK\" ;;"),
        "only a signature-free disk is formatted"
    );
    assert!(cloud_init.contains("\"ext4 \") ;;"), "a lone ext4 signature is reused");
    assert!(cloud_init.contains("refusing data disk with unexpected signatures"));
    assert!(cloud_init.contains("packages: [docker.io, iptables-persistent]"));
    let bootstrap = cloud_init
        .split("horizon-worker-bootstrap.sh")
        .nth(1)
        .expect("bootstrap script");
    let position = |needle: &str| bootstrap.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(position("iptables -I DOCKER-USER -d 169.254.169.254 -j DROP") < position("docker create"));
    assert!(position("iptables -I INPUT -i docker0 -p tcp --dport 22 -j DROP") < position("docker create"));
    assert!(position("netfilter-persistent save") < position("docker create"));
    assert!(position("systemctl disable --now ssh.socket ssh.service") < position("docker create"));
    assert!(position("systemctl enable --now docker") < position("iptables -I DOCKER-USER"));
    assert!(
        cloud_init.contains("CURL=\"curl -sf --max-time 20 --retry 5 --retry-delay 3 --retry-connrefused\""),
        "bounded, retried metadata and registry calls"
    );
    assert!(cloud_init.contains("timeout 60 docker login"), "bounded login");
    assert!(
        cloud_init.contains("PULL_ERROR=$(timeout 900 docker pull -q"),
        "bounded pull attempts"
    );
    assert!(cloud_init.contains("*unauthorized*|*denied*|*\"authentication required\"*|*\"not found\"*|*\"manifest unknown\"*) echo \"$PULL_ERROR\" >&2; exit 1 ;;"), "authentication and missing-image failures are not retried");
    assert!(
        cloud_init.contains("[ \"$attempt\" = 5 ] && { echo \"$PULL_ERROR\" >&2; exit 1; }"),
        "bounded retries"
    );
    assert!(cloud_init.contains("export DOCKER_CONFIG=$(mktemp -d /run/horizon-docker-login.XXXXXX)"));
    assert!(cloud_init.contains("trap 'rm -rf \"$DOCKER_CONFIG\"' EXIT"));
    assert!(cloud_init.contains("RequiresMountsFor=/mnt/horizon-workspace"));
    assert!(
        cloud_init.contains("docker login example.azurecr.io -u 00000000-0000-0000-0000-000000000000 --password-stdin")
    );
    assert!(
        cloud_init.contains("msi_res_id=%2Fsubscriptions%2F"),
        "identity selected by resource id, percent-encoded"
    );
    assert!(
        cloud_init.contains("/dev/disk/azure/data/by-lun/0 /dev/disk/azure/scsi1/lun0"),
        "NVMe and SCSI data disk paths"
    );
    assert!(!cloud_init.contains("HORIZON_TERMINATE_AFTER"));
    assert!(!cloud_init.contains("bootcmd"), "no early-boot systemctl calls");
    for line in cloud_init.lines() {
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }
}

#[test]
fn plan_rejects_unsupported_lifetimes_and_targets_before_any_io() {
    let mut limited = request("");
    limited.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3_600 };
    assert_eq!(
        AzureDeploymentPlan::new(&profile(), &limited),
        Err(AzureError::UnsupportedLifetime)
    );
    let mut other_registry = request("");
    other_registry.target.image = IMAGE.replace("example.azurecr.io", "other.azurecr.io");
    assert_eq!(
        AzureDeploymentPlan::new(&profile(), &other_registry),
        Err(AzureError::InvalidTarget)
    );
    let mut bad_key = request("");
    bad_key.ssh_public_key = "ssh-rsa AAAA".into();
    assert_eq!(
        AzureDeploymentPlan::new(&profile(), &bad_key),
        Err(AzureError::InvalidTarget)
    );
    let mut huge = request("");
    huge.target.disk_gib = 4_096;
    assert_eq!(
        AzureDeploymentPlan::new(&profile(), &huge),
        Err(AzureError::InvalidTarget)
    );
    let mut costly = request("");
    costly.target.max_hourly_cost_micros = Some(1);
    assert!(matches!(
        AzureDeploymentPlan::new(&profile(), &costly),
        Err(AzureError::DeclaredCostExceedsLimit { .. })
    ));
    let mut bad_profile = profile();
    bad_profile.location = "North Europe".into();
    assert_eq!(
        AzureDeploymentPlan::new(&bad_profile, &request("")),
        Err(AzureError::InvalidProfile)
    );
}

type Mutation = Box<dyn Fn(&mut InteractiveWorkerRequest)>;

#[test]
fn target_validation_is_complete_single_sourced_and_shared_with_the_constructor() {
    let ok = request("");
    assert_eq!(AzureDeploymentPlan::validate_target(&profile(), &ok.target), Ok(()));
    // Boundaries: the disk maximum and the accepted image bytes.
    let mut edge = request("");
    edge.target.disk_gib = 4_095;
    assert_eq!(AzureDeploymentPlan::validate_target(&profile(), &edge.target), Ok(()));
    assert!(AzureDeploymentPlan::new(&profile(), &edge).is_ok());
    let mut over = request("");
    over.target.disk_gib = 4_096;
    assert_eq!(
        AzureDeploymentPlan::validate_target(&profile(), &over.target),
        Err(AzureError::InvalidTarget)
    );
    let mut zero = request("");
    zero.target.disk_gib = 0;
    assert_eq!(
        AzureDeploymentPlan::validate_target(&profile(), &zero.target),
        Err(AzureError::InvalidTarget)
    );
    // Every target-only refusal the constructor makes is made by the helper first, with
    // the same error, and every helper acceptance is accepted by the constructor: one
    // contract, no second copy.
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "time-limited lifetime",
            Box::new(|r| r.target.lifetime = WorkerLifetime::TimeLimited { seconds: 600 }),
        ),
        (
            "other registry",
            Box::new(|r| r.target.image = IMAGE.replace("example.azurecr.io", "other.azurecr.io")),
        ),
        (
            "mutable tag",
            Box::new(|r| r.target.image = "example.azurecr.io/horizon-remote-worker:latest".into()),
        ),
        (
            "shell text in image",
            Box::new(|r| r.target.image = format!("{IMAGE};id")),
        ),
        ("space in image", Box::new(|r| r.target.image = format!("{IMAGE} x"))),
        (
            "other profile name",
            Box::new(|r| r.target.profile = "cpu-south".into()),
        ),
        (
            "cost limit below declared",
            Box::new(|r| r.target.max_hourly_cost_micros = Some(1)),
        ),
        (
            "zero cost limit",
            Box::new(|r| r.target.max_hourly_cost_micros = Some(0)),
        ),
        ("disk over maximum", Box::new(|r| r.target.disk_gib = 4_096)),
    ];
    for (label, mutate) in cases {
        let mut request = request("");
        mutate(&mut request);
        let helper = AzureDeploymentPlan::validate_target(&profile(), &request.target);
        let constructor = AzureDeploymentPlan::new(&profile(), &request).map(|_| ());
        assert!(helper.is_err(), "{label}: helper accepts");
        assert_eq!(helper, constructor, "{label}: helper and constructor disagree");
    }
    // What the helper cannot see is still refused by the constructor: the client key.
    let mut bad_key = request("");
    bad_key.ssh_public_key = "ssh-rsa AAAA".into();
    assert_eq!(
        AzureDeploymentPlan::validate_target(&profile(), &bad_key.target),
        Ok(())
    );
    assert_eq!(
        AzureDeploymentPlan::new(&profile(), &bad_key),
        Err(AzureError::InvalidTarget)
    );
    let mut bad_profile = profile();
    bad_profile.location = "North Europe".into();
    assert_eq!(
        AzureDeploymentPlan::validate_target(&bad_profile, &request("").target),
        Err(AzureError::InvalidProfile)
    );
}
