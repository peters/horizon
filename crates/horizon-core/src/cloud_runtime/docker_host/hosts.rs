//! What each saved Docker host can take now, for New cloud and `cloud_offers`, read
//! with the same calls as the Check; nothing is reserved.
use super::super::{Cancellation, settings::Settings};
use horizon_cloud::{
    docker_host::{self, Capacity, Engine, Host},
    offers::Requirements,
};
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

/// A saved host and its capacity, or why it could not be read.
#[derive(Clone, Debug)]
pub struct Reading {
    pub host: Host,
    pub capacity: Result<Capacity, String>,
}

/// How long a reading waits for the hosts. A host that has not answered by then is read
/// as not answering, so it holds back no other host's offer.
const READ_LIMIT: Duration = Duration::from_secs(10);
const LATE: &str = "The Docker host did not answer within 10 seconds";

/// Reads every saved host at the same time while the hosts are on.
#[must_use]
pub fn readings(settings: &Settings, root: &Path, cancel: &Cancellation) -> Vec<Reading> {
    if !settings.docker_hosts_on() {
        return Vec::new();
    }
    // Settings built in code skip validation; one thread per host must stay few.
    if settings.docker_hosts.len() > super::super::settings::MAX_DOCKER_HOSTS {
        return settings
            .docker_hosts
            .iter()
            .map(|host| Reading {
                host: host.clone(),
                capacity: Err("Save at most 16 Docker hosts".into()),
            })
            .collect();
    }
    let stop = Cancellation::default();
    let (sender, receiver) = mpsc::channel();
    for (index, host) in settings.docker_hosts.iter().enumerate() {
        let (host, local_config) = (host.clone(), root.join("docker-host-check"));
        let (stop, sender) = (stop.clone(), sender.clone());
        std::thread::spawn(move || {
            let engine = Engine {
                host: &host,
                local_config,
                cancel: &stop,
                engine_id: std::sync::Mutex::default(),
            };
            let _ = sender.send((index, docker_host::capacity(&engine).map_err(|error| error.to_string())));
        });
    }
    drop(sender);
    let mut read: Vec<Option<Result<Capacity, String>>> = vec![None; settings.docker_hosts.len()];
    let deadline = Instant::now() + READ_LIMIT;
    while read.iter().any(Option::is_none) && !cancel.is_cancelled() {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        match receiver.recv_timeout(wait) {
            Ok((index, capacity)) => read[index] = Some(capacity),
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(_) => break,
        }
    }
    // A host still being read is left to end on its own thread.
    stop.cancel();
    settings
        .docker_hosts
        .iter()
        .zip(read)
        .map(|(host, capacity)| Reading {
            host: host.clone(),
            capacity: capacity.unwrap_or_else(|| Err(LATE.into())),
        })
        .collect()
}

/// Whether saved hosts can answer `requirements`: no host has a GPU or a region, so
/// such requests need no reading.
#[must_use]
pub fn wanted(requirements: &Requirements) -> bool {
    !requirements.gpu && requirements.region.is_none()
}

/// The `cloud_offers` section of the saved hosts, beside the rented providers', or None
/// without one. A host that could not be read is listed under `host_errors` and makes
/// the cost comparison incomplete, since it may have been the cheapest offer.
#[must_use]
pub fn section(readings: &[Reading], requirements: &Requirements) -> Option<serde_json::Value> {
    if readings.is_empty() || !wanted(requirements) {
        return None;
    }
    let offers: Vec<_> = readings
        .iter()
        .filter_map(|reading| docker_host::offer(&reading.host.name, reading.capacity.as_ref().ok()?, requirements))
        .take(requirements.effective_limit())
        .collect();
    let errors: serde_json::Map<_, _> = readings
        .iter()
        .filter_map(|reading| {
            let error = match &reading.capacity {
                Err(error) => error.clone(),
                Ok(capacity) if !capacity.native() => {
                    format!("The worker image has no build for {}", capacity.architecture)
                }
                Ok(_) => return None,
            };
            Some((reading.host.name.clone(), error.into()))
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
    if readings.iter().any(|reading| reading.capacity.is_err()) {
        section["comparison_incomplete"] = true.into();
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
            pinned: None,
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
        assert_eq!(section["comparison_incomplete"], true);
        let gpu = Requirements {
            gpu: true,
            ..Requirements::default()
        };
        assert!(super::section(&readings, &gpu).is_none());
        let region = Requirements {
            region: Some("EUROPE".into()),
            ..Requirements::default()
        };
        assert!(super::section(&readings, &region).is_none());
        let mut riscv = readings[0].clone();
        if let Ok(capacity) = &mut riscv.capacity {
            capacity.architecture = "riscv64".into();
        }
        let section = super::section(&[riscv], &Requirements::default()).unwrap();
        assert_eq!(section["offers"], serde_json::json!([]));
        // A host that was read is known, also when it cannot run the worker image.
        assert!(section.get("comparison_incomplete").is_none());
        assert!(
            section["host_errors"]["build-box"]
                .as_str()
                .unwrap()
                .contains("riscv64")
        );
        let one = Requirements {
            limit: Some(1),
            ..Requirements::default()
        };
        let both = [readings[0].clone(), readings[0].clone()];
        assert_eq!(
            super::section(&both, &one).unwrap()["offers"].as_array().unwrap().len(),
            1
        );
        assert!(super::section(&[], &Requirements::default()).is_none());
    }
}
