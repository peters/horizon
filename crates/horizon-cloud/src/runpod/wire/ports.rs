//! Optional published TCP facts. Missing or ambiguous facts never create a mapping.
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

const MAX_RUNTIME_PORTS: usize = 128;

#[derive(Deserialize)]
struct Runtime {
    ports: Vec<Port>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Port {
    private: u16,
    public: Option<u16>,
    #[serde(rename = "type")]
    kind: String,
    ip: Option<IpAddr>,
}

pub(super) fn with_runtime(
    runtime: Option<serde_json::Value>,
    host: Option<IpAddr>,
    known: Option<BTreeMap<String, u16>>,
) -> Option<BTreeMap<String, u16>> {
    match (runtime, host, known) {
        (Some(runtime), Some(host), Some(known)) => Some(mappings(runtime, host, &known).unwrap_or(known)),
        (_, _, known) => known,
    }
}

fn mappings(value: serde_json::Value, host: IpAddr, known: &BTreeMap<String, u16>) -> Option<BTreeMap<String, u16>> {
    let runtime: Runtime = serde_json::from_value(value).ok()?;
    if runtime.ports.len() > MAX_RUNTIME_PORTS {
        return None;
    }
    let ssh = *known.get("22")?;
    let mut private_ports = BTreeSet::new();
    let mut public_ports = BTreeSet::from([ssh]);
    let mut mappings = known.clone();
    for port in runtime.ports {
        if port.kind != "tcp" {
            continue;
        }
        if port.private == 0 || !private_ports.insert(port.private) {
            return None;
        }
        let (Some(public), Some(ip)) = (port.public, port.ip) else {
            continue;
        };
        if public == 0 || ip != host {
            return None;
        }
        if port.private == 22 {
            if public != ssh {
                return None;
            }
        } else if !public_ports.insert(public) {
            return None;
        } else {
            mappings.insert(port.private.to_string(), public);
        }
    }
    Some(mappings)
}
