//! Provider-neutral, persistable protocol for durable cloud workflows, without
//! provider clients or process handles tied to a Horizon session.

use std::{collections::HashSet, fmt};

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;
use uuid::Uuid;
mod validation;
pub const CLOUD_RUN_PROTOCOL_VERSION: u32 = 1;
macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}
uuid_id!(CloudWorkflowId);
uuid_id!(CloudJobId);
macro_rules! string_enum {
    ($name:ident: $($variant:ident),+ $(,)?) => {
        #[doc = concat!("Stable `snake_case` wire values for `", stringify!($name), "`.")]
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
string_enum!(CloudProvider: Azure, RunPod);
macro_rules! hex_value {
    ($name:ident => $parser:ident; $length:literal; $error:ident; $description:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            #[doc = $description]
            /// # Errors
            #[doc = concat!("Rejects values that are not exactly ", stringify!($length), " hexadecimal characters.")]
            pub fn $parser(value: impl Into<String>) -> Result<Self, CloudProtocolError> {
                let value = value.into();
                if value.len() != $length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(CloudProtocolError::$error(value));
                }
                Ok(Self(value.to_ascii_lowercase()))
            }
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::$parser(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerTarget {
    pub provider: CloudProvider,
    pub profile: String,
    pub image: String,
    pub disk_gib: u32,
    pub lease_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_hourly_cost_micros: Option<u64>,
}
hex_value!(GitCommitSha => parse; 40; InvalidGitCommit; "Parse an exact Git commit SHA.");
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitSource {
    /// Repository identity without credentials, normally `owner/name`.
    pub repository: String,
    pub commit: GitCommitSha,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}
hex_value!(ArtifactDigest => parse_sha256; 64; InvalidSha256; "Parse a SHA-256 digest.");
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    /// Opaque control-plane key. Signed download URLs are never persisted.
    pub storage_key: String,
    pub sha256: ArtifactDigest,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub producer_job_id: CloudJobId,
    pub source: GitSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<ArtifactDigest>,
    /// Credential-free HTTPS URL without query or fragment data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
}
string_enum!(ProgressUnit: Bytes, Tests, Jobs, Steps);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloudProgress {
    Pending,
    Indeterminate {
        phase: String,
        message: String,
    },
    Measured {
        phase: String,
        completed: u64,
        total: u64,
        unit: ProgressUnit,
    },
    Completed,
}
impl CloudProgress {
    #[must_use]
    pub fn basis_points(&self) -> Option<u16> {
        match self {
            Self::Measured { completed, total, .. } if *total > 0 => {
                let bounded = (*completed).min(*total);
                let scaled = u128::from(bounded) * 10_000 / u128::from(*total);
                u16::try_from(scaled).ok()
            }
            Self::Completed => Some(10_000),
            Self::Pending | Self::Indeterminate { .. } | Self::Measured { .. } => None,
        }
    }
}
string_enum!(CloudJobState: Queued, Provisioning, PullingImage, Cloning, Running, Checkpointing, WaitingForApproval, Completed, Failed, Cancelled, Cleaning, Cleaned);
impl CloudJobState {
    #[must_use]
    pub const fn permits(self, next: Self) -> bool {
        use CloudJobState::{
            Cancelled, Checkpointing, Cleaned, Cleaning, Cloning, Completed, Failed, Provisioning, PullingImage,
            Queued, Running, WaitingForApproval,
        };
        matches!(
            (self, next),
            (Queued, Provisioning | WaitingForApproval | Cancelled)
                | (Provisioning, PullingImage | Cloning | Running | Failed | Cancelled)
                | (PullingImage, Cloning | Running | Failed | Cancelled)
                | (Cloning, Running | Failed | Cancelled)
                | (
                    Running,
                    Checkpointing | WaitingForApproval | Completed | Failed | Cancelled
                )
                | (Checkpointing, Running | Completed | Failed | Cancelled)
                | (WaitingForApproval, Queued | Running | Completed | Failed | Cancelled)
                | (Completed | Failed | Cancelled, Cleaning)
                | (Cleaning, Cleaned | Failed)
        )
    }
}
string_enum!(CloudJobOutcome: Succeeded, Failed, Cancelled);
string_enum!(WorkflowNodeKind: Build, Test, Artifact, Approval, Merge, Publish, Deploy, Verify, Cleanup);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u16,
    pub backoff_seconds: u32,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_seconds: 0,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Pending,
    Approved {
        actor: String,
        decided_at_millis: i64,
    },
    Rejected {
        actor: String,
        decided_at_millis: i64,
        reason: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalGate {
    pub action: String,
    pub decision: ApprovalDecision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_job_ids: Vec<CloudJobId>,
}
string_enum!(ReleaseAction: MergePullRequest, PublishPackage, PublishToTest, PublishToProduction);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseGate {
    pub action: ReleaseAction,
    pub repository: String,
    pub exact_commit: GitCommitSha,
    pub approval: ApprovalGate,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentLease {
    pub environment: String,
    pub holder_workflow_id: CloudWorkflowId,
    pub holder_job_id: CloudJobId,
    pub acquired_at_millis: i64,
    pub expires_at_millis: i64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: CloudJobId,
    /// Stable identity shared by retry attempts of the same logical node.
    pub logical_key: String,
    pub label: String,
    pub kind: WorkflowNodeKind,
    pub state: CloudJobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<CloudJobOutcome>,
    pub progress: CloudProgress,
    pub weight: u16,
    pub attempt: u16,
    pub retry: RetryPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<CloudJobId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<CloudJobId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<GitSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerTarget>,
    /// Unique artifact outputs supplied by direct dependency nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalGate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseGate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_lease: Option<EnvironmentLease>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkflowProgress {
    pub basis_points: u16,
    pub estimated: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CloudWorkflow {
    pub protocol_version: u32,
    pub id: CloudWorkflowId,
    pub title: String,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    pub retain_until_millis: i64,
    pub nodes: Vec<WorkflowNode>,
}
impl CloudWorkflow {
    #[must_use]
    pub fn progress(&self) -> WorkflowProgress {
        let superseded: HashSet<_> = self.nodes.iter().filter_map(|node| node.supersedes).collect();
        let latest = self.nodes.iter().filter(|node| !superseded.contains(&node.id));
        let total_weight: u64 = latest.clone().map(|node| u64::from(node.weight)).sum();
        if total_weight == 0 {
            return WorkflowProgress {
                basis_points: 0,
                estimated: true,
            };
        }
        let mut known_weighted = 0_u128;
        let mut estimated = false;
        for node in latest {
            let basis_points = match node.progress.basis_points() {
                Some(value) => u128::from(value),
                None if matches!(node.state, CloudJobState::Completed | CloudJobState::Cleaned) => 10_000,
                None => {
                    estimated = true;
                    0
                }
            };
            known_weighted += basis_points * u128::from(node.weight);
        }
        let value = known_weighted / u128::from(total_weight);
        WorkflowProgress {
            basis_points: u16::try_from(value).unwrap_or(10_000),
            estimated,
        }
    }
}
#[derive(Debug, Error, Eq, PartialEq)]
pub enum CloudProtocolError {
    #[error("unsupported cloud protocol version {0}")]
    UnsupportedVersion(u32),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("Git commit must be a full 40-character hexadecimal SHA: {0}")]
    InvalidGitCommit(String),
    #[error("SHA-256 digest must be 64 hexadecimal characters: {0}")]
    InvalidSha256(String),
    #[error("workflow retention ends before it starts")]
    InvalidRetention,
    #[error("workflow timestamps are not monotonic")]
    InvalidWorkflowTimestamps,
    #[error("workflow contains duplicate node ids")]
    DuplicateNodeId,
    #[error("workflow contains duplicate attempt {attempt} for logical node {logical_key}")]
    DuplicateLogicalAttempt { logical_key: String, attempt: u16 },
    #[error("multiple retry attempts supersede node {0}")]
    ForkedRetryAttempt(CloudJobId),
    #[error("workflow contains duplicate artifact id {0}")]
    DuplicateArtifactId(String),
    #[error("repository must be a credential-free owner/name identity")]
    InvalidRepository,
    #[error("node {0} has an empty logical key or label")]
    EmptyNodeIdentity(CloudJobId),
    #[error("node {0} has an invalid worker target")]
    InvalidWorkerTarget(CloudJobId),
    #[error("node {0} has an invalid retry attempt")]
    InvalidAttempt(CloudJobId),
    #[error("node {0} has an outcome inconsistent with its state")]
    InvalidJobOutcome(CloudJobId),
    #[error("node {node} supersedes missing attempt {previous}")]
    MissingSupersededAttempt { node: CloudJobId, previous: CloudJobId },
    #[error("node {0} does not form a valid immutable retry chain")]
    InvalidSupersededAttempt(CloudJobId),
    #[error("node {0} depends on itself")]
    SelfDependency(CloudJobId),
    #[error("node {node} depends on missing node {dependency}")]
    MissingDependency { node: CloudJobId, dependency: CloudJobId },
    #[error("workflow dependency cycle includes node {0}")]
    DependencyCycle(CloudJobId),
    #[error("node {0} has invalid measured progress")]
    InvalidProgress(CloudJobId),
    #[error("approval node {0} has no approval or release gate")]
    MissingApprovalGate(CloudJobId),
    #[error("node {0} has an invalid approval gate")]
    InvalidApprovalGate(CloudJobId),
    #[error("approval node {node} refers to missing evidence node {evidence}")]
    MissingApprovalEvidence { node: CloudJobId, evidence: CloudJobId },
    #[error("approval node {0} cannot cite itself as evidence")]
    SelfApprovalEvidence(CloudJobId),
    #[error("node {0} has an invalid artifact reference")]
    InvalidArtifactRef(CloudJobId),
    #[error("node {0} has an invalid input artifact reference")]
    InvalidInputArtifact(CloudJobId),
    #[error("provenance from node {0} has an unsafe workflow run URL")]
    InvalidWorkflowRunUrl(CloudJobId),
    #[error("node {0} has an invalid environment lease")]
    InvalidEnvironmentLease(CloudJobId),
}
#[cfg(test)]
#[path = "cloud_run/tests.rs"]
mod tests;
