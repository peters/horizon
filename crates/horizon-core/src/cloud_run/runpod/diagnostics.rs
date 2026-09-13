//! Allowlisted diagnostics only; provider errors may contain private identities and payloads.

use super::RunPodError;

const MAX_CAUSE_DEPTH: usize = 4;

#[derive(Debug, Eq, PartialEq)]
struct Diagnostic {
    category: &'static str,
    operation: &'static str,
    http_status: Option<u16>,
    reconciliation_required: bool,
}

pub(super) fn ensure_failed(error: &RunPodError) {
    let diagnostic = classify(error);
    // Do not inherit caller spans, which can carry private request fields.
    tracing::warn!(
        target: "horizon_core::runpod::creation",
        parent: None,
        category = diagnostic.category,
        operation = diagnostic.operation,
        http_status = diagnostic.http_status,
        reconciliation_required = diagnostic.reconciliation_required,
        "worker ensure failed; retain allocation and recover before any retry"
    );
}

fn classify(mut error: &RunPodError) -> Diagnostic {
    let mut diagnostic = Diagnostic {
        category: "unknown",
        operation: "ensure",
        http_status: None,
        // Only explicit validation/capacity rejection or verified cleanup clears this flag.
        // False is never authority to retry a consumed creation claim.
        reconciliation_required: !matches!(
            error,
            RunPodError::MissingApiKey
                | RunPodError::InvalidApiKey
                | RunPodError::InvalidTarget
                | RunPodError::CapacityUnavailable
                | RunPodError::CreationVerificationFailed { .. }
                | RunPodError::HourlyCostRejected { .. }
                | RunPodError::LeaseDeadlineRejected { .. }
        ),
    };
    // Inspect only this known wrapper, without recursion or traversing arbitrary error sources.
    for _ in 0..MAX_CAUSE_DEPTH {
        let RunPodError::PersistentCreationUnresolved { cause, .. } = error else {
            break;
        };
        diagnostic.reconciliation_required = true;
        error = cause;
    }
    diagnostic.category = match error {
        RunPodError::CapacityUnavailable => "capacity-unavailable",
        RunPodError::RequestFailed { .. } => "request-failed",
        RunPodError::UnexpectedStatus { status, .. } => {
            diagnostic.http_status = (100..=599).contains(status).then_some(*status);
            "unexpected-status"
        }
        RunPodError::InvalidResponse { .. } => "invalid-response",
        RunPodError::InvalidTarget => "invalid-target",
        RunPodError::MissingApiKey | RunPodError::InvalidApiKey => "credential-unavailable",
        RunPodError::InvalidPersistedWorker | RunPodError::ResourceIdentityMismatch => "invalid-identity",
        RunPodError::AmbiguousResource { .. } => "ambiguous-resource",
        RunPodError::CreationFenceFailed { .. } => "creation-fence-failed",
        RunPodError::CreationUnresolved { .. }
        | RunPodError::PersistentCreationUnresolved { .. }
        | RunPodError::PersistentCreationReconciliationRequired { .. } => {
            diagnostic.reconciliation_required = true;
            "creation-unresolved"
        }
        RunPodError::CreationVerificationFailed { .. } => "creation-verification-failed",
        RunPodError::CreationCleanupFailed { .. }
        | RunPodError::DeletionVerificationFailed { .. }
        | RunPodError::LeaseRejectionCleanupFailed { .. }
        | RunPodError::CostRejectionCleanupFailed { .. } => "cleanup-unverified",
        RunPodError::LeaseDeadlineRejected { .. } => "lease-rejected",
        RunPodError::HourlyCostRejected { .. }
        | RunPodError::WorkerRecoveryCostRejected { .. }
        | RunPodError::PersistentWorkerCostRejected { .. } => "cost-rejected",
        RunPodError::StopUnsupportedLifetime
        | RunPodError::StopRetentionUnverified
        | RunPodError::StopStateUnverified
        | RunPodError::StopResourceLost
        | RunPodError::StopVerificationFailed
        | RunPodError::StartIdentityRequired
        | RunPodError::StartUnverified => "unknown",
    };
    if let RunPodError::RequestFailed { operation }
    | RunPodError::UnexpectedStatus { operation, .. }
    | RunPodError::InvalidResponse { operation } = error
    {
        diagnostic.operation = match *operation {
            "pod lookup" => "lookup",
            "pod creation" => "create",
            "pod inspection" => "inspect",
            "pod deletion" => "delete",
            _ => "unknown",
        };
    }
    diagnostic
}

#[cfg(test)]
mod tests;
