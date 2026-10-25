use super::*;
use crate::{
    WorkerSpec,
    azure::{
        catalog::VM_SIZES,
        template::{
            ADDRESS_OUTPUT, CUSTOM_DATA_LIMIT, DATA_DISK_NAME, DEPLOYMENT_NAME, OS_DISK_NAME, Planned, custom_data,
            fitting_sizes, template,
        },
    },
    host,
};
use base64::Engine as _;
use std::collections::BTreeMap;

pub(super) fn spec() -> WorkerSpec {
    let profile: crate::Profile = serde_json::from_value(json!({
        "provider": "azure", "image": "registry.example/worker", "cpu": 4, "memory_gb": 16,
        "storage": {"container_gb": 20, "volume_gb": 64}
    }))
    .unwrap();
    WorkerSpec {
        exact_placement: false,
        operation_id: OPERATION_ID.into(),
        image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
        profile,
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".into(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: vec!["Standard_D4s_v3".into()],
        data_centers: vec!["northeurope".into()],
        startup_metadata: None,
    }
}

pub(super) fn size(name: &str) -> &'static crate::azure::catalog::VmSize {
    VM_SIZES.iter().find(|size| size.name == name).unwrap()
}

fn login() -> host::RegistryLogin {
    host::RegistryLogin {
        server: "registry.example".into(),
        username: "reader".into(),
        password: Credential::new("synthetic-pull-secret".into()).unwrap(),
    }
}

fn resources(template: &Value) -> Vec<&Value> {
    template["resources"].as_array().unwrap().iter().collect()
}

fn of_type<'a>(template: &'a Value, kind: &str) -> &'a Value {
    resources(template)
        .into_iter()
        .find(|resource| resource["type"] == kind)
        .unwrap_or_else(|| panic!("{kind} is missing"))
}

#[test]
fn one_deployment_makes_the_network_disks_and_vm_each_tagged() {
    let template = template();
    let kinds: Vec<&str> = resources(&template)
        .iter()
        .map(|resource| resource["type"].as_str().unwrap())
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
            "Microsoft.Resources/tags",
        ]
    );
    for resource in resources(&template) {
        let tags = if resource["type"] == "Microsoft.Resources/tags" {
            &resource["properties"]["tags"]
        } else {
            assert_eq!(resource["location"], "[parameters('location')]");
            &resource["tags"]
        };
        assert_eq!(*tags, "[parameters('tags')]", "{}", resource["type"]);
    }
    // The OS disk is made from the image and tagged by name once the VM exists.
    let os_tags = of_type(&template, "Microsoft.Resources/tags");
    assert_eq!(
        os_tags["scope"],
        format!("[concat('Microsoft.Compute/disks/', '{OS_DISK_NAME}')]")
    );
    assert_eq!(
        os_tags["dependsOn"],
        json!(["[resourceId('Microsoft.Compute/virtualMachines', 'worker')]"])
    );
    assert_eq!(template["parameters"]["customData"]["type"], "securestring");
    assert_eq!(
        template["outputs"][ADDRESS_OUTPUT]["value"],
        "[reference(resourceId('Microsoft.Network/publicIPAddresses', 'worker-ip')).ipAddress]"
    );
}

#[test]
fn only_ssh_is_open_the_address_is_static_and_the_workspace_outlives_the_vm() {
    let template = template();
    let nsg = of_type(&template, "Microsoft.Network/networkSecurityGroups");
    let rules = nsg["properties"]["securityRules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0]["properties"],
        json!({"priority": 100, "direction": "Inbound", "access": "Allow", "protocol": "Tcp",
            "sourceAddressPrefix": "Internet", "sourcePortRange": "*",
            "destinationAddressPrefix": "*", "destinationPortRange": "22"})
    );
    let nic = of_type(&template, "Microsoft.Network/networkInterfaces");
    assert_eq!(
        nic["properties"]["networkSecurityGroup"]["id"],
        "[resourceId('Microsoft.Network/networkSecurityGroups', 'worker-nsg')]"
    );
    let address = of_type(&template, "Microsoft.Network/publicIPAddresses");
    assert_eq!(address["sku"]["name"], "Standard");
    assert_eq!(address["properties"]["publicIPAllocationMethod"], "Static");
    assert_eq!(address["properties"]["publicIPAddressVersion"], "IPv4");
    let disk = of_type(&template, "Microsoft.Compute/disks");
    assert_eq!(disk["name"], DATA_DISK_NAME);
    assert_eq!(disk["properties"]["creationData"]["createOption"], "Empty");
    let vm = of_type(&template, "Microsoft.Compute/virtualMachines");
    let storage = &vm["properties"]["storageProfile"];
    assert_eq!(
        storage["dataDisks"],
        json!([{"lun": 0, "createOption": "Attach", "deleteOption": "Detach",
            "managedDisk": {"id": "[resourceId('Microsoft.Compute/disks', 'worker-workspace')]"}}])
    );
    assert_eq!(storage["osDisk"]["name"], OS_DISK_NAME);
    assert_eq!(storage["osDisk"]["deleteOption"], "Delete");
    assert_eq!(storage["imageReference"]["offer"], "ubuntu-24_04-lts");
    let os = &vm["properties"]["osProfile"];
    assert_eq!(os["customData"], "[parameters('customData')]");
    assert_eq!(os["linuxConfiguration"]["disablePasswordAuthentication"], true);
    assert!(vm.get("identity").is_none(), "the VM holds no managed identity");
}

#[test]
fn the_parameters_carry_the_operation_tag_and_the_azure_bootstrap() {
    let planned = Planned::new(&spec(), "northeurope", size("Standard_D4s_v3"), Some(login())).unwrap();
    assert_eq!(planned.group, GROUP);
    assert_eq!((planned.os_disk_gb, planned.data_disk_gb), (30, 64));
    assert_eq!(
        *planned.tags(),
        BTreeMap::from([(OPERATION_TAG.to_owned(), OPERATION_ID.to_owned())])
    );
    let parameters = planned.parameters();
    assert_eq!(parameters["tags"]["value"], json!({ OPERATION_TAG: OPERATION_ID }));
    assert_eq!(parameters["vmSize"]["value"], "Standard_D4s_v3");
    assert_eq!(parameters["location"]["value"], "northeurope");
    assert!(
        parameters["adminPublicKey"]["value"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 ")
    );
    let encoded = parameters["customData"]["value"].as_str().unwrap();
    assert!(encoded.len() <= CUSTOM_DATA_LIMIT);
    let document = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded).unwrap()).unwrap();
    assert!(document.starts_with("#cloud-config\n"));
    let config: serde_yaml::Value = serde_yaml::from_str(&document).unwrap();
    assert_eq!(config["packages"][0].as_str(), Some("docker.io"));
    assert_eq!(config["mounts"][0][0].as_str(), Some(host::AZURE_DATA_DISK));
    assert!(document.contains("168.63.129.16"));
    assert!(
        document.contains("/root/.docker/config.json"),
        "the pull login is in the bootstrap"
    );
    let debug = format!("{planned:?}");
    assert!(!debug.contains(encoded) && !debug.contains("synthetic-pull-secret"));
    assert_eq!(DEPLOYMENT_NAME, "horizon-worker");
}

#[test]
fn custom_data_is_refused_above_64_kib_once_encoded() {
    // 49,152 bytes encode to exactly 65,536.
    assert_eq!(custom_data(&"a".repeat(49_152)).unwrap().len(), CUSTOM_DATA_LIMIT);
    assert!(matches!(
        custom_data(&"a".repeat(49_153)),
        Err(CloudError::Invalid(
            "Host configuration exceeds Azure's 64 KiB custom data limit"
        ))
    ));
    // The largest bootstrap the host accepts still fits.
    assert!(custom_data(&"a".repeat(host::USER_DATA_LIMIT)).is_ok());
}

/// A pull login whose token is `length` characters, as a long registry token is.
pub(super) fn login_of(length: usize) -> host::RegistryLogin {
    host::RegistryLogin {
        password: Credential::new("t".repeat(length)).unwrap(),
        ..login()
    }
}

#[test]
fn a_bootstrap_too_large_for_a_host_is_refused_while_planning() {
    let fits = Planned::new(&spec(), "northeurope", size("Standard_D4s_v3"), Some(login_of(16_128))).unwrap();
    assert!(fits.parameters()["customData"]["value"].as_str().unwrap().len() <= CUSTOM_DATA_LIMIT);
    let error = Planned::new(&spec(), "northeurope", size("Standard_D4s_v3"), Some(login_of(32_768))).unwrap_err();
    assert!(error.to_string().contains("user-data limit"), "{error}");
}

#[test]
fn plans_that_cannot_succeed_are_refused_before_any_request() {
    type Change = fn(&mut WorkerSpec);
    let cases: [(Change, &str); 5] = [
        (|spec| spec.profile.storage.volume_gb = 3, "between 4 and 32,767"),
        (|spec| spec.operation_id = "o".repeat(80), "at most 79 characters"),
        (
            |spec| spec.image_digest = "registry.example/worker:latest".into(),
            "immutable image digest",
        ),
        (
            |spec| spec.startup_metadata = Some(crate::StartupMetadata::new("{}".into()).unwrap()),
            "Shared workers",
        ),
        (|spec| spec.profile.storage.container_gb = 4_096, "at most 4,095 GB"),
    ];
    for (change, expected) in cases {
        let mut spec = spec();
        change(&mut spec);
        let error = Planned::new(&spec, "northeurope", size("Standard_D4s_v3"), None).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    assert!(Planned::new(&spec(), "North Europe", size("Standard_D4s_v3"), None).is_err());
}

#[test]
fn sizes_are_tried_in_the_settings_order_and_must_fit_the_profile() {
    let allowed = [
        "Standard_F4s_v2",
        "Standard_D4s_v3",
        "Standard_D8s_v3",
        "Standard_D2s_v3",
    ]
    .map(String::from);
    let names: Vec<&str> = fitting_sizes(&allowed, &spec())
        .unwrap()
        .iter()
        .map(|size| size.name)
        .collect();
    assert_eq!(
        names,
        ["Standard_D4s_v3", "Standard_D8s_v3"],
        "4 vCPU and 16 GB at least"
    );
    assert!(fitting_sizes(&["Standard_D2s_v3".into()], &spec()).is_err());
    assert!(fitting_sizes(&["Standard_NC6".into()], &spec()).is_err());
}
