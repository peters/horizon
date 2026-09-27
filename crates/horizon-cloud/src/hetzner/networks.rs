//! Private networks. Horizon's Hetzner clouds in one network zone share one
//! network, so a companion connection between two of them stays off the public
//! internet. A network costs nothing and outlives the clouds that use it.
use super::{Hetzner, Method, valid_name};
use crate::{Cancellation, CloudError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Label naming the network zone a Horizon network serves.
pub const NETWORK_LABEL: &str = "horizon-network";
/// The address range of every Horizon network and of its one cloud subnet.
const IP_RANGE: &str = "10.72.0.0/16";
const SUBNET_RANGE: &str = "10.72.0.0/17";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Network {
    pub id: u64,
    pub name: String,
    pub ip_range: String,
    #[serde(default)]
    pub subnets: Vec<Subnet>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Subnet {
    #[serde(rename = "type")]
    pub kind: String,
    pub ip_range: String,
    pub network_zone: String,
}

impl Network {
    /// Whether this is Horizon's network for `zone`: labelled for it, with a cloud
    /// subnet there.
    #[must_use]
    pub fn serves(&self, zone: &str) -> bool {
        self.labels.get(NETWORK_LABEL).map(String::as_str) == Some(zone)
            && self
                .subnets
                .iter()
                .any(|subnet| subnet.kind == "cloud" && subnet.network_zone == zone)
    }
}

#[derive(Deserialize)]
struct Single {
    network: Network,
}

impl Hetzner {
    /// Horizon's network for `zone`, created on first use. Networks cost nothing,
    /// so a lost response is looked up again rather than fenced.
    /// # Errors
    /// Refuses invalid zones, several labelled networks and a labelled network
    /// without a subnet in the zone.
    pub fn ensure_network(&self, zone: &str, cancel: &Cancellation) -> Result<Network, CloudError> {
        if !valid_name(zone) {
            return Err(CloudError::Invalid("Invalid Hetzner network zone"));
        }
        if let Some(network) = self.labelled_network(zone, cancel)? {
            return Ok(network);
        }
        let body = json!({
            "name": format!("horizon-{zone}"),
            "ip_range": IP_RANGE,
            "subnets": [{"type": "cloud", "ip_range": SUBNET_RANGE, "network_zone": zone}],
            "labels": {NETWORK_LABEL: zone},
        });
        match self.send(Method::Post, "/networks", Some(body), cancel) {
            Ok(value) => {
                let single: Single = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
                if !single.network.serves(zone) {
                    return Err(CloudError::IdentityMismatch);
                }
                Ok(single.network)
            }
            // Our earlier request succeeded, or someone else holds the name.
            Err(failure) if failure.name_taken() => self.labelled_network(zone, cancel)?.ok_or(CloudError::Invalid(
                "The Hetzner project has a network named for this zone that Horizon does not own",
            )),
            Err(failure) => Err(failure.into()),
        }
    }

    fn labelled_network(&self, zone: &str, cancel: &Cancellation) -> Result<Option<Network>, CloudError> {
        let mut found: Vec<Network> = self.list_all(
            "/networks",
            &format!("label_selector={NETWORK_LABEL}%3D{zone}"),
            "networks",
            cancel,
        )?;
        if found.len() > 1 {
            return Err(CloudError::DuplicateWorkers);
        }
        let Some(network) = found.pop() else { return Ok(None) };
        if !network.serves(zone) {
            return Err(CloudError::Invalid(
                "Horizon's Hetzner network for this zone has no subnet there",
            ));
        }
        Ok(Some(network))
    }
}

#[derive(Deserialize)]
struct Located {
    name: String,
    network_zone: String,
}

impl Hetzner {
    /// The network zone of `location`, such as `eu-central`.
    /// # Errors
    /// Refuses invalid and unknown locations.
    pub fn network_zone(&self, location: &str, cancel: &Cancellation) -> Result<String, CloudError> {
        if !valid_name(location) {
            return Err(CloudError::Invalid("Invalid Hetzner location"));
        }
        let found: Vec<Located> = self.list_all("/locations", &format!("name={location}"), "locations", cancel)?;
        found
            .into_iter()
            .find(|found| found.name == location)
            .map(|found| found.network_zone)
            .ok_or(CloudError::Invalid("Hetzner does not list this location"))
    }
}
