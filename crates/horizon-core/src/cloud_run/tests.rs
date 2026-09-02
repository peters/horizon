use super::*;
fn sha(character: char) -> GitCommitSha {
    GitCommitSha::parse(character.to_string().repeat(40)).expect("valid test sha")
}
fn artifact(id: &str, key: &str) -> ArtifactRef {
    ArtifactRef {
        artifact_id: id.to_string(),
        storage_key: key.to_string(),
        sha256: ArtifactDigest::parse_sha256("d".repeat(64)).expect("valid digest"),
        size_bytes: 42,
        media_type: None,
    }
}
fn node(id: CloudJobId, key: &str, depends_on: Vec<CloudJobId>) -> WorkflowNode {
    WorkflowNode {
        id,
        logical_key: key.to_string(),
        label: key.to_string(),
        kind: WorkflowNodeKind::Build,
        state: CloudJobState::Queued,
        outcome: None,
        progress: CloudProgress::Pending,
        weight: 1,
        attempt: 1,
        retry: RetryPolicy::default(),
        supersedes: None,
        depends_on,
        source: None,
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
fn invalid(nodes: Vec<WorkflowNode>) -> CloudProtocolError {
    workflow(nodes).validate().expect_err("invalid workflow")
}
fn assert_invalid(nodes: Vec<WorkflowNode>, expected: &CloudProtocolError) {
    assert_eq!(&invalid(nodes), expected);
}
fn approved(at_millis: i64) -> ApprovalDecision {
    ApprovalDecision::Approved {
        actor: "release-owner".to_string(),
        decided_at_millis: at_millis,
    }
}
#[test]
fn dependency_validation_is_iterative_and_detects_cycles() {
    let mut nodes = Vec::with_capacity(4_096);
    let mut previous = None;
    for index in 0..4_096 {
        let id = CloudJobId::new();
        nodes.push(node(id, &format!("job-{index}"), previous.into_iter().collect()));
        previous = Some(id);
    }
    assert_eq!(workflow(nodes).validate(), Ok(()));
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    assert!(matches!(
        invalid(vec![node(first, "first", vec![second]), node(second, "second", vec![first])]),
        CloudProtocolError::DependencyCycle(id) if id == first || id == second
    ));
}
#[test]
fn approvals_and_leases_obey_snapshot_time_and_identity() {
    let id = CloudJobId::new();
    let mut gate = node(id, "publish-test", Vec::new());
    gate.state = CloudJobState::WaitingForApproval;
    assert_invalid(vec![gate.clone()], &CloudProtocolError::MissingApprovalGate(id));
    gate.kind = WorkflowNodeKind::Approval;
    gate.approval = Some(ApprovalGate {
        action: "Publish To Test".to_string(),
        decision: ApprovalDecision::Pending,
        evidence_job_ids: vec![id],
    });
    assert_invalid(vec![gate.clone()], &CloudProtocolError::SelfApprovalEvidence(id));
    let approval = gate.approval.as_mut().expect("gate exists");
    approval.evidence_job_ids.clear();
    approval.decision = approved(999);
    assert_invalid(vec![gate.clone()], &CloudProtocolError::InvalidApprovalGate(id));
    gate.approval.as_mut().expect("gate exists").decision = approved(1_500);
    assert_invalid(vec![gate.clone()], &CloudProtocolError::InvalidApprovalGate(id));
    gate.state = CloudJobState::Completed;
    gate.outcome = Some(CloudJobOutcome::Succeeded);
    gate.progress = CloudProgress::Completed;
    gate.approval.as_mut().expect("gate exists").decision = ApprovalDecision::Pending;
    assert_invalid(vec![gate.clone()], &CloudProtocolError::InvalidApprovalGate(id));
    gate.approval.as_mut().expect("gate exists").decision = approved(1_500);
    let mut snapshot = workflow(vec![gate]);
    snapshot.nodes[0].environment_lease = Some(EnvironmentLease {
        environment: "youpark-test".to_string(),
        holder_workflow_id: snapshot.id,
        holder_job_id: id,
        acquired_at_millis: 1_100,
        expires_at_millis: snapshot.retain_until_millis + 1,
    });
    assert_eq!(
        snapshot.validate().expect_err("lease outlives workflow"),
        CloudProtocolError::InvalidEnvironmentLease(id)
    );
}
#[test]
fn provenance_rejects_secrets_without_echoing_them() {
    let id = CloudJobId::new();
    let mut record = ProvenanceRecord {
        producer_job_id: id,
        source: GitSource {
            repository: "fintermobilityas/nativesdk".to_string(),
            commit: sha('b'),
            branch: None,
        },
        image_digest: None,
        workflow_run_url: Some("https://github.com/peters/horizon/actions/runs/1".to_string()),
        published_version: None,
        artifacts: vec![artifact("candidate", "workflows/candidate.nupkg")],
    };
    assert_eq!(record.validate(), Ok(()));
    record.source.repository = "https://user:secret@example.test/repo".to_string();
    let error = record.validate().expect_err("credential-bearing repository");
    assert_eq!(error, CloudProtocolError::InvalidRepository);
    assert!(!error.to_string().contains("secret"));
    record.source.repository = "fintermobilityas/nativesdk".to_string();
    for url in "https://u:p@e.test/r https://e.test/r?q=x https://: https://[invalid".split_ascii_whitespace() {
        record.workflow_run_url = Some(url.to_string());
        assert_eq!(record.validate(), Err(CloudProtocolError::InvalidWorkflowRunUrl(id)));
    }
    record.workflow_run_url = None;
    for key in ["https://e.test/a?x", "C:/outside", "C:relative", "artifact\n"] {
        record.artifacts[0].storage_key = key.to_string();
        assert_eq!(record.validate(), Err(CloudProtocolError::InvalidArtifactRef(id)));
    }
}
#[test]
fn job_state_transition_matrix_is_stable() {
    let json = r#"["queued","provisioning","pulling_image","cloning","running","checkpointing","waiting_for_approval","completed","failed","cancelled","cleaning","cleaned"]"#;
    let states: Vec<CloudJobState> = serde_json::from_str(json).expect("valid state table");
    // Each bit corresponds to the state at the same index, covering every allowed and forbidden edge.
    let allowed = [
        0x242_u16, 0x31c, 0x318, 0x310, 0x3e0, 0x390, 0x391, 0x400, 0x400, 0x400, 0x900, 0,
    ];
    for (from, mask) in states.iter().copied().zip(allowed) {
        for (bit, to) in states.iter().copied().enumerate() {
            assert_eq!(from.permits(to), mask & (1 << bit) != 0, "{from:?}->{to:?}");
        }
    }
}
#[test]
fn v1_json_contract_round_trips() {
    type V1Contract = (CloudWorkflow, ProvenanceRecord);
    let golden = include_str!("v1_contract.json").trim();
    let (mut snapshot, record): V1Contract = serde_json::from_str(golden).expect("valid v1 contract");
    assert_eq!((snapshot.validate(), record.validate()), (Ok(()), Ok(())));
    assert_eq!(
        serde_json::to_string(&(&snapshot, &record)).expect("serialize v1"),
        golden
    );
    snapshot.nodes[8].release = snapshot.nodes[4].release.clone();
    let error = CloudProtocolError::InvalidApprovalGate(snapshot.nodes[8].id);
    assert_eq!(snapshot.validate(), Err(error));
}
#[test]
fn inputs_are_unique_outputs_of_direct_dependencies() {
    let producer_id = CloudJobId::new();
    let consumer_id = CloudJobId::new();
    let mut producer = node(producer_id, "package", Vec::new());
    producer.outputs.push(artifact("candidate", "workflow/candidate"));
    let mut consumer = node(consumer_id, "test", vec![producer_id]);
    consumer.input_artifact_ids.push("candidate".to_string());
    assert_eq!(workflow(vec![producer.clone(), consumer.clone()]).validate(), Ok(()));
    for inputs in [
        vec![String::new()],
        vec!["candidate".to_string(); 2],
        vec!["missing".to_string()],
    ] {
        consumer.input_artifact_ids = inputs;
        assert_invalid(
            vec![producer.clone(), consumer.clone()],
            &CloudProtocolError::InvalidInputArtifact(consumer_id),
        );
    }
    consumer.input_artifact_ids = vec!["candidate".to_string()];
    consumer.depends_on.clear();
    assert_invalid(
        vec![producer, consumer],
        &CloudProtocolError::InvalidInputArtifact(consumer_id),
    );
}
#[test]
fn retries_are_unambiguous_and_keep_outcomes_through_cleanup() {
    let first = CloudJobId::new();
    let mut previous = node(first, "nativesdk-build", Vec::new());
    previous.state = CloudJobState::Cleaned;
    previous.outcome = Some(CloudJobOutcome::Failed);
    previous.weight = 100;
    previous.retry.max_attempts = 2;
    let mut retry = node(CloudJobId::new(), "nativesdk-build", Vec::new());
    retry.attempt = 2;
    retry.retry.max_attempts = 2;
    retry.supersedes = Some(first);
    assert_eq!(workflow(vec![previous.clone(), retry.clone()]).validate(), Ok(()));
    retry.progress = CloudProgress::Completed;
    assert_invalid(
        vec![previous.clone(), retry.clone()],
        &CloudProtocolError::InvalidProgress(retry.id),
    );
    retry.progress = CloudProgress::Pending;
    let mut changed_policy = retry.clone();
    changed_policy.retry.max_attempts = 3;
    assert_invalid(
        vec![previous.clone(), changed_policy],
        &CloudProtocolError::InvalidSupersededAttempt(retry.id),
    );
    let mut completed = retry.clone();
    completed.state = CloudJobState::Completed;
    completed.outcome = Some(CloudJobOutcome::Succeeded);
    completed.progress = CloudProgress::Measured {
        phase: "compile".to_string(),
        completed: 1,
        total: 2,
        unit: ProgressUnit::Steps,
    };
    assert_invalid(
        vec![previous.clone(), completed.clone()],
        &CloudProtocolError::InvalidProgress(completed.id),
    );
    completed.progress = CloudProgress::Completed;
    assert_eq!(
        workflow(vec![previous.clone(), completed]).progress().basis_points,
        10_000
    );
    assert!(matches!(
        invalid(vec![
            previous.clone(),
            node(CloudJobId::new(), "nativesdk-build", Vec::new())
        ]),
        CloudProtocolError::DuplicateLogicalAttempt { attempt: 1, .. }
    ));
    let mut fork = retry.clone();
    fork.id = CloudJobId::new();
    assert_invalid(
        vec![previous.clone(), retry, fork],
        &CloudProtocolError::ForkedRetryAttempt(first),
    );
    previous.attempt = u16::MAX;
    previous.retry.max_attempts = u16::MAX;
    let mut overflow = node(CloudJobId::new(), "nativesdk-build", Vec::new());
    overflow.attempt = u16::MAX;
    overflow.retry.max_attempts = u16::MAX;
    overflow.supersedes = Some(first);
    assert_invalid(
        vec![overflow.clone(), previous],
        &CloudProtocolError::InvalidSupersededAttempt(overflow.id),
    );
}
