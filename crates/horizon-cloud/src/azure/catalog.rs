//! Azure's public retail prices for the CPU VM sizes Horizon offers, with the managed
//! disks and the static public address that a worker keeps. The price list needs no
//! credential or subscription. It carries no vCPU, memory or stock, so the sizes come
//! from a curated table and creation confirms stock.
//!
//! The price list is untrusted: each page is size-limited, its link to the next page
//! must stay on the price endpoint, and a meter is used only when its Linux,
//! pay-as-you-go, US dollar prices agree. A size, disk tier or address without such a
//! price is left out, never guessed.
use super::{REQUEST_TIMEOUT, RESPONSE_LIMIT};
use crate::{Cancellation, CloudError, Reason};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The public retail price list.
pub const ENDPOINT: &str = "https://prices.azure.com/api/retail/prices";
/// The same endpoint as its `NextPageLink` names it.
const ENDPOINT_WITH_PORT: &str = "https://prices.azure.com:443/api/retail/prices";
/// One region's filtered list fits in one page of 1,000 items; this bounds a list
/// that never reports a last page.
const MAX_PAGES: usize = 10;
const CURRENCY: &str = crate::provider::AZURE.currency;
/// The OS disk of the Ubuntu images a worker starts from.
pub const OS_DISK_GB: u32 = 30;

/// A region that is priced, with its label as other providers name regions.
pub struct Region {
    pub name: &'static str,
    pub label: &'static str,
}

pub const REGIONS: &[Region] = &[
    Region {
        name: "northeurope",
        label: "EUROPE",
    },
    Region {
        name: "westeurope",
        label: "EUROPE",
    },
    Region {
        name: "swedencentral",
        label: "EUROPE",
    },
    Region {
        name: "uksouth",
        label: "EUROPE",
    },
    Region {
        name: "germanywestcentral",
        label: "EUROPE",
    },
    Region {
        name: "francecentral",
        label: "EUROPE",
    },
    Region {
        name: "eastus",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "eastus2",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "centralus",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "westus2",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "westus3",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "canadacentral",
        label: "NORTH_AMERICA",
    },
    Region {
        name: "brazilsouth",
        label: "SOUTH_AMERICA",
    },
    Region {
        name: "southeastasia",
        label: "ASIA",
    },
    Region {
        name: "japaneast",
        label: "ASIA",
    },
    Region {
        name: "centralindia",
        label: "ASIA",
    },
    Region {
        name: "australiaeast",
        label: "OCEANIA",
    },
];

/// A VM size Horizon offers, and the retail product that prices it.
pub struct VmSize {
    pub name: &'static str,
    pub product: &'static str,
    pub vcpu: u16,
    pub memory_gb: u16,
}

const DSV3: &str = "Virtual Machines DSv3 Series";
const DDSV4: &str = "Virtual Machines Ddsv4 Series";
const FSV2: &str = "Virtual Machines FSv2 Series";

const fn size(name: &'static str, product: &'static str, vcpu: u16, memory_gb: u16) -> VmSize {
    VmSize {
        name,
        product,
        vcpu,
        memory_gb,
    }
}

/// The x86 families with Premium SSD support that the epic's subscription has quota for.
pub const VM_SIZES: &[VmSize] = &[
    size("Standard_D2s_v3", DSV3, 2, 8),
    size("Standard_D4s_v3", DSV3, 4, 16),
    size("Standard_D8s_v3", DSV3, 8, 32),
    size("Standard_D16s_v3", DSV3, 16, 64),
    size("Standard_D32s_v3", DSV3, 32, 128),
    size("Standard_D2ds_v4", DDSV4, 2, 8),
    size("Standard_D4ds_v4", DDSV4, 4, 16),
    size("Standard_D8ds_v4", DDSV4, 8, 32),
    size("Standard_D16ds_v4", DDSV4, 16, 64),
    size("Standard_D32ds_v4", DDSV4, 32, 128),
    size("Standard_F2s_v2", FSV2, 2, 4),
    size("Standard_F4s_v2", FSV2, 4, 8),
    size("Standard_F8s_v2", FSV2, 8, 16),
    size("Standard_F16s_v2", FSV2, 16, 32),
    size("Standard_F32s_v2", FSV2, 32, 64),
];

const DISK_PRODUCT: &str = "Premium SSD Managed Disks";
/// Premium SSD tiers and the largest disk in GiB that each bills for.
pub const DISK_TIERS: &[(&str, u32)] = &[
    ("P1", 4),
    ("P2", 8),
    ("P3", 16),
    ("P4", 32),
    ("P6", 64),
    ("P10", 128),
    ("P15", 256),
    ("P20", 512),
    ("P30", 1_024),
    ("P40", 2_048),
    ("P50", 4_096),
    ("P60", 8_192),
    ("P70", 16_384),
    ("P80", 32_767),
];

const ADDRESS_PRODUCT: &str = "IP Addresses";
const ADDRESS_METER: &str = "Standard IPv4 Static Public IP";

/// A VM size priced in one region.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Offer {
    pub size: String,
    pub region: String,
    pub vcpu: u16,
    pub memory_gb: u16,
    /// Linux pay-as-you-go compute only.
    pub hourly_usd: f64,
}

/// Retail prices in US dollars. Each map holds only regions whose price was confirmed.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Catalog {
    pub offers: Vec<Offer>,
    /// A Premium SSD LRS disk per month, by region and tier size in GiB.
    pub disk_month_usd: BTreeMap<String, BTreeMap<u32, f64>>,
    /// A Standard static public IPv4 address per hour, by region. It has no monthly cap.
    pub static_ip_hour_usd: BTreeMap<String, f64>,
    /// The label of each region, such as `EUROPE`.
    pub regions: BTreeMap<String, String>,
}

impl Catalog {
    /// Current prices for every size in [`VM_SIZES`] and region in [`REGIONS`], from
    /// the public price list over HTTPS, without redirects or a credential.
    /// # Errors
    /// `Transport`, `Http` or `Cancelled` when a region cannot be read, and
    /// `InvalidResponse` for a page that is oversized, malformed, not in US dollars,
    /// or that links away from the price list or back to an earlier page.
    pub fn fetch(cancel: &Cancellation) -> Result<Self, CloudError> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .https_only(true)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        Self::fetch_with(|url| get(&agent, url), cancel)
    }

    /// As [`Self::fetch`], reading each page through `get`.
    pub(crate) fn fetch_with(
        mut get: impl FnMut(&str) -> Result<Vec<u8>, CloudError>,
        cancel: &Cancellation,
    ) -> Result<Self, CloudError> {
        let mut catalog = Self::default();
        for region in REGIONS {
            let items = pages(&mut get, first_page(region.name), cancel)?;
            catalog.add(region, &items);
        }
        Ok(catalog)
    }

    /// The monthly price of the smallest tier that holds `gb` in `region`. `None` when
    /// no tier holds it or that tier has no confirmed price: a larger tier is never
    /// substituted.
    #[must_use]
    pub fn disk_month(&self, region: &str, gb: u32) -> Option<f64> {
        self.disk_month_usd.get(region)?.get(&tier_gb(gb)?).copied()
    }

    fn add(&mut self, region: &Region, items: &[Item]) {
        let here: Vec<&Item> = items.iter().filter(|item| item.usable_in(region.name)).collect();
        for size in VM_SIZES {
            if let Some(hourly_usd) = agreed(here.iter().filter(|item| item.prices_vm(size))) {
                self.offers.push(Offer {
                    size: size.name.to_owned(),
                    region: region.name.to_owned(),
                    vcpu: size.vcpu,
                    memory_gb: size.memory_gb,
                    hourly_usd,
                });
            }
        }
        let disks: BTreeMap<u32, f64> = DISK_TIERS
            .iter()
            .filter_map(|(tier, gb)| {
                let meter = format!("{tier} LRS Disk");
                agreed(here.iter().filter(|item| item.prices_disk(&meter))).map(|month| (*gb, month))
            })
            .collect();
        if !disks.is_empty() {
            self.disk_month_usd.insert(region.name.to_owned(), disks);
        }
        if let Some(hour) = agreed(here.iter().filter(|item| item.prices_address())) {
            self.static_ip_hour_usd.insert(region.name.to_owned(), hour);
        }
        self.regions.insert(region.name.to_owned(), region.label.to_owned());
    }
}

/// The size in GiB of the smallest tier that holds `gb`.
#[must_use]
pub fn tier_gb(gb: u32) -> Option<u32> {
    DISK_TIERS.iter().map(|(_, size)| *size).find(|size| *size >= gb)
}

/// The first page of one region's VM, disk and address meters. The filter names
/// products, not sizes: the price list refuses a filter with many alternatives.
fn first_page(region: &str) -> String {
    let products = VM_SIZES
        .iter()
        .map(|size| size.product)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|product| format!("productName eq '{product}'"))
        .collect::<Vec<_>>()
        .join(" or ");
    let filter = format!(
        "priceType eq 'Consumption' and armRegionName eq '{region}' and \
         ((serviceName eq 'Virtual Machines' and ({products})) \
         or (serviceName eq 'Storage' and productName eq '{DISK_PRODUCT}') \
         or (serviceName eq 'Virtual Network' and productName eq '{ADDRESS_PRODUCT}'))"
    );
    format!("{ENDPOINT}?$filter={}", filter.replace(' ', "%20").replace('\'', "%27"))
}

fn get(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, CloudError> {
    let mut response = agent
        .get(url)
        .header("User-Agent", "horizon")
        .call()
        .map_err(|_| CloudError::Transport)?;
    let status = response.status().as_u16();
    if !(200..=299).contains(&status) {
        return Err(CloudError::Http(status, Reason::default()));
    }
    response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT)
        .read_to_vec()
        .map_err(|error| match error {
            ureq::Error::BodyExceedsLimit(_) => CloudError::InvalidResponse,
            _ => CloudError::Transport,
        })
}

#[derive(Deserialize)]
struct Page {
    #[serde(rename = "BillingCurrency")]
    billing_currency: String,
    /// Decoded one by one, so an unreadable item leaves only its own meter unpriced.
    #[serde(rename = "Items")]
    items: Vec<serde_json::Value>,
    #[serde(rename = "NextPageLink", default)]
    next_page_link: Option<String>,
}

/// Every item on the pages from `first`, following `NextPageLink`.
fn pages(
    get: &mut impl FnMut(&str) -> Result<Vec<u8>, CloudError>,
    first: String,
    cancel: &Cancellation,
) -> Result<Vec<Item>, CloudError> {
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut next = Some(first);
    while let Some(url) = next {
        cancel.check()?;
        if seen.len() == MAX_PAGES || !seen.insert(url.clone()) {
            return Err(CloudError::InvalidResponse);
        }
        let page: Page = serde_json::from_slice(&get(&url)?).map_err(|_| CloudError::InvalidResponse)?;
        if page.billing_currency != CURRENCY {
            return Err(CloudError::InvalidResponse);
        }
        items.extend(
            page.items
                .into_iter()
                .filter_map(|item| serde_json::from_value(item).ok()),
        );
        next = page.next_page_link.as_deref().map(on_endpoint).transpose()?;
    }
    Ok(items)
}

/// A next-page link, accepted only on the price endpoint.
fn on_endpoint(link: &str) -> Result<String, CloudError> {
    link.strip_prefix(ENDPOINT_WITH_PORT)
        .or_else(|| link.strip_prefix(ENDPOINT))
        .filter(|query| query.starts_with('?') && query.bytes().all(|byte| byte.is_ascii_graphic()))
        .map(|query| format!("{ENDPOINT}{query}"))
        .ok_or(CloudError::InvalidResponse)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    currency_code: String,
    retail_price: f64,
    arm_region_name: String,
    #[serde(default)]
    arm_sku_name: String,
    service_name: String,
    product_name: String,
    sku_name: String,
    meter_name: String,
    unit_of_measure: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    reservation_term: Option<String>,
}

impl Item {
    /// A pay-as-you-go US dollar price for `region`. `isPrimaryMeterRegion` is not
    /// read: it is false for some current meters, such as `DSv3` sizes.
    fn usable_in(&self, region: &str) -> bool {
        self.currency_code == CURRENCY
            && self.kind == "Consumption"
            && self.reservation_term.as_deref().is_none_or(str::is_empty)
            && self.arm_region_name == region
            && self.retail_price >= 0.0
    }

    /// The exact product name leaves out Windows, Cloud Services and Dedicated Host
    /// prices; spot and low-priority meters share the product.
    fn prices_vm(&self, size: &VmSize) -> bool {
        let discounted = |name: &str| name.contains("Spot") || name.contains("Low Priority");
        self.service_name == "Virtual Machines"
            && self.product_name == size.product
            && self.arm_sku_name == size.name
            && self.unit_of_measure == "1 Hour"
            && !discounted(&self.meter_name)
            && !discounted(&self.sku_name)
    }

    /// Exactly `P<n> LRS Disk`: the `Disk Mount` meters price shared disks.
    fn prices_disk(&self, meter: &str) -> bool {
        self.service_name == "Storage"
            && self.product_name == DISK_PRODUCT
            && self.meter_name == meter
            && self.unit_of_measure == "1/Month"
    }

    fn prices_address(&self) -> bool {
        self.service_name == "Virtual Network"
            && self.product_name == ADDRESS_PRODUCT
            && self.sku_name == "Standard"
            && self.meter_name == ADDRESS_METER
            && self.unit_of_measure == "1 Hour"
    }
}

/// The price that every matching item states, or `None` when none matches or two
/// disagree.
fn agreed<'a>(mut items: impl Iterator<Item = &'a &'a Item>) -> Option<f64> {
    let first = items.next()?.retail_price;
    items
        .all(|item| item.retail_price.to_bits() == first.to_bits())
        .then_some(first)
}

#[cfg(test)]
mod tests;
