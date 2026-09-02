use super::*;

fn sha(character: char) -> GitCommitSha {
    GitCommitSha::parse(character.to_string().repeat(40)).expect("valid test sha")
}

fn node(id: CloudJobId, logical_key: &str, depends_on: Vec<CloudJobId>) -> WorkflowNode {
    WorkflowNode {
        id,
        logical_key: logical_key.to_string(),
        label: logical_key.to_string(),
        kind: WorkflowNodeKind::Build,
        state: CloudJobState::Queued,
        progress: CloudProgress::Pending,
        weight: 1,
        attempt: 1,
        retry: RetryPolicy::default(),
        supersedes: None,
        depends_on,
        source: Some(GitSource {
            repository: "fintermobilityas/nativesdk".to_string(),
            commit: sha('a'),
            branch: Some("feature/plate-speedups".to_string()),
        }),
        worker: None,
        input_artifact_ids: Vec::new(),
        outputs: Vec::new(),
        approval: None,
        release: None,
        environment_lease: None,
    }
}

fn workflow(nodes: Vec<WorkflowNode>) -> CloudWorkflow {
    CloudWorkflow {
        protocol_version: CLOUD_RUN_PROTOCOL_VERSION,
        id: CloudWorkflowId::new(),
        title: "NativeSDK to YouPark".to_string(),
        created_at_millis: 1_000,
        updated_at_millis: 2_000,
        retain_until_millis: 604_801_000,
        nodes,
    }
}

#[test]
fn identifiers_round_trip_as_json_strings() {
    let id = CloudWorkflowId::new();
    let json = serde_json::to_string(&id).expect("serialize id");
    assert_eq!(serde_json::from_str::<CloudWorkflowId>(&json).expect("restore id"), id);
}

#[test]
fn commit_and_digest_require_full_hex_values() {
    assert_eq!(
        GitCommitSha::parse("A".repeat(40)).expect("valid sha").as_str(),
        "a".repeat(40)
    );
    assert!(GitCommitSha::parse("abc123").is_err());
    assert_eq!(
        ArtifactDigest::parse_sha256("F".repeat(64))
            .expect("valid digest")
            .as_str(),
        "f".repeat(64)
    );
    assert!(ArtifactDigest::parse_sha256("not-a-digest").is_err());
    assert!(serde_json::from_str::<GitCommitSha>(r#""short""#).is_err());
    assert!(serde_json::from_str::<ArtifactDigest>(r#""short""#).is_err());
}

#[test]
fn workflow_validates_a_cross_repository_dag() {
    let producer = CloudJobId::new();
    let consumer = CloudJobId::new();
    let mut consumer_node = node(consumer, "youpark-tests", vec![producer]);
    consumer_node.source = Some(GitSource {
        repository: "fintermobilityas/youpark".to_string(),
        commit: sha('b'),
        branch: Some("main".to_string()),
    });
    assert_eq!(
        workflow(vec![node(producer, "nativesdk-build", Vec::new()), consumer_node]).validate(),
        Ok(())
    );
}

#[test]
fn workflow_rejects_missing_dependencies_and_cycles() {
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    let missing = CloudJobId::new();
    assert_eq!(
        workflow(vec![node(first, "first", vec![missing])]).validate(),
        Err(CloudProtocolError::MissingDependency {
            node: first,
            dependency: missing,
        })
    );
    let cycle = workflow(vec![
        node(first, "first", vec![second]),
        node(second, "second", vec![first]),
    ])
    .validate();
    assert!(matches!(
        cycle,
        Err(CloudProtocolError::DependencyCycle(id)) if id == first || id == second
    ));
}

#[test]
fn approval_nodes_require_an_explicit_gate() {
    let id = CloudJobId::new();
    let mut approval = node(id, "publish-test", Vec::new());
    approval.kind = WorkflowNodeKind::Approval;
    approval.state = CloudJobState::WaitingForApproval;
    assert_eq!(
        workflow(vec![approval.clone()]).validate(),
        Err(CloudProtocolError::MissingApprovalGate(id))
    );

    approval.approval = Some(ApprovalGate {
        action: "Publish To Test".to_string(),
        decision: ApprovalDecision::Pending,
        evidence_job_ids: Vec::new(),
    });
    assert_eq!(workflow(vec![approval]).validate(), Ok(()));
}

#[test]
fn environment_lease_is_bound_to_its_workflow_and_job() {
    let id = CloudJobId::new();
    let mut workflow = workflow(vec![node(id, "test-release", Vec::new())]);
    workflow.nodes[0].environment_lease = Some(EnvironmentLease {
        environment: "youpark-test".to_string(),
        holder_workflow_id: workflow.id,
        holder_job_id: id,
        acquired_at_millis: 10,
        expires_at_millis: 20,
    });
    assert_eq!(workflow.validate(), Ok(()));

    workflow.nodes[0]
        .environment_lease
        .as_mut()
        .expect("lease exists")
        .holder_job_id = CloudJobId::new();
    assert_eq!(
        workflow.validate(),
        Err(CloudProtocolError::InvalidEnvironmentLease(id))
    );
}

#[test]
fn progress_uses_measured_work_and_marks_unknown_work_as_estimated() {
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    let mut measured = node(first, "image-pull", Vec::new());
    measured.weight = 3;
    measured.state = CloudJobState::PullingImage;
    measured.progress = CloudProgress::Measured {
        phase: "pulling_image".to_string(),
        completed: 50,
        total: 100,
        unit: ProgressUnit::Bytes,
    };
    let mut unknown = node(second, "build", vec![first]);
    unknown.state = CloudJobState::Running;
    unknown.progress = CloudProgress::Indeterminate {
        phase: "building".to_string(),
        message: "Compiling nativesdk".to_string(),
    };
    let progress = workflow(vec![measured, unknown]).progress();
    assert_eq!(progress.basis_points, 3_750);
    assert!(progress.estimated);
}

#[test]
fn protocol_round_trip_preserves_release_provenance() {
    let build_id = CloudJobId::new();
    let mut build = node(build_id, "nativesdk-package", Vec::new());
    build.state = CloudJobState::Completed;
    build.progress = CloudProgress::Completed;
    build.outputs.push(ArtifactRef {
        artifact_id: "nativesdk-local".to_string(),
        storage_key: "workflows/example/nativesdk.nupkg".to_string(),
        sha256: ArtifactDigest::parse_sha256("c".repeat(64)).expect("valid digest"),
        size_bytes: 42,
        media_type: Some("application/zip".to_string()),
    });
    let workflow = workflow(vec![build]);
    let json = serde_json::to_string_pretty(&workflow).expect("serialize workflow");
    let restored: CloudWorkflow = serde_json::from_str(&json).expect("restore workflow");
    assert_eq!(restored, workflow);
    assert_eq!(restored.validate(), Ok(()));
}

#[test]
fn retry_attempts_are_immutable_and_linked() {
    let first_id = CloudJobId::new();
    let second_id = CloudJobId::new();
    let mut first = node(first_id, "nativesdk-build", Vec::new());
    first.state = CloudJobState::Failed;
    first.retry.max_attempts = 2;
    let mut second = node(second_id, "nativesdk-build", Vec::new());
    second.attempt = 2;
    second.retry.max_attempts = 2;
    second.supersedes = Some(first_id);
    assert_eq!(workflow(vec![first, second.clone()]).validate(), Ok(()));

    second.logical_key = "different-job".to_string();
    assert_eq!(
        workflow(vec![node(first_id, "nativesdk-build", Vec::new()), second]).validate(),
        Err(CloudProtocolError::InvalidSupersededAttempt(second_id))
    );
}

#[test]
fn persisted_artifacts_reject_signed_urls() {
    let id = CloudJobId::new();
    let mut producer = node(id, "package", Vec::new());
    producer.outputs.push(ArtifactRef {
        artifact_id: "candidate".to_string(),
        storage_key: "https://storage.example/artifact?sig=secret".to_string(),
        sha256: ArtifactDigest::parse_sha256("d".repeat(64)).expect("valid digest"),
        size_bytes: 42,
        media_type: None,
    });
    assert_eq!(
        workflow(vec![producer]).validate(),
        Err(CloudProtocolError::InvalidArtifactRef(id))
    );
}

#[test]
fn persisted_artifacts_reject_parent_traversal() {
    let id = CloudJobId::new();
    let mut producer = node(id, "package", Vec::new());
    producer.outputs.push(ArtifactRef {
        artifact_id: "candidate".to_string(),
        storage_key: "workflows/../credentials".to_string(),
        sha256: ArtifactDigest::parse_sha256("e".repeat(64)).expect("valid digest"),
        size_bytes: 42,
        media_type: None,
    });
    assert_eq!(
        workflow(vec![producer]).validate(),
        Err(CloudProtocolError::InvalidArtifactRef(id))
    );
}

#[test]
fn job_state_machine_keeps_approval_and_cleanup_explicit() {
    assert!(CloudJobState::Running.permits(CloudJobState::WaitingForApproval));
    assert!(CloudJobState::WaitingForApproval.permits(CloudJobState::Running));
    assert!(!CloudJobState::WaitingForApproval.permits(CloudJobState::Provisioning));
    assert!(CloudJobState::Completed.permits(CloudJobState::Cleaning));
    assert!(CloudJobState::Cleaning.permits(CloudJobState::Cleaned));
    assert!(!CloudJobState::Cleaned.permits(CloudJobState::Running));
}
