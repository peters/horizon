use crate::{Error, Result};
use horizon_media::discovery;
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};

const SERVICE: &str = "_airplay._tcp.local.";

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
    let mut receivers = BTreeMap::new();
    discovery::browse(SERVICE, Duration::from_secs(3), |service| {
        if !service
            .property("model")
            .is_some_and(|model| model.starts_with("AppleTV"))
            || !service.property("features").is_some_and(modern_timing)
        {
            return;
        }
        let Some(id) = service.property("deviceid") else {
            return;
        };
        receivers.insert(
            id.to_owned(),
            Receiver {
                id: id.to_owned(),
                name: service.instance().to_owned(),
                address: service.address,
            },
        );
    })
    .map_err(|e| Error::Backend(e.to_string()))?;
    Ok(receivers.into_values().collect())
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
