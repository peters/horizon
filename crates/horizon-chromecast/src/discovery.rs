use crate::{Error, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

const SERVICE: &str = "_googlecast._tcp.local.";
const CAPABILITY_VIDEO_OUT: u32 = 1 << 0;
const CAPABILITY_GROUP: u32 = 1 << 5;

#[derive(Clone, Debug)]
pub struct Receiver {
    pub id: String,
    pub name: String,
    pub model: String,
    pub address: SocketAddr,
    /// `false` for speakers, soundbars and speaker groups, which play audio only.
    pub video: bool,
}

/// Browses for Cast receivers for `duration`. Does not connect to them.
/// # Errors
/// Returns a discovery error if multicast service discovery cannot start.
pub fn discover(duration: Duration) -> Result<Vec<Receiver>> {
    let daemon = ServiceDaemon::new().map_err(|e| Error::Discovery(e.to_string()))?;
    let result = (|| {
        let events = daemon.browse(SERVICE).map_err(|e| Error::Discovery(e.to_string()))?;
        let deadline = Instant::now() + duration;
        let mut receivers = BTreeMap::new();
        while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = events.recv_timeout(wait) else {
                break;
            };
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            let Some(id) = info.get_property_val_str("id") else {
                continue;
            };
            // IPv4 only: link-local IPv6 needs a scope id the caller cannot use.
            let Some(ip) = info.get_addresses_v4().into_iter().min() else {
                continue;
            };
            if info.port == 0 {
                continue;
            }
            let name = info.get_property_val_str("fn").map_or_else(
                || info.fullname.trim_end_matches(SERVICE).trim_end_matches('.').to_owned(),
                str::to_owned,
            );
            receivers.insert(
                id.to_owned(),
                Receiver {
                    id: id.to_owned(),
                    name,
                    model: info.get_property_val_str("md").unwrap_or_default().to_owned(),
                    address: SocketAddr::new(ip.into(), info.port),
                    video: info.get_property_val_str("ca").is_some_and(shows_video),
                },
            );
        }
        Ok(receivers.into_values().collect())
    })();
    let _ = daemon.shutdown();
    result
}

fn shows_video(capabilities: &str) -> bool {
    capabilities
        .parse::<u32>()
        .is_ok_and(|bits| bits & CAPABILITY_VIDEO_OUT != 0 && bits & CAPABILITY_GROUP == 0)
}

#[cfg(test)]
mod tests {
    use super::shows_video;

    #[test]
    fn video_capability_excludes_speakers_and_groups() {
        assert!(shows_video("4101"));
        assert!(!shows_video("199172"));
        assert!(!shows_video("2085"));
        assert!(!shows_video("video"));
    }
}
