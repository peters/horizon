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
