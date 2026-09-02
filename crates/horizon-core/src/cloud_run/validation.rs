use super::{
    ApprovalDecision, ApprovalGate, ArtifactRef, CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudJobOutcome,
    CloudJobState, CloudProgress, CloudProtocolError as Error, CloudWorkflow, GitSource, ProvenanceRecord,
    WorkflowNode, WorkflowNodeKind,
};
use std::collections::{HashMap, HashSet};
use url::Url;
fn ensure(valid: bool, error: Error) -> Result<(), Error> {
    valid.then_some(()).ok_or(error)
}
impl ProvenanceRecord {
    /// Validate persisted source and artifact references for secret-free storage.
    /// # Errors
    /// Rejects invalid repositories, workflow URLs, and artifact identities.
    pub fn validate(&self) -> Result<(), Error> {
        validate_git_source(&self.source)?;
        ensure(
            self.workflow_run_url.as_deref().is_none_or(valid_public_url),
            Error::InvalidWorkflowRunUrl(self.producer_job_id),
        )?;
        let mut artifact_ids = HashSet::new();
        for artifact in &self.artifacts {
            validate_artifact(self.producer_job_id, artifact)?;
            if !artifact_ids.insert(artifact.artifact_id.as_str()) {
                return Err(Error::DuplicateArtifactId(artifact.artifact_id.clone()));
            }
        }
        Ok(())
    }
}
impl CloudWorkflow {
    /// Validate all persisted cross-node and security invariants.
    /// # Errors
    /// Rejects snapshots with any invalid persisted protocol invariant.
    pub fn validate(&self) -> Result<(), Error> {
        ensure(
            self.protocol_version == CLOUD_RUN_PROTOCOL_VERSION,
            Error::UnsupportedVersion(self.protocol_version),
        )?;
        ensure(!self.title.trim().is_empty(), Error::EmptyField("workflow.title"))?;
        ensure(
            self.retain_until_millis >= self.created_at_millis,
            Error::InvalidRetention,
        )?;
        if self.created_at_millis < 0
            || self.updated_at_millis < self.created_at_millis
            || self.retain_until_millis < self.updated_at_millis
        {
            return Err(Error::InvalidWorkflowTimestamps);
        }
        let nodes: HashMap<_, _> = self.nodes.iter().map(|node| (node.id, node)).collect();
        ensure(nodes.len() == self.nodes.len(), Error::DuplicateNodeId)?;
        let mut artifact_producers = HashMap::new();
        let mut logical_attempts = HashSet::new();
        let mut superseded_attempts = HashSet::new();
        for node in &self.nodes {
            validate_node(self, node, &nodes)?;
            if let Some(previous_id) = node.supersedes
                && !superseded_attempts.insert(previous_id)
            {
                return Err(Error::ForkedRetryAttempt(previous_id));
            }
            if !logical_attempts.insert((node.logical_key.as_str(), node.attempt)) {
                return Err(Error::DuplicateLogicalAttempt {
                    logical_key: node.logical_key.clone(),
                    attempt: node.attempt,
                });
            }
            for artifact in &node.outputs {
                validate_artifact(node.id, artifact)?;
                let duplicate = artifact_producers
                    .insert(artifact.artifact_id.as_str(), node.id)
                    .is_some();
                if duplicate {
                    return Err(Error::DuplicateArtifactId(artifact.artifact_id.clone()));
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
) -> Result<(), Error> {
    if node.logical_key.trim().is_empty() || node.label.trim().is_empty() {
        return Err(Error::EmptyNodeIdentity(node.id));
    }
    if let Some(source) = &node.source {
        validate_git_source(source)?;
    }
    if let Some(worker) = &node.worker
        && (worker.profile.trim().is_empty()
            || !valid_worker_image(&worker.image)
            || worker.disk_gib == 0
            || worker.lease_seconds == 0)
    {
        return Err(Error::InvalidWorkerTarget(node.id));
    }
    if node.weight == 0 || node.attempt == 0 || node.retry.max_attempts == 0 || node.attempt > node.retry.max_attempts {
        return Err(Error::InvalidAttempt(node.id));
    }
    ensure(valid_outcome(node), Error::InvalidJobOutcome(node.id))?;
    validate_retry(node, nodes)?;
    ensure(!node.depends_on.contains(&node.id), Error::InvalidDependency(node.id))?;
    for dependency in &node.depends_on {
        if !nodes.contains_key(dependency) {
            return Err(Error::MissingDependency {
                node: node.id,
                dependency: *dependency,
            });
        }
    }
    ensure(valid_progress(node), Error::InvalidProgress(node.id))?;
    ensure(
        node.approval.is_none() || node.release.is_none(),
        Error::InvalidApprovalGate(node.id),
    )?;
    let needs_approval = node.kind == WorkflowNodeKind::Approval || node.state == CloudJobState::WaitingForApproval;
    if needs_approval && node.approval.is_none() && node.release.is_none() {
        return Err(Error::MissingApprovalGate(node.id));
    }
    if let Some(approval) = &node.approval {
        validate_approval(workflow, node, approval, nodes)?;
    }
    if let Some(release) = &node.release {
        ensure(valid_repository(&release.repository), Error::InvalidRepository)?;
        validate_approval(workflow, node, &release.approval, nodes)?;
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
        return Err(Error::InvalidEnvironmentLease(node.id));
    }
    Ok(())
}
fn validate_retry(node: &WorkflowNode, nodes: &HashMap<CloudJobId, &WorkflowNode>) -> Result<(), Error> {
    let Some(previous_id) = node.supersedes else {
        return ensure(node.attempt == 1, Error::InvalidSupersededAttempt(node.id));
    };
    let Some(previous) = nodes.get(&previous_id) else {
        return Err(Error::MissingSupersededAttempt {
            node: node.id,
            previous: previous_id,
        });
    };
    if previous.logical_key != node.logical_key
        || previous.retry != node.retry
        || previous.attempt.checked_add(1) != Some(node.attempt)
        || !matches!(
            previous.outcome,
            Some(CloudJobOutcome::Failed | CloudJobOutcome::Cancelled)
        )
    {
        return Err(Error::InvalidSupersededAttempt(node.id));
    }
    Ok(())
}
fn validate_approval(
    workflow: &CloudWorkflow,
    node: &WorkflowNode,
    approval: &ApprovalGate,
    nodes: &HashMap<CloudJobId, &WorkflowNode>,
) -> Result<(), Error> {
    let node_id = node.id;
    ensure(!approval.action.trim().is_empty(), Error::InvalidApprovalGate(node_id))?;
    let valid_time = |value| value >= workflow.created_at_millis && value <= workflow.updated_at_millis;
    let approved = matches!(approval.decision, ApprovalDecision::Approved { .. });
    match &approval.decision {
        ApprovalDecision::Pending if node.outcome != Some(CloudJobOutcome::Succeeded) => {}
        ApprovalDecision::Approved {
            actor,
            decided_at_millis,
        } if !actor.trim().is_empty()
            && valid_time(*decided_at_millis)
            && node.state != CloudJobState::WaitingForApproval => {}
        ApprovalDecision::Rejected {
            actor,
            decided_at_millis,
            reason,
        } if !actor.trim().is_empty()
            && valid_time(*decided_at_millis)
            && !reason.trim().is_empty()
            && matches!(node.outcome, Some(CloudJobOutcome::Failed | CloudJobOutcome::Cancelled)) => {}
        ApprovalDecision::Pending | ApprovalDecision::Approved { .. } | ApprovalDecision::Rejected { .. } => {
            return Err(Error::InvalidApprovalGate(node_id));
        }
    }
    for evidence_id in &approval.evidence_job_ids {
        ensure(*evidence_id != node_id, Error::SelfApprovalEvidence(node_id))?;
        let Some(evidence) = nodes.get(evidence_id) else {
            return Err(Error::InvalidApprovalEvidence {
                node: node_id,
                evidence: *evidence_id,
            });
        };
        let terminal = matches!(
            evidence.state,
            CloudJobState::Completed
                | CloudJobState::Failed
                | CloudJobState::Cancelled
                | CloudJobState::Cleaning
                | CloudJobState::Cleaned
        );
        ensure(
            node.depends_on.contains(evidence_id)
                && terminal
                && evidence.outcome.is_some()
                && (!approved || evidence.outcome == Some(CloudJobOutcome::Succeeded)),
            Error::InvalidApprovalEvidence {
                node: node_id,
                evidence: *evidence_id,
            },
        )?;
    }
    Ok(())
}
fn valid_progress(node: &WorkflowNode) -> bool {
    let within_bounds = match node.progress {
        CloudProgress::Measured { completed, total, .. } => total > 0 && completed <= total,
        CloudProgress::Pending | CloudProgress::Indeterminate { .. } | CloudProgress::Completed => true,
    };
    let successful = node.outcome == Some(CloudJobOutcome::Succeeded);
    within_bounds
        && (!matches!(node.progress, CloudProgress::Completed) || successful)
        && (!successful || node.progress.basis_points() == Some(10_000))
}
fn validate_inputs(node: &WorkflowNode, artifact_producers: &HashMap<&str, CloudJobId>) -> Result<(), Error> {
    let mut seen = HashSet::new();
    let dependencies: HashSet<_> = node.depends_on.iter().copied().collect();
    ensure(
        dependencies.len() == node.depends_on.len(),
        Error::InvalidDependency(node.id),
    )?;
    for artifact_id in &node.input_artifact_ids {
        let producer = artifact_producers.get(artifact_id.as_str());
        if artifact_id.trim().is_empty()
            || !seen.insert(artifact_id.as_str())
            || producer.is_none_or(|producer| !dependencies.contains(producer))
        {
            return Err(Error::InvalidInputArtifact(node.id));
        }
    }
    Ok(())
}
fn valid_outcome(node: &WorkflowNode) -> bool {
    use CloudJobOutcome::{Cancelled, Failed as FailedOutcome, Succeeded};
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
            | (Failed, Some(FailedOutcome))
            | (Cleaning | Cleaned, Some(_))
    )
}
fn validate_artifact(node_id: CloudJobId, artifact: &ArtifactRef) -> Result<(), Error> {
    let key = artifact.storage_key.trim();
    if artifact.artifact_id.trim().is_empty()
        || key.is_empty()
        || key.starts_with('/')
        || key.as_bytes().get(1) == Some(&b':') && key.as_bytes()[0].is_ascii_alphabetic()
        || Url::parse(key).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        || key.contains(['?', '#'])
        || key.contains('\\')
        || key.contains('%')
        || key.split('/').any(|segment| matches!(segment, "" | "." | ".."))
        || artifact.storage_key.chars().any(char::is_control)
    {
        return Err(Error::InvalidArtifactRef(node_id));
    }
    Ok(())
}
fn valid_worker_image(value: &str) -> bool {
    let digest = value.split_once('@').map(|(_, digest)| digest);
    !value.is_empty()
        && !value.starts_with('@')
        && !value.contains("://")
        && !value.contains(['?', '#', '\\'])
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && digest.is_none_or(|digest| {
            digest
                .strip_prefix("sha256:")
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
}
fn validate_git_source(source: &GitSource) -> Result<(), Error> {
    ensure(valid_repository(&source.repository), Error::InvalidRepository)?;
    ensure(
        source.branch.as_deref().is_none_or(valid_branch),
        Error::InvalidGitBranch,
    )
}
fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "@"
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !["..", "@{", "//"].iter().any(|needle| branch.contains(needle))
        && !branch
            .chars()
            .any(|character| character.is_control() || character.is_whitespace() || "~^:?*[\\".contains(character))
        && branch
            .split('/')
            .all(|segment| !segment.starts_with('.') && !segment.as_bytes().ends_with(b".lock"))
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
    !value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
        && !value.contains('\\')
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
}
fn ensure_acyclic(nodes: &HashMap<CloudJobId, &WorkflowNode>) -> Result<(), Error> {
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
        .find_map(|(id, count)| (count > 0).then_some(Error::DependencyCycle(id)))
        .map_or(Ok(()), Err)
}
