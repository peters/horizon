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
fn node(id: CloudJobId, logical_key: &str, depends_on: Vec<CloudJobId>) -> WorkflowNode {
    WorkflowNode {
        id,
        logical_key: logical_key.to_string(),
        label: logical_key.to_string(),
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
#[test]
fn workflow_validates_deep_dags_without_recursion() {
    let mut nodes = Vec::with_capacity(4_096);
    let mut previous = None;
    for index in 0..4_096 {
        let id = CloudJobId::new();
        nodes.push(node(id, &format!("job-{index}"), previous.into_iter().collect()));
        previous = Some(id);
    }
    assert_eq!(workflow(nodes).validate(), Ok(()));
}
#[test]
fn workflow_rejects_dependency_cycles() {
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    assert!(matches!(
        workflow(vec![node(first, "first", vec![second]), node(second, "second", vec![first])]).validate(),
        Err(CloudProtocolError::DependencyCycle(id)) if id == first || id == second
    ));
}
#[test]
fn approvals_and_environment_leases_are_bound_to_their_node() {
    let id = CloudJobId::new();
    let mut gate = node(id, "publish-test", Vec::new());
    gate.kind = WorkflowNodeKind::Approval;
    gate.state = CloudJobState::WaitingForApproval;
    assert_eq!(
        workflow(vec![gate.clone()]).validate(),
        Err(CloudProtocolError::MissingApprovalGate(id))
    );
    gate.approval = Some(ApprovalGate {
        action: "Publish To Test".to_string(),
        decision: ApprovalDecision::Pending,
        evidence_job_ids: vec![id],
    });
    assert_eq!(
        workflow(vec![gate.clone()]).validate(),
        Err(CloudProtocolError::SelfApprovalEvidence(id))
    );
    gate.approval.as_mut().expect("gate exists").evidence_job_ids.clear();
    let mut workflow = workflow(vec![gate]);
    workflow.nodes[0].environment_lease = Some(EnvironmentLease {
        environment: "youpark-test".to_string(),
        holder_workflow_id: workflow.id,
        holder_job_id: id,
        acquired_at_millis: 10,
        expires_at_millis: 20,
    });
    assert_eq!(workflow.validate(), Ok(()));
}
#[test]
fn progress_counts_only_latest_retry_attempts() {
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    let mut old = node(first, "image-pull", Vec::new());
    old.state = CloudJobState::Cleaned;
    old.outcome = Some(CloudJobOutcome::Failed);
    old.progress = CloudProgress::Completed;
    old.weight = 100;
    old.retry.max_attempts = 2;
    let mut retry = node(second, "image-pull", Vec::new());
    retry.state = CloudJobState::PullingImage;
    retry.progress = CloudProgress::Measured {
        phase: "pulling_image".to_string(),
        completed: 50,
        total: 100,
        unit: ProgressUnit::Bytes,
    };
    retry.weight = 3;
    retry.attempt = 2;
    retry.retry.max_attempts = 2;
    retry.supersedes = Some(first);
    let workflow = workflow(vec![old, retry]);
    assert_eq!(workflow.validate(), Ok(()));
    assert_eq!(
        workflow.progress(),
        WorkflowProgress {
            basis_points: 5_000,
            estimated: false
        }
    );
}
#[test]
fn provenance_is_round_trippable_and_secret_free() {
    let id = CloudJobId::new();
    let source = GitSource {
        repository: "fintermobilityas/nativesdk".to_string(),
        commit: sha('b'),
        branch: Some("feature/plate-speedups".to_string()),
    };
    let output = artifact("candidate", "workflows/example/nativesdk.nupkg");
    let mut provenance = ProvenanceRecord {
        producer_job_id: id,
        source,
        image_digest: None,
        workflow_run_url: None,
        published_version: None,
        artifacts: vec![output],
    };
    assert_eq!(provenance.validate(), Ok(()));
    let json = serde_json::to_string(&provenance).expect("serialize provenance");
    assert_eq!(
        serde_json::from_str::<ProvenanceRecord>(&json).expect("restore provenance"),
        provenance
    );
    assert!(serde_json::from_str::<GitCommitSha>(r#""short""#).is_err());
    assert!(ArtifactDigest::parse_sha256("short").is_err());
    provenance.source.repository = "https://user:secret@example.test/repo".to_string();
    assert!(matches!(
        provenance.validate(),
        Err(CloudProtocolError::InvalidRepository(_))
    ));
    provenance.source.repository = "fintermobilityas/nativesdk".to_string();
    for key in [
        "https://storage.example/artifact?sig=secret",
        "workflows/../credentials",
    ] {
        provenance.artifacts[0].storage_key = key.to_string();
        assert_eq!(provenance.validate(), Err(CloudProtocolError::InvalidArtifactRef(id)));
    }
}
#[test]
fn retries_preserve_outcomes_and_reject_ambiguous_chains() {
    let first = CloudJobId::new();
    let second = CloudJobId::new();
    let mut previous = node(first, "nativesdk-build", Vec::new());
    previous.state = CloudJobState::Cleaned;
    previous.outcome = Some(CloudJobOutcome::Failed);
    previous.retry.max_attempts = 2;
    let mut retry = node(second, "nativesdk-build", Vec::new());
    retry.attempt = 2;
    retry.retry.max_attempts = 2;
    retry.supersedes = Some(first);
    assert_eq!(workflow(vec![previous.clone(), retry.clone()]).validate(), Ok(()));

    let duplicate = node(CloudJobId::new(), "nativesdk-build", Vec::new());
    assert!(matches!(
        workflow(vec![previous.clone(), duplicate]).validate(),
        Err(CloudProtocolError::DuplicateLogicalAttempt { attempt: 1, .. })
    ));
    let mut fork = retry.clone();
    fork.id = CloudJobId::new();
    assert_eq!(
        workflow(vec![previous.clone(), retry, fork]).validate(),
        Err(CloudProtocolError::ForkedRetryAttempt(first))
    );

    previous.attempt = u16::MAX;
    previous.retry.max_attempts = u16::MAX;
    let mut overflow = node(CloudJobId::new(), "nativesdk-build", Vec::new());
    overflow.attempt = u16::MAX;
    overflow.retry.max_attempts = u16::MAX;
    overflow.supersedes = Some(first);
    assert_eq!(
        workflow(vec![overflow.clone(), previous]).validate(),
        Err(CloudProtocolError::InvalidSupersededAttempt(overflow.id))
    );
    let mut completed = node(CloudJobId::new(), "completed", Vec::new());
    completed.state = CloudJobState::Completed;
    assert_eq!(
        workflow(vec![completed.clone()]).validate(),
        Err(CloudProtocolError::InvalidJobOutcome(completed.id))
    );
}
