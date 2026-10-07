//! Start and stop the resources owned directly by a controller lane.
use crate::local::{Lease, Local};
use crate::{Error, Result};
use horizon_app_process::{Event, Kind};
use horizon_app_provider::{native_launch, tunnel::LocalPort};
use horizon_app_testing::contract::{Contract, Port};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) fn close(resources: &mut Vec<Lease>) -> Result<()> {
    // Tunnel first, then backends. Retain any resource whose cleanup is unconfirmed.
    resources.retain(|resource| resource.close().is_err());
    if resources.is_empty() {
        Ok(())
    } else {
        Err(Error::CleanupUncertain)
    }
}
pub(crate) fn start(
    local: &Local,
    contract: &Contract,
    claims: &Mutex<BTreeMap<u16, Uuid>>,
    owner: Uuid,
    deadline: Instant,
    resources: &mut Vec<Lease>,
) -> Result<(Uuid, BTreeMap<String, String>)> {
    let mut ports = BTreeMap::new();
    for (name, declaration) in &contract.tunnel.ports {
        let port = match declaration {
            Port::Fixed(port) => *port,
            Port::Managed(backend) => {
                let remaining = budget(deadline)?;
                let startup = Duration::from_secs(backend.timeout_seconds).min(remaining);
                let process = local.process(backend.start.clone(), Kind::Backend, startup, remaining)?;
                resources.insert(0, process);
                let ready_by = deadline.min(Instant::now() + startup);
                loop {
                    match resources
                        .first()
                        .ok_or(Error::Unavailable)?
                        .next(wait_budget(ready_by)?)?
                    {
                        Event::Started {} => (),
                        Event::Ready { port } => break port,
                        _ => return Err(horizon_app_process::Error::Failed.into()),
                    }
                }
            }
        };
        if ports.values().any(|existing| *existing == port) {
            return Err(horizon_app_provider::Error::TunnelPortRefused.into());
        }
        {
            let mut claims = claims.lock().map_err(|_| Error::Unavailable)?;
            if claims.contains_key(&port) {
                return Err(horizon_app_provider::Error::TunnelPortRefused.into());
            }
            claims.insert(port, owner);
        }
        ports.insert(name.clone(), port);
    }
    let resolved = contract
        .launch_arguments
        .values()
        .map(|value| contract.resolve_value_with_ports(value, &ports))
        .collect::<horizon_app_testing::Result<Vec<_>>>()?;
    let live = ports
        .values()
        .map(|port| LocalPort {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, *port)),
            tls: resolved.iter().any(|value| {
                url::Url::parse(value).is_ok_and(|url| {
                    matches!(url.scheme(), "https" | "wss") && url.port_or_known_default() == Some(*port)
                })
            }),
        })
        .collect::<Vec<_>>();
    let arguments = native_launch::arguments(contract, &ports, &live)?;
    let tunnel = local.tunnel(live, budget(deadline)?)?;
    let tunnel_id = tunnel.id;
    resources.insert(0, tunnel);
    budget(deadline)?;
    Ok((tunnel_id, arguments))
}
fn budget(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < Duration::from_secs(1) {
        return Err(horizon_app_runtime::Error::OperationExpired.into());
    }
    Ok(remaining)
}

// Event waits accept fractional seconds; whole-second guardian APIs keep their separate floor.
fn wait_budget(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(horizon_app_runtime::Error::OperationExpired.into());
    }
    Ok(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_second_backend_can_wait_after_startup_elapsed_but_expired_waits_refuse() {
        let ready_by = Instant::now() + Duration::from_millis(900);
        assert!(budget(ready_by).is_err());
        let remaining = wait_budget(ready_by).unwrap();
        assert!(!remaining.is_zero() && remaining < Duration::from_secs(1));
        assert!(wait_budget(Instant::now()).is_err());
    }
}
