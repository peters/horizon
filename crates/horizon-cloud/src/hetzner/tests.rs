use super::*;
use serde_json::json;
use std::{
    io::Write,
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};
mod keys;
mod servers;
mod volumes;

const OPERATION: &str = "op-1";

type Requests = Arc<Mutex<Vec<String>>>;

/// A provider answering each connection with the next scripted response.
fn provider(responses: Vec<(u16, Value)>) -> (Hetzner, Requests, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut input = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                input.extend_from_slice(&buffer[..read]);
                if let Some(end) = input.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&input[..end]).to_ascii_lowercase();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if input.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            observed.lock().unwrap().push(String::from_utf8(input).unwrap());
            let body = if body.is_null() {
                String::new()
            } else {
                body.to_string()
            };
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let mut hetzner = Hetzner::new(Credential::new("secret-test-key".into()).unwrap());
    hetzner.endpoint = format!("http://{address}");
    hetzner.poll = Duration::from_millis(1);
    (hetzner, requests, task)
}

fn listing(key: &str, items: Value) -> Value {
    let mut page = json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items;
    page
}

fn action(id: u64, status: &str) -> Value {
    json!({"id": id, "command": "test", "status": status, "error": null})
}

fn error(code: &str, message: &str) -> Value {
    json!({"error": {"code": code, "message": message, "details": {}}})
}

fn request_body(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn failures_keep_their_codes_and_map_to_cloud_errors() {
    let (hetzner, requests, task) = provider(vec![
        (401, error("unauthorized", "unable to authenticate")),
        (403, error("forbidden", "insufficient permissions")),
        (403, error("resource_limit_exceeded", "server limit reached")),
        (422, error("invalid_input", "invalid server type")),
        (412, error("resource_unavailable", "server type cpx22 unavailable")),
        (409, error("uniqueness_error", "name is already used")),
        (500, error("server_error", "echoed secret-test-key")),
        (503, json!("<html>busy</html>")),
        (403, error("maintenance", "maintenance in progress")),
    ]);
    let cancel = Cancellation::default();
    let fail = || hetzner.send(Method::Get, "/servers", None, &cancel).unwrap_err();
    assert!(matches!(fail().into(), CloudError::Unauthorized));
    assert!(matches!(fail().into(), CloudError::Unauthorized));
    assert_eq!(
        CloudError::from(fail()).to_string(),
        "Provider rejected the request or capacity is unavailable: server limit reached"
    );
    assert!(matches!(fail().into(), CloudError::Rejected(_)));
    let capacity = fail();
    assert!(capacity.capacity() && capacity.definite());
    let taken = fail();
    assert!(taken.name_taken() && !taken.definite());
    assert_eq!(CloudError::from(fail()).to_string(), "Provider returned HTTP 500");
    let busy = fail();
    assert!(!busy.definite());
    assert_eq!(CloudError::from(busy).to_string(), "Provider returned HTTP 503");
    assert_eq!(
        CloudError::from(fail()).to_string(),
        "Provider returned HTTP 403: maintenance in progress"
    );
    task.join().unwrap();
    for request in requests.lock().unwrap().iter() {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("\r\nauthorization: bearer secret-test-key\r\n")
        );
    }
    cancel.cancel();
    assert!(matches!(
        hetzner.send(Method::Get, "/servers", None, &cancel),
        Err(Failure::Local(CloudError::Cancelled))
    ));
}

#[test]
fn pagination_follows_next_pages_and_refuses_loops() {
    let page = |items: Value, next: Value| json!({"servers": items, "meta": {"pagination": {"next_page": next}}});
    let (hetzner, requests, task) = provider(vec![
        (200, page(json!([1, 2]), json!(2))),
        (200, page(json!([3]), Value::Null)),
        (200, page(json!([1]), json!(1))),
        (200, json!({"servers": []})),
    ]);
    let cancel = Cancellation::default();
    let all: Vec<u64> = hetzner
        .list_all("/servers", "label_selector=a%3Db", "servers", &cancel)
        .unwrap();
    assert_eq!(all, [1, 2, 3]);
    assert!(hetzner.list_all::<u64>("/servers", "", "servers", &cancel).is_err());
    assert!(hetzner.list_all::<u64>("/servers", "", "servers", &cancel).is_err());
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /servers?label_selector=a%3Db&page=1&per_page=50 "));
    assert!(requests[1].starts_with("GET /servers?label_selector=a%3Db&page=2&per_page=50 "));
    assert!(requests[2].starts_with("GET /servers?page=1&per_page=50 "));
}

#[test]
fn actions_are_awaited_and_failed_actions_carry_their_message() {
    let (hetzner, requests, task) = provider(vec![
        (200, json!({"action": action(7, "success")})),
        (
            200,
            json!({"action": {"id": 8, "status": "error", "error": {"code": "x", "message": "volume busy"}}}),
        ),
        (200, json!({"action": action(99, "success")})),
    ]);
    let cancel = Cancellation::default();
    let running = |id| serde_json::from_value::<Action>(action(id, "running")).unwrap();
    hetzner.wait(&running(7), &cancel).unwrap();
    assert_eq!(
        hetzner.wait(&running(8), &cancel).unwrap_err().to_string(),
        "Provider rejected the request or capacity is unavailable: volume busy"
    );
    assert!(matches!(
        hetzner.wait(&running(9), &cancel),
        Err(CloudError::InvalidResponse)
    ));
    let unknown = serde_json::from_value::<Action>(action(1, "paused")).unwrap();
    assert!(matches!(
        hetzner.wait(&unknown, &cancel),
        Err(CloudError::InvalidResponse)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap()[0].starts_with("GET /actions/7 "));
}

#[test]
fn catalog_lists_current_x86_offers_cheapest_first_with_live_availability() {
    let price = |location: &str, hourly: &str, monthly: &str| json!({"location": location, "price_hourly": {"net": hourly, "gross": "0"}, "price_monthly": {"net": monthly, "gross": "0"}});
    let standing = |name: &str, available: bool, deprecated: bool| {
        let deprecation = if deprecated {
            json!({"announced": "2025-10-16T00:00:00Z"})
        } else {
            Value::Null
        };
        json!({"id": 1, "name": name, "available": available, "recommended": available, "deprecation": deprecation})
    };
    let kind = |name: &str, architecture: &str, prices: Value, locations: Value| {
        json!({"id": 1, "name": name, "cores": 8, "memory": 16.0, "disk": 320, "cpu_type": "shared",
            "architecture": architecture, "deprecation": null, "prices": prices, "locations": locations})
    };
    let types = json!([
        kind(
            "cpx42",
            "x86",
            json!([price("hel1", "0.1114", "69.49"), price("fsn1", "0.1114", "69.49")]),
            json!([standing("hel1", true, false), standing("fsn1", false, false)])
        ),
        // Deprecated in hel1 only, and priced in a location it does not list.
        kind(
            "cpx41",
            "x86",
            json!([
                price("ash", "0.1931", "120.49"),
                price("hel1", "0.05", "32"),
                price("sin", "0.08", "50")
            ]),
            json!([standing("ash", true, false), standing("hel1", false, true)])
        ),
        kind(
            "cax31",
            "arm",
            json!([price("hel1", "0.02", "12")]),
            json!([standing("hel1", true, false)])
        ),
    ]);
    let pricing = json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.0572"}},
        "primary_ips": [{"type": "ipv4", "prices": [{"location": "hel1", "price_monthly": {"net": "0.50"}}]},
                        {"type": "ipv6", "prices": [{"location": "hel1", "price_monthly": {"net": "0"}}]}]}});
    let locations = json!([
        {"id": 1, "name": "hel1", "network_zone": "eu-central", "country": "FI", "city": "Helsinki"},
        {"id": 2, "name": "ash", "network_zone": "us-east", "country": "US", "city": "Ashburn, VA"},
        {"id": 3, "name": "sin", "network_zone": "ap-southeast", "country": "SG", "city": "Singapore"},
    ]);
    let (hetzner, requests, task) = provider(vec![
        (200, listing("server_types", types.clone())),
        (200, listing("locations", locations.clone())),
        (200, pricing),
        (200, listing("server_types", types)),
        (200, listing("locations", locations)),
        (
            200,
            json!({"pricing": {"currency": "USD", "volume": {"price_per_gb_month": {"net": "0.05"}}, "primary_ips": []}}),
        ),
    ]);
    let catalog = hetzner.catalog(&Cancellation::default()).unwrap();
    let summary: Vec<_> = catalog
        .offers
        .iter()
        .map(|offer| (offer.server_type.as_str(), offer.location.as_str(), offer.available))
        .collect();
    assert_eq!(
        summary,
        [
            ("cpx42", "fsn1", false),
            ("cpx42", "hel1", true),
            ("cpx41", "ash", true)
        ]
    );
    assert!(catalog.offers[1].recommended && !catalog.offers[0].recommended);
    assert!((catalog.offers[1].monthly_eur - 69.49).abs() < 1e-9);
    assert!((catalog.volume_gb_month_eur - 0.0572).abs() < 1e-9);
    assert_eq!(catalog.ipv4_month_eur.len(), 1);
    assert_eq!(catalog.regions["hel1"], "EUROPE");
    assert_eq!(catalog.regions["ash"], "NORTH_AMERICA");
    assert_eq!(catalog.regions["sin"], "ASIA");
    let encoded = serde_json::to_value(&catalog).unwrap();
    assert_eq!(serde_json::from_value::<catalog::Catalog>(encoded).unwrap(), catalog);
    assert!(matches!(
        hetzner.catalog(&Cancellation::default()),
        Err(CloudError::InvalidResponse)
    ));
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.contains("/datacenters")),
        "the datacenters endpoint is removed after 1 October 2026"
    );
}

#[test]
fn resource_names_are_host_names_owned_by_one_operation() {
    assert_eq!(resource_name("op-1").unwrap(), "horizon-cloud-op-1");
    for invalid in ["", "Op", "op_1", "-op", "op-", "op.1", &"a".repeat(49)] {
        assert!(resource_name(invalid).is_err(), "{invalid}");
    }
}
