use super::super::{
    AzureAccessToken, AzureArmHttp, AzureCliCredential, AzureDeploymentState, AzureError, AzureLongRunningState,
    AzureManagementTransport, AzureRunCommand, REQUEST_TIMEOUT, valid_vm_resource_id,
};
use super::OTHER_SUB as FOREIGN_SUB;
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
    header: Option<(&'static str, String)>,
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
        header: None,
    }
}

fn with_header(mut expectation: Expectation, name: &'static str, value: String) -> Expectation {
    expectation.header = Some((name, value));
    expectation
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
            let mut response = Response::builder().status(expectation.status);
            if let Some((name, value)) = expectation.header {
                response = response.header(name, value);
            }
            Ok(response.body(Body::builder().data(expectation.body)).expect("response"))
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
        expect("GET", url.clone(), 200, r#"{"properties":{}}"#, None),
        expect(
            "PUT",
            url.clone(),
            202,
            "",
            Some(
                serde_json::json!({"properties":{"mode":"Incremental","template":{"resources":[]},"parameters":{"vmName":{"value":"worker"}}}}),
            ),
        ),
        expect(
            "PUT",
            url.clone(),
            202,
            r#"{"status":"InProgress"}"#,
            Some(
                serde_json::json!({"properties":{"mode":"Incremental","template":{"resources":[]},"parameters":{"vmName":{"value":"worker"}}}}),
            ),
        ),
        expect(
            "PUT",
            url,
            202,
            "private-garbage",
            Some(
                serde_json::json!({"properties":{"mode":"Incremental","template":{"resources":[]},"parameters":{"vmName":{"value":"worker"}}}}),
            ),
        ),
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
    let accepted = transport
        .put_deployment(GROUP, "worker", &template, &parameters)
        .expect("asynchronous acceptance");
    assert_eq!(
        (accepted.provisioning_state.as_str(), accepted.outputs.len()),
        ("Accepted", 0),
        "a bodiless 202 is accepted"
    );
    let envelope = transport
        .put_deployment(GROUP, "worker", &template, &parameters)
        .expect("async envelope");
    assert_eq!(
        envelope.provisioning_state, "Accepted",
        "a 202 without the deployment shape is still accepted"
    );
    let garbage = transport.put_deployment(GROUP, "worker", &template, &parameters);
    assert_eq!(
        garbage,
        Err(AzureError::InvalidResponse {
            operation: "deployment submission"
        }),
        "a malformed 202 body is not accepted"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 7);
}

#[test]
fn deployment_operations_reject_invalid_segments_before_any_request() {
    let (transport, calls) = http(vec![]);
    let (template, parameters) = (serde_json::json!({}), serde_json::json!({}));
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
        0,
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

fn run_command_body() -> serde_json::Value {
    serde_json::json!({"commandId": "RunShellScript", "script": [AzureRunCommand::HostKey.script()]})
}

fn operation_url() -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/providers/Microsoft.Compute/locations/northeurope/operations/op-1?api-version=2024-03-01"
    )
}

#[test]
fn deallocate_uses_the_exact_path_and_maps_long_running_states() {
    let (transport, _) = http(vec![
        expect("POST", vm_url("/deallocate"), 202, "", None),
        expect("POST", vm_url("/deallocate"), 200, "", None),
        expect("POST", vm_url("/deallocate"), 409, "private-conflict", None),
        expect("POST", vm_url("/deallocate"), 404, "", None),
    ]);
    assert_eq!(
        transport.deallocate_vm(GROUP, "worker"),
        Ok(Some(AzureLongRunningState::Accepted))
    );
    assert_eq!(
        transport.deallocate_vm(GROUP, "worker"),
        Ok(Some(AzureLongRunningState::Completed))
    );
    let conflict = transport.deallocate_vm(GROUP, "worker");
    assert_eq!(
        conflict,
        Err(AzureError::UnexpectedStatus {
            operation: "virtual machine deallocation",
            status: 409
        })
    );
    assert!(!format!("{conflict:?}").contains("private-"));
    assert_eq!(transport.deallocate_vm(GROUP, "worker"), Ok(None), "absent VM");
    for (group, name) in [("bad/group", "worker"), (GROUP, ""), (GROUP, "name with space")] {
        assert_eq!(
            transport.deallocate_vm(group, name),
            Err(AzureError::ResourceIdentityMismatch)
        );
        assert_eq!(
            transport.run_command(group, name, AzureRunCommand::HostKey),
            Err(AzureError::ResourceIdentityMismatch)
        );
    }
}

#[test]
fn run_command_polls_only_owned_operations_and_extracts_stdout() {
    let operation = operation_url();
    let done = r#"{"status":"Succeeded","properties":{"output":{"value":[{"code":"ProvisioningState/succeeded","message":"Enable succeeded: \n[stdout]\nssh-ed25519 AAAAC3 host\n\n[stderr]\n"}]}}}"#;
    let (transport, _) = http(vec![
        with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "azure-asyncoperation",
            operation.clone(),
        ),
        expect("GET", operation.clone(), 200, r#"{"status":"InProgress"}"#, None),
        with_header(
            expect("GET", operation.clone(), 429, "throttled", None),
            "retry-after",
            "2".into(),
        ),
        expect("GET", operation.clone(), 200, done, None),
        expect(
            "POST",
            vm_url("/runCommand"),
            200,
            r#"{"value":[{"code":"ProvisioningState/succeeded","message":"[stdout]\nsync output\n[stderr]\n"}]}"#,
            Some(run_command_body()),
        ),
        expect("POST", vm_url("/runCommand"), 404, "", Some(run_command_body())),
        with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "azure-asyncoperation",
            format!("https://management.azure.com/subscriptions/{FOREIGN_SUB}/operations/op-2"),
        ),
        with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "location",
            operation.clone(),
        ),
        with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "azure-asyncoperation",
            operation.clone(),
        ),
        expect(
            "GET",
            operation,
            200,
            r#"{"status":"Failed","error":{"message":"private-detail"}}"#,
            None,
        ),
    ]);
    let run = || transport.run_command(GROUP, "worker", AzureRunCommand::HostKey);
    assert_eq!(
        run(),
        Ok(Some("ssh-ed25519 AAAAC3 host".into())),
        "a throttled poll with Retry-After is waited out, not failed"
    );
    assert_eq!(run(), Ok(Some("sync output".into())));
    assert_eq!(run(), Ok(None), "absent VM");
    let foreign = run();
    assert_eq!(
        foreign,
        Err(AzureError::InvalidResponse {
            operation: "virtual machine run command"
        }),
        "a poll URL outside this subscription is never followed"
    );
    assert_eq!(
        run(),
        Err(AzureError::InvalidResponse {
            operation: "virtual machine run command"
        }),
        "a Location-only answer uses a different protocol and is refused"
    );
    let failed = run();
    assert_eq!(
        failed,
        Err(AzureError::RequestFailed {
            operation: "virtual machine run command"
        })
    );
    assert!(!format!("{failed:?}").contains("private-"));
}

#[test]
fn run_command_refuses_operation_urls_that_could_escape_the_subscription() {
    let escapes = [
        format!("https://management.azure.com/subscriptions/{SUB}/../../subscriptions/{FOREIGN_SUB}/operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/%2e%2e/{FOREIGN_SUB}/operations/op"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations\\..\\{FOREIGN_SUB}/op"),
        format!("https://management.azure.com/subscriptions/{SUB}//operations/op"),
        format!(
            "https://management.azure.com/subscriptions/{SUB}/operations/op?api-version=2024-03-01&x=/../{FOREIGN_SUB}"
        ),
        format!("https://management.azure.com/subscriptions/{SUB}/operations/op#frag"),
        format!("https://management.azure.com/subscriptions/{SUB}/operations/op@evil.example/x"),
        format!("HTTPS://management.azure.com/subscriptions/{SUB}/operations/op"),
    ];
    let (transport, calls) = http(
        escapes
            .iter()
            .map(|poll| {
                with_header(
                    expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
                    "azure-asyncoperation",
                    poll.clone(),
                )
            })
            .collect(),
    );
    for poll in &escapes {
        assert_eq!(
            transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
            Err(AzureError::InvalidResponse {
                operation: "virtual machine run command"
            }),
            "{poll}"
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        escapes.len(),
        "no escape candidate was ever polled"
    );
}

#[test]
fn run_command_output_is_taken_only_from_a_clean_success() {
    let cases: [(&str, Result<Option<String>, AzureError>); 8] = [
        (
            r#"{"value":[{"code":"ProvisioningState/succeeded","message":"Enable succeeded: \n[stdout]\nssh-ed25519 AAAAC3 host"}]}"#,
            Err(AzureError::InvalidResponse {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"ComponentStatus/StdOut/succeeded","message":"ssh-ed25519 AAAAC3 host\n"},{"code":"ComponentStatus/StdErr/succeeded","message":""}]}"#,
            Ok(Some("ssh-ed25519 AAAAC3 host".into())),
        ),
        (
            r#"{"value":[{"code":"ProvisioningState/succeeded","message":"[stdout]\nok\n[stderr]\ncat: no such file"}]}"#,
            Err(AzureError::RequestFailed {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"ComponentStatus/StdOut/succeeded","message":"ok"},{"code":"ComponentStatus/StdErr/succeeded","message":"warning"}]}"#,
            Err(AzureError::RequestFailed {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"ComponentStatus/StdOut/succeeded","message":"ok"},{"code":"ProvisioningState/failed","message":"boom"}]}"#,
            Err(AzureError::RequestFailed {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"ProvisioningState/succeeded","message":"[stdout]\none\n[stderr]\n"},{"code":"ComponentStatus/StdOut/succeeded","message":"two"}]}"#,
            Err(AzureError::InvalidResponse {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"ComponentStatus/StdErr/succeeded","message":""}]}"#,
            Err(AzureError::InvalidResponse {
                operation: "virtual machine run command",
            }),
        ),
        (
            r#"{"value":[{"code":"Something/Else/succeeded","message":"ok"}]}"#,
            Err(AzureError::InvalidResponse {
                operation: "virtual machine run command",
            }),
        ),
    ];
    let (transport, _) = http(
        cases
            .iter()
            .map(|(body, _)| expect("POST", vm_url("/runCommand"), 200, body, Some(run_command_body())))
            .collect(),
    );
    for (body, expected) in &cases {
        assert_eq!(
            &transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
            expected,
            "{body}"
        );
    }
}

#[test]
fn a_slow_credential_refresh_consumes_the_poll_budget_before_any_request() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let agent = ureq::Agent::config_builder()
        .middleware(move |_: Request<SendBody>, _: MiddlewareNext| {
            count.fetch_add(1, Ordering::SeqCst);
            panic!("no request may be issued once the budget is spent");
        })
        .build()
        .new_agent();
    let slow = || {
        std::thread::sleep(Duration::from_millis(30));
        AzureAccessToken::new("synthetic-token-value", Duration::from_secs(3_600))
    };
    let transport = AzureArmHttp::with_agent(agent, SUB, slow).expect("transport");
    assert_eq!(
        transport.get_vm_within(GROUP, "worker", Duration::from_millis(10)),
        Err(AzureError::OperationTimedOut {
            operation: "virtual machine lookup"
        })
    );
    assert_eq!(
        transport.get_resource_group_within(GROUP, Duration::from_millis(10)),
        Err(AzureError::OperationTimedOut {
            operation: "resource group lookup"
        })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn run_command_polling_is_bounded() {
    let operation = operation_url();
    let (transport, calls) = http(
        std::iter::once(with_header(
            expect("POST", vm_url("/runCommand"), 202, "", Some(run_command_body())),
            "azure-asyncoperation",
            operation.clone(),
        ))
        .chain((0..9).map(|_| expect("GET", operation.clone(), 200, r#"{"status":"InProgress"}"#, None)))
        .collect(),
    );
    assert_eq!(
        transport.run_command(GROUP, "worker", AzureRunCommand::HostKey),
        Err(AzureError::OperationTimedOut {
            operation: "virtual machine run command"
        })
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        10,
        "one submission plus nine bounded polls"
    );
}
