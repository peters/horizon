use super::*;
use horizon_app_testing::contract::Form;
use serde_json::json;

fn decoded(value: Value) -> Result<Decoded> {
    let bytes = serde_json::to_vec(&value).map_err(|_| Error::ProviderRejected)?.len();
    Ok(Decoded { value, bytes })
}

fn app() -> UploadedApp {
    UploadedApp::from_response(&json!({"app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"})).unwrap()
}

fn target() -> Device {
    Device {
        platform: Platform::Ios,
        form: Form::Phone,
        model: "iPhone 15".into(),
        os_version: "27.0".into(),
    }
}

fn session(operation: Uuid) -> Session {
    Session {
        id: "b".repeat(40),
        metadata: json!({
            "hashed_id":"b".repeat(40), "name":session_name(operation), "build_name":build_name(operation),
            "os":"ios", "os_version":"27.0", "device":"iPhone 15", "browser":null,
            "browser_version":"app", "status":"running", "app_details":{"app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "video_url":"https://example.com/private-token", "public_url":"https://example.com/secret"
        }),
    }
}

#[test]
fn native_evidence_requires_exact_operation_app_and_catalog_target() {
    let operation = Uuid::new_v4();
    let valid = session(operation);
    valid.verify(&target(), &app(), operation).unwrap();
    for (pointer, value) in [
        ("/browser", json!("safari")),
        ("/browser_version", json!("27")),
        ("/os", json!("android")),
        ("/device", json!("iPad Pro")),
        ("/os_version", json!("26.0")),
        ("/name", json!("another-operation")),
        ("/build_name", json!("another-build")),
        (
            "/app_details/app_url",
            json!("bs://cccccccccccccccccccccccccccccccccccccccc"),
        ),
    ] {
        let mut changed = session(operation);
        *changed.metadata.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            changed.verify(&target(), &app(), operation),
            Err(Error::DeviceUnverified)
        );
    }
    assert_eq!(
        valid.verify(&target(), &app(), Uuid::new_v4()),
        Err(Error::DeviceUnverified)
    );
}

#[test]
fn paginated_discovery_preserves_query_and_never_returns_truncated_results() {
    let mut calls = Vec::new();
    let rows = pages(&mut Budget::new(), "/fixed?status=running", |path, _, _| {
        calls.push(path.to_owned());
        decoded(if calls.len() == 1 {
            json!(vec![json!({}); PAGE])
        } else {
            json!([])
        })
    })
    .unwrap();
    assert_eq!(rows.len(), PAGE);
    assert_eq!(
        calls,
        [
            "/fixed?status=running&limit=100&offset=0",
            "/fixed?status=running&limit=100&offset=100"
        ]
    );
    let mut count = 0;
    assert_eq!(
        pages(&mut Budget::new(), "/fixed", |_, _, _| {
            count += 1;
            decoded(json!(vec![json!({}); PAGE]))
        })
        .err(),
        Some(Error::ReconcileIncomplete)
    );
    assert_eq!(count, MAX_PAGES);
    assert_eq!(
        pages(&mut Budget::new(), "/fixed", |_, _, _| decoded(json!(vec![
            json!({});
            PAGE + 1
        ])))
        .err(),
        Some(Error::ProviderRejected)
    );
    assert_eq!(
        pages(&mut Budget::new(), "/fixed", |_, _, _| decoded(
            json!({"message":"secret"})
        ))
        .err(),
        Some(Error::ProviderRejected)
    );
}

#[test]
fn exact_upload_intent_deduplicates_repeated_rows_but_refuses_multiple_results() {
    let name = build_name(Uuid::new_v4());
    let row = json!({"custom_id":name, "app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
    assert!(owned_upload(Vec::new(), &name).unwrap().is_none());
    let found = owned_upload(vec![row.clone(), row.clone()], &name).unwrap().unwrap();
    assert_eq!(found.use_for_driver(str::len), 45);
    let other = json!({"custom_id":name, "app_url":"bs://cccccccccccccccccccccccccccccccccccccccc"});
    assert_eq!(
        owned_upload(vec![row, other], &name).err(),
        Some(Error::ReconcileIncomplete)
    );
    assert_eq!(
        owned_upload(
            vec![json!({"custom_id":"another-run", "app_url":"bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"})],
            &name
        )
        .err(),
        Some(Error::ReconcileIncomplete)
    );
}

#[test]
fn nested_scans_share_one_request_row_byte_and_elapsed_budget() {
    let mut budget = Budget::new();
    budget.requests = 3;
    let mut calls = 0;
    for _ in 0..3 {
        pages(&mut budget, "/build/sessions", |_, _, _| {
            calls += 1;
            decoded(json!([]))
        })
        .unwrap();
    }
    assert_eq!(
        pages(&mut budget, "/another/sessions", |_, _, _| {
            calls += 1;
            decoded(json!([]))
        })
        .err(),
        Some(Error::ReconcileIncomplete)
    );
    assert_eq!(calls, 3);
    let mut budget = Budget::new();
    budget.rows = 1;
    assert_eq!(
        pages(&mut budget, "/fixed", |_, _, _| decoded(json!([{}, {}]))).err(),
        Some(Error::ReconcileIncomplete)
    );
    let mut budget = Budget::new();
    budget.bytes = 2;
    assert_eq!(
        pages(&mut budget, "/fixed", |_, _, _| decoded(
            json!([{"private":"synthetic"}])
        ))
        .err(),
        Some(Error::ReconcileIncomplete)
    );
    let mut budget = Budget::new();
    budget.deadline = Instant::now();
    assert_eq!(
        pages(&mut budget, "/fixed", |_, _, _| panic!(
            "expired discovery performed HTTP"
        ))
        .err(),
        Some(Error::ReconcileIncomplete)
    );
    let mut budget = Budget::new();
    budget.deadline = Instant::now() + Duration::from_millis(10);
    assert_eq!(
        pages(&mut budget, "/fixed", |_, timeout, _| {
            assert!(timeout <= Duration::from_millis(10));
            std::thread::sleep(Duration::from_millis(20));
            decoded(json!([]))
        })
        .err(),
        Some(Error::ReconcileIncomplete)
    );
}

#[test]
fn unknown_states_and_path_like_provider_ids_fail_closed() {
    for state in ["running", "queued", "passed", "failed", "error"] {
        assert!(active(&json!({"status":state})).unwrap());
    }
    for state in ["done", "timeout"] {
        assert!(!active(&json!({"status":state})).unwrap());
    }
    assert_eq!(active(&json!({"status":"new-state"})), Err(Error::ReconcileIncomplete));
    for id in [
        "../another-session",
        "token?query=secret",
        "https://evil.invalid",
        "short",
    ] {
        assert_eq!(check_id(id), Err(Error::ProviderRejected));
    }
    check_id(&"a".repeat(40)).unwrap();
    check_id("g072e0ed204cd8b63618478fd6a37c7d7d34869").unwrap();
    check_id("22db03a61227fb42db41e395t97a9ab1d3744462").unwrap();
}

#[test]
fn whitespace_padded_http_pages_charge_decoded_bytes_and_limit_each_remaining_read() {
    let mut body = serde_json::to_vec(&vec![json!({}); PAGE]).unwrap();
    body.resize(1000 * 1024, b' ');
    let mut calls = 0;
    let mut limits = Vec::new();
    let mut budget = Budget::new();
    let result = pages(&mut budget, "/fixed", |_, _, limit| {
        calls += 1;
        limits.push(limit);
        let response = ureq::http::Response::builder()
            .status(200)
            .body(ureq::Body::builder().data(body.clone()))
            .unwrap();
        crate::api::decode_measured(response, limit)
    });
    assert!(
        result.is_err(),
        "padded discovery must not pass the 8 MiB decoded budget"
    );
    assert_eq!(calls, 9);
    assert_eq!(&limits[..8], &[1024 * 1024; 8]);
    assert_eq!(limits[8], 192 * 1024);
    assert_eq!(budget.bytes, 192 * 1024);
}
