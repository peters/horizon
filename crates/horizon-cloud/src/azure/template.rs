//! The one ARM deployment that makes a worker inside its operation's resource group:
//! the network security group, the virtual network and subnet, the Standard static
//! public address, the network interface, the workspace data disk and the VM with
//! its managed OS disk. Every resource name is fixed inside the group, so the group
//! alone carries the operation's identity; every resource carries the
//! `horizon-operation` tag, and the group is the billing and delete scope.
//!
//! [`Planned::new`] checks the whole request and renders the host bootstrap before
//! anything is sent, so a request that cannot succeed sends nothing.
use super::{
    OPERATION_TAG, WORKER_VM_NAME,
    catalog::{OS_DISK_GB, VM_SIZES, VmSize},
    resource_group_name,
    url::{valid_location, valid_resource_group_name},
};
use crate::{CloudError, WorkerSpec, host};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The deployment that creates the worker. A retry addresses the same name.
pub const DEPLOYMENT_NAME: &str = "horizon-worker";
pub const NETWORK_API_VERSION: &str = "2023-11-01";
pub const DISK_API_VERSION: &str = "2023-10-02";
pub const TAGS_API_VERSION: &str = "2021-04-01";
/// Azure's limit for `osProfile.customData`, which counts the base64 text.
pub const CUSTOM_DATA_LIMIT: usize = 64 * 1024;
/// The host bootstrap is first held to the smallest provider limit, so its
/// base64 form always fits Azure's. The size check below still refuses first.
const _: () = assert!(host::USER_DATA_LIMIT.div_ceil(3) * 4 <= CUSTOM_DATA_LIMIT);
/// The workspace data disk, which ARM detaches rather than deletes with the VM.
pub const DATA_DISK_NAME: &str = "worker-workspace";
/// The OS disk, named so the template can tag it once the VM has made it.
pub const OS_DISK_NAME: &str = "worker-os";
pub const NSG_NAME: &str = "worker-nsg";
pub const VNET_NAME: &str = "worker-vnet";
pub const SUBNET_NAME: &str = "workers";
pub const PUBLIC_IP_NAME: &str = "worker-ip";
pub const NIC_NAME: &str = "worker-nic";
/// The host's admin user. Its key is a throwaway, and setup masks the host's sshd.
pub const ADMIN_USERNAME: &str = "horizon";
/// The template output that names the public address.
pub const ADDRESS_OUTPUT: &str = "publicIp";
/// Azure's largest managed OS disk.
pub const OS_DISK_MAX_GB: u32 = 4_095;
/// The Premium SSD account type of both disks. Every size in the catalog takes it.
const DISK_SKU: &str = "Premium_LRS";

/// One worker's deployment, checked and rendered. The custom data may carry a
/// registry login, so it is held in wiped storage and never printed.
pub struct Planned {
    pub group: String,
    pub region: String,
    pub size: &'static VmSize,
    pub os_disk_gb: u32,
    pub data_disk_gb: u32,
    tags: BTreeMap<String, String>,
    admin_key: String,
    custom_data: zeroize::Zeroizing<String>,
}

impl std::fmt::Debug for Planned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Planned")
            .field("group", &self.group)
            .field("region", &self.region)
            .field("size", &self.size.name)
            .field("custom_data", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// The catalog sizes in `allowed`, in that order, that meet the profile's CPU and memory.
/// # Errors
/// `Invalid` for a name outside the catalog or when none fits.
pub fn fitting_sizes(allowed: &[String], spec: &WorkerSpec) -> Result<Vec<&'static VmSize>, CloudError> {
    let mut fitting = Vec::new();
    for name in allowed {
        let size = VM_SIZES
            .iter()
            .find(|size| size.name == name)
            .ok_or(CloudError::Invalid("An Azure VM size is not one Horizon offers"))?;
        if size.vcpu >= spec.profile.cpu && size.memory_gb >= spec.profile.memory_gb {
            fitting.push(size);
        }
    }
    if fitting.is_empty() {
        return Err(CloudError::Invalid(
            "No allowed Azure VM size has the profile's CPU and memory",
        ));
    }
    Ok(fitting)
}

impl Planned {
    /// Checks `spec` for `region` and `size` and renders the host bootstrap.
    /// # Errors
    /// `Invalid` for an operation id that cannot name a group, a malformed region,
    /// a workspace disk outside Azure's range, an invalid spec or host plan, and a
    /// bootstrap above [`CUSTOM_DATA_LIMIT`] once encoded.
    pub fn new(
        spec: &WorkerSpec,
        region: &str,
        size: &'static VmSize,
        login: Option<host::RegistryLogin>,
    ) -> Result<Self, CloudError> {
        spec.validate()?;
        if spec.startup_metadata.is_some() {
            return Err(CloudError::Invalid("Shared workers are not available on Azure yet"));
        }
        let group = resource_group_name(&spec.operation_id);
        if !crate::valid_id(&spec.operation_id) || !valid_resource_group_name(&group) {
            return Err(CloudError::Invalid(
                "An Azure cloud needs an ID of letters, digits, hyphens and underscores, at most 79 characters",
            ));
        }
        if !valid_location(region) {
            return Err(CloudError::Invalid("Invalid Azure region"));
        }
        let (low, high) = crate::provider::AZURE.cpu_volume_gb;
        let data_disk_gb = u32::from(spec.profile.storage.volume_gb);
        if !(low..=high).contains(&data_disk_gb) {
            return Err(CloudError::Invalid(
                "An Azure workspace disk must be between 4 and 32,767 GB",
            ));
        }
        let os_disk_gb = OS_DISK_GB.max(u32::from(spec.profile.storage.container_gb));
        if os_disk_gb > OS_DISK_MAX_GB {
            return Err(CloudError::Invalid("An Azure OS disk can be at most 4,095 GB"));
        }
        let plan = host::Plan {
            image: spec.image_digest.clone(),
            environment: spec.container_environment()?,
            registry: login,
            workspace_device: host::AZURE_DATA_DISK.to_owned(),
            shm_gb: crate::hetzner::cloud::shared_memory_gb(spec.profile.memory_gb),
            exposure_ingress: spec.profile.bootstrap.exposure_ingress,
            platform: host::Platform::Azure,
            purpose: host::Purpose::Worker,
        };
        let custom_data = custom_data(&plan.cloud_config()?)?;
        Ok(Self {
            group,
            region: region.to_owned(),
            size,
            os_disk_gb,
            data_disk_gb,
            tags: tags(&spec.operation_id),
            admin_key: crate::hetzner::cloud::throwaway_public_key()?,
            custom_data,
        })
    }

    /// The tags of the group and of every resource in it.
    #[must_use]
    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    /// The deployment parameters. `customData` is a secure parameter, so ARM keeps
    /// it out of the deployment's history.
    #[must_use]
    pub fn parameters(&self) -> Value {
        json!({
            "location": { "value": self.region },
            "vmSize": { "value": self.size.name },
            "adminPublicKey": { "value": self.admin_key },
            "customData": { "value": self.custom_data.as_str() },
            "osDiskGb": { "value": self.os_disk_gb },
            "dataDiskGb": { "value": self.data_disk_gb },
            "tags": { "value": self.tags },
        })
    }
}

/// The one tag that names the owning operation.
#[must_use]
pub fn tags(operation_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(OPERATION_TAG.to_owned(), operation_id.to_owned())])
}

/// `document` as base64 `customData`, refused above Azure's limit.
/// # Errors
/// `Invalid` when the encoded bootstrap exceeds [`CUSTOM_DATA_LIMIT`].
pub fn custom_data(document: &str) -> Result<zeroize::Zeroizing<String>, CloudError> {
    let encoded = document.len().div_ceil(3) * 4;
    if encoded > CUSTOM_DATA_LIMIT {
        return Err(CloudError::Invalid(
            "Host configuration exceeds Azure's 64 KiB custom data limit",
        ));
    }
    let mut text = zeroize::Zeroizing::new(String::with_capacity(encoded));
    base64::engine::general_purpose::STANDARD.encode_string(document.as_bytes(), &mut text);
    Ok(text)
}

fn id(kind: &str, name: &str) -> String {
    format!("[resourceId('{kind}', '{name}')]")
}

/// A resource of `kind` named `name` in the deployment's region, with the tags.
fn resource(kind: &str, api_version: &str, name: &str, body: Value) -> Value {
    let mut resource = json!({
        "type": kind, "apiVersion": api_version, "name": name,
        "location": "[parameters('location')]", "tags": "[parameters('tags')]",
    });
    if let (Some(resource), Value::Object(body)) = (resource.as_object_mut(), body) {
        resource.extend(body);
    }
    resource
}

/// The ARM template. Only the parameters differ between workers.
#[must_use]
pub fn template() -> Value {
    let nsg = id("Microsoft.Network/networkSecurityGroups", NSG_NAME);
    let vnet = id("Microsoft.Network/virtualNetworks", VNET_NAME);
    let address = id("Microsoft.Network/publicIPAddresses", PUBLIC_IP_NAME);
    let nic = id("Microsoft.Network/networkInterfaces", NIC_NAME);
    let disk = id("Microsoft.Compute/disks", DATA_DISK_NAME);
    let vm = id("Microsoft.Compute/virtualMachines", WORKER_VM_NAME);
    let subnet = format!("[resourceId('Microsoft.Network/virtualNetworks/subnets', '{VNET_NAME}', '{SUBNET_NAME}')]");
    let (low, high) = crate::provider::AZURE.cpu_volume_gb;
    json!({
        "$schema": "https://schema.management.azure.com/schemas/2019-04-01/deploymentTemplate.json#",
        "contentVersion": "1.0.0.0",
        "parameters": {
            "location": { "type": "string" },
            "vmSize": { "type": "string" },
            "adminPublicKey": { "type": "string" },
            "customData": { "type": "securestring" },
            "osDiskGb": { "type": "int", "minValue": OS_DISK_GB, "maxValue": OS_DISK_MAX_GB },
            "dataDiskGb": { "type": "int", "minValue": low, "maxValue": high },
            "tags": { "type": "object" },
        },
        "resources": [
            resource("Microsoft.Network/networkSecurityGroups", NETWORK_API_VERSION, NSG_NAME, json!({
                "properties": { "securityRules": [{ "name": "worker-ssh", "properties": {
                    "priority": 100, "direction": "Inbound", "access": "Allow", "protocol": "Tcp",
                    "sourceAddressPrefix": "Internet", "sourcePortRange": "*",
                    "destinationAddressPrefix": "*", "destinationPortRange": "22" } }] }
            })),
            resource("Microsoft.Network/virtualNetworks", NETWORK_API_VERSION, VNET_NAME, json!({
                "properties": { "addressSpace": { "addressPrefixes": ["10.0.0.0/16"] },
                    "subnets": [{ "name": SUBNET_NAME, "properties": { "addressPrefix": "10.0.0.0/24" } }] }
            })),
            resource("Microsoft.Network/publicIPAddresses", NETWORK_API_VERSION, PUBLIC_IP_NAME, json!({
                "sku": { "name": "Standard" },
                "properties": { "publicIPAllocationMethod": "Static", "publicIPAddressVersion": "IPv4" }
            })),
            resource("Microsoft.Compute/disks", DISK_API_VERSION, DATA_DISK_NAME, json!({
                "sku": { "name": DISK_SKU },
                "properties": { "creationData": { "createOption": "Empty" }, "diskSizeGB": "[parameters('dataDiskGb')]" }
            })),
            resource("Microsoft.Network/networkInterfaces", NETWORK_API_VERSION, NIC_NAME, json!({
                "dependsOn": [nsg, vnet, address],
                "properties": {
                    "networkSecurityGroup": { "id": nsg },
                    "ipConfigurations": [{ "name": "primary", "properties": {
                        "privateIPAllocationMethod": "Dynamic", "subnet": { "id": subnet },
                        "publicIPAddress": { "id": address } } }] }
            })),
            resource("Microsoft.Compute/virtualMachines", super::COMPUTE_API_VERSION, WORKER_VM_NAME, json!({
                "dependsOn": [nic, disk],
                "properties": {
                    "hardwareProfile": { "vmSize": "[parameters('vmSize')]" },
                    "storageProfile": {
                        "imageReference": { "publisher": "Canonical", "offer": "ubuntu-24_04-lts", "sku": "server", "version": "latest" },
                        "osDisk": { "name": OS_DISK_NAME, "createOption": "FromImage", "diskSizeGB": "[parameters('osDiskGb')]",
                            "deleteOption": "Delete", "managedDisk": { "storageAccountType": DISK_SKU } },
                        "dataDisks": [{ "lun": 0, "createOption": "Attach", "deleteOption": "Detach",
                            "managedDisk": { "id": disk } }] },
                    "osProfile": { "computerName": WORKER_VM_NAME, "adminUsername": ADMIN_USERNAME,
                        "customData": "[parameters('customData')]",
                        "linuxConfiguration": { "disablePasswordAuthentication": true, "ssh": { "publicKeys": [{
                            "path": format!("/home/{ADMIN_USERNAME}/.ssh/authorized_keys"),
                            "keyData": "[parameters('adminPublicKey')]" }] } } },
                    "networkProfile": { "networkInterfaces": [{ "id": nic }] } }
            })),
            // ARM cannot tag an OS disk it makes from an image, so the disk is tagged
            // once the VM exists.
            {
                "type": "Microsoft.Resources/tags", "apiVersion": TAGS_API_VERSION, "name": "default",
                "scope": format!("[concat('Microsoft.Compute/disks/', '{OS_DISK_NAME}')]"),
                "dependsOn": [vm],
                "properties": { "tags": "[parameters('tags')]" }
            },
        ],
        "outputs": {
            ADDRESS_OUTPUT: { "type": "string", "value": format!("[reference({}).ipAddress]", &address[1..address.len() - 1]) }
        },
    })
}
