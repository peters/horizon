use super::super::{AzureError, AzureWorker, resource_group_name, valid_resource_group_id};
use super::{IMAGE, OTHER_SUB, SUB};
use crate::cloud_run::{
    CloudJobId, CloudWorkflowId,
    interactive_worker::{InteractiveWorkerLease, InteractiveWorkerLifetime},
};

#[test]
fn worker_identity_is_exact_and_resource_ids_are_parsed_strictly() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let group = resource_group_name(workflow_id, job_id);
    assert!(group.starts_with("horizon-ws-"));
    assert_eq!(group.len(), 84);
    let group_id = format!("/subscriptions/{SUB}/resourceGroups/{group}");
    let worker = AzureWorker {
        workflow_id,
        job_id,
        subscription_id: SUB.into(),
        resource_group: group.clone(),
        group_id: group_id.clone(),
        image: IMAGE.into(),
        lifetime: InteractiveWorkerLifetime::Persistent,
    };
    assert_eq!(worker.validate(), Ok(()));
    assert!(valid_resource_group_id(
        &group_id.replace("resourceGroups", "resourcegroups"),
        SUB,
        &group
    ));
    assert!(valid_resource_group_id(
        &group_id.replace(SUB, &SUB.to_ascii_uppercase()),
        SUB,
        &group
    ));
    for (bad_id, label) in [
        (group_id.replace(&group, "horizon-ws-other"), "group"),
        (group_id.replace(SUB, OTHER_SUB), "subscription"),
        (
            format!("{group_id}/providers/Microsoft.Compute/virtualMachines/worker"),
            "trailing segments",
        ),
        (
            group_id.replace("/subscriptions/", "/Subscriptions/"),
            "fixed segment casing",
        ),
        (format!("{group_id} "), "whitespace"),
        (String::new(), "empty"),
    ] {
        assert!(!valid_resource_group_id(&bad_id, SUB, &group), "{label}");
        let mut broken = worker.clone();
        broken.group_id = bad_id;
        assert_eq!(broken.validate(), Err(AzureError::InvalidPersistedWorker), "{label}");
    }
    let mut renamed = worker.clone();
    renamed.resource_group = "horizon-ws-renamed".into();
    assert_eq!(renamed.validate(), Err(AzureError::InvalidPersistedWorker));
    for terminate_after in ["tomorrow", "2030-01-01T00:00:00Z"] {
        let mut leased = worker.clone();
        leased.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: terminate_after.into(),
        });
        assert_eq!(
            leased.validate(),
            Err(AzureError::InvalidPersistedWorker),
            "{terminate_after}"
        );
        let encoded = serde_json::to_string(&leased).expect("encode");
        assert!(
            serde_json::from_str::<AzureWorker>(&encoded).is_err(),
            "a lease never decodes: {terminate_after}"
        );
    }
    let encoded = serde_json::to_string(&worker).expect("encode");
    assert_eq!(serde_json::from_str::<AzureWorker>(&encoded).expect("decode"), worker);
    assert!(serde_json::from_str::<AzureWorker>(&encoded.replace("\"image\"", "\"extra\":1,\"image\"")).is_err());
    let tampered = encoded.replace(&group, "horizon-ws-other");
    let error = serde_json::from_str::<AzureWorker>(&tampered).expect_err("tampered handle is rejected while decoding");
    assert!(error.to_string().contains("persisted Azure worker identity is invalid"));
}
