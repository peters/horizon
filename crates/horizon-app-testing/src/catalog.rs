use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::contract::{Form, MatrixEntry, Platform, printable, version};
use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub platform: Platform,
    pub form: Form,
    pub model: String,
    pub os_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedDevice {
    pub matrix_index: usize,
    pub device: Device,
}

#[derive(Deserialize)]
struct ProviderRow {
    os: String,
    os_version: String,
    device: String,
    #[serde(rename = "realMobile", alias = "real_mobile")]
    real_mobile: bool,
}

/// Decode the App Automate catalog, which is distinct from the browser catalog.
/// # Errors
/// Refuses malformed or oversized responses without copying response bodies into errors.
pub fn decode(bytes: &[u8]) -> Result<Vec<Device>> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(Error::CatalogInvalid);
    }
    let rows: Vec<ProviderRow> = serde_json::from_slice(bytes).map_err(|_| Error::CatalogInvalid)?;
    if rows.is_empty() || rows.len() > 50_000 {
        return Err(Error::CatalogInvalid);
    }
    let mut devices = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if !row.real_mobile {
            continue;
        }
        let platform = match row.os.to_ascii_lowercase().as_str() {
            "ios" => Platform::Ios,
            "android" => Platform::Android,
            _ => return Err(Error::CatalogInvalid),
        };
        if !printable(&row.device, 128) || version(&row.os_version).is_none() {
            return Err(Error::CatalogInvalid);
        }
        if seen.insert((platform, row.device.clone(), row.os_version.clone())) {
            let model = row.device.to_ascii_lowercase();
            let tablet = model.starts_with("ipad")
                || model.contains("galaxy tab")
                || model.contains("pixel tablet")
                || model.starts_with("nexus 9")
                || model.starts_with("nexus 10");
            devices.push(Device {
                platform,
                form: if tablet { Form::Tablet } else { Form::Phone },
                model: row.device,
                os_version: row.os_version,
            });
        }
    }
    devices.sort_by(|a, b| (a.platform, &a.model, &a.os_version).cmp(&(b.platform, &b.model, &b.os_version)));
    if devices.is_empty() {
        return Err(Error::CatalogInvalid);
    }
    Ok(devices)
}

/// Resolve every entry from one catalog snapshot; never silently omit an unavailable entry.
/// # Errors
/// A missing platform, model, form or OS fails the whole resolution before allocations.
pub fn resolve(matrix: &[MatrixEntry], catalog: &[Device]) -> Result<Vec<ResolvedDevice>> {
    matrix
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut candidates: Vec<_> = catalog
                .iter()
                .filter(|device| {
                    device.platform == entry.platform
                        && device.form == entry.form
                        && entry
                            .device
                            .as_ref()
                            .is_none_or(|model| model.eq_ignore_ascii_case(&device.model))
                })
                .filter_map(|device| version(&device.os_version).map(|v| (device, v)))
                .collect();
            let wanted = if entry.os == "latest" || entry.os.starts_with("latest-") {
                let majors: BTreeSet<_> = candidates.iter().map(|(_, v)| v[0]).collect();
                let offset = entry
                    .os
                    .strip_prefix("latest-")
                    .map_or(Ok(0), str::parse::<u32>)
                    .map_err(|_| Error::ContractInvalid)?;
                vec![
                    *majors
                        .iter()
                        .rev()
                        .nth(offset as usize)
                        .ok_or(Error::MatrixUnavailable)?,
                ]
            } else {
                version(&entry.os).ok_or(Error::ContractInvalid)?
            };
            candidates.retain(|(_, v)| v.starts_with(&wanted));
            candidates.sort_by(|(a, av), (b, bv)| bv.cmp(av).then_with(|| a.model.cmp(&b.model)));
            let device = candidates.first().ok_or(Error::MatrixUnavailable)?.0.clone();
            Ok(ResolvedDevice {
                matrix_index: index,
                device,
            })
        })
        .collect()
}
