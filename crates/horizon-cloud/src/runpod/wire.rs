//! v2 wire types are separate from the durable worker journal format.
use crate::{CloudError, NetworkVolume, Worker};
use serde::Deserialize;
use std::{collections::BTreeMap, net::IpAddr};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Pod {
    pub id: String,
    name: String,
    image: String,
    status: String,
    disk: u32,
    mounts: Mounts,
    env: BTreeMap<String, String>,
    cpu: Option<Compute>,
    gpu: Option<Gpu>,
    data_center_id: Option<String>,
    ssh: Ssh,
    cost: f64,
    started_at: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Compute {
    vcpu_count: f64,
    memory: f64,
}
#[derive(Deserialize)]
struct Gpu {
    #[serde(flatten)]
    compute: Compute,
    #[serde(default = "default_gpu_count")]
    count: u32,
}
fn default_gpu_count() -> u32 {
    1
}
#[derive(Deserialize)]
struct Mounts {
    #[serde(default)]
    network: Vec<Mount>,
    persistent: Option<Persistent>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mount {
    volume_id: String,
    path: String,
}
#[derive(Deserialize)]
struct Persistent {
    size: u32,
    path: String,
}
#[derive(Deserialize)]
struct Ssh {
    direct: Option<Direct>,
}
#[derive(Deserialize)]
struct Direct {
    host: String,
    port: u16,
    username: String,
}

impl Pod {
    pub(super) fn worker(self) -> Result<Worker, CloudError> {
        if !crate::valid_id(&self.id)
            || !matches!(
                self.status.as_str(),
                "PROVISIONING" | "STARTING" | "RUNNING" | "EXITED" | "ERROR" | "TERMINATED"
            )
            || !self.cost.is_finite()
            || self.cost < 0.0
            || self.mounts.network.len() > 1
            || (self.mounts.persistent.is_some() && !self.mounts.network.is_empty())
            || (self.cpu.is_some() && self.gpu.is_some())
            || self.gpu.as_ref().is_some_and(|gpu| gpu.count == 0)
        {
            return Err(CloudError::InvalidResponse);
        }
        let gpu_count = self.gpu.as_ref().map(|gpu| gpu.count);
        let compute = self.cpu.or(self.gpu.map(|gpu| gpu.compute));
        let (vcpu_count, memory_in_gb) = match compute {
            Some(compute) => (Some(resource(compute.vcpu_count)?), Some(resource(compute.memory)?)),
            None => (None, None),
        };
        // Other account Pods may use valid endpoints unsupported by Horizon's
        // numeric-root transport. Keep their identity/mounts in account scans.
        let direct = self.ssh.direct.and_then(|direct| {
            if direct.port == 0 || direct.username != "root" {
                return None;
            }
            Some((direct.host.parse::<IpAddr>().ok()?, direct.port))
        });
        let (public_ip, port_mappings) = direct.map_or((None, None), |(host, port)| {
            (Some(host), Some(BTreeMap::from([("22".to_owned(), port)])))
        });
        let (network_volume, volume_in_gb, volume_mount_path) =
            if let Some(mount) = self.mounts.network.into_iter().next() {
                if !crate::valid_id(&mount.volume_id) {
                    return Err(CloudError::InvalidResponse);
                }
                (
                    Some(NetworkVolume {
                        id: Some(mount.volume_id),
                        size: None,
                        data_center_id: self.data_center_id.clone(),
                    }),
                    Some(0),
                    Some(mount.path),
                )
            } else if let Some(mount) = self.mounts.persistent {
                (None, Some(mount.size), Some(mount.path))
            } else {
                (None, None, None)
            };
        Ok(Worker {
            id: self.id,
            name: self.name,
            image_name: self.image,
            desired_status: self.status,
            public_ip,
            port_mappings,
            cost_per_hr: Some(self.cost),
            adjusted_cost_per_hr: None,
            last_started_at: self.started_at,
            memory_in_gb,
            vcpu_count,
            gpu_count,
            container_disk_in_gb: Some(self.disk),
            volume_in_gb,
            volume_mount_path,
            network_volume,
            data_center_id: self.data_center_id,
            env: self.env,
        })
    }
}

// GPU allocations may be fractional. Rounding down never certifies more than assigned.
fn resource(value: f64) -> Result<u32, CloudError> {
    if !value.is_finite() || value < 1.0 || value > f64::from(u32::MAX) {
        return Err(CloudError::InvalidResponse);
    }
    value
        .floor()
        .to_string()
        .parse()
        .map_err(|_| CloudError::InvalidResponse)
}

pub(super) fn worker(value: serde_json::Value) -> Result<Worker, CloudError> {
    serde_json::from_value::<Pod>(value)
        .map_err(|_| CloudError::InvalidResponse)?
        .worker()
}
