//! Tailnet-managed host trust for keyless Docker host connections.
use super::{Result, Ssh};
use crate::cloud_runtime::command::Runner;
use serde::Deserialize;
use std::{collections::BTreeMap, net::IpAddr, process::Command, time::Duration};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(default)]
    peer: BTreeMap<String, Peer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Peer {
    #[serde(default, rename = "DNSName")]
    dns_name: String,
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Vec<IpAddr>,
    #[serde(default)]
    expired: bool,
    #[serde(default, rename = "sshHostKeys")]
    ssh_host_keys: Vec<String>,
}

pub(super) struct Missing {
    pub detail: &'static str,
    pub remedy: &'static str,
}

pub(super) fn missing(ssh: &Ssh, runner: &Runner<'_>) -> Result<Option<Missing>> {
    let output = runner.run_parsed(
        "Tailscale status",
        Command::new("tailscale").args(["status", "--json"]),
        Duration::from_secs(8),
    );
    let Ok(output) = output else {
        runner.cancel.check()?;
        return Ok(Some(Missing {
            detail: "The local Tailscale client did not answer",
            remedy: "Install Tailscale, start it and sign in on this computer.",
        }));
    };
    Ok(status_missing(&output, &ssh.host))
}

fn status_missing(output: &str, host: &str) -> Option<Missing> {
    let Ok(status) = serde_json::from_str::<Status>(output) else {
        return Some(Missing {
            detail: "The local Tailscale status could not be read",
            remedy: "Update the Tailscale client and check again.",
        });
    };
    if status.backend_state != "Running" {
        return Some(Missing {
            detail: "Tailscale is stopped or signed out on this computer",
            remedy: "Start Tailscale and sign in to the host's tailnet.",
        });
    }
    let host = host.trim_end_matches('.');
    let ip = host.parse::<IpAddr>().ok();
    let mut peers = status.peer.values().filter(|peer| {
        ip.is_some_and(|ip| peer.tailscale_ips.contains(&ip))
            || peer.dns_name.trim_end_matches('.').eq_ignore_ascii_case(host)
            || peer
                .dns_name
                .split('.')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(host))
    });
    let Some(peer) = peers.next() else {
        return Some(Missing {
            detail: "This host is not in the local Tailscale network map",
            remedy: "Use its MagicDNS name or Tailscale IP and check tailnet access.",
        });
    };
    if peers.next().is_some() {
        return Some(Missing {
            detail: "This short name matches more than one Tailscale host",
            remedy: "Use the host's full MagicDNS name or Tailscale IP.",
        });
    }
    if peer.expired {
        return Some(Missing {
            detail: "The host's Tailscale node key has expired",
            remedy: "Reauthenticate the host in Tailscale before deployment.",
        });
    }
    if !peer
        .ssh_host_keys
        .iter()
        .any(|key| !key.trim().is_empty() && !key.contains(['\r', '\n']))
    {
        return Some(Missing {
            detail: "Tailscale does not advertise SSH host keys for this host",
            remedy: "Enable Tailscale SSH on the Linux host and allow this user in the tailnet SSH policy.",
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const STATUS: &str = r#"{"BackendState":"Running","Peer":{"node":{"DNSName":"build.tail.test.","TailscaleIPs":["100.64.1.2"],"sshHostKeys":["ssh-ed25519 synthetic"]}}}"#;

    #[test]
    fn trust_comes_from_the_tailnet_for_names_and_addresses() {
        for host in ["build", "BUILD.tail.test.", "100.64.1.2"] {
            assert!(status_missing(STATUS, host).is_none(), "{host}");
        }
        assert!(status_missing(STATUS, "other").is_some());
    }

    #[test]
    fn untrusted_or_unavailable_tailnets_block_before_ssh() {
        for status in [
            "not json".to_owned(),
            STATUS.replace("Running", "NeedsLogin"),
            STATUS.replace("ssh-ed25519 synthetic", ""),
            STATUS.replace("\"DNSName\"", "\"Expired\":true,\"DNSName\""),
            STATUS.replace("ssh-ed25519 synthetic", "ssh-ed25519 synthetic\\ninvalid"),
        ] {
            assert!(status_missing(&status, "build").is_some());
        }
    }

    #[test]
    fn ambiguous_short_names_require_a_full_name_or_ip() {
        let mut status: serde_json::Value = serde_json::from_str(STATUS).unwrap();
        status["Peer"]["other"] = serde_json::json!({
            "DNSName": "build.other.test.", "TailscaleIPs": ["100.64.1.3"],
            "sshHostKeys": ["ssh-ed25519 other"]
        });
        let output = status.to_string();
        assert!(status_missing(&output, "build").is_some());
        assert!(status_missing(&output, "build.tail.test").is_none());
        assert!(status_missing(&output, "100.64.1.2").is_none());
    }
}
