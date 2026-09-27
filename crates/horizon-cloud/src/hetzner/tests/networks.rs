use super::*;

fn network(zone: &str, subnet_zone: &str) -> Value {
    json!({"id": 7, "name": format!("horizon-{zone}"), "ip_range": "10.72.0.0/16",
        "subnets": [{"type": "cloud", "ip_range": "10.72.0.0/17", "network_zone": subnet_zone}],
        "labels": {"horizon-network": zone}})
}

#[test]
fn the_zone_network_is_reused_or_created_once() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("networks", json!([network("eu-central", "eu-central")]))),
        (200, listing("networks", json!([]))),
        (201, json!({ "network": network("eu-central", "eu-central") })),
    ]);
    let cancel = Cancellation::default();
    assert_eq!(hetzner.ensure_network("eu-central", &cancel).unwrap().id, 7);
    assert_eq!(hetzner.ensure_network("eu-central", &cancel).unwrap().id, 7);
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /networks?label_selector=horizon-network%3Deu-central&page=1"));
    assert_eq!(
        request_body(&requests[2]),
        json!({"name": "horizon-eu-central", "ip_range": "10.72.0.0/16",
            "subnets": [{"type": "cloud", "ip_range": "10.72.0.0/17", "network_zone": "eu-central"}],
            "labels": {"horizon-network": "eu-central"}})
    );
}

#[test]
fn a_taken_name_reconciles_and_a_network_without_the_zone_is_refused() {
    let (hetzner, _, task) = provider(vec![
        (200, listing("networks", json!([]))),
        (409, error("uniqueness_error", "name is already used")),
        (200, listing("networks", json!([network("eu-central", "eu-central")]))),
        (200, listing("networks", json!([]))),
        (409, error("uniqueness_error", "name is already used")),
        (200, listing("networks", json!([]))),
        (200, listing("networks", json!([network("eu-central", "us-east")]))),
    ]);
    let cancel = Cancellation::default();
    assert_eq!(hetzner.ensure_network("eu-central", &cancel).unwrap().id, 7);
    assert!(matches!(
        hetzner.ensure_network("eu-central", &cancel),
        Err(CloudError::Invalid(_))
    ));
    assert!(matches!(
        hetzner.ensure_network("eu-central", &cancel),
        Err(CloudError::Invalid(_))
    ));
    assert!(matches!(
        hetzner.ensure_network("EU Central", &cancel),
        Err(CloudError::Invalid(_))
    ));
    task.join().unwrap();
}

#[test]
fn a_location_names_its_network_zone() {
    let (hetzner, requests, task) = provider(vec![
        (
            200,
            listing("locations", json!([{"name": "hel1", "network_zone": "eu-central"}])),
        ),
        (200, listing("locations", json!([]))),
    ]);
    let cancel = Cancellation::default();
    assert_eq!(hetzner.network_zone("hel1", &cancel).unwrap(), "eu-central");
    assert!(hetzner.network_zone("hel1", &cancel).is_err());
    task.join().unwrap();
    assert!(requests.lock().unwrap()[0].starts_with("GET /locations?name=hel1&page=1"));
}

#[test]
fn a_server_is_reached_only_through_its_one_address_on_horizons_network() {
    use crate::hetzner::{networks::horizon_address, servers::PrivateNet};
    let net = |network, ip: [u8; 4]| PrivateNet { network, ip: ip.into() };
    assert_eq!(horizon_address(&[net(7, [10, 72, 0, 3])]), Some([10, 72, 0, 3].into()));
    // Another network attached first is skipped.
    assert_eq!(
        horizon_address(&[net(9, [192, 168, 0, 5]), net(7, [10, 72, 0, 3])]),
        Some([10, 72, 0, 3].into())
    );
    // No address, one outside the subnet, or two inside it: none is published.
    assert_eq!(horizon_address(&[]), None);
    assert_eq!(horizon_address(&[net(7, [10, 72, 200, 3])]), None);
    assert_eq!(horizon_address(&[net(7, [10, 72, 0, 3]), net(8, [10, 72, 0, 4])]), None);
}

#[test]
fn a_labelled_network_with_other_address_ranges_is_refused_rather_than_joined() {
    let mut wider = network("eu-central", "eu-central");
    wider["ip_range"] = json!("10.0.0.0/8");
    let mut elsewhere = network("eu-central", "eu-central");
    elsewhere["subnets"][0]["ip_range"] = json!("10.72.128.0/17");
    let (hetzner, _, task) = provider(vec![
        (200, listing("networks", json!([wider]))),
        (200, listing("networks", json!([elsewhere]))),
    ]);
    let cancel = Cancellation::default();
    for _ in 0..2 {
        // Servers on it would publish no private address Horizon recognizes.
        let refused = hetzner.ensure_network("eu-central", &cancel);
        assert!(
            matches!(refused, Err(CloudError::Invalid(message)) if message.contains("address range")),
            "{refused:?}"
        );
    }
    task.join().unwrap();
}
