use super::{
    ApprovalDecision, ApprovalGate, ArtifactRef, CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudJobOutcome,
    CloudJobState, CloudProgress, CloudProtocolError, CloudWorkflow, ProvenanceRecord, WorkflowNode, WorkflowNodeKind,
};
use std::collections::{HashMap, HashSet};
impl ProvenanceRecord {
    /// Validate persisted source and artifact references for secret-free storage.
    ///
    /// # Errors
    ///
    /// Returns [`CloudProtocolError`] for an invalid repository, workflow URL,
    /// artifact key, or duplicate artifact identity.
    pub fn validate(&self) -> Result<(), CloudProtocolError> {
        if !valid_repository(&self.source.repository) {
            return Err(CloudProtocolError::InvalidRepository);
        }
        if self
            .workflow_run_url
            .as_deref()
            .is_some_and(|url| !valid_public_url(url))
        {
            return Err(CloudProtocolError::InvalidWorkflowRunUrl(self.producer_job_id));
        }
        let mut artifact_ids = HashSet::new();
        for artifact in &self.artifacts {
            validate_artifact(self.producer_job_id, artifact)?;
            if !artifact_ids.insert(artifact.artifact_id.as_str()) {
                return Err(CloudProtocolError::DuplicateArtifactId(artifact.artifact_id.clone()));
            }
        }
        Ok(())
    }
}
impl CloudWorkflow {
    /// Validate all persisted cross-node and security invariants.
    ///
    /// # Errors
    ///
    /// Returns [`CloudProtocolError`] when the protocol version, DAG, retry
    /// chain, gate, lease, worker, source, progress, or artifact contract is
    /// invalid.
    pub fn validate(&self) -> Result<(), CloudProtocolError> {
        if self.protocol_version != CLOUD_RUN_PROTOCOL_VERSION {
            return Err(CloudProtocolError::UnsupportedVersion(self.protocol_version));
        }
        if self.title.trim().is_empty() {
            return Err(CloudProtocolError::EmptyField("workflow.title"));
        }
        if self.retain_until_millis < self.created_at_millis {
            return Err(CloudProtocolError::InvalidRetention);
        }
        if self.created_at_millis < 0
            || self.updated_at_millis < self.created_at_millis
            || self.retain_until_millis < self.updated_at_millis
        {
            return Err(CloudProtocolError::InvalidWorkflowTimestamps);
        }
        let nodes: HashMap<_, _> = self.nodes.iter().map(|node| (node.id, node)).collect();
        if nodes.len() != self.nodes.len() {
            return Err(CloudProtocolError::DuplicateNodeId);
        }
        let mut artifact_producers = HashMap::new();
        let mut logical_attempts = HashSet::new();
        let mut superseded_attempts = HashSet::new();
        for node in &self.nodes {
            validate_node(self, node, &nodes)?;
            if let Some(previous_id) = node.supersedes
                && !superseded_attempts.insert(previous_id)
            {
                return Err(CloudProtocolError::ForkedRetryAttempt(previous_id));
            }
            if !logical_attempts.insert((node.logical_key.as_str(), node.attempt)) {
                return Err(CloudProtocolError::DuplicateLogicalAttempt {
                    logical_key: node.logical_key.clone(),
                    attempt: node.attempt,
                });
            }
            for artifact in &node.outputs {
                validate_artifact(node.id, artifact)?;
                if artifact_producers
                    .insert(artifact.artifact_id.as_str(), node.id)
                    .is_some()
                {
                    return Err(CloudProtocolError::DuplicateArtifactId(artifact.artifact_id.clone()));
                }
            }
        }
        for node in &self.nodes {
            validate_inputs(node, &artifact_producers)?;
        }
        ensure_acyclic(&nodes)
    }
}
fn validate_node(
    workflow: &CloudWorkflow,
    node: &WorkflowNode,
    nodes: &HashMap<CloudJobId, &WorkflowNode>,
) -> Result<(), CloudProtocolError> {
    if node.logical_key.trim().is_empty() || node.label.trim().is_empty() {
        return Err(CloudProtocolError::EmptyNodeIdentity(node.id));
    }
    if let Some(source) = &node.source
        && !valid_repository(&source.repository)
    {
        return Err(CloudProtocolError::InvalidRepository);
    }
    if let Some(worker) = &node.worker
        && (worker.profile.trim().is_empty()
            || worker.image.trim().is_empty()
            || worker.disk_gib == 0
            || worker.lease_seconds == 0)
    {
        return Err(CloudProtocolError::InvalidWorkerTarget(node.id));
    }
    if node.weight == 0 || node.attempt == 0 || node.retry.max_attempts == 0 || node.attempt > node.retry.max_attempts {
        return Err(CloudProtocolError::InvalidAttempt(node.id));
    }
    if !valid_outcome(node) {
        return Err(CloudProtocolError::InvalidJobOutcome(node.id));
    }
    validate_retry(node, nodes)?;
    if node.depends_on.contains(&node.id) {
        return Err(CloudProtocolError::SelfDependency(node.id));
    }
    for dependency in &node.depends_on {
        if !nodes.contains_key(dependency) {
            return Err(CloudProtocolError::MissingDependency {
                node: node.id,
                dependency: *dependency,
            });
        }
    }
    if let CloudProgress::Measured { completed, total, .. } = node.progress
        && (total == 0 || completed > total)
    {
        return Err(CloudProtocolError::InvalidProgress(node.id));
    }
    if node.kind == WorkflowNodeKind::Approval && node.approval.is_none() && node.release.is_none() {
        return Err(CloudProtocolError::MissingApprovalGate(node.id));
    }
    if let Some(approval) = &node.approval {
        validate_approval(workflow, node.id, approval, nodes)?;
    }
    if let Some(release) = &node.release {
        if !valid_repository(&release.repository) {
            return Err(CloudProtocolError::InvalidRepository);
        }
        validate_approval(workflow, node.id, &release.approval, nodes)?;
    }
    if let Some(lease) = &node.environment_lease
        && (lease.environment.trim().is_empty()
            || lease.holder_workflow_id != workflow.id
            || lease.holder_job_id != node.id
            || lease.acquired_at_millis < workflow.created_at_millis
            || lease.acquired_at_millis > workflow.updated_at_millis
            || lease.expires_at_millis <= lease.acquired_at_millis
            || lease.expires_at_millis > workflow.retain_until_millis)
    {
        return Err(CloudProtocolError::InvalidEnvironmentLease(node.id));
    }
    Ok(())
}
fn validate_retry(node: &WorkflowNode, nodes: &HashMap<CloudJobId, &WorkflowNode>) -> Result<(), CloudProtocolError> {
    let Some(previous_id) = node.supersedes else {
        return if node.attempt == 1 {
            Ok(())
        } else {
            Err(CloudProtocolError::InvalidSupersededAttempt(node.id))
        };
    };
    let Some(previous) = nodes.get(&previous_id) else {
        return Err(CloudProtocolError::MissingSupersededAttempt {
            node: node.id,
            previous: previous_id,
        });
    };
    if previous.logical_key != node.logical_key
        || previous.attempt.checked_add(1) != Some(node.attempt)
        || !matches!(
            previous.outcome,
            Some(CloudJobOutcome::Failed | CloudJobOutcome::Cancelled)
        )
    {
        return Err(CloudProtocolError::InvalidSupersededAttempt(node.id));
    }
    Ok(())
}
fn validate_approval(
    workflow: &CloudWorkflow,
    node_id: CloudJobId,
    approval: &ApprovalGate,
    nodes: &HashMap<CloudJobId, &WorkflowNode>,
) -> Result<(), CloudProtocolError> {
    if approval.action.trim().is_empty() {
        return Err(CloudProtocolError::InvalidApprovalGate(node_id));
    }
    let valid_time = |value| value >= workflow.created_at_millis && value <= workflow.updated_at_millis;
    match &approval.decision {
        ApprovalDecision::Pending => {}
        ApprovalDecision::Approved {
            actor,
            decided_at_millis,
        } if !actor.trim().is_empty() && valid_time(*decided_at_millis) => {}
        ApprovalDecision::Rejected {
            actor,
            decided_at_millis,
            reason,
        } if !actor.trim().is_empty() && valid_time(*decided_at_millis) && !reason.trim().is_empty() => {}
        ApprovalDecision::Approved { .. } | ApprovalDecision::Rejected { .. } => {
            return Err(CloudProtocolError::InvalidApprovalGate(node_id));
        }
    }
    for evidence_id in &approval.evidence_job_ids {
        if *evidence_id == node_id {
            return Err(CloudProtocolError::SelfApprovalEvidence(node_id));
        }
        if !nodes.contains_key(evidence_id) {
            return Err(CloudProtocolError::MissingApprovalEvidence {
                node: node_id,
                evidence: *evidence_id,
            });
        }
    }
    Ok(())
}
fn validate_inputs(
    node: &WorkflowNode,
    artifact_producers: &HashMap<&str, CloudJobId>,
) -> Result<(), CloudProtocolError> {
    let mut seen = HashSet::new();
    for artifact_id in &node.input_artifact_ids {
        let producer = artifact_producers.get(artifact_id.as_str());
        if artifact_id.trim().is_empty()
            || !seen.insert(artifact_id.as_str())
            || producer.is_none_or(|producer| !node.depends_on.contains(producer))
        {
            return Err(CloudProtocolError::InvalidInputArtifact(node.id));
        }
    }
    Ok(())
}
fn valid_outcome(node: &WorkflowNode) -> bool {
    use CloudJobOutcome::{Cancelled, Succeeded};
    use CloudJobState::{
        Cancelled as CancelledState, Checkpointing, Cleaned, Cleaning, Cloning, Completed, Failed, Provisioning,
        PullingImage, Queued, Running, WaitingForApproval,
    };
    matches!(
        (node.state, node.outcome),
        (
            Queued | Provisioning | PullingImage | Cloning | Running | Checkpointing | WaitingForApproval,
            None
        ) | (Completed, Some(Succeeded))
            | (CancelledState, Some(Cancelled))
            | (Failed | Cleaning | Cleaned, Some(_))
    )
}
fn validate_artifact(node_id: CloudJobId, artifact: &ArtifactRef) -> Result<(), CloudProtocolError> {
    let key = artifact.storage_key.trim();
    if artifact.artifact_id.trim().is_empty()
        || key.is_empty()
        || key.starts_with('/')
        || key.contains("://")
        || key.contains(['?', '#'])
        || key.contains('\\')
        || key.split('/').any(|segment| matches!(segment, "" | "." | ".."))
        || key.chars().any(char::is_control)
    {
        return Err(CloudProtocolError::InvalidArtifactRef(node_id));
    }
    Ok(())
}
fn valid_repository(repository: &str) -> bool {
    let mut segments = repository.split('/');
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && !matches!(segment, "." | "..")
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(owner), Some(name), None) if valid_segment(owner) && valid_segment(name)
    )
}
fn valid_public_url(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !value.contains(['?', '#', '\\'])
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}
fn ensure_acyclic(nodes: &HashMap<CloudJobId, &WorkflowNode>) -> Result<(), CloudProtocolError> {
    let mut remaining: HashMap<_, _> = nodes.iter().map(|(id, node)| (*id, node.depends_on.len())).collect();
    let mut dependents: HashMap<CloudJobId, Vec<CloudJobId>> = HashMap::new();
    for (id, node) in nodes {
        for dependency in &node.depends_on {
            dependents.entry(*dependency).or_default().push(*id);
        }
    }
    let mut ready: Vec<_> = remaining
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect();
    let mut visited = 0;
    while let Some(id) = ready.pop() {
        visited += 1;
        for dependent in dependents.get(&id).into_iter().flatten() {
            if let Some(count) = remaining.get_mut(dependent) {
                *count -= 1;
                if *count == 0 {
                    ready.push(*dependent);
                }
            }
        }
    }
    if visited == nodes.len() {
        return Ok(());
    }
    remaining
        .into_iter()
        .find_map(|(id, count)| (count > 0).then_some(CloudProtocolError::DependencyCycle(id)))
        .map_or(Ok(()), Err)
}
