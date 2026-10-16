use super::*;

#[test]
fn metadata_commands_pin_the_system_socket_without_client_fallback_or_mutation() {
    let client = Client {
        program: PathBuf::from("/usr/bin/tailscale"),
        cancel: Cancellation::default(),
    };
    let command = client.command(&["whois", "--json", "100.64.0.2"]);
    assert_eq!(command.get_program(), "/usr/bin/tailscale");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--socket=/run/tailscale/tailscaled.sock",
            "whois",
            "--json",
            "100.64.0.2"
        ]
    );
    assert!(command.get_envs().next().is_none());
}

#[test]
fn only_running_kernel_identity_is_eligible_and_duplicate_fields_are_refused() {
    let parse = |value: &str| serde_json::from_str::<Status>(value).unwrap().identity();
    let valid = r#"{"BackendState":"Running","TUN":true,"Self":{"ID":"n-desktop","TailscaleIPs":["100.64.0.1","fd7a:115c:a1e0::1"]},"User":{"ignored":{"LoginName":"fixture@example.invalid"}}}"#;
    assert_eq!(parse(valid).unwrap().addresses, [Ipv4Addr::new(100, 64, 0, 1)]);
    for invalid in [
        valid.replace("Running", "Stopped"),
        valid.replace("true", "false"),
        valid.replace("n-desktop", ""),
    ] {
        assert!(parse(&invalid).is_err());
    }
    assert!(serde_json::from_str::<Status>(&valid.replace("\"TUN\":true", "\"TUN\":true,\"TUN\":false")).is_err());
}

#[test]
fn whois_uses_stable_id_and_direct_node_addresses_not_numeric_id_or_subnet_routes() {
    let reply = r#"{"Node":{"ID":42,"StableID":"n-peer","Name":"DEVICE.EXAMPLE.TS.NET.","Addresses":["100.64.0.2/32","fd7a:115c:a1e0::2/128"],"AllowedIPs":["192.168.1.0/24"]}}"#;
    let peer = serde_json::from_str::<WhoIs>(reply).unwrap().peer().unwrap();
    assert_eq!(peer.id, "n-peer");
    assert_eq!(peer.name, "device.example.ts.net");
    assert_eq!(peer.addresses, [Ipv4Addr::new(100, 64, 0, 2)]);
    assert!(serde_json::from_str::<WhoIs>(&reply.replace("\"StableID\":\"n-peer\",", "")).is_err());
    assert!(
        serde_json::from_str::<WhoIs>(&reply.replace("100.64.0.2/32", "192.168.1.0/24"))
            .unwrap()
            .peer()
            .is_err()
    );
    assert!(
        serde_json::from_str::<WhoIs>(&reply.replace("DEVICE.EXAMPLE.TS.NET.", "bad\\n.example.ts.net"))
            .unwrap()
            .peer()
            .is_err()
    );
}
