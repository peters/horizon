//! Cloud offers for agents on this worker, ranked from the prices the owning Horizon
//! last sent. The worker holds no provider account, so it never fetches prices itself.
#[cfg(test)]
mod tests;

use horizon_browser_control::manifest::{
    self,
    provider_usage::{UsageRequest, UsageResult},
};
use horizon_cloud::offers::{Requirements, offers};
use horizon_cloud_protocol::offers::{MAX_BYTES, Snapshot};
use std::{
    io::{self, Read},
    path::Path,
};

const SNAPSHOT: &str = "/run/sshd/cloud-offers.json";
/// The owning Horizon sends prices at most 15 minutes old while it runs; older ones are
/// not offered as current.
const MAX_AGE_MILLIS: u64 = 20 * 60 * 1000;

pub(crate) fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    match args.as_slice() {
        [command] if command == "publish" => publish(io::stdin().lock(), Path::new(SNAPSHOT)),
        _ => Err(io::Error::other("Usage: horizon-cloud-worker cloud-offers publish")),
    }
}

fn publish(reader: impl Read, path: &Path) -> io::Result<()> {
    let snapshot = decode(reader)?;
    super::companions::files::write(path, &serde_json::to_vec(&snapshot)?)
}

fn decode(reader: impl Read) -> io::Result<Snapshot> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("Cloud offer snapshot is too large"));
    }
    let snapshot: Snapshot =
        serde_json::from_slice(&bytes).map_err(|_| io::Error::other("Invalid cloud offer snapshot"))?;
    snapshot.validate().map_err(io::Error::other)?;
    Ok(snapshot)
}

/// The answer to an agent's `cloud_offers` request on this worker.
pub(crate) fn answer(request: &UsageRequest) -> UsageResult {
    answer_from(
        request,
        Path::new(SNAPSHOT),
        manifest::host_instance(),
        manifest::now_millis(),
    )
}

fn answer_from(request: &UsageRequest, path: &Path, host: &str, now: i64) -> UsageResult {
    let mut result = request.result(Vec::new(), None);
    match ranked(request, path, host, now) {
        Ok(offers) => result.offers = Some(offers),
        Err(error) => result.error = Some(error),
    }
    result
}

fn ranked(request: &UsageRequest, path: &Path, host: &str, now: i64) -> Result<serde_json::Value, String> {
    if !request.actor.starts_with("horizon:cloud-") || request.host_instance != host {
        return Err("cloud_offers_unavailable".to_owned());
    }
    if now >= request.deadline_at_millis.saturating_sub(1_000) {
        return Err("cloud_offers_timed_out".to_owned());
    }
    let invalid = |error: String| format!("cloud_offers_invalid_request: {error}");
    let requirements: Requirements =
        serde_json::from_value(request.cloud_offers.clone().unwrap_or_default()).map_err(|e| invalid(e.to_string()))?;
    requirements.validate().map_err(|error| invalid(error.to_owned()))?;
    let file = std::fs::File::open(path).map_err(|_| {
        "cloud_offers_unavailable: no prices from the Horizon that owns this cloud yet; they arrive while it runs"
            .to_owned()
    })?;
    let snapshot =
        decode(file).map_err(|_| "cloud_offers_unavailable: the prices on this worker cannot be read".to_owned())?;
    let age = u64::try_from(now)
        .unwrap_or(0)
        .saturating_sub(snapshot.observed_at_millis);
    if age > MAX_AGE_MILLIS {
        return Err(format!(
            "cloud_offers_stale: the newest prices on this worker are {} minutes old; they refresh while the Horizon that owns this cloud runs",
            age / 60_000
        ));
    }
    Ok(serde_json::json!({
        "provider": snapshot.list.provider,
        "observed_at_millis": snapshot.observed_at_millis,
        "observed_seconds_ago": age / 1_000,
        "offers": offers(&snapshot.list, &snapshot.preferences, &requirements),
    }))
}
