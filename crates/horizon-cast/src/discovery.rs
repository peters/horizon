use crate::{Error, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Receiver {
    pub id: String,
    pub name: String,
    pub address: SocketAddr,
}

/// Discover current Apple TV receivers for three seconds. Does not connect or pair.
/// # Errors
/// Returns a discovery error if multicast service discovery cannot start.
pub fn discover() -> Result<Vec<Receiver>> {
    let daemon = ServiceDaemon::new().map_err(|e| Error::Backend(e.to_string()))?;
    let result = (|| {
        let events = daemon
            .browse("_airplay._tcp.local.")
            .map_err(|e| Error::Backend(e.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut receivers = BTreeMap::new();
        while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = events.recv_timeout(wait) else {
                break;
            };
            if let ServiceEvent::ServiceResolved(info) = event {
                if !info
                    .get_property_val_str("model")
                    .is_some_and(|model| model.starts_with("AppleTV"))
                    || !info.get_property_val_str("features").is_some_and(modern_timing)
                {
                    continue;
                }
                let Some(id) = info.get_property_val_str("deviceid") else {
                    continue;
                };
                // IPv4 matches the initial Linux LAN MVP; no unscoped link-local IPv6.
                let Some(ip) = info.get_addresses_v4().into_iter().min() else {
                    continue;
                };
                if info.port == 0 {
                    continue;
                }
                let name = info
                    .fullname
                    .strip_suffix("._airplay._tcp.local.")
                    .unwrap_or(&info.fullname)
                    .to_owned();
                receivers.insert(
                    id.to_owned(),
                    Receiver {
                        id: id.to_owned(),
                        name,
                        address: SocketAddr::new(ip.into(), info.port),
                    },
                );
            }
        }
        Ok(receivers.into_values().collect())
    })();
    let _ = daemon.shutdown();
    result
}

fn modern_timing(features: &str) -> bool {
    let mut words = features.split(',');
    let word = |part: &str| u32::from_str_radix(part.trim().trim_start_matches("0x"), 16).ok();
    let Some(low) = words.next().and_then(word) else {
        return false;
    };
    let high = words.next().and_then(word).unwrap_or(0);
    words.next().is_none() && (u64::from(low) | (u64::from(high) << 32)) & (1 << 41) != 0
}
#[cfg(test)]
mod tests {
    use super::modern_timing;
    #[test]
    fn requires_advertised_modern_timing() {
        assert!(modern_timing("0x0,0x200"));
        assert!(!modern_timing("0xffffffff,0x1ff"));
        assert!(!modern_timing("0x0,0x200,0x0"));
        assert!(!modern_timing("invalid"));
    }
}
