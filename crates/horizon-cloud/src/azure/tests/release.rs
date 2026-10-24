use super::provision::{absent, worker_body, worker_url};
use super::*;
use crate::azure::release::{keeps_workspace, release};

const OTHER_INSTANCE: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";

fn vm_delete_url() -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker?api-version=2024-03-01"
    )
}

fn disk_url(name: &str) -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/{name}?api-version=2023-10-02"
    )
}

/// The workspace disk, attached to `holder` or to nothing.
fn workspace(holder: Option<&str>, operation: &str) -> String {
    let mut body = json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-workspace"),
        "name": "worker-workspace", "location": "northeurope", "tags": { OPERATION_TAG: operation },
        "properties": { "diskSizeGB": 64, "diskState": if holder.is_some() { "Attached" } else { "Unattached" } },
    });
    if let Some(holder) = holder {
        body["managedBy"] = json!(holder);
    }
    body.to_string()
}

fn vm_with(edit: impl FnOnce(&mut Value)) -> String {
    let mut body = worker_body(VM_INSTANCE);
    edit(&mut body);
    body.to_string()
}

/// The reads that prove the release: the VM gone, the OS disk gone and the workspace disk free.
fn proven() -> Vec<Exchange> {
    vec![
        get(worker_url(), 404, absent()),
        get(disk_url("worker-os"), 404, absent()),
        get(disk_url("worker-workspace"), 200, workspace(None, OPERATION_ID)),
    ]
}

/// Releases the VM, checking that each delete request was announced just before it
/// was sent and that nothing else was.
fn released(client: &Scripted) -> Result<(), CloudError> {
    let announced = std::cell::RefCell::new(Vec::new());
    let result = release(
        &client.azure,
        OPERATION_ID,
        VM_INSTANCE,
        &Cancellation::default(),
        &mut || {
            announced.borrow_mut().push(client.sent().len());
            Ok(())
        },
    );
    let deletes: Vec<usize> = client
        .sent()
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("DELETE ").then_some(index))
        .collect();
    assert_eq!(
        *announced.borrow(),
        deletes,
        "a release is announced only before its delete"
    );
    result
}

fn waited(client: &Scripted) -> Duration {
    client.sleeps().into_iter().sum()
}

fn writes(client: &Scripted) -> Vec<String> {
    client
        .sent()
        .into_iter()
        .filter(|line| !line.starts_with("GET "))
        .collect()
}

#[test]
fn a_release_deletes_only_the_vm_and_proves_the_workspace_disk_kept() {
    let mut script = vec![
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        bodiless("DELETE", vm_delete_url(), 202, "").header("azure-asyncoperation", operation_url("release")),
        get(
            operation_url("release"),
            200,
            json!({"status": "InProgress"}).to_string(),
        ),
        get(
            operation_url("release"),
            200,
            json!({"status": "Succeeded"}).to_string(),
        ),
    ];
    script.extend(proven());
    let client = scripted(script);
    released(&client).unwrap();
    client.done();
    assert_eq!(
        writes(&client),
        [format!("DELETE {}", vm_delete_url())],
        "only the VM is deleted: no disk, address, network or group"
    );
}

#[test]
fn a_release_whose_answer_was_lost_reads_the_vm_and_sends_no_second_delete() {
    let deleting = vm_with(|body| body["properties"]["provisioningState"] = json!("Deleting"));
    let mut script = vec![get(worker_url(), 200, deleting)];
    script.extend(proven());
    let client = scripted(script);
    released(&client).unwrap();
    client.done();
    assert!(writes(&client).is_empty(), "a VM ARM is deleting is only read");
    assert_eq!(waited(&client), Duration::from_millis(BACKOFF_MS[0]));
}

#[test]
fn a_released_vm_is_proven_gone_again_without_a_request() {
    let client = scripted(proven());
    released(&client).unwrap();
    client.done();
    assert!(writes(&client).is_empty());
}

#[test]
fn the_release_waits_for_the_os_disk_and_the_detach() {
    let client = scripted(vec![
        get(worker_url(), 404, absent()),
        get(disk_url("worker-os"), 200, json!({
            "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-os"),
            "name": "worker-os", "tags": {}
        }).to_string()),
        get(worker_url(), 404, absent()),
        get(disk_url("worker-os"), 404, absent()),
        get(
            disk_url("worker-workspace"),
            200,
            workspace(Some(&format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker")), OPERATION_ID),
        ),
        get(worker_url(), 404, absent()),
        get(disk_url("worker-os"), 404, absent()),
        get(disk_url("worker-workspace"), 200, workspace(None, OPERATION_ID)),
    ]);
    released(&client).unwrap();
    client.done();
    assert_eq!(waited(&client), Duration::from_millis(BACKOFF_MS[0] + BACKOFF_MS[1]));
    assert!(writes(&client).is_empty());
}

#[test]
fn a_vm_that_would_delete_its_workspace_disk_is_never_released() {
    for edit in [
        |body: &mut Value| body["properties"]["storageProfile"]["dataDisks"][0]["deleteOption"] = json!("Delete"),
        |body: &mut Value| body["properties"]["storageProfile"]["dataDisks"] = json!([]),
    ] {
        let client = scripted(vec![get(worker_url(), 200, vm_with(edit))]);
        let refusal = released(&client).unwrap_err().to_string();
        assert!(refusal.contains("would delete its workspace disk"), "{refusal}");
        assert!(writes(&client).is_empty(), "nothing is deleted");
    }
}

#[test]
fn another_instance_or_owner_is_never_deleted() {
    let client = scripted(vec![get(worker_url(), 200, worker_body(OTHER_INSTANCE).to_string())]);
    assert!(matches!(released(&client), Err(CloudError::IdentityMismatch)));
    let foreign = vm_with(|body| body["tags"] = json!({ OPERATION_TAG: "op-2" }));
    let client = scripted(vec![get(worker_url(), 200, foreign)]);
    assert!(matches!(released(&client), Err(CloudError::IdentityMismatch)));
    assert!(writes(&client).is_empty());
    let mut script = proven();
    script[2] = get(disk_url("worker-workspace"), 200, workspace(None, "op-2"));
    let client = scripted(script);
    assert!(matches!(released(&client), Err(CloudError::IdentityMismatch)));
}

#[test]
fn a_missing_workspace_disk_is_refused_rather_than_made_again_empty() {
    let mut script = proven();
    script[2] = get(disk_url("worker-workspace"), 404, absent());
    let client = scripted(script);
    let refusal = released(&client).unwrap_err().to_string();
    assert!(refusal.contains("workspace disk is missing"), "{refusal}");
}

#[test]
fn a_release_that_cannot_be_recorded_sends_no_delete() {
    let client = scripted(vec![get(worker_url(), 200, worker_body(VM_INSTANCE).to_string())]);
    let result = release(
        &client.azure,
        OPERATION_ID,
        VM_INSTANCE,
        &Cancellation::default(),
        &mut || Err(CloudError::Persistence),
    );
    assert!(matches!(result, Err(CloudError::Persistence)));
    assert!(writes(&client).is_empty());
}

#[test]
fn a_bad_operation_id_sends_nothing() {
    let client = scripted(Vec::new());
    assert!(matches!(
        release(
            &client.azure,
            "bad id",
            VM_INSTANCE,
            &Cancellation::default(),
            &mut || Ok(())
        ),
        Err(CloudError::Invalid(_))
    ));
    assert!(client.sent().is_empty());
}

#[test]
fn only_a_detached_workspace_disk_is_kept() {
    let group_id = GROUP;
    let vm = |option: &str| {
        let mut body = worker_body(VM_INSTANCE);
        body["properties"]["storageProfile"]["dataDisks"][0]["deleteOption"] = json!(option);
        resource::vm_view(&body, SUB, group_id, WORKER_VM_NAME).unwrap()
    };
    assert!(keeps_workspace(&vm("Detach"), SUB, GROUP));
    assert!(keeps_workspace(&vm("detach"), SUB, GROUP));
    assert!(!keeps_workspace(&vm("Delete"), SUB, GROUP));
    assert!(
        !keeps_workspace(&vm(""), SUB, GROUP),
        "an option ARM did not report is not trusted"
    );
}

#[test]
fn a_disk_of_another_name_is_an_identity_mismatch() {
    let client =
        scripted(vec![get(disk_url("worker-workspace"), 200, json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-os"),
        "name": "worker-os"
    }).to_string())]);
    assert!(matches!(
        client
            .azure
            .get_disk(GROUP, "worker-workspace", &Cancellation::default()),
        Err(Failure::Local(CloudError::IdentityMismatch))
    ));
}

#[test]
fn a_delete_of_a_vm_that_is_gone_is_complete() {
    let client = scripted(vec![bodiless("DELETE", vm_delete_url(), 404, absent())]);
    assert!(matches!(
        client.azure.delete_vm(GROUP, WORKER_VM_NAME, &Cancellation::default()),
        Ok(LongRunning::Completed)
    ));
}
