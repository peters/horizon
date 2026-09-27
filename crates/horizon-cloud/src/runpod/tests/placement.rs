use super::*;

fn center(id: &str, availability: &str) -> Value {
    json!({"id":id,"networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":availability}]})
}
fn catalog(centers: &[Value]) -> String {
    json!({"dataCenters":centers}).to_string()
}
fn cpu(id: &str, centers: &[(&str, &str)]) -> Value {
    let rows: Vec<_> = centers
        .iter()
        .map(|(id, level)| json!({"id":id,"availability":level}))
        .collect();
    json!({"id":id,"vcpu":{"min":2,"max":32},"ramGbPerVcpu":4,"dataCenters":rows})
}
fn stock(rows: &[Value]) -> String {
    json!({"cpus":rows}).to_string()
}
fn place(provider: &RunPod, worker: &WorkerSpec) -> Result<String, CloudError> {
    provider
        .workspace_volume_spec(worker, &Cancellation::default())
        .map(|spec| spec.data_center_id)
}

#[test]
fn family_capacity_without_exact_size_never_hosts_storage() {
    let (provider, requests, task) = server(vec![
        (200, catalog(&[center("first", "HIGH"), center("second", "HIGH")])),
        (200, stock(&[cpu("cpu3g", &[("first", "NONE"), ("second", "HIGH")])])),
    ]);
    assert_eq!(place(&provider, &spec()).unwrap(), "second");
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("GET /cpus?include=AVAILABILITY&product=POD&vcpuCount=4 "));
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
}

#[test]
fn preference_outranks_stock_and_unconfigured_centers_cannot_win() {
    let mut spec = spec();
    spec.data_centers = vec!["preferred".into(), "other".into()];
    for (level, expected) in [("LOW", "preferred"), ("NONE", "other")] {
        let (provider, _, task) = server(vec![
            (
                200,
                catalog(&[
                    center("unconfigured", "HIGH"),
                    center("other", "HIGH"),
                    center("preferred", "LOW"),
                ]),
            ),
            (
                200,
                stock(&[cpu(
                    "cpu3g",
                    &[("unconfigured", "HIGH"), ("other", "HIGH"), ("preferred", level)],
                )]),
            ),
        ]);
        assert_eq!(place(&provider, &spec).unwrap(), expected);
        task.join().unwrap();
    }
}

#[test]
fn best_stocked_configured_flavor_ranks_the_center() {
    let mut spec = spec();
    spec.cpu_flavors.push("cpu5g".into());
    for (a, b, expected) in [("LOW", "MEDIUM", "second"), ("HIGH", "MEDIUM", "first")] {
        let (provider, _, task) = server(vec![
            (200, catalog(&[center("first", "HIGH"), center("second", "HIGH")])),
            (
                200,
                stock(&[cpu("cpu3g", &[("first", a)]), cpu("cpu5g", &[("second", b)])]),
            ),
        ]);
        assert_eq!(place(&provider, &spec).unwrap(), expected);
        task.join().unwrap();
    }
}

#[test]
fn unavailable_everywhere_may_omit_centers_but_never_certifies_capacity() {
    for entry in [
        cpu("cpu3g", &[]),
        json!({"id":"cpu3g","vcpu":{"min":2,"max":32},"ramGbPerVcpu":4}),
    ] {
        let (provider, _, task) = server(vec![(200, catalog(&[center("first", "HIGH")])), (200, stock(&[entry]))]);
        assert!(matches!(place(&provider, &spec()), Err(CloudError::Invalid(_))));
        task.join().unwrap();
    }
}

#[test]
fn malformed_or_incompatible_stock_is_not_masked_by_an_alternative() {
    let mut spec = spec();
    spec.cpu_flavors.push("cpu5g".into());
    let base = cpu("cpu3g", &[("first", "HIGH")]);
    let mut cases = vec![
        json!({}),
        json!({"id":"cpu3g"}),
        cpu("cpu3g", &[("first", "UNKNOWN")]),
        cpu("cpu3g", &[("first", "HIGH"), ("first", "LOW")]),
    ];
    for (field, value) in [
        ("ramGbPerVcpu", json!(2.5)),
        ("vcpu", json!({"min":8,"max":32})),
        ("dataCenters", Value::Null),
    ] {
        let mut invalid = base.clone();
        invalid[field] = value;
        cases.push(invalid);
    }
    for invalid in cases {
        let (provider, _, task) = server(vec![
            (200, catalog(&[center("first", "HIGH")])),
            (200, stock(&[invalid, cpu("cpu5g", &[("first", "HIGH")])])),
        ]);
        assert!(matches!(place(&provider, &spec), Err(CloudError::InvalidResponse)));
        task.join().unwrap();
    }
    for rows in [
        vec![],
        vec![base.clone(), base],
        vec![cpu("other", &[("first", "HIGH")])],
    ] {
        let (provider, _, task) = server(vec![(200, catalog(&[center("first", "HIGH")])), (200, stock(&rows))]);
        assert!(matches!(place(&provider, &spec), Err(CloudError::InvalidResponse)));
        task.join().unwrap();
    }
}

#[test]
fn provider_errors_remain_errors_and_never_capacity_verdicts() {
    for answer in [
        (403, "{}".into()),
        (500, "{}".into()),
        (200, "not json".into()),
        (200, "{}".into()),
    ] {
        let (provider, _, task) = server(vec![(200, catalog(&[center("first", "HIGH")])), answer]);
        assert!(!matches!(
            place(&provider, &spec()),
            Ok(_) | Err(CloudError::Invalid(_))
        ));
        task.join().unwrap();
    }
}

#[test]
fn no_allowed_family_or_storage_skips_stock_query() {
    let mut spec = spec();
    spec.data_centers = vec!["preferred".into()];
    let mut wrong_tier = center("preferred", "HIGH");
    wrong_tier["networkVolumeTypes"] = json!(["HIGH_PERFORMANCE"]);
    for center in [center("preferred", "NONE"), center("unconfigured", "HIGH"), wrong_tier] {
        let (provider, requests, task) = server(vec![(200, catalog(&[center]))]);
        assert!(matches!(place(&provider, &spec), Err(CloudError::Invalid(_))));
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
