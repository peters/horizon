use super::super::{
    AzureAccessToken, AzureArmHttp, AzureCliCredential, AzureDeploymentState, AzureError, AzureLongRunningState,
    AzureManagementTransport, REQUEST_TIMEOUT, valid_vm_resource_id,
};
use super::{GROUP, OTHER_SUB, SUB};
use std::{
    collections::BTreeMap,
    io::Read as _,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use ureq::{
    Body, SendBody,
    http::{Request, Response},
    middleware::MiddlewareNext,
};

struct Expectation {
    method: &'static str,
    url: String,
    status: u16,
    body: String,
    request_body: Option<serde_json::Value>,
}

fn expect(
    method: &'static str,
    url: String,
    status: u16,
    body: &str,
    request_body: Option<serde_json::Value>,
) -> Expectation {
    Expectation {
        method,
        url,
        status,
        body: body.to_string(),
        request_body,
    }
}

fn http(expectations: Vec<Expectation>) -> (AzureArmHttp, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let queue = Arc::new(Mutex::new(expectations));
    let agent = ureq::Agent::config_builder()
        .middleware(move |request: Request<SendBody>, _: MiddlewareNext| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            let expectation = queue.lock().expect("queue").remove(0);
            assert_eq!(request.method(), expectation.method, "call {index}");
            assert_eq!(request.uri().to_string(), expectation.url, "call {index}");
            assert_eq!(request.headers()["Authorization"], "Bearer synthetic-token-value");
            let mut payload = String::new();
            request
                .into_body()
                .into_reader()
                .read_to_string(&mut payload)
                .expect("payload");
            match expectation.request_body {
                Some(expected) => assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&payload).expect("json"),
                    expected
                ),
                None => assert!(payload.is_empty(), "unexpected body on call {index}"),
            }
            Ok(Response::builder()
                .status(expectation.status)
                .body(Body::builder().data(expectation.body))
                .expect("response"))
        })
        .build()
        .new_agent();
    let credential = || AzureAccessToken::new("synthetic-token-value", Duration::from_secs(3_600));
    (
        AzureArmHttp::with_agent(agent, SUB, credential).expect("transport"),
        calls,
    )
}

fn group_url(name: &str) -> String {
    format!("https://management.azure.com/subscriptions/{SUB}/resourcegroups/{name}?api-version=2022-09-01")
}

fn resource_url(provider_path: &str, name: &str, api: &str) -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resourcegroups/{GROUP}/providers/{provider_path}/{name}?api-version={api}"
    )
}

fn vm_url(suffix: &str) -> String {
    resource_url(
        "Microsoft.Compute/virtualMachines",
        &format!("worker{suffix}"),
        "2024-03-01",
    )
}

fn vm_body(power: &str, name: &str, group: &str) -> String {
    format!(
        r#"{{"id":"/subscriptions/{SUB}/resourceGroups/{group}/providers/Microsoft.Compute/virtualMachines/worker","name":"{name}","location":"northeurope","tags":{{"horizon-job-id":"j"}},"properties":{{"provisioningState":"Succeeded","hardwareProfile":{{"vmSize":"Standard_D4s_v3"}},"instanceView":{{"statuses":[{{"code":"ProvisioningState/succeeded"}},{{"code":"PowerState/{power}"}}]}}}}}}"#
    )
}

#[test]
fn resource_group_reads_are_typed_bounded_and_identity_checked() {
    let present = format!(
        r#"{{"id":"/subscriptions/{SUB}/resourceGroups/{GROUP}","name":"{GROUP}","location":"northeurope","properties":{{"provisioningState":"Succeeded"}},"tags":{{"horizon-job-id":"j","numeric":1}}}}"#
    );
    let foreign = format!(
        r#"{{"id":"/subscriptions/{OTHER_SUB}/resourceGroups/{GROUP}","name":"{GROUP}","location":"northeurope"}}"#
    );
    let nameless = format!(r#"{{"name":"{GROUP}","location":"northeurope"}}"#);
    let oversized = format!(
        r#"{{"id":"/subscriptions/{SUB}/resourceGroups/{GROUP}","name":"{GROUP}","padding":"{}"}}"#,
        "x".repeat(2 * 1024 * 1024)
    );
    let (transport, calls) = http(vec![
        expect("GET", group_url(GROUP), 404, "", None),
        expect("GET", group_url(GROUP), 200, &present, None),
        expect(
            "GET",
            group_url(GROUP),
            200,
            r#"{"id":"/subscriptions/x/resourceGroups/HORIZON-WS-other","name":"HORIZON-WS-other","location":"northeurope"}"#,
            None,
        ),
        expect("GET", group_url(GROUP), 200, &foreign, None),
        expect("GET", group_url(GROUP), 200, &nameless, None),
        expect("GET", group_url(GROUP), 200, "private-malformed", None),
        expect("GET", group_url(GROUP), 200, &oversized, None),
        expect("GET", group_url(GROUP), 302, "private-redirect", None),
    ]);
    assert_eq!(transport.get_resource_group(GROUP), Ok(None));
    let info = transport.get_resource_group(GROUP).expect("group").expect("present");
    assert_eq!(
        (
            info.name.as_str(),
            info.location.as_str(),
            info.provisioning_state.as_str()
        ),
        (GROUP, "northeurope", "Succeeded")
    );
    assert_eq!(
        info.tags,
        [("horizon-job-id".to_string(), "j".to_string())].into(),
        "non-string tags are dropped"
    );
    assert_eq!(info.id, format!("/subscriptions/{SUB}/resourceGroups/{GROUP}"));
    assert_eq!(
        transport.get_resource_group(GROUP),
        Err(AzureError::ResourceIdentityMismatch),
        "renamed"
    );
    assert_eq!(
        transport.get_resource_group(GROUP),
        Err(AzureError::ResourceIdentityMismatch),
        "foreign subscription id"
    );
    assert_eq!(
        transport.get_resource_group(GROUP),
        Err(AzureError::InvalidResponse {
            operation: "resource group lookup"
        }),
        "a group without its resource id is not trusted"
    );
    let malformed = Err(AzureError::InvalidResponse {
        operation: "resource group lookup",
    });
    assert_eq!(transport.get_resource_group(GROUP), malformed);
    assert_eq!(
        transport.get_resource_group(GROUP),
        malformed,
        "oversized body is rejected"
    );
    let redirect = transport
        .get_resource_group(GROUP)
        .expect_err("redirects are not followed");
    assert_eq!(
        redirect,
        AzureError::UnexpectedStatus {
            operation: "resource group lookup",
            status: 302
        }
    );
    assert!(!format!("{redirect:?} {redirect}").contains("private-"));
    assert_eq!(calls.load(Ordering::SeqCst), 8);
    for invalid in ["", "bad/name", "name?x=1", &"g".repeat(91), "trailing."] {
        assert_eq!(
            transport.get_resource_group(invalid),
            Err(AzureError::ResourceIdentityMismatch)
        );
        assert_eq!(
            transport.delete_resource_group(invalid),
            Err(AzureError::ResourceIdentityMismatch)
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        8,
        "invalid input never reaches the network"
    );
}

#[test]
fn resource_group_writes_send_exact_bodies_and_map_long_running_states() {
    let tags: BTreeMap<String, String> = [("horizon-job-id".to_string(), "j".to_string())].into();
    let created = format!(
        r#"{{"id":"/subscriptions/{SUB}/resourceGroups/{GROUP}","name":"{GROUP}","location":"northeurope","tags":{{"horizon-job-id":"j"}}}}"#
    );
    let (transport, calls) = http(vec![
        expect(
            "PUT",
            group_url(GROUP),
            201,
            &created,
            Some(serde_json::json!({"location":"northeurope","tags":{"horizon-job-id":"j"}})),
        ),
        expect("DELETE", group_url(GROUP), 202, "", None),
        expect("DELETE", group_url(GROUP), 200, "", None),
        expect("DELETE", group_url(GROUP), 409, "private-conflict-payload", None),
    ]);
    let info = transport
        .create_resource_group(GROUP, "northeurope", &tags)
        .expect("created");
    assert_eq!((info.name.as_str(), info.tags), (GROUP, tags.clone()));
    assert_eq!(
        info.id,
        format!("/subscriptions/{SUB}/resourceGroups/{GROUP}"),
        "the returned id is retained verbatim"
    );
    assert_eq!(
        transport.delete_resource_group(GROUP),
        Ok(AzureLongRunningState::Accepted)
    );
    assert_eq!(
        transport.delete_resource_group(GROUP),
        Ok(AzureLongRunningState::Completed),
        "empty 200 body"
    );
    let conflict = transport.delete_resource_group(GROUP);
    assert_eq!(
        conflict,
        Err(AzureError::UnexpectedStatus {
            operation: "resource group deletion",
            status: 409
        })
    );
    assert!(!format!("{conflict:?}").contains("private-"));
    assert_eq!(
        transport.create_resource_group(GROUP, "North Europe", &tags),
        Err(AzureError::InvalidProfile)
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "an invalid location never reaches the network"
    );
}

#[test]
fn deployments_are_submitted_incrementally_and_read_back_with_string_outputs() {
    let url = resource_url("Microsoft.Resources/deployments", "worker", "2022-09-01");
    let template = serde_json::json!({"resources": []});
    let parameters = serde_json::json!({"vmName": {"value": "worker"}});
    let (transport, calls) = http(vec![
        expect(
            "PUT",
            url.clone(),
            201,
            r#"{"properties":{"provisioningState":"Accepted"}}"#,
            Some(
                serde_json::json!({"properties":{"mode":"Incremental","template":{"resources":[]},"parameters":{"vmName":{"value":"worker"}}}}),
            ),
        ),
        expect(
            "GET",
            url.clone(),
            200,
            r#"{"properties":{"provisioningState":"Succeeded","outputs":{"publicIp":{"type":"String","value":"203.0.113.9"},"count":{"type":"Int","value":2}}}}"#,
            None,
        ),
        expect("GET", url.clone(), 404, "", None),
        expect("GET", url, 200, r#"{"properties":{}}"#, None),
    ]);
    let accepted = transport
        .put_deployment(GROUP, "worker", &template, &parameters)
        .expect("deployment");
    assert_eq!(accepted.provisioning_state, "Accepted");
    assert!(!accepted.is_terminal());
    let done = transport
        .get_deployment(GROUP, "worker")
        .expect("lookup")
        .expect("present");
    assert_eq!(
        done,
        AzureDeploymentState {
            provisioning_state: "Succeeded".into(),
            outputs: [("publicIp".to_string(), "203.0.113.9".to_string())].into(),
        }
    );
    assert!(done.is_terminal());
    assert_eq!(transport.get_deployment(GROUP, "worker"), Ok(None));
    assert_eq!(
        transport.get_deployment(GROUP, "worker"),
        Err(AzureError::InvalidResponse {
            operation: "deployment lookup"
        })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    for (group, name) in [
        ("bad/group", "worker"),
        (GROUP, "worker/../other"),
        (GROUP, ""),
        (GROUP, "name with space"),
    ] {
        assert_eq!(
            transport.get_deployment(group, name),
            Err(AzureError::ResourceIdentityMismatch)
        );
        assert_eq!(
            transport.put_deployment(group, name, &template, &parameters),
            Err(AzureError::ResourceIdentityMismatch)
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "invalid segments never reach the network"
    );
}

#[test]
fn virtual_machine_views_verify_the_returned_identity() {
    let expanded = format!("{}&$expand=instanceView", vm_url(""));
    let (transport, calls) = http(vec![
        expect("GET", expanded.clone(), 200, &vm_body("running", "worker", GROUP), None),
        expect("GET", expanded.clone(), 200, &vm_body("running", "other", GROUP), None),
        expect(
            "GET",
            expanded.clone(),
            200,
            &vm_body("running", "worker", "horizon-ws-foreign"),
            None,
        ),
        expect("GET", expanded.clone(), 200, r#"{"name":"worker"}"#, None),
        expect("GET", expanded, 404, "", None),
    ]);
    let vm = transport.get_vm(GROUP, "worker").expect("vm").expect("present");
    assert_eq!(
        (vm.name.as_str(), vm.vm_size.as_str(), vm.location.as_str()),
        ("worker", "Standard_D4s_v3", "northeurope")
    );
    assert!(vm.id.ends_with("/virtualMachines/worker"));
    assert_eq!(
        (vm.provisioning_state.as_str(), vm.power_state.as_deref()),
        ("Succeeded", Some("PowerState/running"))
    );
    assert_eq!(vm.tags.get("horizon-job-id").map(String::as_str), Some("j"));
    assert_eq!(
        transport.get_vm(GROUP, "worker"),
        Err(AzureError::ResourceIdentityMismatch),
        "renamed VM"
    );
    assert_eq!(
        transport.get_vm(GROUP, "worker"),
        Err(AzureError::ResourceIdentityMismatch),
        "foreign group id"
    );
    assert_eq!(
        transport.get_vm(GROUP, "worker"),
        Err(AzureError::InvalidResponse {
            operation: "virtual machine lookup"
        }),
        "missing id"
    );
    assert_eq!(transport.get_vm(GROUP, "worker"), Ok(None));
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        transport.get_vm(GROUP, "worker/../other"),
        Err(AzureError::ResourceIdentityMismatch)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[test]
fn vm_resource_ids_are_matched_exactly() {
    let vm_id =
        format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker");
    assert!(valid_vm_resource_id(&vm_id, SUB, GROUP, "worker"));
    assert!(valid_vm_resource_id(
        &vm_id.replace("resourceGroups", "resourcegroups"),
        SUB,
        GROUP,
        "worker"
    ));
    assert!(valid_vm_resource_id(&vm_id, SUB, &GROUP.to_ascii_uppercase(), "worker"));
    assert!(valid_vm_resource_id(
        &vm_id.replace("/worker", "/other"),
        SUB,
        GROUP,
        "other"
    ));
    assert!(!valid_vm_resource_id(&vm_id, SUB, GROUP, "other"));
    let group_id = format!("/subscriptions/{SUB}/resourceGroups/{GROUP}");
    assert!(super::super::valid_resource_group_id(&group_id, SUB, GROUP));
    assert!(super::super::valid_resource_group_id(
        &group_id.replace("resourceGroups", "resourcegroups"),
        SUB,
        GROUP
    ));
    assert!(super::super::valid_resource_group_id(
        &group_id,
        SUB,
        &GROUP.to_ascii_uppercase()
    ));
    for bad in [
        group_id.replace(SUB, OTHER_SUB),
        group_id.replace(GROUP, "horizon-ws-other"),
        format!("{group_id}/providers/x"),
        format!("{group_id} "),
        String::new(),
    ] {
        assert!(!super::super::valid_resource_group_id(&bad, SUB, GROUP), "{bad}");
    }
    for (bad, label) in [
        (vm_id.replace("/worker", "/Worker"), "vm name casing"),
        (vm_id.replace("/worker", "/other"), "vm name"),
        (vm_id.replace(GROUP, "horizon-ws-other"), "group"),
        (vm_id.replace(SUB, OTHER_SUB), "subscription"),
        (format!("{vm_id}/extensions/x"), "trailing segments"),
        (vm_id.replace("Microsoft.Compute", "Microsoft.Storage"), "provider"),
    ] {
        assert!(!valid_vm_resource_id(&bad, SUB, GROUP, "worker"), "{label}");
    }
}
#[test]
fn credential_failures_stop_before_any_request_and_production_agent_is_pinned() {
    let agent = ureq::Agent::config_builder()
        .middleware(
            |_: Request<SendBody>, _: MiddlewareNext| -> Result<Response<Body>, ureq::Error> {
                panic!("no request may be sent without a credential")
            },
        )
        .build()
        .new_agent();
    let unavailable = || -> Result<AzureAccessToken, AzureError> {
        Err(AzureError::CredentialUnavailable {
            reason: "Azure CLI did not return a token",
        })
    };
    let transport = AzureArmHttp::with_agent(agent, SUB, unavailable).expect("transport");
    assert_eq!(
        transport.get_resource_group(GROUP),
        Err(unavailable().expect_err("error"))
    );
    assert_eq!(
        transport.delete_resource_group(GROUP),
        Err(unavailable().expect_err("error"))
    );
    let rejected = AzureArmHttp::with_agent(ureq::Agent::new_with_defaults(), "bad", unavailable);
    assert_eq!(rejected.err(), Some(AzureError::InvalidProfile));
    let production = AzureArmHttp::new(SUB, AzureCliCredential::new(SUB).expect("credential")).expect("transport");
    assert!(production.config().https_only());
    assert_eq!(production.config().max_redirects(), 0);
    assert_eq!(production.config().timeouts().global, Some(REQUEST_TIMEOUT));
    assert!(!production.config().http_status_as_error());
}
