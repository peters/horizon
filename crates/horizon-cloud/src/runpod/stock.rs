//! Stock of an exact CPU pod size per data center. The REST catalog rates only
//! flavor families, which can report capacity where the requested size is absent.
use super::{RunPod, flavors::Flavor, json};
use crate::{Cancellation, CloudError, valid_id};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write;

/// Most available first, so the derived order ranks placements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Level {
    High,
    Medium,
    Low,
}

impl RunPod {
    /// Best stock level per data center across `flavors` at `cpu` vCPUs.
    /// Data centers without stock are absent; a malformed answer is an error.
    pub(super) fn cpu_stock(
        &self,
        centers: &[String],
        flavors: &[&Flavor],
        cpu: u16,
        cancel: &Cancellation,
    ) -> Result<HashMap<String, Level>, CloudError> {
        let centers: Vec<&String> = centers.iter().filter(|center| valid_id(center)).collect();
        if centers.is_empty() || flavors.is_empty() {
            return Ok(HashMap::new());
        }
        let mut query = String::from("query{");
        let mut lookups = Vec::new();
        for center in centers {
            for flavor in flavors {
                let alias = format!("s{}", lookups.len());
                let _ = write!(
                    query,
                    "{alias}:cpuFlavors{{id specifics(input:{{dataCenterId:{},instanceId:{}}}){{stockStatus}}}} ",
                    json!(center),
                    json!(flavor.instance_id(cpu)),
                );
                lookups.push((alias, center, flavor.id));
            }
        }
        query.push('}');
        let body = json!({ "query": query });
        let response: Response =
            serde_json::from_value(self.request_url("POST", &self.graphql_endpoint, Some(body), cancel, None)?)
                .map_err(|_| CloudError::InvalidResponse)?;
        let data = match response {
            Response {
                data: Some(data),
                errors,
            } if errors.is_empty() => data,
            _ => return Err(CloudError::InvalidResponse),
        };
        let mut stock: HashMap<String, Level> = HashMap::new();
        for (alias, center, flavor) in lookups {
            // A well-formed answer names every alias; a null value means no stock.
            let Some(entries) = data.get(&alias).ok_or(CloudError::InvalidResponse)? else {
                continue;
            };
            let specifics = entries
                .iter()
                .find(|entry| entry.id == flavor)
                .and_then(|entry| entry.specifics.as_ref())
                .ok_or(CloudError::InvalidResponse)?;
            let level = level(specifics.stock_status.as_deref())?;
            if let Some(level) = level {
                let best = stock.entry(center.clone()).or_insert(level);
                *best = (*best).min(level);
            }
        }
        Ok(stock)
    }
}

fn level(status: Option<&str>) -> Result<Option<Level>, CloudError> {
    let Some(status) = status else { return Ok(None) };
    match status.to_ascii_lowercase().as_str() {
        "high" => Ok(Some(Level::High)),
        "medium" => Ok(Some(Level::Medium)),
        "low" => Ok(Some(Level::Low)),
        "none" => Ok(None),
        _ => Err(CloudError::InvalidResponse),
    }
}

#[derive(Deserialize)]
struct Response {
    data: Option<HashMap<String, Option<Vec<Entry>>>>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    specifics: Option<Specifics>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Specifics {
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    stock_status: Option<String>,
}
