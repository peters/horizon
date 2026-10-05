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
            let Some(form) = device_form(platform, &row.device) else {
                continue;
            };
            devices.push(Device {
                platform,
                form,
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

// The provider catalog has no form field. Unknown families cannot prove a requested form.
fn device_form(platform: Platform, name: &str) -> Option<Form> {
    let model = name.to_ascii_lowercase();
    match platform {
        Platform::Ios if model.starts_with("ipad ") || model == "ipad" => Some(Form::Tablet),
        Platform::Ios if model.starts_with("iphone ") => Some(Form::Phone),
        Platform::Ios => None,
        Platform::Android => {
            if [
                "samsung galaxy tab ",
                "google pixel tablet",
                "nexus 7",
                "nexus 9",
                "nexus 10",
            ]
            .iter()
            .any(|prefix| model.starts_with(prefix))
            {
                return Some(Form::Tablet);
            }
            // Numeric model families exclude newly introduced tablet/watch product names.
            [
                "google pixel ",
                "samsung galaxy s",
                "samsung galaxy a",
                "samsung galaxy note ",
                "samsung galaxy z fold",
                "samsung galaxy z flip",
                "oneplus ",
                "nexus ",
            ]
            .iter()
            .any(|prefix| {
                model
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.as_bytes().first())
                    .is_some_and(u8::is_ascii_digit)
            })
            .then_some(Form::Phone)
        }
    }
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
