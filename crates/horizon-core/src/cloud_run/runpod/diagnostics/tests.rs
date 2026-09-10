use super::*;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

const PRIVATE: &str = "private-sentinel-token-path-resource";

#[test]
fn classifications_preserve_only_allowlisted_categories_and_operations() {
    for (error, category, operation) in [
        (RunPodError::CapacityUnavailable, "capacity-unavailable", "ensure"),
        (
            RunPodError::RequestFailed {
                operation: "pod creation",
            },
            "request-failed",
            "create",
        ),
        (
            RunPodError::InvalidResponse {
                operation: "pod lookup",
            },
            "invalid-response",
            "lookup",
        ),
        (
            RunPodError::RequestFailed {
                operation: "pod inspection",
            },
            "request-failed",
            "inspect",
        ),
        (
            RunPodError::RequestFailed {
                operation: "pod deletion",
            },
            "request-failed",
            "delete",
        ),
        (
            RunPodError::InvalidResponse { operation: PRIVATE },
            "invalid-response",
            "unknown",
        ),
        (
            RunPodError::CreationFenceFailed { reason: PRIVATE.into() },
            "creation-fence-failed",
            "ensure",
        ),
        (
            RunPodError::AmbiguousResource {
                name: PRIVATE.into(),
                count: usize::MAX,
            },
            "ambiguous-resource",
            "ensure",
        ),
        (RunPodError::InvalidTarget, "invalid-target", "ensure"),
        (RunPodError::ResourceIdentityMismatch, "invalid-identity", "ensure"),
        (
            RunPodError::CreationCleanupFailed { pod_id: PRIVATE.into() },
            "cleanup-unverified",
            "ensure",
        ),
    ] {
        let diagnostic = classify(&error);
        assert_eq!(diagnostic.category, category);
        assert_eq!(diagnostic.operation, operation);
        assert_eq!(diagnostic.http_status, None);
        assert!(!format!("{diagnostic:?}").contains(PRIVATE));
    }
}

fn wrapped(cause: RunPodError) -> RunPodError {
    RunPodError::PersistentCreationUnresolved {
        name: PRIVATE.into(),
        cause: Box::new(cause),
    }
}

#[test]
fn nested_causes_have_a_fixed_bound_and_keep_uncertainty_separate() {
    let mut error = RunPodError::CapacityUnavailable;
    assert!(!classify(&error).reconciliation_required);
    for _ in 0..MAX_CAUSE_DEPTH {
        error = wrapped(error);
        let diagnostic = classify(&error);
        assert_eq!(diagnostic.category, "capacity-unavailable");
        assert!(diagnostic.reconciliation_required);
    }
    error = wrapped(error);
    assert_eq!(classify(&error).category, "creation-unresolved");
    assert!(classify(&error).reconciliation_required);
    for error in [
        RunPodError::CreationUnresolved { name: PRIVATE.into() },
        RunPodError::PersistentCreationReconciliationRequired {
            name: PRIVATE.into(),
            pod_id: PRIVATE.into(),
        },
    ] {
        assert_eq!(classify(&error).category, "creation-unresolved");
        assert!(classify(&error).reconciliation_required);
    }
}

#[test]
fn verified_creation_cleanup_clears_only_the_direct_event_reconciliation_flag() {
    for is_wrapped in [false, true] {
        let error = RunPodError::CreationVerificationFailed { pod_id: PRIVATE.into() };
        let error = if is_wrapped { wrapped(error) } else { error };
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || ensure_failed(&error));
        let events = capture.0.lock().expect("capture");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["category"], "creation-verification-failed");
        assert_eq!(events[0]["operation"], "ensure");
        assert_eq!(events[0]["reconciliation_required"], is_wrapped.to_string());
        assert!(!events[0].contains_key("http_status"));
        assert!(!format!("{events:?}").contains(PRIVATE));
    }
}

#[test]
fn http_status_is_numeric_and_limited_to_the_http_range() {
    for status in [0, 99, 100, 200, 403, 429, 500, 599, 600, u16::MAX] {
        let error = wrapped(RunPodError::UnexpectedStatus {
            operation: "pod creation",
            status,
        });
        let diagnostic = classify(&error);
        assert_eq!(diagnostic.category, "unexpected-status");
        assert_eq!(diagnostic.operation, "create");
        assert_eq!(diagnostic.http_status, (100..=599).contains(&status).then_some(status));
        assert!(diagnostic.reconciliation_required);
    }
}

#[test]
fn retained_cost_and_unverified_identity_or_cleanup_require_reconciliation() {
    use super::super::{CloudJobId, CloudWorkflowId, RunPodWorker};
    use crate::cloud_run::interactive_worker::InteractiveWorkerLifetime;

    let worker = Box::new(RunPodWorker {
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        pod_id: PRIVATE.into(),
        name: PRIVATE.into(),
        image: PRIVATE.into(),
        lifetime: InteractiveWorkerLifetime::Persistent,
        hourly_cost_micros: Some(100),
    });
    for error in [
        RunPodError::PersistentWorkerCostRejected {
            worker: worker.clone(),
            actual: Some(100),
            maximum: 1,
        },
        RunPodError::WorkerRecoveryCostRejected {
            worker: worker.clone(),
            actual: Some(100),
            maximum: 1,
        },
        RunPodError::CostRejectionCleanupFailed { worker: worker.clone() },
        RunPodError::LeaseRejectionCleanupFailed { worker },
        RunPodError::CreationCleanupFailed { pod_id: PRIVATE.into() },
        RunPodError::DeletionVerificationFailed { pod_id: PRIVATE.into() },
        RunPodError::AmbiguousResource {
            name: PRIVATE.into(),
            count: 2,
        },
        RunPodError::ResourceIdentityMismatch,
        RunPodError::CreationFenceFailed { reason: PRIVATE.into() },
        RunPodError::RequestFailed {
            operation: "pod creation",
        },
    ] {
        let diagnostic = classify(&error);
        assert!(diagnostic.reconciliation_required);
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || ensure_failed(&error));
        let events = capture.0.lock().expect("capture");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["reconciliation_required"], "true");
        assert!(!format!("{diagnostic:?} {events:?}").contains(PRIVATE));
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        assert_eq!(event.metadata().target(), "horizon_core::runpod::creation");
        assert_eq!(*event.metadata().level(), tracing::Level::WARN);
        assert!(event.is_root());
        let mut names = event
            .metadata()
            .fields()
            .iter()
            .map(|field| field.name())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "category",
                "http_status",
                "message",
                "operation",
                "reconciliation_required"
            ]
        );
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.0.lock().expect("capture").push(fields.0);
    }
}

#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        assert_eq!(field.name(), "http_status");
        self.0.insert(field.name().into(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        assert_eq!(field.name(), "reconciliation_required");
        self.0.insert(field.name().into(), value.to_string());
    }
}

#[test]
fn event_fields_are_allowlisted_and_exclude_private_errors_and_parent_spans() {
    let capture = Capture::default();
    tracing::subscriber::with_default(capture.clone(), || {
        let span = tracing::info_span!("private_parent", token = PRIVATE);
        let _entered = span.enter();
        ensure_failed(&wrapped(RunPodError::UnexpectedStatus {
            operation: "pod creation",
            status: 403,
        }));
        ensure_failed(&RunPodError::CreationFenceFailed { reason: PRIVATE.into() });
        ensure_failed(&RunPodError::InvalidResponse { operation: PRIVATE });
    });
    let events = capture.0.lock().expect("capture");
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        BTreeMap::from([
            ("category".into(), "unexpected-status".into()),
            ("http_status".into(), "403".into()),
            (
                "message".into(),
                "worker ensure failed; retain allocation and recover before any retry".into()
            ),
            ("operation".into(), "create".into()),
            ("reconciliation_required".into(), "true".into()),
        ])
    );
    assert!(!events[1].contains_key("http_status"));
    assert_eq!(events[2]["operation"], "unknown");
    assert!(!format!("{events:?}").contains(PRIVATE));
}

#[test]
fn provider_ensure_emits_once_without_changing_the_error_or_issuing_cleanup() {
    use super::super::{
        ApiPod, CloudJobId, CloudWorkflowId, CreatePodRequest, RunPodCleanup, RunPodClient,
        RunPodInteractiveWorkerProvider, RunPodSshEndpoint, RunPodWorker, Transport,
        tests::{interactive_request, profile},
    };
    use crate::cloud_run::interactive_worker::InteractiveWorkerProvider as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RejectedCreate(Arc<AtomicUsize>);
    impl Transport for RejectedCreate {
        fn list_by_name(&self, _: &str) -> Result<Vec<ApiPod>, RunPodError> {
            Ok(Vec::new())
        }
        fn create(&self, _: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(RunPodError::CapacityUnavailable)
        }
        fn get(&self, _: &str) -> Result<Option<ApiPod>, RunPodError> {
            panic!("unexpected inspection")
        }
        fn stop(&self, _: &str) -> Result<(), RunPodError> {
            panic!("unexpected Stop")
        }
        fn delete(&self, _: &str) -> Result<RunPodCleanup, RunPodError> {
            panic!("unexpected deletion")
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(RejectedCreate(Arc::clone(&calls))),
        profile(),
        |_: &RunPodWorker, _: &RunPodSshEndpoint, _: &str| None,
    );
    let request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
    let capture = Capture::default();
    let result = tracing::subscriber::with_default(capture.clone(), || provider.ensure_worker(&request));
    assert_eq!(result, Err(RunPodError::CapacityUnavailable));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = capture.0.lock().expect("capture");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["category"], "capacity-unavailable");
}

#[test]
fn invalid_requests_emit_once_without_any_provider_operation() {
    use super::super::{
        ApiPod, CloudJobId, CloudProvider, CloudWorkflowId, CreatePodRequest, RunPodCleanup, RunPodClient,
        RunPodInteractiveWorkerProvider, RunPodSshEndpoint, RunPodWorker, Transport,
        tests::{interactive_request, profile},
    };
    use crate::cloud_run::interactive_worker::InteractiveWorkerProvider as _;

    struct NoProviderCalls;
    impl Transport for NoProviderCalls {
        fn list_by_name(&self, _: &str) -> Result<Vec<ApiPod>, RunPodError> {
            panic!("unexpected lookup")
        }
        fn create(&self, _: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
            panic!("unexpected creation")
        }
        fn get(&self, _: &str) -> Result<Option<ApiPod>, RunPodError> {
            panic!("unexpected inspection")
        }
        fn stop(&self, _: &str) -> Result<(), RunPodError> {
            panic!("unexpected Stop")
        }
        fn delete(&self, _: &str) -> Result<RunPodCleanup, RunPodError> {
            panic!("unexpected deletion")
        }
    }
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(NoProviderCalls),
        profile(),
        |_: &RunPodWorker, _: &RunPodSshEndpoint, _: &str| panic!("unexpected host-key lookup"),
    );
    for provider_mismatch in [false, true] {
        let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
        if provider_mismatch {
            request.target.provider = CloudProvider::LocalDocker;
        } else {
            request.ssh_public_key = PRIVATE.into();
        }
        let capture = Capture::default();
        let result = tracing::subscriber::with_default(capture.clone(), || provider.ensure_worker(&request));
        assert_eq!(result, Err(RunPodError::InvalidTarget));
        let events = capture.0.lock().expect("capture");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["category"], "invalid-target");
        assert_eq!(events[0]["operation"], "ensure");
        assert_eq!(events[0]["reconciliation_required"], "false");
        assert!(!events[0].contains_key("http_status"));
        assert!(!format!("{events:?}").contains(PRIVATE));
    }
}
