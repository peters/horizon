//! Cloud offers for agents on this worker, ranked from the prices the owning Horizon
//! last sent. The worker holds no provider account; only public FX reference rates are fetched.
mod exchange;
#[cfg(test)]
mod tests;

use horizon_browser_control::manifest::{
    self,
    provider_usage::{UsageRequest, UsageResult},
};
use horizon_cloud::offers::{Requirements, hetzner_section, offers};
use horizon_cloud_protocol::offers::{HetznerSnapshot, MAX_BYTES, Snapshot};
use std::{
    io::{self, Read, Write},
    path::Path,
};

const SNAPSHOT: &str = "/run/sshd/cloud-offers.json";
/// Hetzner's catalog, beside the price list, when the owning Horizon has a Hetzner binding.
const HETZNER: &str = "cloud-offers-hetzner.json";
const RUNPOD_UNCONFIGURED: &str = "cloud-offers-runpod-unconfigured.json";
/// One directory per agent session this worker runs.
const SESSIONS: &str = "/workspace/sessions";
/// The owning Horizon sends prices at most 15 minutes old while it runs; older ones are
/// not offered as current.
const MAX_AGE_MILLIS: u64 = 20 * 60 * 1000;
/// Clock difference tolerated between this worker and the owning Horizon. Prices dated
/// later than that could never go stale, so they are refused.
const MAX_SKEW_MILLIS: u64 = 5 * 60 * 1000;

pub(crate) fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    match args.as_slice() {
        [command] if command == "publish" => publish(io::stdin().lock(), Path::new(SNAPSHOT), manifest::now_millis()),
        [command] if command == "publish-hetzner" => publish_hetzner(
            io::stdin().lock(),
            &hetzner_path(Path::new(SNAPSHOT)),
            manifest::now_millis(),
        ),
        [command] if command == "clear-runpod" => clear_runpod(io::stdin().lock(), Path::new(SNAPSHOT)),
        _ => Err(io::Error::other(
            "Usage: horizon-cloud-worker cloud-offers publish|publish-hetzner|clear-runpod",
        )),
    }
}

fn publish(reader: impl Read, path: &Path, now: i64) -> io::Result<()> {
    let snapshot = decode(reader)?;
    if future(&snapshot, now) {
        return Err(io::Error::other(
            "Cloud offer prices are dated in the future; check this computer's and the worker's clocks",
        ));
    }
    write_private(path, &serde_json::to_vec(&snapshot)?)?;
    if let Err(error) = std::fs::remove_file(path.with_file_name(RUNPOD_UNCONFIGURED))
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    Ok(())
}

/// Removes the `RunPod` prices, as when the owning Horizon no longer has a `RunPod` key,
/// so agents stop being offered them at once rather than once they go stale. Other
/// providers' catalogs stay. The request body carries nothing and is only drained.
fn clear_runpod(reader: impl Read, path: &Path) -> io::Result<()> {
    bounded(reader)?;
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    write_private(&path.with_file_name(RUNPOD_UNCONFIGURED), b"{}")
}

fn hetzner_path(snapshot: &Path) -> std::path::PathBuf {
    snapshot.with_file_name(HETZNER)
}

fn publish_hetzner(reader: impl Read, path: &Path, now: i64) -> io::Result<()> {
    let snapshot = decode_hetzner(reader)?;
    if dated_ahead(snapshot.observed_at_millis, now) {
        return Err(io::Error::other(
            "Hetzner offer prices are dated in the future; check this computer's and the worker's clocks",
        ));
    }
    write_private(path, &serde_json::to_vec(&snapshot)?)
}

fn decode_hetzner(reader: impl Read) -> io::Result<HetznerSnapshot> {
    let snapshot: HetznerSnapshot =
        serde_json::from_slice(&bounded(reader)?).map_err(|_| io::Error::other("Invalid Hetzner offer snapshot"))?;
    snapshot.validate().map_err(io::Error::other)?;
    Ok(snapshot)
}

/// Replaces `path` atomically. Each publication writes a file of its own first, so
/// overlapping ones never share a pending file.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid cloud offer snapshot path"))?;
    // Created readable by its owner only.
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn future(snapshot: &Snapshot, now: i64) -> bool {
    dated_ahead(snapshot.observed_at_millis, now)
}

fn dated_ahead(observed_at_millis: u64, now: i64) -> bool {
    observed_at_millis > u64::try_from(now).unwrap_or(0).saturating_add(MAX_SKEW_MILLIS)
}

fn bounded(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("Cloud offer snapshot is too large"));
    }
    Ok(bytes)
}

fn decode(reader: impl Read) -> io::Result<Snapshot> {
    let snapshot: Snapshot =
        serde_json::from_slice(&bounded(reader)?).map_err(|_| io::Error::other("Invalid cloud offer snapshot"))?;
    snapshot.validate().map_err(io::Error::other)?;
    Ok(snapshot)
}

/// Whether `actor` is an agent session this worker still runs.
fn live(actor: &str, sessions: &Path) -> bool {
    actor.strip_prefix("horizon:cloud-").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            && sessions.join(id).is_dir()
    })
}

/// The answer to an agent's `cloud_offers` request on this worker.
pub(crate) fn answer(request: &UsageRequest) -> UsageResult {
    answer_from(
        request,
        (Path::new(SNAPSHOT), Path::new(SESSIONS)),
        manifest::host_instance(),
        manifest::now_millis(),
    )
}

fn answer_from(request: &UsageRequest, (path, sessions): (&Path, &Path), host: &str, now: i64) -> UsageResult {
    let mut result = request.result(Vec::new(), None);
    match ranked(request, (path, sessions), host, now) {
        Ok(offers) => result.offers = Some(offers),
        Err(error) => result.error = Some(error),
    }
    result
}

fn ranked(
    request: &UsageRequest,
    (path, sessions): (&Path, &Path),
    host: &str,
    now: i64,
) -> Result<serde_json::Value, String> {
    if !live(&request.actor, sessions) || request.host_instance != host {
        return Err("cloud_offers_unavailable".to_owned());
    }
    if now >= request.deadline_at_millis.saturating_sub(1_000) {
        return Err("cloud_offers_timed_out".to_owned());
    }
    let requirements: Requirements = serde_json::from_value(request.cloud_offers.clone().unwrap_or_default())
        .map_err(|error| format!("cloud_offers_invalid_request: {error}"))?;
    rank_with(
        &requirements,
        path,
        now,
        request.deadline_at_millis.saturating_sub(now) > 6_000,
    )
}

/// Offers for `requirements` from the prices on this worker, for agents' MCP tools.
pub(crate) fn rank_here(requirements: &Requirements) -> Result<serde_json::Value, String> {
    rank(requirements, Path::new(SNAPSHOT), manifest::now_millis())
}

const NO_PRICES_YET: &str =
    "cloud_offers_unavailable: no prices from the Horizon that owns this cloud yet; they arrive while it runs";

fn rank(requirements: &Requirements, path: &Path, now: i64) -> Result<serde_json::Value, String> {
    rank_with(requirements, path, now, true)
}

fn rank_with(
    requirements: &Requirements,
    path: &Path,
    now: i64,
    fetch_rates: bool,
) -> Result<serde_json::Value, String> {
    let mut answer = native_rank(requirements, path, now)?;
    let rates = exchange::quote(fetch_rates && horizon_cloud::offers::comparison::needs_rates(&answer));
    horizon_cloud::offers::comparison::append(&mut answer, rates.as_ref());
    Ok(answer)
}

fn native_rank(requirements: &Requirements, path: &Path, now: i64) -> Result<serde_json::Value, String> {
    requirements
        .validate()
        .map_err(|error| format!("cloud_offers_invalid_request: {error}"))?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // A Horizon set up without a RunPod key sends only its other providers.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let other_providers = other_providers(requirements, &hetzner_path(path), now);
            if other_providers.is_empty() {
                return Err(NO_PRICES_YET.to_owned());
            }
            let configured = path.with_file_name(RUNPOD_UNCONFIGURED).is_file();
            return Ok(serde_json::json!({
                "comparison_incomplete": (!configured).then_some(true),
                "provider": "RunPod",
                "unavailable": "The Horizon that owns this cloud sends no RunPod prices",
                "offers": [],
                "other_providers": other_providers,
            }));
        }
        Err(_) => return Err(NO_PRICES_YET.to_owned()),
    };
    let snapshot =
        decode(file).map_err(|_| "cloud_offers_unavailable: the prices on this worker cannot be read".to_owned())?;
    if future(&snapshot, now) {
        return Err("cloud_offers_unavailable: the prices on this worker are dated in the future".to_owned());
    }
    let age = u64::try_from(now)
        .unwrap_or(0)
        .saturating_sub(snapshot.observed_at_millis);
    if age > MAX_AGE_MILLIS {
        // Current offers from other providers are not held back by stale RunPod
        // prices, as when the owning Horizon stopped using RunPod.
        let other_providers = other_providers(requirements, &hetzner_path(path), now);
        if other_providers.iter().any(|section| section.get("offers").is_some()) {
            return Ok(serde_json::json!({
                "provider": snapshot.list.provider,
                "unavailable": format!(
                    "The RunPod prices on this worker are {} minutes old; they refresh while the Horizon that owns this cloud runs",
                    age / 60_000
                ),
                "offers": [],
                "comparison_incomplete": true,
                "other_providers": other_providers,
            }));
        }
        return Err(format!(
            "cloud_offers_stale: the newest prices on this worker are {} minutes old; they refresh while the Horizon that owns this cloud runs",
            age / 60_000
        ));
    }
    Ok(serde_json::json!({
        "provider": snapshot.list.provider,
        "comparison_incomplete": !requirements.gpu && !hetzner_path(path).is_file(),
        "observed_at_millis": snapshot.observed_at_millis,
        "observed_seconds_ago": age / 1_000,
        "offers": offers(&snapshot.list, &snapshot.preferences, requirements),
        "other_providers": other_providers(requirements, &hetzner_path(path), now),
    }))
}

/// Offers from providers besides the price list, each in its own currency and never
/// ranked together here. Empty when the owning Horizon sent no such catalog.
fn other_providers(requirements: &Requirements, path: &Path, now: i64) -> Vec<serde_json::Value> {
    if requirements.gpu {
        return Vec::new();
    }
    let unavailable = |error: String| serde_json::json!({"provider": "Hetzner", "error": error});
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // No Hetzner binding on the owning Horizon, or no catalog sent yet.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => {
            return vec![unavailable(
                "cloud_offers_unavailable: the Hetzner prices on this worker cannot be read".to_owned(),
            )];
        }
    };
    let Ok(snapshot) = decode_hetzner(file) else {
        return vec![unavailable(
            "cloud_offers_unavailable: the Hetzner prices on this worker cannot be read".to_owned(),
        )];
    };
    if dated_ahead(snapshot.observed_at_millis, now) {
        return vec![unavailable(
            "cloud_offers_unavailable: the Hetzner prices on this worker are dated in the future".to_owned(),
        )];
    }
    let age = u64::try_from(now)
        .unwrap_or(0)
        .saturating_sub(snapshot.observed_at_millis);
    if age > MAX_AGE_MILLIS {
        return vec![unavailable(format!(
            "cloud_offers_stale: the newest Hetzner prices on this worker are {} minutes old",
            age / 60_000
        ))];
    }
    let mut section = hetzner_section(&snapshot.catalog, requirements);
    section["observed_at_millis"] = serde_json::json!(snapshot.observed_at_millis);
    section["observed_seconds_ago"] = serde_json::json!(age / 1_000);
    vec![section]
}
