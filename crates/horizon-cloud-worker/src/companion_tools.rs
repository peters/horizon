//! Read-only agent discovery. All commands share the same validated catalog.
mod mcp;
#[cfg(test)]
mod tests;

use horizon_cloud_protocol::companion::{Access, Catalog, Companion, Status};
use std::{
    io::{self, Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const CATALOG: &str = "/run/sshd/companions/catalog.json";
const MAX_CATALOG_BYTES: u64 = 256 * 1024;
const FRESH_SECONDS: u64 = 60;

pub(crate) fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    match args.as_slice() {
        [command] if command == "mcp" => mcp::run(),
        [command] if command == "publish" => {
            let catalog = decode(io::stdin().lock())?;
            super::companions::publish_catalog(&catalog)
        }
        [command] if command == "list" => output(&list(Path::new(CATALOG), now())?),
        [command, alias] if command == "inspect" => output(&inspect(
            Path::new(CATALOG),
            alias,
            now(),
            super::companions::probe_access,
        )?),
        _ => Err(io::Error::other(
            "Usage: horizon-cloud-worker companions list|inspect <alias>|mcp|publish",
        )),
    }
}

fn output(value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer(io::stdout().lock(), value)?;
    io::stdout().write_all(b"\n")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn decode(reader: impl Read) -> io::Result<Catalog> {
    let mut bytes = Vec::new();
    reader.take(MAX_CATALOG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(io::Error::other("Companion catalog is too large"));
    }
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|_| io::Error::other("Invalid companion catalog"))?;
    catalog.validate().map_err(io::Error::other)?;
    Ok(catalog)
}

fn load(path: &Path) -> io::Result<Catalog> {
    let file = std::fs::File::open(path).map_err(|_| {
        io::Error::other("Companion discovery is unavailable; refresh companions in the owning Horizon cloud panel")
    })?;
    decode(file)
}

fn list(path: &Path, current_time: u64) -> io::Result<Catalog> {
    let mut catalog = load(path)?;
    let fresh = current_time >= catalog.observed_at && current_time - catalog.observed_at <= FRESH_SECONDS;
    for entry in &mut catalog.companions {
        if !fresh && entry.status == Status::Ready {
            entry.status = Status::Unverified;
        }
    }
    Ok(catalog)
}

fn inspect(
    path: &Path,
    alias: &str,
    current_time: u64,
    probe: impl FnOnce(&Access) -> io::Result<bool>,
) -> io::Result<Companion> {
    let catalog = list(path, current_time)?;
    let mut entry = catalog
        .companions
        .into_iter()
        .find(|entry| entry.alias == alias)
        .ok_or_else(|| io::Error::other("Unknown companion alias"))?;
    if let Some(access) = &entry.access
        && matches!(entry.status, Status::Ready | Status::Unverified | Status::Unreachable)
    {
        entry.status = if probe(access).unwrap_or(false) {
            Status::Ready
        } else {
            Status::Unreachable
        };
        let latest = load(path)?;
        let current = latest.companions.into_iter().find(|current| current.alias == alias);
        if latest.source_cloud_id != catalog.source_cloud_id
            || current.as_ref().is_none_or(|current| {
                !current.selected
                    || current.access != entry.access
                    || current.target_cloud_id != entry.target_cloud_id
                    || current.repository != entry.repository
                    || current.profile != entry.profile
                    || !matches!(current.status, Status::Ready | Status::Unverified | Status::Unreachable)
            })
        {
            return Err(io::Error::other(
                "Companion selection changed during inspection; inspect again",
            ));
        }
    }
    Ok(entry)
}
