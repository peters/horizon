//! Multicast DNS browsing shared by each transport's receiver discovery.
//! Transports keep their own filtering and TXT record interpretation.
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    time::{Duration, Instant},
};

/// One resolved DNS-SD service instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// Full DNS-SD name, for example `Living Room._airplay._tcp.local.`.
    pub fullname: String,
    pub address: SocketAddr,
    instance: String,
    properties: BTreeMap<String, String>,
}

impl Service {
    /// TXT record value for `key`.
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    /// The instance label, without the service type and domain.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }
}

#[derive(Debug)]
pub struct DiscoveryError(String);

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DiscoveryError {}

/// Browses `service_type` (for example `_googlecast._tcp.local.`) for
/// `duration` and returns the latest resolution of every instance that has an
/// IPv4 address and a port. Does not connect to the services.
/// # Errors
/// Returns an error if multicast service discovery cannot start.
pub fn browse(service_type: &str, duration: Duration) -> Result<Vec<Service>, DiscoveryError> {
    let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError(e.to_string()))?;
    let result = (|| {
        let events = daemon.browse(service_type).map_err(|e| DiscoveryError(e.to_string()))?;
        let deadline = Instant::now() + duration;
        let mut services = BTreeMap::new();
        while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = events.recv_timeout(wait) else {
                break;
            };
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            // IPv4 only: link-local IPv6 needs a scope id callers cannot use.
            let Some(ip) = info.get_addresses_v4().into_iter().min() else {
                continue;
            };
            if info.port == 0 {
                continue;
            }
            let properties = info
                .get_properties()
                .iter()
                .map(|property| (property.key().to_owned(), property.val_str().to_owned()))
                .collect();
            services.insert(
                info.fullname.clone(),
                Service {
                    instance: instance_name(&info.fullname, service_type).to_owned(),
                    fullname: info.fullname.clone(),
                    address: SocketAddr::new(ip.into(), info.port),
                    properties,
                },
            );
        }
        Ok(services.into_values().collect())
    })();
    let _ = daemon.shutdown();
    result
}

fn instance_name<'a>(fullname: &'a str, service_type: &str) -> &'a str {
    fullname
        .strip_suffix(service_type)
        .map_or(fullname, |name| name.strip_suffix('.').unwrap_or(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_name_strips_type_and_domain() {
        assert_eq!(
            instance_name("Living Room._airplay._tcp.local.", "_airplay._tcp.local."),
            "Living Room"
        );
        assert_eq!(
            instance_name("Speaker._googlecast._tcp.local.", "_airplay._tcp.local."),
            "Speaker._googlecast._tcp.local."
        );
    }

    #[test]
    fn properties_are_looked_up_by_key() {
        let service = Service {
            fullname: "TV._googlecast._tcp.local.".to_owned(),
            address: "192.0.2.10:8009".parse().unwrap(),
            instance: "TV".to_owned(),
            properties: BTreeMap::from([("fn".to_owned(), "Living Room TV".to_owned())]),
        };
        assert_eq!(service.property("fn"), Some("Living Room TV"));
        assert_eq!(service.property("md"), None);
        assert_eq!(service.instance(), "TV");
    }
}
