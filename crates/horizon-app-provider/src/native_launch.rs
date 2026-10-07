//! `BrowserStack`'s fixed native-device loopback alias. Project declarations remain loopback-only.
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use horizon_app_testing::contract::Contract;

use crate::tunnel::LocalPort;
use crate::{Error, Result};

/// # Errors
/// Resolve the selected contract's arguments only against its exact live tunnel bindings.
/// Native HTTP/WebSocket endpoints use `BrowserStack`'s fixed `bs-local.com` alias, never a project-selected host.
/// IPv6-only native routing remains refused until provider forwarding is qualified.
pub fn arguments(
    contract: &Contract,
    ports: &BTreeMap<String, u16>,
    live: &[LocalPort],
) -> Result<BTreeMap<String, String>> {
    contract.validate().map_err(|_| Error::TunnelPortRefused)?;
    contract
        .resolve_value_with_ports("native-routing-check", ports)
        .map_err(|_| Error::TunnelPortRefused)?;
    let declared = ports.values().copied().collect::<BTreeSet<_>>();
    let mut bindings = BTreeMap::new();
    for binding in live {
        if binding.address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST)
            || !declared.contains(&binding.address.port())
            || bindings.insert(binding.address.port(), binding.tls).is_some()
        {
            return Err(Error::TunnelPortRefused);
        }
    }
    if bindings.len() != declared.len() {
        return Err(Error::TunnelPortRefused);
    }
    contract
        .launch_arguments
        .iter()
        .map(|(name, template)| {
            let value = contract
                .resolve_value_with_ports(template, ports)
                .map_err(|_| Error::TunnelPortRefused)?;
            let Ok(url) = url::Url::parse(&value) else {
                return Ok((name.clone(), value));
            };
            if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
                return Ok((name.clone(), value));
            }
            let port = url.port_or_known_default().ok_or(Error::TunnelPortRefused)?;
            if !matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
                || !url.username().is_empty()
                || url.password().is_some()
                || bindings.get(&port).copied() != Some(matches!(url.scheme(), "https" | "wss"))
            {
                return Err(Error::TunnelPortRefused);
            }
            // Keep an explicit port, including 80/443; URL serializers omit default ports.
            // Preserve an absent root path: base-URL consumers may append their own leading slash.
            let explicit_path = value.split_once("://").is_some_and(|(_, rest)| {
                rest.split(['?', '#'])
                    .next()
                    .is_some_and(|authority| authority.contains('/'))
            });
            let path = if url.path() == "/" && !explicit_path {
                ""
            } else {
                url.path()
            };
            let mut resolved = format!("{}://bs-local.com:{port}{path}", url.scheme());
            if let Some(query) = url.query() {
                resolved.push('?');
                resolved.push_str(query);
            }
            if let Some(fragment) = url.fragment() {
                resolved.push('#');
                resolved.push_str(fragment);
            }
            Ok((name.clone(), resolved))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(value: &str, port: u16) -> Contract {
        Contract::from_agents(&format!("```yaml\nremote-device-testing:\n  version: 1\n  provider: browserstack\n  apps:\n    ios:\n      build: [build]\n      artifact: App.ipa\n      bundle_id: com.example.app\n  matrix: [{{platform: ios, form: phone}}]\n  launch_arguments: {{BASE_URL: '{value}', MODE: synthetic}}\n  tunnel:\n    ports: {{backend: {port}}}\n  recipes: [recipe.md]\n```" )).unwrap()
    }
    fn binding(port: u16, tls: bool) -> LocalPort {
        LocalPort {
            address: format!("127.0.0.1:{port}").parse().unwrap(),
            tls,
        }
    }
    #[test]
    fn fixed_alias_keeps_exact_port_path_and_plain_launch_values() {
        let contract = project("http://localhost:{tunnel.port.backend}/config?a=1#section", 8080);
        let values = arguments(&contract, &[("backend".into(), 8080)].into(), &[binding(8080, false)]).unwrap();
        assert_eq!(values["BASE_URL"], "http://bs-local.com:8080/config?a=1#section");
        assert_eq!(values["MODE"], "synthetic");
    }
    #[test]
    fn base_url_without_a_path_does_not_gain_a_slash() {
        for (value, suffix) in [
            ("http://localhost:{tunnel.port.backend}", ""),
            ("http://localhost:{tunnel.port.backend}/", "/"),
        ] {
            let values = arguments(
                &project(value, 8080),
                &[("backend".into(), 8080)].into(),
                &[binding(8080, false)],
            )
            .unwrap();
            assert_eq!(values["BASE_URL"], format!("http://bs-local.com:8080{suffix}"));
        }
    }
    #[test]
    fn default_ports_remain_explicit_and_tls_matches_the_exact_binding() {
        for (scheme, port, tls) in [
            ("http", 80, false),
            ("https", 443, true),
            ("ws", 80, false),
            ("wss", 443, true),
        ] {
            let contract = project(&format!("{scheme}://127.0.0.1:{port}/"), port);
            let declared = [("backend".into(), port)].into();
            assert_eq!(
                arguments(&contract, &declared, &[binding(port, tls)]).unwrap()["BASE_URL"],
                format!("{scheme}://bs-local.com:{port}/")
            );
            assert!(arguments(&contract, &declared, &[binding(port, !tls)]).is_err());
        }
    }
    #[test]
    fn provider_alias_cannot_be_chosen_by_the_project_and_extra_or_external_bindings_are_refused() {
        let contract = project("http://127.0.0.1:8080", 8080);
        let declared = [("backend".into(), 8080)].into();
        for address in ["0.0.0.0:8080", "192.168.1.1:8080", "[::1]:8080"] {
            assert!(
                arguments(
                    &contract,
                    &declared,
                    &[LocalPort {
                        address: address.parse().unwrap(),
                        tls: false
                    }]
                )
                .is_err()
            );
        }
        assert!(arguments(&contract, &declared, &[binding(8080, false), binding(8081, false)]).is_err());
        assert!(arguments(&contract, &declared, &[binding(8080, false), binding(8080, false)]).is_err());
        assert!(arguments(&contract, &declared, &[]).is_err());
        let mut foreign = contract;
        for host in ["bs-local.com", "bs-local.com.example.org", "localhost.example.org"] {
            foreign
                .launch_arguments
                .insert("BASE_URL".into(), format!("http://{host}:8080"));
            assert!(arguments(&foreign, &declared, &[binding(8080, false)]).is_err());
        }
    }
}
