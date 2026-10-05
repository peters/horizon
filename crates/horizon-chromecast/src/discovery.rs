use crate::Result;
use horizon_media::discovery;
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};

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
    let mut receivers = BTreeMap::new();
    discovery::browse(SERVICE, duration, |service| {
        let Some(id) = service.property("id") else {
            return;
        };
        receivers.insert(
            id.to_owned(),
            Receiver {
                id: id.to_owned(),
                name: service.property("fn").unwrap_or(service.instance()).to_owned(),
                model: service.property("md").unwrap_or_default().to_owned(),
                address: service.address,
                video: service.property("ca").is_some_and(shows_video),
            },
        );
    })?;
    Ok(receivers.into_values().collect())
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
