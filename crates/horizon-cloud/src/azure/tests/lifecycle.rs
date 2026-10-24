use super::provision::{absent, address_read, bound_group, worker_body, worker_url};
use super::template::spec;
use super::*;
use crate::{
    CreateState, WorkerStatus,
    azure::{
        lifecycle::{DEALLOCATED, Power, RESOURCES_API_VERSION, UNRESOLVED_GRACE, delete, inspect_bound, power},
        provision::{Journal, Records, STOPPED_ALLOCATED},
    },
};

fn action_url(action: &str) -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker/{action}?api-version=2024-03-01"
    )
}

fn sweep_url() -> String {
    format!(
        "https://management.azure.com/subscriptions/{SUB}/resources?$filter=tagName%20eq%20'horizon-operation'%20and%20tagValue%20eq%20'{OPERATION_ID}'&api-version={RESOURCES_API_VERSION}"
    )
}

fn group_id() -> String {
    format!("/subscriptions/{SUB}/resourceGroups/{GROUP}")
}

/// The worker VM in `power` state.
fn vm_in(power: &str) -> String {
    let mut body = worker_body(VM_INSTANCE);
    body["properties"]["instanceView"]["statuses"][1]["code"] = json!(power);
    body.to_string()
}

/// The reads of one look at the bound worker: its VM, then the deployment's address.
fn read_vm(power: &str) -> Vec<Exchange> {
    vec![get(worker_url(), 200, vm_in(power)), address_read()]
}

fn script(parts: Vec<Vec<Exchange>>) -> Vec<Exchange> {
    parts.into_iter().flatten().collect()
}

fn group_in(state: &str, operation: &str) -> String {
    json!({
        "id": group_id(), "name": GROUP, "location": "northeurope",
        "properties": { "provisioningState": state }, "tags": { OPERATION_TAG: operation },
    })
    .to_string()
}

fn swept(ids: &[&str]) -> String {
    json!({ "value": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>() }).to_string()
}

fn status(value: &str) -> String {
    json!({ "status": value }).to_string()
}

/// Every record the delete saves, in order.
struct Recorded {
    operation: CreateState,
    journal: Journal,
    log: Vec<String>,
}

impl Recorded {
    fn new(operation: CreateState, journal: Journal) -> Self {
        Self {
            operation,
            journal,
            log: Vec::new(),
        }
    }

    fn delete(&mut self, client: &Scripted) -> Result<(), CloudError> {
        let (mut operation, mut journal) = (self.operation.clone(), self.journal.clone());
        let result = delete(
            &client.azure,
            OPERATION_ID,
            &mut operation,
            &mut journal,
            self,
            &Cancellation::default(),
        );
        assert_eq!(operation, self.operation, "the fence moves only once it is saved");
        assert_eq!(journal, self.journal);
        result
    }
}

impl Records for Recorded {
    fn before_mutation(&mut self) -> Result<(), CloudError> {
        self.log.push("mutation".into());
        Ok(())
    }

    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError> {
        let group = match journal.group {
            CreateState::Prepared => "prepared",
            CreateState::Requested => "requested",
            CreateState::Bound { .. } => "bound",
            CreateState::Terminated { .. } => "terminated",
        };
        self.log.push(format!(
            "group {group} deleting={} swept={}",
            journal.deleting, journal.swept
        ));
        self.journal = journal.clone();
        Ok(())
    }

    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError> {
        self.log.push(format!("worker {operation:?}"));
        self.operation = operation.clone();
        Ok(())
    }
}

fn bound_vm() -> CreateState {
    CreateState::Bound {
        worker_id: VM_INSTANCE.into(),
    }
}

#[test]
fn a_stop_deallocates_and_never_powers_off() {
    let client = scripted(script(vec![
        read_vm("PowerState/running"),
        vec![
            bodiless("POST", action_url("deallocate"), 202, "")
                .header("azure-asyncoperation", operation_url("dealloc")),
            get(operation_url("dealloc"), 200, status("InProgress")),
            get(operation_url("dealloc"), 200, status("Succeeded")),
        ],
        read_vm("PowerState/deallocating"),
        read_vm(DEALLOCATED),
        // A second stop of the deallocated VM only reads it.
        read_vm(DEALLOCATED),
    ]));
    let cancel = Cancellation::default();
    let announced = std::cell::RefCell::new(Vec::new());
    let mut announce = || {
        announced.borrow_mut().push(client.sent().len());
        Ok(())
    };
    let stopped = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Deallocate,
        &cancel,
        &mut announce,
    )
    .unwrap();
    assert!(stopped.deallocated());
    assert_eq!(stopped.worker.id, VM_INSTANCE);
    assert_eq!(stopped.worker.status(), WorkerStatus::Stopped);
    assert_eq!(
        *announced.borrow(),
        [2],
        "announced once, after the reads and before the request"
    );
    let again = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Deallocate,
        &cancel,
        &mut announce,
    )
    .unwrap();
    assert!(again.deallocated());
    assert_eq!(announced.borrow().len(), 1, "a deallocated VM is not announced again");
    let posts: Vec<_> = client
        .sent()
        .into_iter()
        .filter(|line| line.starts_with("POST "))
        .collect();
    assert_eq!(posts, [format!("POST {}", action_url("deallocate"))]);
    assert!(client.sent().iter().all(|line| !line.contains("powerOff")));
    client.done();
}

#[test]
fn a_powered_off_vm_still_bills_so_it_is_not_stopped_and_a_stop_deallocates_it() {
    let client = scripted(script(vec![
        read_vm("PowerState/stopped"),
        read_vm("PowerState/stopped"),
        vec![bodiless("POST", action_url("deallocate"), 200, "")],
        read_vm(DEALLOCATED),
    ]));
    let cancel = Cancellation::default();
    let found = inspect_bound(&client.azure, &spec(), VM_INSTANCE, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(found.worker.desired_status, STOPPED_ALLOCATED);
    assert_ne!(found.worker.status(), WorkerStatus::Stopped);
    assert!(!found.worker.is_starting_or_running());
    let stopped = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Deallocate,
        &cancel,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(stopped.deallocated());
    client.done();
}

#[test]
fn a_deallocation_under_way_is_awaited_and_not_sent_again() {
    let client = scripted(script(vec![
        read_vm("PowerState/deallocating"),
        read_vm("PowerState/deallocating"),
        read_vm(DEALLOCATED),
    ]));
    let announced = std::cell::Cell::new(0);
    let stopped = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Deallocate,
        &Cancellation::default(),
        &mut || {
            announced.set(announced.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert!(stopped.deallocated());
    assert_eq!(announced.get(), 1, "the earlier request is still pending");
    assert!(client.sent().iter().all(|line| line.starts_with("GET ")));
    assert_eq!(client.sleeps(), [Duration::from_millis(250); 2]);
    client.done();
}

#[test]
fn a_start_powers_the_same_vm_on_with_its_address() {
    let client = scripted(script(vec![
        read_vm(DEALLOCATED),
        vec![bodiless("POST", action_url("start"), 200, "")],
        read_vm("PowerState/starting"),
        read_vm("PowerState/running"),
    ]));
    let started = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Start,
        &Cancellation::default(),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(started.running());
    assert_eq!(started.worker.id, VM_INSTANCE);
    assert_eq!(started.worker.status(), WorkerStatus::Running);
    assert_eq!(started.worker.ssh_address(), Some("192.0.2.10:22".parse().unwrap()));
    client.done();
}

#[test]
fn a_power_change_refuses_a_lost_or_foreign_vm_before_any_request() {
    let mut foreign = worker_body(VM_INSTANCE);
    foreign["tags"] = json!({ OPERATION_TAG: "op-2" });
    let client = scripted(vec![
        get(worker_url(), 404, absent()),
        get(
            worker_url(),
            200,
            worker_body("4a2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f").to_string(),
        ),
        get(worker_url(), 200, foreign.to_string()),
    ]);
    let cancel = Cancellation::default();
    let announced = std::cell::Cell::new(0);
    let mut announce = || {
        announced.set(announced.get() + 1);
        Ok(())
    };
    for expected in ["lost", "lost", "foreign"] {
        let error = power(
            &client.azure,
            &spec(),
            VM_INSTANCE,
            Power::Deallocate,
            &cancel,
            &mut announce,
        )
        .unwrap_err();
        match expected {
            "lost" => assert!(matches!(error, CloudError::WorkerLost), "{error:?}"),
            _ => assert!(matches!(error, CloudError::IdentityMismatch), "{error:?}"),
        }
    }
    assert_eq!(announced.get(), 0);
    client.done();
}

#[test]
fn a_failed_start_is_a_refusal_with_its_reason() {
    let client = scripted(script(vec![
        read_vm(DEALLOCATED),
        vec![
            bodiless("POST", action_url("start"), 202, "").header("azure-asyncoperation", operation_url("start")),
            get(
                operation_url("start"),
                200,
                json!({ "status": "Failed", "error": { "message": "Allocation failed in this region" } }).to_string(),
            ),
        ],
    ]));
    let error = power(
        &client.azure,
        &spec(),
        VM_INSTANCE,
        Power::Start,
        &Cancellation::default(),
        &mut || Ok(()),
    )
    .unwrap_err();
    assert!(
        matches!(&error, CloudError::Rejected(reason) if reason.suffix().contains("Allocation failed")),
        "{error:?}"
    );
    client.done();
}

#[test]
fn a_delete_saves_its_intent_then_proves_the_group_absent_and_sweeps_the_tag() {
    let client = scripted(vec![
        get(group_url(GROUP), 200, group_in("Succeeded", OPERATION_ID)),
        bodiless("DELETE", group_url(GROUP), 202, ""),
        get(group_url(GROUP), 200, group_in("Deleting", OPERATION_ID)),
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")),
        get(sweep_url(), 200, swept(&[])),
    ]);
    let mut records = Recorded::new(bound_vm(), bound_group());
    assert!(records.journal.retains());
    records.delete(&client).unwrap();
    assert_eq!(
        records.log,
        [
            "group bound deleting=true swept=false",
            "mutation",
            "group terminated deleting=true swept=false",
            "group terminated deleting=true swept=true",
            &format!(
                "worker {:?}",
                CreateState::Terminated {
                    worker_id: VM_INSTANCE.into()
                }
            ),
        ]
    );
    assert_eq!(records.journal.group, CreateState::Terminated { worker_id: group_id() });
    assert!(records.journal.proves_absent(OPERATION_ID));
    assert!(!records.journal.retains());
    assert_eq!(
        client
            .sent()
            .into_iter()
            .filter(|line| !line.starts_with("GET "))
            .collect::<Vec<_>>(),
        [format!("DELETE {}", group_url(GROUP))],
        "one delete request"
    );
    // A delete again of the proven cloud sends nothing.
    let log = records.log.len();
    records.delete(&client).unwrap();
    assert_eq!(records.log.len(), log);
    client.done();
}

#[test]
fn a_lost_delete_answer_is_reconciled_by_reading_the_group() {
    let client = scripted(vec![
        get(group_url(GROUP), 200, group_in("Succeeded", OPERATION_ID)),
        bodiless("DELETE", group_url(GROUP), 503, ""),
        // The retry finds the group being deleted and sends nothing.
        get(group_url(GROUP), 200, group_in("Deleting", OPERATION_ID)),
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")),
        get(sweep_url(), 200, swept(&[])),
    ]);
    let mut records = Recorded::new(bound_vm(), bound_group());
    assert!(matches!(records.delete(&client), Err(CloudError::Transport)));
    assert!(records.journal.deleting && records.journal.retains());
    assert_eq!(records.operation, bound_vm());
    records.delete(&client).unwrap();
    assert!(records.journal.proves_absent(OPERATION_ID));
    assert_eq!(
        client.sent().iter().filter(|line| line.starts_with("DELETE ")).count(),
        1
    );
    client.done();
}

#[test]
fn a_group_that_is_not_the_operations_is_never_deleted() {
    let client = scripted(vec![get(group_url(GROUP), 200, group_in("Succeeded", "op-2"))]);
    let mut records = Recorded::new(bound_vm(), bound_group());
    assert!(matches!(records.delete(&client), Err(CloudError::IdentityMismatch)));
    assert!(records.journal.retains() && !records.journal.proves_absent(OPERATION_ID));
    assert!(client.sent().iter().all(|line| line.starts_with("GET ")));
    client.done();
}

#[test]
fn a_tagged_resource_left_after_the_group_keeps_the_cloud_unconfirmed() {
    let leftover = format!("/subscriptions/{SUB}/resourceGroups/other/providers/Microsoft.Compute/disks/copy");
    let mut script = vec![get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone"))];
    script.extend((0..6).map(|_| get(sweep_url(), 200, swept(&[&leftover]))));
    // The retry reads the group again and sweeps; a further page is not a proof.
    script.push(get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")));
    script.push(get(
        sweep_url(),
        200,
        json!({ "value": [], "nextLink": sweep_url() }).to_string(),
    ));
    script.push(get(sweep_url(), 200, swept(&[])));
    let client = scripted(script);
    let mut records = Recorded::new(bound_vm(), bound_group());
    let error = records.delete(&client).unwrap_err();
    assert!(error.to_string().contains("still lists resources tagged"), "{error}");
    assert!(!error.to_string().contains("copy"), "an untrusted id is never printed");
    assert_eq!(records.journal.group, CreateState::Terminated { worker_id: group_id() });
    assert!(records.journal.retains() && !records.journal.proves_absent(OPERATION_ID));
    assert_eq!(records.operation, bound_vm(), "the VM fence ends only with the proof");
    records.delete(&client).unwrap();
    assert!(records.journal.proves_absent(OPERATION_ID));
    assert!(client.sent().iter().all(|line| line.starts_with("GET ")));
    client.done();
}

#[test]
fn a_group_whose_create_answer_was_lost_is_trusted_absent_only_after_the_grace() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")),
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")),
        get(sweep_url(), 200, swept(&[])),
    ]);
    let mut records = Recorded::new(
        CreateState::Requested,
        Journal {
            region: Some("northeurope".into()),
            group: CreateState::Requested,
            ..Journal::default()
        },
    );
    records.delete(&client).unwrap();
    assert!(records.journal.proves_absent(OPERATION_ID));
    assert_eq!(
        records.operation,
        CreateState::Prepared,
        "no VM can exist without its group"
    );
    let waited: Duration = client.sleeps().iter().sum();
    assert_eq!(waited, UNRESOLVED_GRACE);
    client.done();
}

#[test]
fn a_cloud_that_never_requested_its_group_is_proven_absent_without_a_request() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "gone")),
        get(sweep_url(), 503, ""),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, Journal::default());
    assert!(matches!(records.delete(&client), Err(CloudError::Transport)));
    assert!(
        !records.journal.retains(),
        "a failed sweep of a never-requested group retains nothing"
    );
    assert_eq!(records.journal.group, CreateState::Prepared);
    client.done();
}

#[test]
fn only_a_complete_journal_of_this_operation_proves_absence() {
    let proven = Journal {
        region: Some("northeurope".into()),
        group: CreateState::Terminated { worker_id: group_id() },
        deleting: true,
        swept: true,
        ..Journal::default()
    };
    assert!(proven.proves_absent(OPERATION_ID));
    assert!(!proven.retains());
    assert!(!proven.proves_absent("op-2"), "another cloud's group");
    for journal in [
        Journal {
            swept: false,
            ..proven.clone()
        },
        Journal {
            deleting: false,
            ..proven.clone()
        },
        Journal {
            group: bound_group().group,
            ..proven.clone()
        },
        Journal {
            group: CreateState::Terminated {
                worker_id: format!("/subscriptions/{SUB}/resourceGroups/horizon-ws-op-2"),
            },
            ..proven.clone()
        },
        Journal {
            group: CreateState::Terminated {
                worker_id: format!("/subscriptions/Not-A-Uuid/resourceGroups/{GROUP}"),
            },
            ..proven.clone()
        },
        Journal {
            group: CreateState::Terminated {
                worker_id: format!("{}/extra", group_id()),
            },
            ..proven.clone()
        },
    ] {
        assert!(!journal.proves_absent(OPERATION_ID), "{journal:?}");
    }
    // A journal written before deletion keeps its bytes.
    let saved = serde_json::to_value(bound_group()).unwrap();
    assert!(saved.get("deleting").is_none() && saved.get("swept").is_none());
}

#[test]
fn a_cloud_being_deleted_is_never_provisioned_again() {
    let client = scripted(Vec::new());
    let journal = Journal {
        deleting: true,
        ..bound_group()
    };
    let error = crate::azure::provision::provision(
        &client.azure,
        crate::azure::provision::Request {
            spec: &spec(),
            policy: &crate::azure::provision::Policy {
                sizes: vec!["Standard_D4s_v3".into()],
                regions: vec!["northeurope".into()],
            },
            login: None,
        },
        &mut bound_vm(),
        &mut journal.clone(),
        &mut Recorded::new(bound_vm(), journal),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("being deleted"), "{error}");
    assert!(client.sent().is_empty());
}
