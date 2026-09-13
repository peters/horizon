//! The single ARM deployment that materialises one persistent Azure worker: a locked
//! down network path to the container's SSH port, a static public address, an Ubuntu
//! VM with the image-pull identity, a managed data disk that becomes `/workspace`,
//! and cloud-init that logs in to the registry with that identity and starts the
//! digest-pinned worker image. Everything is derived from validated inputs; nothing
//! operator-controlled is interpolated into YAML or shell.
use super::{AzureError, AzureProfile, COMPUTE_API_VERSION, DATA_DISK_NAME, WORKER_VM_NAME, resource_group_name};

/// Network and disk resource API versions used inside the template.
const NETWORK_API_VERSION: &str = "2023-11-01";
const DISK_API_VERSION: &str = "2023-10-02";
use crate::cloud_run::{
    CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudWorkflowId, WorkerLifetime, WorkerTarget,
    interactive_worker::InteractiveWorkerRequest,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

/// Deployment name inside the worker's resource group; one worker, one deployment.
pub const DEPLOYMENT_NAME: &str = "worker";
/// Public IP resource name; the provider reads the address back from it on reconnect.
pub const PUBLIC_IP_NAME: &str = "worker-pip";
/// Host port forwarded to the container's SSH daemon; the VM's own port 22 stays closed.
pub const SSH_PORT: u16 = 2222;
/// Tag keys that carry the worker identity on the resource group and every resource.
pub const TAG_WORKFLOW: &str = "horizon-workflow-id";
pub const TAG_JOB: &str = "horizon-job-id";
pub const TAG_PROTOCOL: &str = "horizon-cloud-protocol-version";
pub const TAG_LIFETIME: &str = "horizon-worker-lifetime";
pub const TAG_IMAGE_DIGEST: &str = "horizon-worker-image-digest";
pub const TAG_CLIENT_KEY_DIGEST: &str = "horizon-client-key-sha256";
pub const TAG_DISK_GIB: &str = "horizon-worker-disk-gib";
/// SHA-256 of the profile name: tag-safe regardless of the characters a name contains.
pub const TAG_PROFILE: &str = "horizon-worker-profile-sha256";
/// SHA-256 of the complete image reference (registry, repository and digest), so two
/// repositories sharing one manifest digest never pass as the same worker.
pub const TAG_IMAGE_REF_DIGEST: &str = "horizon-worker-image-ref-sha256";
const LIFETIME_PERSISTENT: &str = "persistent";
const ADMIN_USERNAME: &str = "azureuser";
const OS_DISK_GIB: u32 = 30;
const WORKSPACE_MOUNT: &str = "/mnt/horizon-workspace";
const CLIENT_KEY_PATH: &str = "/etc/horizon-worker/client-key.pub";
const MAX_DATA_DISK_GIB: u32 = 4_095;

/// Everything the transport needs to submit the worker deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AzureDeploymentPlan {
    pub resource_group: String,
    pub location: String,
    pub tags: BTreeMap<String, String>,
    pub template: serde_json::Value,
    pub parameters: serde_json::Value,
}

impl AzureDeploymentPlan {
    /// Derive the deployment for one request against one profile.
    /// # Errors
    /// Rejects requests the profile cannot serve and any non-persistent lifetime: a
    /// time-limited Azure worker needs a provider-side power bound that this adapter
    /// does not provide yet, so it is refused rather than silently made persistent.
    pub fn new(profile: &AzureProfile, request: &InteractiveWorkerRequest) -> Result<Self, AzureError> {
        Self::validate_target(profile, &request.target)?;
        // Beyond the target: the request's own shape and the client key.
        if !request.is_valid_for(crate::cloud_run::CloudProvider::Azure) {
            return Err(AzureError::InvalidTarget);
        }
        let tags = worker_tags(request);
        let cloud_init = cloud_init(profile, request);
        let parameters = serde_json::json!({
            "vmName": { "value": WORKER_VM_NAME },
            "vmSize": { "value": profile.vm_size },
            "adminPublicKey": { "value": request.ssh_public_key },
            "customData": { "value": STANDARD.encode(cloud_init) },
            "identityId": { "value": profile.image_pull_identity_id },
            "dataDiskGib": { "value": request.target.disk_gib },
            "diskSku": { "value": profile.disk_sku.as_azure_name() },
            "tags": { "value": tags },
        });
        Ok(Self {
            resource_group: resource_group_name(request.workflow_id, request.job_id),
            location: profile.location.clone(),
            tags,
            template: template(),
            parameters,
        })
    }
}

impl AzureDeploymentPlan {
    /// The complete target-only contract a deployment applies, for a caller that must
    /// refuse a target before saving intent, reading credentials or allocating and has
    /// no request or client key yet: the profile fit (provider, profile name, image on
    /// the declared registry, cost limit, provider-neutral target shape), a persistent
    /// lifetime, a data disk within the single-sourced maximum, and an image reference
    /// made only of bytes that can never reach a shell line. [`Self::new`] applies
    /// exactly this and then the request and key checks; there is no second copy.
    /// # Errors
    /// [`AzureError::InvalidProfile`], [`AzureError::InvalidTarget`],
    /// [`AzureError::DeclaredCostExceedsLimit`] or [`AzureError::UnsupportedLifetime`].
    pub fn validate_target(profile: &AzureProfile, target: &WorkerTarget) -> Result<(), AzureError> {
        profile.validate_target(target)?;
        if target.lifetime != WorkerLifetime::Persistent {
            return Err(AzureError::UnsupportedLifetime);
        }
        if target.disk_gib > MAX_DATA_DISK_GIB || !shell_safe(&target.image) {
            return Err(AzureError::InvalidTarget);
        }
        Ok(())
    }
}

/// Identity tags: exact workflow and job, protocol version, lifetime policy, image
/// digest, the SHA-256 of the client key, the data disk size, the profile name and the
/// SHA-256 of the full image reference, so ownership and the complete target are
/// provable without reading the guest.
#[must_use]
pub fn worker_tags(request: &InteractiveWorkerRequest) -> BTreeMap<String, String> {
    identity_tags(
        request.workflow_id,
        request.job_id,
        &request.target,
        &request.ssh_public_key,
    )
}

/// The same tags computed from a persisted handle's parts, for ownership checks.
#[must_use]
pub fn identity_tags(
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    ssh_public_key: &str,
) -> BTreeMap<String, String> {
    let digest = target
        .image
        .rsplit_once("@sha256:")
        .map(|(_, digest)| digest)
        .unwrap_or_default();
    [
        (TAG_WORKFLOW, workflow_id.to_string()),
        (TAG_JOB, job_id.to_string()),
        (TAG_PROTOCOL, CLOUD_RUN_PROTOCOL_VERSION.to_string()),
        (TAG_LIFETIME, LIFETIME_PERSISTENT.to_string()),
        (TAG_IMAGE_DIGEST, digest.to_string()),
        (TAG_CLIENT_KEY_DIGEST, client_key_digest(ssh_public_key)),
        (TAG_DISK_GIB, target.disk_gib.to_string()),
        (TAG_PROFILE, sha256_hex(&target.profile)),
        (TAG_IMAGE_REF_DIGEST, sha256_hex(&target.image)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect()
}

/// Lowercase hex SHA-256 of the exact client public key string.
#[must_use]
pub fn client_key_digest(ssh_public_key: &str) -> String {
    sha256_hex(ssh_public_key)
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Image references and registry names only ever contain these bytes; anything else
/// is refused before it could reach a shell line.
fn shell_safe(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':' | b'@'))
}

/// cloud-init for a persistent worker: Docker from the distribution, a data disk that
/// carries no signature at all formatted once as ext4 (a partition table or any other
/// filesystem signature is refused, never overwritten) and
/// mounted before Docker starts, a registry login through the VM's user-assigned
/// identity kept in a temporary Docker config that an EXIT trap removes on every
/// path, then the digest-pinned image started with the client key. The key arrives
/// as a base64 root-only file, never inline. Before the container can start, the
/// host is sealed against it: boot-persistent firewall rules drop container traffic
/// to the instance metadata service (no fresh identity tokens from inside) and to
/// the host's SSH port over the bridge, and host SSH is disabled outright, so the
/// admin key required by the image is inert. The ARM run command remains the
/// operator's path into the host.
fn cloud_init(profile: &AzureProfile, request: &InteractiveWorkerRequest) -> String {
    let registry = &profile.registry_login_server;
    let image = &request.target.image;
    let client_id_lookup = format!(
        "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=https%3A%2F%2Fmanagement.azure.com%2F&msi_res_id={}",
        percent_encode(&profile.image_pull_identity_id)
    );
    format!(
        r#"#cloud-config
package_update: true
packages: [docker.io, iptables-persistent]
write_files:
  - path: {CLIENT_KEY_PATH}
    permissions: '0600'
    owner: root:root
    encoding: b64
    content: {key}
  - path: /etc/systemd/system/docker.service.d/horizon-workspace.conf
    permissions: '0644'
    owner: root:root
    content: |
      [Unit]
      RequiresMountsFor={WORKSPACE_MOUNT}
  - path: /usr/local/sbin/horizon-worker-bootstrap.sh
    permissions: '0700'
    owner: root:root
    content: |
      #!/bin/bash
      set -euo pipefail
      field() {{ python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }}
      DISK=
      for _ in $(seq 1 120); do
        for candidate in /dev/disk/azure/data/by-lun/0 /dev/disk/azure/scsi1/lun0; do
          if [ -e "$candidate" ]; then DISK=$candidate; break 2; fi
        done
        sleep 1
      done
      [ -n "$DISK" ]
      SIGNATURES=$(wipefs --noheadings --output TYPE "$DISK" | sort -u | tr '\n' ' ')
      case "$SIGNATURES" in
        "") mkfs.ext4 -q -L horizonws "$DISK" ;;
        "ext4 ") ;;
        *) echo "refusing data disk with unexpected signatures: $SIGNATURES" >&2; exit 1 ;;
      esac
      mkdir -p {WORKSPACE_MOUNT}
      UUID=$(blkid -o value -s UUID "$DISK")
      grep -q "$UUID" /etc/fstab || echo "UUID=$UUID {WORKSPACE_MOUNT} ext4 defaults,nofail 0 2" >> /etc/fstab
      systemctl daemon-reload
      mountpoint -q {WORKSPACE_MOUNT} || mount {WORKSPACE_MOUNT}
      chown root:root {WORKSPACE_MOUNT} && chmod 0755 {WORKSPACE_MOUNT}
      systemctl enable --now docker
      iptables -I DOCKER-USER -d 169.254.169.254 -j DROP
      iptables -I INPUT -i docker0 -p tcp --dport 22 -j DROP
      netfilter-persistent save
      systemctl disable --now ssh.socket ssh.service
      export DOCKER_CONFIG=$(mktemp -d /run/horizon-docker-login.XXXXXX)
      trap 'rm -rf "$DOCKER_CONFIG"' EXIT
      CURL="curl -sf --max-time 20 --retry 5 --retry-delay 3 --retry-connrefused"
      AAD=$($CURL -H Metadata:true '{client_id_lookup}' | field access_token)
      REFRESH=$($CURL -X POST 'https://{registry}/oauth2/exchange' --data-urlencode grant_type=access_token --data-urlencode service={registry} --data-urlencode "access_token=$AAD" | field refresh_token)
      printf '%s' "$REFRESH" | timeout 60 docker login {registry} -u 00000000-0000-0000-0000-000000000000 --password-stdin >/dev/null
      unset AAD REFRESH
      for attempt in 1 2 3 4 5; do
        if PULL_ERROR=$(timeout 900 docker pull -q '{image}' 2>&1 >/dev/null); then break; fi
        case "$PULL_ERROR" in
          *unauthorized*|*denied*|*"authentication required"*|*"not found"*|*"manifest unknown"*) echo "$PULL_ERROR" >&2; exit 1 ;;
        esac
        [ "$attempt" = 5 ] && {{ echo "$PULL_ERROR" >&2; exit 1; }}
        sleep $((attempt * 10))
      done
      docker logout {registry} >/dev/null 2>&1 || true
      docker create --name horizon-worker --restart unless-stopped -p {SSH_PORT}:22 --mount type=bind,src={WORKSPACE_MOUNT},dst=/workspace -e "HORIZON_SSH_PUBLIC_KEY=$(cat {CLIENT_KEY_PATH})" '{image}' >/dev/null
      docker start horizon-worker >/dev/null
runcmd:
  - [ systemctl, daemon-reload ]
  - [ bash, /usr/local/sbin/horizon-worker-bootstrap.sh ]
"#,
        key = STANDARD.encode(&request.ssh_public_key),
    )
}

/// Percent-encode a validated resource ID for the instance metadata query string.
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(byte).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn arm_id(kind: &str, name_expression: &str) -> String {
    format!("[resourceId('{kind}', {name_expression})]")
}

/// The ARM template. Names are fixed relative to the VM name so every resource of a
/// worker is addressable from its identity alone. The data disk is a declared, tagged
/// resource attached by ID and detached rather than deleted with the VM; the OS disk
/// is created from the image (ARM cannot tag it at creation), is owned through the
/// group, and is deleted with the VM.
fn template() -> serde_json::Value {
    let nsg = arm_id("Microsoft.Network/networkSecurityGroups", "variables('nsg')");
    let vnet = arm_id("Microsoft.Network/virtualNetworks", "variables('vnet')");
    let pip = arm_id("Microsoft.Network/publicIPAddresses", "variables('pip')");
    let nic = arm_id("Microsoft.Network/networkInterfaces", "variables('nic')");
    let data_disk = arm_id("Microsoft.Compute/disks", "variables('data')");
    let subnet = "[resourceId('Microsoft.Network/virtualNetworks/subnets', variables('vnet'), 'workers')]";
    let common = |kind: &str, api: &str, name: &str| {
        serde_json::json!({
            "type": kind, "apiVersion": api, "name": name,
            "location": "[parameters('location')]", "tags": "[parameters('tags')]",
        })
    };
    let with = |mut base: serde_json::Value, extra: serde_json::Value| {
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        base
    };
    serde_json::json!({
        "$schema": "https://schema.management.azure.com/schemas/2019-04-01/deploymentTemplate.json#",
        "contentVersion": "1.0.0.0",
        "parameters": {
            "vmName": { "type": "string" }, "vmSize": { "type": "string" },
            "adminPublicKey": { "type": "securestring" }, "customData": { "type": "securestring" },
            "identityId": { "type": "string" }, "dataDiskGib": { "type": "int", "minValue": 1, "maxValue": MAX_DATA_DISK_GIB },
            "diskSku": { "type": "string", "allowedValues": ["StandardSSD_LRS", "Premium_LRS"] },
            "tags": { "type": "object" },
            "location": { "type": "string", "defaultValue": "[resourceGroup().location]" },
        },
        "variables": {
            "nsg": "[concat(parameters('vmName'), '-nsg')]", "vnet": "[concat(parameters('vmName'), '-vnet')]",
            "pip": "[concat(parameters('vmName'), '-pip')]",
            "nic": "[concat(parameters('vmName'), '-nic')]",
            // One name for creation and observation: the observer verifies the retained
            // disk against the same constant.
            "data": DATA_DISK_NAME,
        },
        "resources": [
            with(common("Microsoft.Network/networkSecurityGroups", NETWORK_API_VERSION, "[variables('nsg')]"), serde_json::json!({
                "properties": { "securityRules": [{ "name": "worker-ssh", "properties": {
                    "priority": 100, "direction": "Inbound", "access": "Allow", "protocol": "Tcp",
                    "sourceAddressPrefix": "Internet", "sourcePortRange": "*",
                    "destinationAddressPrefix": "*", "destinationPortRange": SSH_PORT.to_string() } }] }
            })),
            with(common("Microsoft.Network/virtualNetworks", NETWORK_API_VERSION, "[variables('vnet')]"), serde_json::json!({
                "dependsOn": [nsg],
                "properties": { "addressSpace": { "addressPrefixes": ["10.0.0.0/16"] }, "subnets": [{ "name": "workers",
                    "properties": { "addressPrefix": "10.0.0.0/24", "networkSecurityGroup": { "id": nsg } } }] }
            })),
            with(common("Microsoft.Network/publicIPAddresses", NETWORK_API_VERSION, "[variables('pip')]"), serde_json::json!({
                "sku": { "name": "Standard" },
                "properties": { "publicIPAllocationMethod": "Static", "publicIPAddressVersion": "IPv4" }
            })),
            with(common("Microsoft.Compute/disks", DISK_API_VERSION, "[variables('data')]"), serde_json::json!({
                "sku": { "name": "[parameters('diskSku')]" },
                "properties": { "creationData": { "createOption": "Empty" }, "diskSizeGB": "[parameters('dataDiskGib')]" }
            })),
            with(common("Microsoft.Network/networkInterfaces", NETWORK_API_VERSION, "[variables('nic')]"), serde_json::json!({
                "dependsOn": [vnet, pip],
                "properties": { "ipConfigurations": [{ "name": "primary", "properties": {
                    "privateIPAllocationMethod": "Dynamic", "subnet": { "id": subnet }, "publicIPAddress": { "id": pip } } }] }
            })),
            with(common("Microsoft.Compute/virtualMachines", COMPUTE_API_VERSION, "[parameters('vmName')]"), serde_json::json!({
                "dependsOn": [nic, data_disk],
                "identity": { "type": "UserAssigned", "userAssignedIdentities": { "[parameters('identityId')]": {} } },
                "properties": {
                    "hardwareProfile": { "vmSize": "[parameters('vmSize')]" },
                    "storageProfile": {
                        "imageReference": { "publisher": "Canonical", "offer": "ubuntu-24_04-lts", "sku": "server", "version": "latest" },
                        "osDisk": { "createOption": "FromImage", "diskSizeGB": OS_DISK_GIB, "deleteOption": "Delete",
                            "managedDisk": { "storageAccountType": "[parameters('diskSku')]" } },
                        "dataDisks": [{ "lun": 0, "createOption": "Attach", "deleteOption": "Detach",
                            "managedDisk": { "id": data_disk } }] },
                    "osProfile": { "computerName": "[parameters('vmName')]", "adminUsername": ADMIN_USERNAME,
                        "customData": "[parameters('customData')]",
                        "linuxConfiguration": { "disablePasswordAuthentication": true, "ssh": { "publicKeys": [{
                            "path": format!("/home/{ADMIN_USERNAME}/.ssh/authorized_keys"), "keyData": "[parameters('adminPublicKey')]" }] } } },
                    "networkProfile": { "networkInterfaces": [{ "id": nic }] } }
            })),
        ],
        "outputs": { "publicIp": { "type": "string", "value": "[reference(variables('pip')).ipAddress]" } },
    })
}
