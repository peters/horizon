use super::*;

fn server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let (mut provider, requests, task) = super::server(responses);
    provider.catalog_endpoint.clone_from(&provider.endpoint);
    provider.graphql_endpoint = format!("{}/graphql", provider.endpoint);
    (provider, requests, task)
}
fn center(id: &str, availability: &str) -> Value {
    json!({"id":id,"networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":availability}]})
}
fn catalog(centers: &[Value]) -> String {
    json!({ "dataCenters": centers }).to_string()
}
/// One stock answer per queried data center and flavor, in query order. The
/// provider repeats every flavor for each lookup; only the queried one counts.
fn stock(levels: &[(&str, Option<&str>)]) -> String {
    let data: serde_json::Map<String, Value> = levels
        .iter()
        .enumerate()
        .map(|(index, (flavor, level))| {
            let entries: Vec<Value> = ["cpu3c", "cpu3g", "cpu5g"]
                .into_iter()
                .map(|id| {
                    let status = if id == *flavor { *level } else { Some("High") };
                    json!({"id":id,"specifics":{"stockStatus":status}})
                })
                .collect();
            (format!("s{index}"), Value::Array(entries))
        })
        .collect();
    json!({ "data": data }).to_string()
}
fn place(provider: &RunPod, worker: &WorkerSpec) -> Result<String, CloudError> {
    provider
        .workspace_volume_spec(worker, &Cancellation::default())
        .map(|spec| spec.data_center_id)
}
fn query(request: &str) -> String {
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    serde_json::from_str::<Value>(body).unwrap()["query"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn family_capacity_without_the_exact_size_in_stock_never_hosts_storage() {
    // The catalog rates every flavor family HIGH; only the size lookup tells them apart.
    let (provider, requests, task) = server(vec![
        (200, catalog(&[center("AP-JP-1", "HIGH"), center("EU-NL-1", "HIGH")])),
        (200, stock(&[("cpu3g", None), ("cpu3g", Some("High"))])),
    ]);
    assert_eq!(place(&provider, &spec()).unwrap(), "EU-NL-1");
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /datacenters?include=CPU_AVAILABILITY&networkVolumeTypes=STANDARD "));
    assert!(requests[1].starts_with("POST /graphql "));
    let query = query(&requests[1]);
    assert!(query.contains(r#"s0:cpuFlavors{id specifics(input:{dataCenterId:"AP-JP-1",instanceId:"cpu3g-4-16"})"#));
    assert!(query.contains(r#"s1:cpuFlavors{id specifics(input:{dataCenterId:"EU-NL-1",instanceId:"cpu3g-4-16"})"#));
}

#[test]
fn configured_preference_outranks_stock_level_and_limits_the_lookup() {
    let mut worker = spec();
    worker.data_centers = vec!["preferred".into(), "other".into()];
    let centers = [
        center("unconfigured", "HIGH"),
        center("other", "HIGH"),
        center("preferred", "LOW"),
    ];
    for (preferred, expected) in [(Some("Low"), "preferred"), (None, "other")] {
        let (provider, requests, task) = server(vec![
            (200, catalog(&centers)),
            (200, stock(&[("cpu3g", Some("High")), ("cpu3g", preferred)])),
        ]);
        assert_eq!(place(&provider, &worker).unwrap(), expected);
        task.join().unwrap();
        assert!(!query(&requests.lock().unwrap()[1]).contains("unconfigured"));
    }
}

#[test]
fn the_best_stocked_configured_flavor_ranks_a_data_center() {
    let mut worker = spec();
    worker.cpu_flavors = vec!["cpu3g".into(), "cpu5g".into()];
    let cases = [
        ([Some("Low"), None], [None, Some("medium")], "second"),
        ([Some("Low"), Some("High")], [Some("Medium"), None], "first"),
        ([Some("High"), Some("Low")], [Some("Medium"), None], "first"),
    ];
    for (first, second, expected) in cases {
        let (provider, requests, task) = server(vec![
            (200, catalog(&[center("first", "HIGH"), center("second", "HIGH")])),
            (
                200,
                stock(&[
                    ("cpu3g", first[0]),
                    ("cpu5g", first[1]),
                    ("cpu3g", second[0]),
                    ("cpu5g", second[1]),
                ]),
            ),
        ]);
        assert_eq!(place(&provider, &worker).unwrap(), expected);
        task.join().unwrap();
        assert!(query(&requests.lock().unwrap()[1]).contains(r#"instanceId:"cpu5g-4-16""#));
    }
}

#[test]
fn missing_or_unconfirmed_stock_refuses_storage() {
    let centers = [center("available", "HIGH")];
    let unconfirmed = [
        (200, stock(&[("cpu3g", None)])),
        (200, stock(&[("cpu3g", Some("NONE"))])),
        (200, json!({"data":{"s0":null}}).to_string()),
    ];
    for answer in unconfirmed {
        let (provider, _, task) = server(vec![(200, catalog(&centers)), answer]);
        assert!(matches!(place(&provider, &spec()), Err(CloudError::Invalid(_))));
        task.join().unwrap();
    }
    let malformed = [
        (
            200,
            json!({"errors":[{"message":"unavailable"}],"data":null}).to_string(),
        ),
        (
            200,
            json!({"errors":[{"message":"partial"}],"data":{"s0":[]}}).to_string(),
        ),
        (200, json!({"data":{}}).to_string()),
        (200, "not json".to_owned()),
        (500, "{}".to_owned()),
    ];
    for answer in malformed {
        let (provider, _, task) = server(vec![(200, catalog(&centers)), answer]);
        // Provider failures surface as such, not as a capacity verdict.
        assert!(!matches!(
            place(&provider, &spec()),
            Ok(_) | Err(CloudError::Invalid(_))
        ));
        task.join().unwrap();
    }
}

#[test]
fn catalogs_without_family_capacity_skip_the_stock_lookup() {
    let mut worker = spec();
    worker.data_centers = vec!["preferred".into()];
    let catalogs = [
        catalog(&[center("preferred", "NONE")]),
        catalog(&[center("unconfigured", "HIGH")]),
        json!({"dataCenters":[{"id":"preferred","networkVolumeTypes":["HIGH_PERFORMANCE"],"cpuAvailability":[{"id":"cpu3g","availability":"HIGH"}]}]}).to_string(),
    ];
    for catalog in catalogs {
        let (provider, requests, task) = server(vec![(200, catalog)]);
        // A stray stock lookup would be refused by the mock server as a transport error.
        assert!(matches!(place(&provider, &worker), Err(CloudError::Invalid(_))));
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn sized_catalog_entries_do_not_certify_flavor_capacity() {
    for id in ["cpu3g-2-4", "cpu3g-4-16", "cpu3g-unknown"] {
        let catalog = json!({"dataCenters":[{
            "id":"available","networkVolumeTypes":["STANDARD"],
            "cpuAvailability":[{"id":id,"availability":"HIGH"}]
        }]});
        let (provider, requests, task) = server(vec![(200, catalog.to_string())]);
        assert!(matches!(place(&provider, &spec()), Err(CloudError::Invalid(_))));
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn malformed_stock_cannot_be_masked_by_a_healthy_alternative() {
    let malformed = [
        json!([]),
        json!([{"id":"other","specifics":{"stockStatus":"High"}}]),
        json!([{"id":"cpu3g"}]),
        json!([{"id":"cpu3g","specifics":null}]),
        json!([{"id":"cpu3g","specifics":{}}]),
    ];
    for same_center in [false, true] {
        let mut worker = spec();
        let centers = if same_center {
            worker.cpu_flavors = vec!["cpu3g".into(), "cpu5g".into()];
            vec![center("first", "HIGH")]
        } else {
            vec![center("first", "HIGH"), center("second", "HIGH")]
        };
        for malformed in &malformed {
            for index in 0..2 {
                let mut answer: Value = serde_json::from_str(&stock(&[
                    ("cpu3g", Some("High")),
                    (if same_center { "cpu5g" } else { "cpu3g" }, Some("High")),
                ]))
                .unwrap();
                let mut invalid = malformed.clone();
                if same_center && index == 1 {
                    for entry in invalid.as_array_mut().unwrap() {
                        if entry["id"] == "cpu3g" {
                            entry["id"] = json!("cpu5g");
                        }
                    }
                }
                answer["data"][format!("s{index}")] = invalid;
                let (provider, requests, task) = server(vec![(200, catalog(&centers)), (200, answer.to_string())]);
                assert!(matches!(place(&provider, &worker), Err(CloudError::InvalidResponse)));
                task.join().unwrap();
                assert_eq!(requests.lock().unwrap().len(), 2);
            }
        }
    }
}
