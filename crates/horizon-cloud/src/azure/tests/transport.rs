use super::*;
use crate::azure::url::{
    resource_id_parts, valid_location, valid_resource_group_id, valid_resource_name, valid_vm_resource_id,
};

#[test]
fn a_production_agent_pins_https_no_redirect_and_the_request_timeout() {
    let azure = Azure::new(Credential::new(TOKEN.into()).unwrap(), SUB).unwrap();
    let config = azure.config();
    assert!(config.https_only());
    assert_eq!(config.max_redirects(), 0);
    assert_eq!(config.timeouts().global, Some(REQUEST_TIMEOUT));
    assert!(!config.http_status_as_error());
    assert_eq!(azure.endpoint, MANAGEMENT_ENDPOINT);
    assert_eq!(azure.subscription_id(), SUB);
    let credential = || Credential::new(TOKEN.into()).unwrap();
    assert!(matches!(
        Azure::loopback(credential(), SUB, "192.0.2.1:443".parse().unwrap()),
        Err(CloudError::Invalid(_))
    ));
    let local = Azure::loopback(credential(), SUB, "127.0.0.1:8080".parse().unwrap()).unwrap();
    assert_eq!(local.endpoint, "http://127.0.0.1:8080");
    assert!(local.owns_operation_url(&format!("http://127.0.0.1:8080/subscriptions/{SUB}/operations/op")));
    assert!(!local.owns_operation_url(&format!("{MANAGEMENT_ENDPOINT}/subscriptions/{SUB}/operations/op")));
}

#[test]
fn a_malformed_subscription_id_is_refused_before_any_request() {
    for malformed in [
        "",
        "not-a-uuid",
        "00000000-0000-4000-8000-00000000000",
        "00000000-0000-4000-8000-0000000000011",
        "00000000-0000-4000-8000-00000000000A",
        "00000000000040008000000000000001aaaa",
        "00000000-0000-4000-8000-00000000000/",
        "../00000-0000-4000-8000-000000000001",
    ] {
        let client = Azure::new(Credential::new(TOKEN.into()).unwrap(), malformed);
        assert!(
            matches!(client, Err(CloudError::Invalid("Invalid Azure subscription id"))),
            "{malformed:?}"
        );
    }
}

#[test]
fn a_bad_resource_group_or_name_is_refused_before_any_request() {
    let client = scripted(Vec::new());
    let cancel = Cancellation::default();
    let long_group = "g".repeat(91);
    for group in [
        "",
        "bad/group",
        "group?x=1",
        "trailing.",
        "space group",
        long_group.as_str(),
    ] {
        let failure = client.azure.get_resource_group(group, &cancel).unwrap_err();
        assert!(failure.definite(), "{group:?}");
        assert!(matches!(
            failure.into(),
            CloudError::Invalid("Invalid Azure resource group name")
        ));
        assert!(matches!(
            client.azure.get_vm(group, WORKER_VM_NAME, &cancel).unwrap_err().into(),
            CloudError::Invalid("Invalid Azure resource group name")
        ));
    }
    let long_name = "n".repeat(129);
    for name in ["", "a/b", "a.b", "name?api-version=1", "a%2fb", long_name.as_str()] {
        assert!(!valid_resource_name(name), "{name:?}");
        assert!(matches!(
            client.azure.get_vm(GROUP, name, &cancel).unwrap_err().into(),
            CloudError::Invalid("Invalid Azure resource name")
        ));
        assert!(matches!(
            client.azure.get_deployment(GROUP, name, &cancel).unwrap_err().into(),
            CloudError::Invalid("Invalid Azure resource name")
        ));
        let failure = client
            .azure
            .put_deployment(GROUP, name, &json!({}), &json!({}), &cancel)
            .unwrap_err();
        assert!(failure.definite());
    }
    assert!(valid_location("northeurope") && valid_location("westus2"));
    for location in ["", "n", "North Europe", "2west", "west-us"] {
        assert!(!valid_location(location), "{location:?}");
    }
    assert_eq!(client.sent(), Vec::<String>::new());
}

#[test]
fn resource_group_reads_are_typed_bounded_and_identity_checked() {
    let renamed = group_body("horizon-ws-other", Some(OPERATION_ID));
    let foreign = group_body(GROUP, None).replace(SUB, OTHER_SUB);
    let nameless = json!({ "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}") }).to_string();
    let client = scripted(vec![
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "not found")),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(
            group_url(GROUP),
            200,
            group_body(&GROUP.to_uppercase(), Some(OPERATION_ID)),
        ),
        get(group_url(GROUP), 200, renamed),
        get(group_url(GROUP), 200, foreign),
        get(group_url(GROUP), 200, nameless),
        get(group_url(GROUP), 200, "private-malformed"),
        get(group_url(GROUP), 302, "private-redirect").header("location", "https://example.com/"),
    ]);
    let cancel = Cancellation::default();
    let read = || {
        client
            .azure
            .get_resource_group(GROUP, &cancel)
            .map_err(CloudError::from)
    };
    assert!(matches!(read(), Ok(None)));
    let group = read().unwrap().unwrap();
    assert_eq!(group.id, format!("/subscriptions/{SUB}/resourceGroups/{GROUP}"));
    assert_eq!(
        (
            group.name.as_str(),
            group.location.as_str(),
            group.provisioning_state.as_str()
        ),
        (GROUP, "northeurope", "Succeeded")
    );
    assert_eq!(
        group.tags,
        [(OPERATION_TAG.to_string(), OPERATION_ID.to_string())].into(),
        "a tag that is not a string is dropped"
    );
    assert!(read().unwrap().is_some(), "ARM compares group names without case");
    assert!(matches!(read(), Err(CloudError::IdentityMismatch)), "another group");
    assert!(
        matches!(read(), Err(CloudError::IdentityMismatch)),
        "another subscription"
    );
    assert!(matches!(read(), Err(CloudError::InvalidResponse)), "no name");
    assert!(matches!(read(), Err(CloudError::InvalidResponse)), "not JSON");
    let redirect = read().unwrap_err();
    assert!(
        matches!(redirect, CloudError::Http(302, _)),
        "a redirect is not followed"
    );
    assert!(!format!("{redirect:?} {redirect}").contains("private-"));
    client.done();
}

#[test]
fn a_resource_group_body_over_the_limit_is_an_invalid_response() {
    let padded = |padding: usize| {
        let mut body: Value = serde_json::from_str(&group_body(GROUP, Some(OPERATION_ID))).unwrap();
        body["padding"] = Value::String("x".repeat(padding));
        body.to_string()
    };
    let base = padded(0).len();
    let limit = usize::try_from(RESPONSE_LIMIT).unwrap();
    let client = scripted(vec![
        get(group_url(GROUP), 200, padded(limit - base - 1)),
        get(group_url(GROUP), 200, padded(limit - base)),
    ]);
    let cancel = Cancellation::default();
    assert!(
        client.azure.get_resource_group(GROUP, &cancel).unwrap().is_some(),
        "one byte under the limit"
    );
    let failure = client.azure.get_resource_group(GROUP, &cancel).unwrap_err();
    assert!(!failure.definite());
    assert!(matches!(failure.into(), CloudError::InvalidResponse));
    client.done();
}

#[test]
fn virtual_machine_views_carry_identity_instance_power_and_disks() {
    let url = vm_url(GROUP, WORKER_VM_NAME);
    let client = scripted(vec![
        get(url.clone(), 200, vm_body(WORKER_VM_NAME, json!(VM_INSTANCE))),
        get(url.clone(), 200, vm_body(WORKER_VM_NAME, Value::Null)),
        get(
            url.clone(),
            200,
            vm_body(WORKER_VM_NAME, json!(VM_INSTANCE.to_uppercase())),
        ),
        get(url.clone(), 200, vm_body(WORKER_VM_NAME, json!(7))),
        get(url.clone(), 200, vm_body("other", json!(VM_INSTANCE))),
        get(url, 404, ""),
    ]);
    let cancel = Cancellation::default();
    let read = || {
        client
            .azure
            .get_vm(GROUP, WORKER_VM_NAME, &cancel)
            .map_err(CloudError::from)
    };
    let vm = read().unwrap().unwrap();
    assert_eq!(
        vm.id,
        format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker")
    );
    assert_eq!(vm.instance_id.as_deref(), Some(VM_INSTANCE));
    assert_eq!(vm.name, WORKER_VM_NAME);
    assert_eq!(
        (vm.location.as_str(), vm.vm_size.as_str()),
        ("northeurope", "Standard_D4s_v5")
    );
    assert_eq!(vm.provisioning_state, "Succeeded");
    assert_eq!(vm.power_state.as_deref(), Some("PowerState/running"));
    assert_eq!(vm.tags[OPERATION_TAG], OPERATION_ID);
    assert_eq!(
        vm.data_disks,
        [DataDisk {
            id: format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-data"),
            lun: Some(0),
            size_gb: None,
            delete_option: "Detach".into(),
        }]
    );
    assert_eq!(vm.data_disk_count, 2, "a disk without an id is counted, not named");
    assert_eq!(read().unwrap().unwrap().instance_id, None, "no vmId yet");
    assert!(matches!(read(), Err(CloudError::IdentityMismatch)), "an uppercase vmId");
    assert!(
        matches!(read(), Err(CloudError::IdentityMismatch)),
        "a vmId that is not a string"
    );
    assert!(matches!(read(), Err(CloudError::IdentityMismatch)), "another VM name");
    assert!(matches!(read(), Ok(None)));
    client.done();
}

#[test]
fn vm_resource_ids_are_matched_exactly() {
    let id = |sub: &str, group: &str, provider: &str, kind: &str, name: &str| {
        format!("/subscriptions/{sub}/resourceGroups/{group}/providers/{provider}/{kind}/{name}")
    };
    let exact = id(SUB, GROUP, "Microsoft.Compute", "virtualMachines", "worker");
    assert!(valid_vm_resource_id(&exact, SUB, GROUP, "worker"));
    assert!(valid_vm_resource_id(
        &id(
            SUB,
            &GROUP.to_uppercase(),
            "microsoft.compute",
            "VIRTUALMACHINES",
            "worker"
        ),
        SUB,
        GROUP,
        "worker"
    ));
    assert!(valid_vm_resource_id(
        &exact.replace("resourceGroups", "resourcegroups"),
        SUB,
        GROUP,
        "worker"
    ));
    for wrong in [
        id(OTHER_SUB, GROUP, "Microsoft.Compute", "virtualMachines", "worker"),
        id(
            SUB,
            "horizon-ws-other",
            "Microsoft.Compute",
            "virtualMachines",
            "worker",
        ),
        id(SUB, GROUP, "Microsoft.Network", "virtualMachines", "worker"),
        id(SUB, GROUP, "Microsoft.Compute", "disks", "worker"),
        id(SUB, GROUP, "Microsoft.Compute", "virtualMachines", "Worker"),
        id(SUB, GROUP, "Microsoft.Compute", "virtualMachines", "worker2"),
        format!("{exact}/extensions/x"),
        format!("{exact} "),
        exact.trim_start_matches('/').to_string(),
        format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines"),
        format!("{exact}{}", "x".repeat(512)),
    ] {
        assert!(!valid_vm_resource_id(&wrong, SUB, GROUP, "worker"), "{wrong}");
    }
    let parts = resource_id_parts(&exact, SUB).unwrap();
    assert_eq!(
        (parts.group, parts.provider, parts.kind, parts.name),
        (GROUP, "Microsoft.Compute", "virtualMachines", "worker")
    );
    let group_id = format!("/subscriptions/{SUB}/resourceGroups/{GROUP}");
    assert!(valid_resource_group_id(&group_id, SUB, GROUP));
    assert!(!valid_resource_group_id(&format!("{group_id}/"), SUB, GROUP));
    assert!(!valid_resource_group_id(&group_id.replace(SUB, OTHER_SUB), SUB, GROUP));
}

#[test]
fn conflicts_are_uncertain_and_throttles_are_definite() {
    let url = deployment_url("worker-deployment");
    let client = scripted(
        [
            (409, arm_error("Conflict", "conflict")),
            (409, arm_error("AnotherOperationInProgress", "busy")),
            (409, arm_error("ResourceGroupBeingDeleted", "deleting")),
            (409, arm_error("ScopeLocked", "locked")),
            (409, String::new()),
            (429, arm_error("TooManyRequests", "slow down")),
            (500, arm_error("InternalServerError", "boom")),
            (503, "<html>busy</html>".to_string()),
        ]
        .into_iter()
        .map(|(status, body)| put(url.clone(), deployment_request(), status, body).header("retry-after", "17"))
        .collect(),
    );
    let cancel = Cancellation::default();
    for code in [
        "Conflict",
        "AnotherOperationInProgress",
        "ResourceGroupBeingDeleted",
        "ScopeLocked",
        "",
    ] {
        let failure = submit(&client, &cancel).unwrap_err();
        assert!(
            failure.conflict() && !failure.definite() && !failure.capacity(),
            "{code:?}"
        );
        assert!(matches!(failure, Failure::Provider { code: ref seen, .. } if seen == code));
        assert!(matches!(failure.into(), CloudError::Http(409, _)), "{code:?}");
    }
    let throttled = submit(&client, &cancel).unwrap_err();
    assert!(throttled.definite() && !throttled.conflict());
    assert!(matches!(
        throttled,
        Failure::Provider { retry_after: Some(wait), .. } if wait == Duration::from_secs(17)
    ));
    let throttled = CloudError::from(throttled);
    assert_eq!(throttled.to_string(), "Provider returned HTTP 429: slow down");
    for _ in 0..2 {
        let failure = submit(&client, &cancel).unwrap_err();
        assert!(!failure.definite());
        assert!(matches!(failure.into(), CloudError::Transport));
    }
    assert_eq!(client.sent().len(), 8, "no request was repeated");
    client.done();
}

#[test]
fn arm_authentication_failures_are_not_rejections() {
    let client = scripted(vec![
        get(
            group_url(GROUP),
            401,
            arm_error("InvalidAuthenticationToken", "bad token"),
        ),
        get(group_url(GROUP), 401, ""),
        get(group_url(GROUP), 403, arm_error("AuthenticationFailed", "no")),
        get(group_url(GROUP), 403, arm_error("AuthorizationFailed", "no role")),
        get(
            group_url(GROUP),
            403,
            arm_error("ExpiredAuthenticationToken", "expired"),
        ),
        get(
            group_url(GROUP),
            403,
            arm_error("RequestDisallowedByPolicy", "policy forbids it"),
        ),
        get(
            group_url(GROUP),
            400,
            arm_error("InvalidApiVersionParameter", "bad version"),
        ),
    ]);
    let cancel = Cancellation::default();
    let read = || client.azure.get_resource_group(GROUP, &cancel).unwrap_err();
    for _ in 0..5 {
        let failure = read();
        assert!(failure.definite());
        assert!(matches!(failure.into(), CloudError::Unauthorized));
    }
    let policy = CloudError::from(read());
    assert_eq!(
        policy.to_string(),
        "Provider rejected the request or capacity is unavailable: policy forbids it"
    );
    assert!(matches!(policy, CloudError::Rejected(_)));
    assert!(matches!(read().into(), CloudError::Rejected(_)));
    client.done();
}

#[test]
fn capacity_codes_are_capacity_and_nothing_was_created() {
    let url = deployment_url("worker-deployment");
    let nested = json!({ "error": { "code": "InvalidTemplateDeployment", "message": "preflight failed",
        "details": [{ "code": "SkuNotAvailable", "message": "size" }] } })
    .to_string();
    let mut script: Vec<Exchange> = [
        (400, "SkuNotAvailable"),
        (409, "QuotaExceeded"),
        (409, "ZonalAllocationFailed"),
        (409, "AllocationFailed"),
    ]
    .into_iter()
    .map(|(status, code)| {
        put(
            url.clone(),
            deployment_request(),
            status,
            arm_error(code, "no capacity"),
        )
    })
    .collect();
    script.push(put(url.clone(), deployment_request(), 400, nested));
    script.push(put(
        url,
        deployment_request(),
        500,
        arm_error("AllocationFailed", "server side"),
    ));
    let client = scripted(script);
    let cancel = Cancellation::default();
    for _ in 0..5 {
        let failure = submit(&client, &cancel).unwrap_err();
        assert!(failure.capacity() && failure.definite() && !failure.conflict());
        assert!(matches!(failure.into(), CloudError::Capacity(_)));
    }
    let server = submit(&client, &cancel).unwrap_err();
    assert!(
        !server.capacity() && !server.definite(),
        "a 5xx is never a capacity proof"
    );
    client.done();
}

#[test]
fn a_409_with_a_nested_capacity_code_is_still_a_conflict() {
    let nested = json!({ "error": { "code": "AnotherOperationInProgress",
        "message": "another operation holds the resource",
        "details": [{ "code": "AllocationFailed", "message": "an earlier operation" }] } })
    .to_string();
    let client = scripted(vec![get(group_url(GROUP), 409, nested)]);
    let cancel = Cancellation::default();
    let failure = client.azure.get_resource_group(GROUP, &cancel).unwrap_err();
    assert!(!failure.capacity(), "a nested code cannot prove that ARM made nothing");
    assert!(failure.conflict() && !failure.definite());
    assert!(matches!(failure.into(), CloudError::Http(409, _)));
    client.done();
}

#[test]
fn failure_bodies_are_bounded_and_never_echo_the_token() {
    let limit = usize::try_from(FAILURE_BODY_LIMIT).unwrap();
    let sized = |size: usize| {
        let base = arm_error("BadRequest", "").len();
        arm_error("BadRequest", &"m".repeat(size - base))
    };
    let client = scripted(vec![
        get(
            group_url(GROUP),
            400,
            arm_error("BadRequest", &format!("token {TOKEN} was sent")),
        ),
        get(
            group_url(GROUP),
            400,
            arm_error("BadRequest", "t-o-k-e-n synthetic-arm-to ken"),
        ),
        get(group_url(GROUP), 400, sized(limit)),
        get(group_url(GROUP), 400, sized(limit + 1)),
        get(
            group_url(GROUP),
            400,
            json!({ "code": "BadRequest", "message": "flat body" }).to_string(),
        ),
    ]);
    let cancel = Cancellation::default();
    let read = || CloudError::from(client.azure.get_resource_group(GROUP, &cancel).unwrap_err());
    let plain = "Provider rejected the request or capacity is unavailable";
    for _ in 0..2 {
        let echoed = read();
        assert_eq!(echoed.to_string(), plain);
        assert!(!format!("{echoed:?}").contains("synthetic-arm"));
    }
    assert!(
        read().to_string().starts_with(&format!("{plain}: mmm")),
        "a body at the limit"
    );
    assert_eq!(read().to_string(), plain, "a body over the limit gives no reason");
    assert_eq!(read().to_string(), format!("{plain}: flat body"));
    client.done();
}

#[test]
fn deployments_are_submitted_incrementally_and_return_their_operation() {
    let operation = operation_url("deploy-1");
    let url = deployment_url("worker-deployment");
    let client = scripted(vec![
        accepted(&operation, Some("5")),
        put(url.clone(), deployment_request(), 202, "").header("azure-asyncoperation", operation.clone()),
        put(
            url.clone(),
            deployment_request(),
            200,
            deployment_body("worker-deployment", "Succeeded"),
        ),
        put(
            url.clone(),
            deployment_request(),
            201,
            deployment_body("worker-deployment", "Accepted"),
        ),
        put(
            url.clone(),
            deployment_request(),
            201,
            deployment_body("other-deployment", "Accepted"),
        )
        .header("azure-asyncoperation", operation.clone()),
        get(url.clone(), 200, deployment_body("worker-deployment", "Running")),
        get(url, 404, ""),
    ]);
    let cancel = Cancellation::default();
    let deployed = submit(&client, &cancel).unwrap();
    assert_eq!(deployed.state.provisioning_state, "Accepted");
    assert_eq!(deployed.state.outputs["address"], "192.0.2.10");
    let LongRunning::Accepted(reference) = deployed.operation else {
        panic!("an accepted deployment has an operation");
    };
    assert_eq!(reference.url(), operation, "the URL is kept exactly as ARM sent it");
    assert_eq!(reference.retry_after(), Some(Duration::from_secs(5)));
    let bodiless = submit(&client, &cancel).unwrap();
    assert_eq!(bodiless.state.provisioning_state, "Accepted");
    assert!(matches!(bodiless.operation, LongRunning::Accepted(_)));
    let finished = submit(&client, &cancel).unwrap();
    assert_eq!(finished.operation, LongRunning::Completed);
    let untracked = submit(&client, &cancel).unwrap_err();
    assert!(
        !untracked.definite(),
        "a running deployment without an operation is unknown"
    );
    assert!(matches!(untracked.into(), CloudError::InvalidResponse));
    assert!(matches!(
        submit(&client, &cancel).unwrap_err().into(),
        CloudError::IdentityMismatch
    ));
    let state = client
        .azure
        .get_deployment(GROUP, "worker-deployment", &cancel)
        .unwrap()
        .unwrap();
    assert!(!state.is_terminal());
    assert!(
        client
            .azure
            .get_deployment(GROUP, "worker-deployment", &cancel)
            .unwrap()
            .is_none()
    );
    client.done();
}

#[test]
fn a_cancelled_call_sends_nothing() {
    let client = scripted(Vec::new());
    let cancel = Cancellation::default();
    cancel.cancel();
    let failure = submit(&client, &cancel).unwrap_err();
    assert!(failure.definite());
    assert!(matches!(failure.into(), CloudError::Cancelled));
    assert!(matches!(
        client.azure.get_resource_group(GROUP, &cancel).unwrap_err().into(),
        CloudError::Cancelled
    ));
    assert!(matches!(
        client.azure.get_vm(GROUP, WORKER_VM_NAME, &cancel).unwrap_err().into(),
        CloudError::Cancelled
    ));
    assert!(matches!(
        client
            .azure
            .get_deployment(GROUP, "worker-deployment", &cancel)
            .unwrap_err()
            .into(),
        CloudError::Cancelled
    ));
    assert!(matches!(
        client.azure.reconcile(OPERATION_ID, &cancel),
        Err(CloudError::Cancelled)
    ));
    let reference = crate::azure::operation::operation_ref(
        &[("azure-asyncoperation", operation_url("deploy-1"))]
            .into_iter()
            .map(|(name, value)| (ureq::http::HeaderName::from_static(name), value.parse().unwrap()))
            .collect(),
    )
    .unwrap();
    assert!(matches!(
        client.azure.await_operation(&reference, &cancel),
        Err(CloudError::Cancelled)
    ));
    assert_eq!(client.sent(), Vec::<String>::new());
}
