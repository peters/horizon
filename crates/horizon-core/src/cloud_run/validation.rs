use std::collections::{HashMap, HashSet};

use super::{
    ApprovalDecision, ApprovalGate, ArtifactRef, CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudJobState, CloudProgress,
    CloudProtocolError, CloudWorkflow, CloudWorkflowId, WorkflowNode, WorkflowNodeKind,
};

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
        let mut artifact_ids = HashSet::new();
        for node in &self.nodes {
            validate_node(self.id, node, &nodes)?;
            for artifact in &node.outputs {
                validate_artifact(node.id, artifact)?;
                if !artifact_ids.insert(artifact.artifact_id.as_str()) {
                    return Err(CloudProtocolError::DuplicateArtifactId(artifact.artifact_id.clone()));
                }
            }
        }
        ensure_acyclic(&nodes)
    }
}

fn validate_node(
    workflow_id: CloudWorkflowId,
    node: &WorkflowNode,
    nodes: &HashMap<CloudJobId, &WorkflowNode>,
) -> Result<(), CloudProtocolError> {
    if node.logical_key.trim().is_empty() || node.label.trim().is_empty() {
        return Err(CloudProtocolError::EmptyNodeIdentity(node.id));
    }
    if let Some(source) = &node.source
        && !valid_repository(&source.repository)
    {
        return Err(CloudProtocolError::InvalidRepository(source.repository.clone()));
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
        validate_approval(node.id, approval, nodes)?;
    }
    if let Some(release) = &node.release {
        if !valid_repository(&release.repository) {
            return Err(CloudProtocolError::InvalidRepository(release.repository.clone()));
        }
        validate_approval(node.id, &release.approval, nodes)?;
    }
    if let Some(lease) = &node.environment_lease
        && (lease.environment.trim().is_empty()
            || lease.holder_workflow_id != workflow_id
            || lease.holder_job_id != node.id
            || lease.acquired_at_millis < 0
            || lease.expires_at_millis <= lease.acquired_at_millis)
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
        || node.attempt != previous.attempt.saturating_add(1)
        || !matches!(previous.state, CloudJobState::Failed | CloudJobState::Cancelled)
    {
        return Err(CloudProtocolError::InvalidSupersededAttempt(node.id));
    }
    Ok(())
}

fn validate_approval(
    node_id: CloudJobId,
    approval: &ApprovalGate,
    nodes: &HashMap<CloudJobId, &WorkflowNode>,
) -> Result<(), CloudProtocolError> {
    if approval.action.trim().is_empty() {
        return Err(CloudProtocolError::InvalidApprovalGate(node_id));
    }
    match &approval.decision {
        ApprovalDecision::Pending => {}
        ApprovalDecision::Approved {
            actor,
            decided_at_millis,
        } if !actor.trim().is_empty() && *decided_at_millis >= 0 => {}
        ApprovalDecision::Rejected {
            actor,
            decided_at_millis,
            reason,
        } if !actor.trim().is_empty() && *decided_at_millis >= 0 && !reason.trim().is_empty() => {}
        ApprovalDecision::Approved { .. } | ApprovalDecision::Rejected { .. } => {
            return Err(CloudProtocolError::InvalidApprovalGate(node_id));
        }
    }
    for evidence_id in &approval.evidence_job_ids {
        if !nodes.contains_key(evidence_id) {
            return Err(CloudProtocolError::MissingApprovalEvidence {
                node: node_id,
                evidence: *evidence_id,
            });
        }
    }
    Ok(())
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

fn ensure_acyclic(nodes: &HashMap<CloudJobId, &WorkflowNode>) -> Result<(), CloudProtocolError> {
    fn visit(
        id: CloudJobId,
        nodes: &HashMap<CloudJobId, &WorkflowNode>,
        visiting: &mut HashSet<CloudJobId>,
        visited: &mut HashSet<CloudJobId>,
    ) -> Result<(), CloudProtocolError> {
        if visited.contains(&id) {
            return Ok(());
        }
        if !visiting.insert(id) {
            return Err(CloudProtocolError::DependencyCycle(id));
        }
        if let Some(node) = nodes.get(&id) {
            for dependency in &node.depends_on {
                visit(*dependency, nodes, visiting, visited)?;
            }
        }
        visiting.remove(&id);
        visited.insert(id);
        Ok(())
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for id in nodes.keys().copied() {
        visit(id, nodes, &mut visiting, &mut visited)?;
    }
    Ok(())
}
