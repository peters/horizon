//! The first explicit Azure Stop through the configured path on real stores with a fake
//! provider: admission before the client, durable intent before dispatch, one Stop,
//! retained intent on uncertainty, and binding drift fenced around the provider call.
use super::configured_azure::{AzureFixture, Shape, azure_profile, drift, drift_binding};
use super::*;
use crate::cloud_run::azure::AzureProfile;
use crate::remote_provider_config::RemoteProviderConfig;
use crate::remote_workspace::{
    RemoteEnvironmentSummary,
    stop::configured_azure::{RetainedAzure, stop_with},
};

type Rejected = ConfiguredAzureStopError;
type StopHook = Box<dyn Fn(&InteractiveWorker) + Send + Sync>;

struct Stopper {
    result: Result<InteractiveWorkerStop, &'static str>,
    calls: Mutex<usize>,
    on_stop: Option<StopHook>,
}

impl Stopper {
    fn answering(result: Result<InteractiveWorkerStop, &'static str>) -> Self {
        Self {
            result,
            calls: Mutex::new(0),
            on_stop: None,
        }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("calls")
    }
}

impl InteractiveWorkerProvider for &Stopper {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no inspection")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no recovery")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

impl InteractiveWorkerStopProvider for &Stopper {
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        assert!(worker.is_valid_for(CloudProvider::Azure));
        *self.calls.lock().expect("calls") += 1;
        if let Some(hook) = &self.on_stop {
            hook(worker);
        }
        self.result.map_err(std::io::Error::other)
    }
}

fn retained() -> AzureFixture {
    AzureFixture::new(&Shape {
        intent: false,
        ..Shape::default()
    })
}

fn stop(fixture: &AzureFixture, stopper: &Stopper) -> Result<RemoteEnvironmentSummary, Rejected> {
    stop_with(
        &fixture.store,
        &fixture.profile,
        &fixture.current().workspace().environment_summary(),
        |_| Ok(stopper),
        |provider, allocation| stop_allocation(&fixture.store, provider, allocation),
    )
}

fn refused(fixture: &AzureFixture, profile: &AzureProfile, summary: Option<RemoteEnvironmentSummary>) -> Rejected {
    let before = fixture.current();
    let expected = summary.unwrap_or_else(|| before.workspace().environment_summary());
    let error = stop_with(
        &fixture.store,
        profile,
        &expected,
        |_: &RetainedAzure| -> Result<&Stopper, ConfiguredStopConfirmationError> {
            panic!("must not construct the client")
        },
        |_, _| panic!("no provider work"),
    )
    .expect_err("refused");
    assert_eq!(fixture.current(), before);
    error
}

fn phase(fixture: &AzureFixture) -> RemoteRuntimePhase {
    fixture
        .current()
        .workspace()
        .state()
        .runtime
        .as_ref()
        .expect("runtime")
        .phase
}

#[test]
fn public_azure_stop_refuses_unsupported_or_missing_named_profiles_without_writes() {
    let fixture = retained();
    let before = fixture.current();
    for provider in [CloudProvider::LocalDocker, CloudProvider::RunPod, CloudProvider::Azure] {
        let mut expected = before.workspace().environment_summary();
        expected.provider = provider;
        let result = stop_configured_azure_environment(&fixture.store, &RemoteProviderConfig::default(), &expected);
        if provider == CloudProvider::Azure {
            assert!(matches!(result, Err(Rejected::Configuration(_))), "{result:?}");
        } else {
            assert_eq!(result, Err(Rejected::UnsupportedProvider));
        }
        assert_eq!(fixture.current(), before);
    }
    // A configured profile with existing intent is refused by admission, so the public
    // path proves its dispatch without ever reaching the Azure CLI.
    let fixture = AzureFixture::new(&Shape::default());
    let before = fixture.current();
    let config = RemoteProviderConfig {
        azure: vec![fixture.profile.clone()],
        ..Default::default()
    };
    assert_eq!(
        stop_configured_azure_environment(&fixture.store, &config, &before.workspace().environment_summary()),
        Err(Rejected::ExistingStopIntent)
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn admission_requires_exact_persistent_worker_pin_profile_and_binding_before_the_client() {
    let shapes: [(&str, Shape, Rejected); 4] = [
        (
            "no pin",
            Shape {
                intent: false,
                pin: false,
                ..Shape::default()
            },
            Rejected::Stop(Error::MissingTrust),
        ),
        (
            "timed worker",
            Shape {
                intent: false,
                binding: false,
                lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
                ..Shape::default()
            },
            Rejected::Stop(Error::UnsupportedLifetime),
        ),
        (
            "no binding",
            Shape {
                intent: false,
                binding: false,
                ..Shape::default()
            },
            Rejected::InvalidBinding,
        ),
        ("existing intent", Shape::default(), Rejected::ExistingStopIntent),
    ];
    for (label, shape, expected) in shapes {
        let fixture = AzureFixture::new(&shape);
        assert_eq!(refused(&fixture, &fixture.profile, None), expected, "{label}");
    }
    let fixture = retained();
    let current = fixture.current();
    let mut state = current.workspace().state().clone();
    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(current.workspace(), &state)
        .expect("management fixture");
    assert_eq!(
        refused(&fixture, &fixture.profile, None),
        Rejected::Stop(Error::ManagementConflict)
    );
    let fixture = retained();
    let mut drifted = fixture.profile.clone();
    drifted.vm_size = "Standard_D2s_v3".into();
    assert_eq!(refused(&fixture, &drifted, None), Rejected::InvalidBinding);
    let mut renamed = fixture.profile.clone();
    renamed.name = "other".into();
    assert_eq!(refused(&fixture, &renamed, None), Rejected::InvalidBinding);
    let before = fixture.current();
    let mut summary = before.workspace().environment_summary();
    summary.panel_count += 1;
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(summary)),
        Rejected::Stop(Error::StateChanged)
    );
    // The store refuses a foreign owner outright rather than reporting absence.
    let mut foreign = before.workspace().environment_summary();
    foreign.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(foreign)),
        Rejected::Stop(Error::StorageUnavailable)
    );
    let mut retagged = before.workspace().environment_summary();
    retagged.worker_identity.as_mut().expect("worker").resource_id = "foreign-worker".into();
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(retagged)),
        Rejected::Stop(Error::StateChanged)
    );
    assert_eq!(
        fixture.directory.path().read_dir().expect("root").count(),
        1,
        "only the control store, no identity"
    );
}

#[test]
fn one_stop_records_intent_before_dispatch_and_preserves_every_other_field() {
    let fixture = retained();
    let before = fixture.current();
    let store = fixture.store.clone();
    let mut stopper = Stopper::answering(Ok(InteractiveWorkerStop::Stopped));
    stopper.on_stop = Some(Box::new(move |worker| {
        let current = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("read")
            .expect("allocation");
        let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
        assert!(matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. }));
        assert_eq!(runtime.worker.as_ref(), Some(worker));
    }));
    let result = stop(&fixture, &stopper).expect("verified fake Stop");
    let after = fixture.current();
    assert_eq!(result, after.workspace().environment_summary());
    let mut expected = before.workspace().state().clone();
    expected.runtime.as_mut().expect("runtime").phase =
        after.workspace().state().runtime.as_ref().expect("runtime").phase;
    assert!(
        matches!(result.saved_phase, Some(RemoteRuntimePhase::Stopped { requested_at_millis, observed_at_millis })
            if requested_at_millis > 0 && observed_at_millis >= requested_at_millis)
    );
    assert_eq!(after.workspace().state(), &expected);
    assert_eq!(after.workflow(), before.workflow());
    assert_eq!(result.revision, before.workspace().revision() + 2);
    assert_eq!(stopper.calls(), 1);
    assert_eq!(stop(&fixture, &stopper), Err(Rejected::ExistingStopIntent));
    assert_eq!(fixture.current(), after);
    assert_eq!(stopper.calls(), 1);
    assert_eq!(
        fixture.directory.path().read_dir().expect("root").count(),
        1,
        "only the control store, no identity"
    );
}

#[test]
fn uncertainty_or_absence_keeps_original_intent_and_never_authorizes_another_stop() {
    for result in [Err("private-provider-marker"), Ok(InteractiveWorkerStop::AlreadyAbsent)] {
        let fixture = retained();
        let before = fixture.current();
        let stopper = Stopper::answering(result);
        let error = stop(&fixture, &stopper).expect_err("unverified");
        assert_eq!(
            error,
            Rejected::Stop(if result.is_err() {
                Error::ProviderUnavailable
            } else {
                Error::ResourceAbsent
            })
        );
        assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
        let retained = fixture.current();
        assert!(matches!(phase(&fixture), RemoteRuntimePhase::Stopping { .. }));
        let mut expected = before.workspace().state().clone();
        expected.runtime.as_mut().expect("runtime").phase =
            retained.workspace().state().runtime.as_ref().expect("runtime").phase;
        assert_eq!(retained.workspace().state(), &expected);
        assert_eq!(retained.workflow(), before.workflow());
        let reopened =
            CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()).expect("fresh client");
        assert_eq!(
            stop_with(
                &reopened,
                &fixture.profile,
                &retained.workspace().environment_summary(),
                |_: &RetainedAzure| -> Result<&Stopper, ConfiguredStopConfirmationError> {
                    panic!("existing intent precedes the client")
                },
                |_, _| panic!("never replay")
            ),
            Err(Rejected::ExistingStopIntent)
        );
        assert_eq!(fixture.current(), retained);
        assert_eq!(stopper.calls(), 1);
    }
}

#[test]
fn drift_during_client_construction_refuses_dispatch_even_on_error() {
    for failure in [false, true] {
        let fixture = retained();
        let before = fixture.current();
        let stopper = Stopper::answering(Ok(InteractiveWorkerStop::Stopped));
        let result = stop_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            |_| {
                drift(&fixture);
                if failure {
                    Err(ConfiguredStopConfirmationError::InvalidBinding)
                } else {
                    Ok(&stopper)
                }
            },
            |_, _| panic!("no dispatch after snapshot drift"),
        );
        assert_eq!(result, Err(Rejected::Stop(Error::StateChanged)));
        assert_eq!(fixture.current().workspace(), before.workspace());
        assert_eq!(stopper.calls(), 0);
    }
    // A client that cannot be built without drift is reported as such, with no intent.
    let fixture = retained();
    let before = fixture.current();
    assert_eq!(
        stop_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            |_: &RetainedAzure| -> Result<&Stopper, ConfiguredStopConfirmationError> {
                Err(ConfiguredStopConfirmationError::InvalidBinding)
            },
            |_, _| panic!("no dispatch"),
        ),
        Err(Rejected::InvalidBinding)
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn exact_allocation_cas_does_not_reload_a_new_workflow_before_stop() {
    let fixture = retained();
    let before = fixture.current();
    let stopper = Stopper::answering(Ok(InteractiveWorkerStop::Stopped));
    let result = stop_with(
        &fixture.store,
        &fixture.profile,
        &before.workspace().environment_summary(),
        |_| Ok(&stopper),
        |provider, exact| {
            drift(&fixture);
            stop_allocation(&fixture.store, provider, exact)
        },
    );
    assert_eq!(result, Err(Rejected::Stop(Error::StateChanged)));
    assert_eq!(fixture.current().workspace(), before.workspace());
    assert_eq!(stopper.calls(), 0);
}

#[test]
fn binding_drift_during_the_provider_stop_retains_intent_without_completion() {
    for result in [Ok(InteractiveWorkerStop::Stopped), Err("in-flight failure")] {
        let fixture = retained();
        let before = fixture.current();
        let path = fixture.store.path().to_path_buf();
        let mut stopper = Stopper::answering(result);
        stopper.on_stop = Some(Box::new(move |_| drift_binding(&path)));
        assert_eq!(
            stop(&fixture, &stopper),
            Err(Rejected::Stop(Error::StateChanged)),
            "{result:?}"
        );
        assert_eq!(stopper.calls(), 1);
        let retained = fixture.current();
        assert!(
            matches!(phase(&fixture), RemoteRuntimePhase::Stopping { .. }),
            "intent stays for the saved-Stop check: {result:?}"
        );
        let mut expected = before.workspace().state().clone();
        expected.runtime.as_mut().expect("runtime").phase =
            retained.workspace().state().runtime.as_ref().expect("runtime").phase;
        assert_eq!(retained.workspace().state(), &expected, "no completion was written");
    }
}

#[test]
fn the_production_client_for_stop_is_lazy_and_unsupported_profiles_never_reach_it() {
    let fixture = retained();
    let before = fixture.current();
    let admitted = RetainedAzure::load(
        &fixture.store,
        &fixture.profile,
        &before.workspace().environment_summary(),
    )
    .expect("admitted");
    let client = RetainedAzure::client(&fixture.store, &azure_profile()).expect("client");
    assert_eq!(client.provider(), CloudProvider::Azure);
    drop((admitted, client));
    assert_eq!(fixture.current(), before);
    assert_eq!(fixture.directory.path().read_dir().expect("root").count(), 1);
}
