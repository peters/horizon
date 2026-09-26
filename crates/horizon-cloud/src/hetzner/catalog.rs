//! Prices and live availability of x86 server types per location, for choosing
//! a worker before requesting one. Prices are Hetzner's net euro amounts.
use super::{Hetzner, Method};
use crate::{Cancellation, CloudError};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub server_type: String,
    pub location: String,
    pub cores: u32,
    pub memory_gb: f64,
    pub disk_gb: u32,
    /// Dedicated vCPUs rather than shared ones.
    pub dedicated: bool,
    pub hourly_eur: f64,
    /// The most a server is billed in a month, running or powered off.
    pub monthly_eur: f64,
    /// Whether Hetzner lists the type as orderable in this location right now. The
    /// flag is advisory: a create can succeed for an unlisted type, and fail for a
    /// listed one, so placement still relies on the create response.
    pub available: bool,
    /// Hetzner's suggested type for new servers in this location.
    pub recommended: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    /// Cheapest first.
    pub offers: Vec<Offer>,
    pub volume_gb_month_eur: f64,
    /// A primary IPv4 address per month, by location.
    pub ipv4_month_eur: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct ServerType {
    name: String,
    cores: u32,
    memory: f64,
    disk: u32,
    cpu_type: String,
    architecture: String,
    prices: Vec<LocationPrice>,
    locations: Vec<TypeLocation>,
}
/// A server type's standing in one location. Types and locations are deprecated
/// separately, so a type can still be current in some locations.
#[derive(Deserialize)]
struct TypeLocation {
    name: String,
    #[serde(default)]
    deprecation: Option<serde_json::Value>,
    available: bool,
    #[serde(default)]
    recommended: bool,
}
#[derive(Deserialize)]
struct LocationPrice {
    location: String,
    price_hourly: Amount,
    price_monthly: Amount,
}
#[derive(Deserialize)]
struct Amount {
    net: String,
}
#[derive(Deserialize)]
struct PricingEnvelope {
    pricing: Pricing,
}
#[derive(Deserialize)]
struct Pricing {
    currency: String,
    volume: VolumePrice,
    primary_ips: Vec<PrimaryIp>,
}
#[derive(Deserialize)]
struct VolumePrice {
    price_per_gb_month: Amount,
}
#[derive(Deserialize)]
struct PrimaryIp {
    #[serde(rename = "type")]
    kind: String,
    prices: Vec<IpPrice>,
}
#[derive(Deserialize)]
struct IpPrice {
    location: String,
    price_monthly: Amount,
}

impl Hetzner {
    /// Every x86 server type in every location that prices it and has not deprecated it.
    /// # Errors
    /// Reports provider failures, non-euro pricing and malformed amounts.
    pub fn catalog(&self, cancel: &Cancellation) -> Result<Catalog, CloudError> {
        let types: Vec<ServerType> = self.list_all("/server_types", "", "server_types", cancel)?;
        let pricing: PricingEnvelope = serde_json::from_value(self.send(Method::Get, "/pricing", None, cancel)?)
            .map_err(|_| CloudError::InvalidResponse)?;
        let pricing = pricing.pricing;
        if pricing.currency != "EUR" {
            return Err(CloudError::InvalidResponse);
        }
        let mut offers = Vec::new();
        for kind in types.iter().filter(|kind| kind.architecture == "x86") {
            for price in &kind.prices {
                let Some(standing) = kind.locations.iter().find(|location| location.name == price.location) else {
                    continue;
                };
                if standing.deprecation.is_some() {
                    continue;
                }
                offers.push(Offer {
                    server_type: kind.name.clone(),
                    location: price.location.clone(),
                    cores: kind.cores,
                    memory_gb: kind.memory,
                    disk_gb: kind.disk,
                    dedicated: kind.cpu_type == "dedicated",
                    hourly_eur: amount(&price.price_hourly)?,
                    monthly_eur: amount(&price.price_monthly)?,
                    available: standing.available,
                    recommended: standing.recommended,
                });
            }
        }
        offers.sort_by(|a, b| {
            a.hourly_eur
                .total_cmp(&b.hourly_eur)
                .then_with(|| a.server_type.cmp(&b.server_type))
                .then_with(|| a.location.cmp(&b.location))
        });
        let ipv4_month_eur = pricing
            .primary_ips
            .iter()
            .filter(|ip| ip.kind == "ipv4")
            .flat_map(|ip| &ip.prices)
            .map(|price| Ok((price.location.clone(), amount(&price.price_monthly)?)))
            .collect::<Result<_, CloudError>>()?;
        Ok(Catalog {
            offers,
            volume_gb_month_eur: amount(&pricing.volume.price_per_gb_month)?,
            ipv4_month_eur,
        })
    }
}

fn amount(value: &Amount) -> Result<f64, CloudError> {
    value
        .net
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
        .ok_or(CloudError::InvalidResponse)
}
