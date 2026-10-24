use super::*;

#[test]
fn reconcile_proves_absence_with_one_read() {
    assert_eq!(resource_group_name(OPERATION_ID), GROUP);
    assert_eq!(GROUP, format!("{RESOURCE_GROUP_PREFIX}{OPERATION_ID}"));
    let client = scripted(vec![get(
        group_url(GROUP),
        404,
        arm_error("ResourceGroupNotFound", "not found"),
    )]);
    let cancel = Cancellation::default();
    assert_eq!(client.azure.reconcile(OPERATION_ID, &cancel).unwrap(), Found::Absent);
    assert_eq!(
        client.sent(),
        [format!("GET {}", group_url(GROUP))],
        "no VM read after a group 404"
    );
    for invalid in ["", "op/1", "op 1", &"o".repeat(101)] {
        assert!(matches!(
            client.azure.reconcile(invalid, &cancel),
            Err(CloudError::Invalid(_))
        ));
    }
    assert!(matches!(
        client.azure.reconcile(&"o".repeat(80), &cancel),
        Err(CloudError::Invalid("Invalid Azure resource group name"))
    ));
    assert_eq!(
        client.sent().len(),
        1,
        "an operation id that cannot name a group sends nothing"
    );
    client.done();
}

#[test]
fn reconcile_adopts_a_matching_group_and_vm() {
    let client = scripted(vec![
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(
            vm_url(GROUP, WORKER_VM_NAME),
            404,
            arm_error("ResourceNotFound", "no VM"),
        ),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(
            vm_url(GROUP, WORKER_VM_NAME),
            200,
            vm_body(WORKER_VM_NAME, json!(VM_INSTANCE)),
        ),
    ]);
    let cancel = Cancellation::default();
    let Found::GroupOnly(group) = client.azure.reconcile(OPERATION_ID, &cancel).unwrap() else {
        panic!("the group exists without its VM");
    };
    assert_eq!(group.id, format!("/subscriptions/{SUB}/resourceGroups/{GROUP}"));
    let Found::Created { group, vm } = client.azure.reconcile(OPERATION_ID, &cancel).unwrap() else {
        panic!("the group and its VM exist");
    };
    assert_eq!(group.tags[OPERATION_TAG], OPERATION_ID);
    assert_eq!(vm.name, WORKER_VM_NAME);
    assert_eq!(
        vm.id,
        format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker")
    );
    assert_eq!(vm.instance_id.as_deref(), Some(VM_INSTANCE));
    client.done();
}

#[test]
fn reconcile_refuses_a_foreign_group_and_a_wrong_vm_name() {
    let wrong_vm_id = vm_body(WORKER_VM_NAME, json!(VM_INSTANCE))
        .replace(&format!("resourceGroups/{GROUP}/"), "resourceGroups/horizon-ws-other/");
    let client = scripted(vec![
        get(group_url(GROUP), 200, group_body(GROUP, Some("op-2"))),
        get(group_url(GROUP), 200, group_body(GROUP, None)),
        get(group_url(GROUP), 200, group_body("horizon-ws-op-2", Some(OPERATION_ID))),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(
            vm_url(GROUP, WORKER_VM_NAME),
            200,
            vm_body("worker2", json!(VM_INSTANCE)),
        ),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(vm_url(GROUP, WORKER_VM_NAME), 200, wrong_vm_id),
    ]);
    let cancel = Cancellation::default();
    for case in [
        "another operation's tag",
        "no ownership tag",
        "another group's name",
        "another VM name",
        "a VM id in another group",
    ] {
        assert!(
            matches!(
                client.azure.reconcile(OPERATION_ID, &cancel),
                Err(CloudError::IdentityMismatch)
            ),
            "{case}"
        );
    }
    let sent = client.sent();
    assert_eq!(
        sent.iter().filter(|line| line.contains("virtualMachines")).count(),
        2,
        "a group without proof of ownership is never read further"
    );
    assert!(
        sent.iter().all(|line| line.starts_with("GET ")),
        "a foreign group is never written to"
    );
    client.done();
}

#[test]
fn reconcile_never_writes() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, ""),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(vm_url(GROUP, WORKER_VM_NAME), 404, ""),
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(vm_url(GROUP, WORKER_VM_NAME), 200, vm_body(WORKER_VM_NAME, Value::Null)),
        get(group_url(GROUP), 200, group_body(GROUP, Some("op-2"))),
        get(group_url(GROUP), 409, arm_error("Conflict", "busy")),
        get(group_url(GROUP), 503, ""),
    ]);
    let cancel = Cancellation::default();
    let results: Vec<_> = (0..6).map(|_| client.azure.reconcile(OPERATION_ID, &cancel)).collect();
    assert!(matches!(results[0], Ok(Found::Absent)));
    assert!(matches!(results[1], Ok(Found::GroupOnly(_))));
    assert!(matches!(results[2], Ok(Found::Created { .. })));
    assert!(matches!(results[3], Err(CloudError::IdentityMismatch)));
    assert!(matches!(results[4], Err(CloudError::Http(409, _))));
    assert!(matches!(results[5], Err(CloudError::Transport)));
    let sent = client.sent();
    assert_eq!(sent.len(), 8);
    assert!(
        sent.iter().all(|line| line.starts_with("GET ")),
        "every reconcile request is a GET: {sent:?}"
    );
    client.done();
}
