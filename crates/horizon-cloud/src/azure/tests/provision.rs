use super::template::spec;
use super::*;
use crate::{
    CreateState, Worker, WorkerSpec,
    azure::{
        provision::{Journal, Policy, Records, Request, inspect_worker, provision},
        template::{DEPLOYMENT_NAME, template},
    },
    host,
};
use base64::Engine as _;

const ADDRESS: &str = "192.0.2.10";

/// The records a caller keeps, and the order it was told of each.
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
}

impl Records for Recorded {
    fn before_mutation(&mut self) -> Result<(), CloudError> {
        self.log.push("mutation".into());
        Ok(())
    }

    fn mutation_refused(&mut self) {
        self.log.push("refused".into());
    }

    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError> {
        self.log.push(format!("group {}", state(&journal.group)));
        self.journal = journal.clone();
        Ok(())
    }

    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError> {
        self.log.push(format!("worker {}", state(operation)));
        self.operation = operation.clone();
        Ok(())
    }
}

fn state(state: &CreateState) -> &'static str {
    match state {
        CreateState::Prepared => "prepared",
        CreateState::Requested => "requested",
        CreateState::Bound { .. } => "bound",
        CreateState::Terminated { .. } => "terminated",
    }
}

fn policy() -> Policy {
    Policy {
        sizes: vec!["Standard_D4s_v3".into(), "Standard_D8s_v3".into()],
        regions: vec!["northeurope".into()],
    }
}

pub(super) fn bound_group() -> Journal {
    Journal {
        region: Some("northeurope".into()),
        group: CreateState::Bound {
            worker_id: format!("/subscriptions/{SUB}/resourceGroups/{GROUP}"),
        },
        ..Journal::default()
    }
}

/// Provisions with the records' fences, as a caller that saved them would.
fn run(client: &Scripted, records: &mut Recorded, login: Option<host::RegistryLogin>) -> Result<Worker, CloudError> {
    let (mut operation, mut journal) = (records.operation.clone(), records.journal.clone());
    let result = provision(
        &client.azure,
        Request {
            spec: &spec(),
            policy: &policy(),
            login,
        },
        &mut operation,
        &mut journal,
        records,
        &Cancellation::default(),
    );
    assert_eq!(operation, records.operation, "the fence moves only once it is saved");
    assert_eq!(journal, records.journal);
    result
}

pub(super) fn worker_url() -> String {
    vm_url(GROUP, WORKER_VM_NAME)
}

fn create_url() -> String {
    deployment_url(DEPLOYMENT_NAME)
}

pub(super) fn worker_body(instance: &str) -> Value {
    json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/virtualMachines/worker"),
        "name": "worker",
        "location": "northeurope",
        "tags": { OPERATION_TAG: OPERATION_ID },
        "properties": {
            "vmId": instance,
            "provisioningState": "Succeeded",
            "hardwareProfile": { "vmSize": "Standard_D4s_v3" },
            "storageProfile": {
                "osDisk": { "name": "worker-os", "diskSizeGB": 30 },
                "dataDisks": [{ "lun": 0, "diskSizeGB": 64, "deleteOption": "Detach", "managedDisk": {
                    "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Compute/disks/worker-workspace") } }]
            },
            "instanceView": { "statuses": [{ "code": "ProvisioningState/succeeded" }, { "code": "PowerState/running" }] }
        }
    })
}

fn deployed(state: &str) -> String {
    json!({
        "id": format!("/subscriptions/{SUB}/resourceGroups/{GROUP}/providers/Microsoft.Resources/deployments/{DEPLOYMENT_NAME}"),
        "name": DEPLOYMENT_NAME,
        "properties": { "provisioningState": state, "outputs": { "publicIp": { "type": "String", "value": ADDRESS } } }
    })
    .to_string()
}

pub(super) fn absent() -> String {
    arm_error("ResourceNotFound", "not found")
}

/// The reads that end every provisioning: the deployment's address output.
pub(super) fn address_read() -> Exchange {
    get(create_url(), 200, deployed("Succeeded"))
}

fn writes(client: &Scripted) -> Vec<String> {
    client
        .sent()
        .into_iter()
        .filter(|line| !line.starts_with("GET "))
        .collect()
}

#[test]
fn a_new_cloud_creates_its_group_then_one_deployment_and_binds_the_vm() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, arm_error("ResourceGroupNotFound", "none")),
        put(
            group_url(GROUP),
            Value::Null,
            201,
            group_body(GROUP, Some(OPERATION_ID)),
        )
        .any_body(),
        put(create_url(), Value::Null, 201, deployed("Accepted"))
            .any_body()
            .header("azure-asyncoperation", operation_url("create-1")),
        get(
            operation_url("create-1"),
            200,
            json!({"status": "Succeeded"}).to_string(),
        ),
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        address_read(),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, Journal::default());
    let worker = run(&client, &mut records, None).unwrap();
    assert_eq!(
        records.log,
        [
            "group requested",
            "mutation",
            "group bound",
            "worker requested",
            "mutation",
            "worker bound"
        ],
        "each fence is saved before the create it guards"
    );
    assert_eq!(
        records.operation,
        CreateState::Bound {
            worker_id: VM_INSTANCE.into()
        }
    );
    assert_eq!(records.journal, bound_group());
    let bodies = client.bodies();
    assert_eq!(
        bodies[0],
        json!({"location": "northeurope", "tags": { OPERATION_TAG: OPERATION_ID }})
    );
    let properties = &bodies[1]["properties"];
    assert_eq!(properties["mode"], "Incremental");
    assert_eq!(properties["template"], template());
    assert_eq!(
        properties["parameters"]["tags"]["value"],
        json!({ OPERATION_TAG: OPERATION_ID })
    );
    assert_eq!(properties["parameters"]["vmSize"]["value"], "Standard_D4s_v3");
    let custom = base64::engine::general_purpose::STANDARD
        .decode(properties["parameters"]["customData"]["value"].as_str().unwrap())
        .unwrap();
    assert!(custom.starts_with(b"#cloud-config\n"));
    assert_eq!(worker.id, VM_INSTANCE);
    assert_eq!(worker.name, spec().name());
    assert_eq!(worker.status(), crate::WorkerStatus::Running);
    let endpoint = worker.ssh_endpoint().unwrap();
    assert_eq!((endpoint.host().as_str(), endpoint.port()), (ADDRESS, 22));
    assert_eq!((worker.vcpu_count, worker.memory_in_gb), (Some(4), Some(16)));
    assert_eq!((worker.container_disk_in_gb, worker.volume_in_gb), (Some(30), Some(64)));
    assert_eq!(worker.data_center(), Some("northeurope"));
    assert_eq!(worker.env["HORIZON_CLOUD_OPERATION"], OPERATION_ID);
    assert!(worker.cost_per_hr.is_none());
    client.done();
}

#[test]
fn a_lost_create_answer_is_reconciled_and_never_sent_again() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, ""),
        put(
            group_url(GROUP),
            Value::Null,
            201,
            group_body(GROUP, Some(OPERATION_ID)),
        )
        .any_body(),
        // The answer to the create is lost.
        put(create_url(), Value::Null, 503, "").any_body(),
        // The next attempt only reads: no VM yet, and the deployment still runs.
        get(worker_url(), 404, absent()),
        get(create_url(), 200, deployed("Running")),
        get(create_url(), 200, deployed("Running")),
        get(create_url(), 200, deployed("Succeeded")),
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        address_read(),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, Journal::default());
    assert!(matches!(run(&client, &mut records, None), Err(CloudError::Transport)));
    assert_eq!(
        records.operation,
        CreateState::Requested,
        "an unknown outcome keeps the fence"
    );
    let sent = client.sent().len();
    let worker = run(&client, &mut records, None).unwrap();
    assert_eq!(worker.id, VM_INSTANCE);
    assert!(client.sent()[sent..].iter().all(|line| line.starts_with("GET ")));
    assert_eq!(
        writes(&client)
            .iter()
            .filter(|line| line.contains("/deployments/"))
            .count(),
        1,
        "exactly one create"
    );
    assert!(!records.log.contains(&"refused".to_owned()));
    assert_eq!(
        records.operation,
        CreateState::Bound {
            worker_id: VM_INSTANCE.into()
        }
    );
    assert_eq!(
        client.sleeps().iter().sum::<Duration>(),
        Duration::from_millis(500),
        "one backoff step between the deployment reads"
    );
    client.done();
}

#[test]
fn a_conflict_keeps_the_fence_and_an_unfound_create_is_unresolved() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, ""),
        put(
            group_url(GROUP),
            Value::Null,
            201,
            group_body(GROUP, Some(OPERATION_ID)),
        )
        .any_body(),
        put(
            create_url(),
            Value::Null,
            409,
            arm_error("AnotherOperationInProgress", "busy"),
        )
        .any_body(),
        get(worker_url(), 404, absent()),
        get(create_url(), 404, absent()),
        get(group_url(GROUP), 404, ""),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, Journal::default());
    assert!(matches!(
        run(&client, &mut records, None),
        Err(CloudError::Http(409, _))
    ));
    assert_eq!(records.operation, CreateState::Requested, "a 409 is never a refusal");
    assert!(matches!(
        run(&client, &mut records, None),
        Err(CloudError::CreationUnresolved)
    ));
    assert_eq!(
        records.operation,
        CreateState::Requested,
        "finding nothing never opens the fence"
    );
    // A group whose create answer was lost and that is not found is not created again.
    let mut lost_group = Recorded::new(
        CreateState::Prepared,
        Journal {
            region: Some("northeurope".into()),
            group: CreateState::Requested,
            ..Journal::default()
        },
    );
    assert!(matches!(
        run(&client, &mut lost_group, None),
        Err(CloudError::CreationUnresolved)
    ));
    assert!(lost_group.log.is_empty());
    assert_eq!(
        writes(&client).len(),
        2,
        "one group create and one deployment, never repeated"
    );
    client.done();
}

#[test]
fn a_bound_worker_is_only_read_and_must_be_the_same_instance() {
    let mut two_disks = worker_body(VM_INSTANCE);
    two_disks["properties"]["storageProfile"]["dataDisks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"lun": 1, "managedDisk": {"id": "x"}}));
    let mut foreign = worker_body(VM_INSTANCE);
    foreign["tags"] = json!({ OPERATION_TAG: "op-2" });
    let mut stopped = worker_body(VM_INSTANCE);
    stopped["properties"]["instanceView"]["statuses"][1]["code"] = json!("PowerState/deallocated");
    let client = scripted(vec![
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        address_read(),
        get(worker_url(), 200, stopped.to_string()),
        address_read(),
        get(
            worker_url(),
            200,
            worker_body("4a2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f").to_string(),
        ),
        get(worker_url(), 404, absent()),
        get(worker_url(), 200, two_disks.to_string()),
        get(worker_url(), 200, foreign.to_string()),
    ]);
    let bound = || {
        Recorded::new(
            CreateState::Bound {
                worker_id: VM_INSTANCE.into(),
            },
            bound_group(),
        )
    };
    assert_eq!(run(&client, &mut bound(), None).unwrap().id, VM_INSTANCE);
    assert_eq!(
        run(&client, &mut bound(), None).unwrap().status(),
        crate::WorkerStatus::Stopped
    );
    assert!(
        matches!(run(&client, &mut bound(), None), Err(CloudError::WorkerLost)),
        "a VM made again under the name"
    );
    assert!(matches!(run(&client, &mut bound(), None), Err(CloudError::WorkerLost)));
    let error = run(&client, &mut bound(), None).unwrap_err();
    assert!(error.to_string().contains("workspace disk"), "{error}");
    assert!(matches!(
        run(&client, &mut bound(), None),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(writes(&client).is_empty(), "a bound worker is never written to");
    client.done();
}

#[test]
fn a_request_that_cannot_succeed_sends_nothing() {
    let client = scripted(Vec::new());
    let token = |length: usize| host::RegistryLogin {
        server: "registry.example".into(),
        username: "reader".into(),
        password: Credential::new("t".repeat(length)).unwrap(),
    };
    let cancel = Cancellation::default();
    let attempt = |spec: &WorkerSpec, policy: &Policy, journal: Journal, login| {
        let mut records = Recorded::new(CreateState::Prepared, journal);
        let (mut operation, mut journal) = (records.operation.clone(), records.journal.clone());
        let result = provision(
            &client.azure,
            Request { spec, policy, login },
            &mut operation,
            &mut journal,
            &mut records,
            &cancel,
        );
        assert!(records.log.is_empty(), "no record is written");
        result.unwrap_err().to_string()
    };
    let mut small = spec();
    small.profile.storage.volume_gb = 3;
    let unknown = Policy {
        sizes: vec!["Standard_NC6".into()],
        ..policy()
    };
    let nowhere = Policy {
        regions: Vec::new(),
        ..policy()
    };
    let moved = Journal {
        region: Some("westeurope".into()),
        ..Journal::default()
    };
    let refusals = [
        attempt(&small, &policy(), Journal::default(), None),
        attempt(&spec(), &unknown, Journal::default(), None),
        attempt(&spec(), &nowhere, Journal::default(), None),
        attempt(&spec(), &policy(), moved, None),
        attempt(&spec(), &policy(), Journal::default(), Some(token(32_768))),
        attempt(
            &spec(),
            &policy(),
            Journal::default(),
            Some(host::RegistryLogin {
                server: "other.example".into(),
                ..token(8)
            }),
        ),
    ];
    for (refusal, expected) in refusals.iter().zip([
        "between 4 and 32,767 GB",
        "not one Horizon offers",
        "Add an Azure region",
        "no longer allowed",
        "user-data limit",
        "different registry",
    ]) {
        assert!(refusal.contains(expected), "{refusal}");
    }
    assert!(client.sent().is_empty(), "every refusal comes before the first request");
}

#[test]
fn a_sold_out_size_moves_to_the_next_and_proves_nothing_was_made() {
    let client = scripted(vec![
        get(group_url(GROUP), 404, ""),
        put(
            group_url(GROUP),
            Value::Null,
            201,
            group_body(GROUP, Some(OPERATION_ID)),
        )
        .any_body(),
        put(create_url(), Value::Null, 409, arm_error("SkuNotAvailable", "sold out")).any_body(),
        put(create_url(), Value::Null, 400, arm_error("QuotaExceeded", "no quota")).any_body(),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, Journal::default());
    let error = run(&client, &mut records, None).unwrap_err();
    assert!(matches!(error, CloudError::Capacity(_)), "{error}");
    assert_eq!(
        records.log[3..],
        [
            "worker requested",
            "mutation",
            "refused",
            "worker prepared",
            "worker requested",
            "mutation",
            "refused",
            "worker prepared"
        ]
    );
    let sizes: Vec<Value> = client.bodies()[1..]
        .iter()
        .map(|body| body["properties"]["parameters"]["vmSize"]["value"].clone())
        .collect();
    assert_eq!(sizes, [json!("Standard_D4s_v3"), json!("Standard_D8s_v3")]);
    assert!(records.journal.retains(), "the empty group stays bound");
    client.done();
}

#[test]
fn a_found_group_is_adopted_only_with_the_operation_tag_and_never_written() {
    let client = scripted(vec![
        get(group_url(GROUP), 200, group_body(GROUP, Some("op-2"))),
        get(
            group_url(GROUP),
            200,
            group_body(GROUP, Some(OPERATION_ID)).replace("northeurope", "westeurope"),
        ),
        // A group and worker an attempt made, though its records were lost.
        get(group_url(GROUP), 200, group_body(GROUP, Some(OPERATION_ID))),
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        address_read(),
    ]);
    let mut foreign = Recorded::new(CreateState::Prepared, Journal::default());
    assert!(matches!(
        run(&client, &mut foreign, None),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(foreign.log.is_empty());
    let mut moved = Recorded::new(CreateState::Prepared, Journal::default());
    let error = run(&client, &mut moved, None).unwrap_err();
    assert!(error.to_string().contains("region"), "{error}");
    assert!(moved.log.is_empty(), "a group in another region is not bound");
    let mut lost = Recorded::new(CreateState::Prepared, Journal::default());
    assert_eq!(run(&client, &mut lost, None).unwrap().id, VM_INSTANCE);
    assert_eq!(lost.log, ["group bound", "worker bound"]);
    assert!(writes(&client).is_empty());
    client.done();
}

#[test]
fn a_failed_deployment_keeps_the_fence_until_arm_shows_it_made_no_vm() {
    let client = scripted(vec![
        // The group is bound and this call did not make it, so it is read for a worker first.
        get(worker_url(), 404, absent()),
        get(create_url(), 404, absent()),
        put(create_url(), Value::Null, 201, deployed("Accepted"))
            .any_body()
            .header("azure-asyncoperation", operation_url("create-2")),
        get(
            operation_url("create-2"),
            200,
            json!({"status": "Failed", "error": {"message": "AllocationFailed"}}).to_string(),
        ),
        get(worker_url(), 404, absent()),
        get(create_url(), 200, deployed("Failed")),
        get(worker_url(), 404, absent()),
    ]);
    let mut records = Recorded::new(CreateState::Prepared, bound_group());
    assert!(matches!(run(&client, &mut records, None), Err(CloudError::Rejected(_))));
    assert_eq!(
        records.operation,
        CreateState::Requested,
        "ARM may still have made part of it"
    );
    assert!(matches!(run(&client, &mut records, None), Err(CloudError::Rejected(_))));
    assert_eq!(
        records.operation,
        CreateState::Prepared,
        "a deployment that ended without a VM made none"
    );
    assert_eq!(writes(&client).len(), 1);
    client.done();
}

#[test]
fn a_waiting_caller_reads_the_bound_worker_with_the_same_checks() {
    let mut elsewhere = worker_body(VM_INSTANCE);
    elsewhere["location"] = json!("westeurope");
    let client = scripted(vec![
        get(worker_url(), 200, worker_body(VM_INSTANCE).to_string()),
        address_read(),
        get(worker_url(), 200, elsewhere.to_string()),
        get(
            worker_url(),
            200,
            worker_body("4a2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f").to_string(),
        ),
    ]);
    let cancel = Cancellation::default();
    let read = || inspect_worker(&client.azure, &spec(), &policy(), "northeurope", VM_INSTANCE, &cancel);
    assert_eq!(
        read().unwrap().public_ip.map(|ip| ip.to_string()).as_deref(),
        Some(ADDRESS)
    );
    assert!(read().unwrap_err().to_string().contains("size or region"));
    assert!(matches!(read(), Err(CloudError::WorkerLost)));
    assert!(writes(&client).is_empty());
    client.done();
}
