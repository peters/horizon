//! What each saved Docker host can take now, for New cloud and `cloud_offers`, read
//! with the same calls as the Check; nothing is reserved.
use super::super::{Cancellation, settings::Settings};
use horizon_cloud::{
    docker_host::{self, Capacity, Engine, Host},
    offers::Requirements,
};
use std::path::Path;

/// A saved host and its capacity, or why it could not be read.
#[derive(Clone, Debug)]
pub struct Reading {
    pub host: Host,
    pub capacity: Result<Capacity, String>,
}

/// Reads every saved host while the hosts are on.
#[must_use]
pub fn readings(settings: &Settings, root: &Path, cancel: &Cancellation) -> Vec<Reading> {
    if !settings.docker_hosts_on() {
        return Vec::new();
    }
    // At the same time, so a host that does not answer holds back no other.
    std::thread::scope(|scope| {
        let reads: Vec<_> = settings
            .docker_hosts
            .iter()
            .map(|host| {
                scope.spawn(move || {
                    let engine = Engine {
                        host,
                        local_config: root.join("docker-host-check"),
                        cancel,
                        engine_id: std::sync::Mutex::default(),
                    };
                    docker_host::capacity(&engine).map_err(|error| error.to_string())
                })
            })
            .collect();
        settings
            .docker_hosts
            .iter()
            .zip(reads)
            .map(|(host, read)| Reading {
                host: host.clone(),
                capacity: read
                    .join()
                    .unwrap_or_else(|_| Err("The Docker host could not be read".into())),
            })
            .collect()
    })
}

/// The `cloud_offers` section of the saved hosts, beside the rented providers', or None
/// without one. A host that could not be read is listed under `host_errors`.
#[must_use]
pub fn section(readings: &[Reading], requirements: &Requirements) -> Option<serde_json::Value> {
    if readings.is_empty() || requirements.gpu {
        return None;
    }
    let offers: Vec<_> = readings
        .iter()
        .filter_map(|reading| docker_host::offer(&reading.host.name, reading.capacity.as_ref().ok()?, requirements))
        .collect();
    let errors: serde_json::Map<_, _> = readings
        .iter()
        .filter_map(|reading| {
            Some((
                reading.host.name.clone(),
                reading.capacity.as_ref().err()?.clone().into(),
            ))
        })
        .collect();
    let mut section = serde_json::json!({
        "provider": docker_host::PROVIDER,
        "currency": horizon_cloud::provider::DOCKER_HOST.currency,
        "offers": offers,
    });
    if !errors.is_empty() {
        section["host_errors"] = errors.into();
    }
    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_offered_at_no_rent_and_unread_hosts_are_named() {
        let host = |name: &str| Host {
            name: name.into(),
            access: docker_host::Access::ThisPc,
            address: None,
            docker: None,
        };
        let capacity = Capacity {
            cpus: 8,
            memory_bytes: 16 << 30,
            free_nano_cpus: 6_000_000_000,
            free_memory_bytes: 12 << 30,
            architecture: "x86_64".into(),
            version: "28.5.1".into(),
        };
        let readings = [
            Reading {
                host: host("build-box"),
                capacity: Ok(capacity),
            },
            Reading {
                host: host("asleep"),
                capacity: Err("The Docker host did not answer in time".into()),
            },
        ];
        let section = section(&readings, &Requirements::default()).unwrap();
        assert_eq!(section["provider"], "docker-host");
        assert_eq!(section["offers"][0]["id"], "build-box");
        assert_eq!(section["offers"][0]["vcpu"], 6);
        assert_eq!(section["offers"][0]["estimated_total"], 0.0);
        assert_eq!(
            section["host_errors"]["asleep"],
            "The Docker host did not answer in time"
        );
        let gpu = Requirements {
            gpu: true,
            ..Requirements::default()
        };
        assert!(super::section(&readings, &gpu).is_none());
        assert!(super::section(&[], &Requirements::default()).is_none());
    }
}
