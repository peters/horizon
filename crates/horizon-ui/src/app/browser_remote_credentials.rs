//! The values a remote create needs that only the OS credential store holds,
//! read off the frame. The workbench thread reads one value at a time, a frame
//! polls its answer without waiting, and the create plans its request once the
//! last value is at hand. The panel opens only after the plan leased its
//! provider slot.

use std::collections::VecDeque;
use std::time::Instant;

use horizon_core::browser::manifest::{self, BrowserCreateRequest};
use horizon_core::browser::remote::{CredentialReference, RemoteProviderProfile};
use horizon_core::remote_browser_credential::{
    CredentialValueReply, ReadCredentialStore, RemoteCredentialError, ResolveError, os_store_references,
    target_provider_profile,
};

use super::HorizonApp;
use super::browser_create_failure::CREATE_TIMEOUT;
use super::browser_create_lifecycle::{CreateStage, PendingBrowserCreate};
use super::browser_remote_create::{CreateRefusal, RemoteTarget};
use super::browser_request_claims::{HOST_RETIRING, fail_create};
use super::browser_requests::ActorPanel;

/// A remote create's OS-store values that have not all arrived yet, and what
/// the host needs to plan the request once they have: the configuration, the
/// target in it, and the agent panel the panel is created beside.
pub(super) struct RemoteCredentialRead {
    /// The agent panel the create was asked beside.
    actor: ActorPanel,
    target: RemoteTarget,
    /// The provider whose references are read, as the reads address them.
    profile: RemoteProviderProfile,
    /// The reference the workbench reads now, and its answer's channel.
    reference: CredentialReference,
    reply: CredentialValueReply,
    /// The references still to ask for, in authentication order.
    remaining: VecDeque<CredentialReference>,
    /// The values that arrived, offered to the request as the OS store.
    values: ReadCredentialStore,
}

/// What one frame left of a remote create that has no panel yet: the create
/// while its values or its slot are still on their way, and `None` once its
/// request is answered or its panel is open and audited.
pub(super) type RemoteCreateStep = Option<PendingBrowserCreate>;

/// What one poll of the workbench's answer left the create to do.
enum RemoteCredentialStep {
    /// No value has arrived yet.
    Waiting,
    /// Every value is at hand: the request can be planned.
    Answered,
    /// The workbench refused a value, or a read could not be asked for.
    Refused(CreateRefusal),
}

impl HorizonApp {
    /// Starts a remote create: resolves its target from configuration, asks the
    /// workbench for the values only the OS store holds, and leases the
    /// provider's slot on the coordination worker. The panel opens when both
    /// are at hand, so a missing or locked credential is still reported to the
    /// agent before any panel exists.
    pub(super) fn begin_remote_create(
        &mut self,
        request: BrowserCreateRequest,
        actor_panel: ActorPanel,
        started_at: Instant,
    ) {
        let target = request.target.clone().unwrap_or_default();
        let target = match self.remote_target(&target) {
            Ok(target) => target,
            Err(refusal) => {
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    refusal.code,
                    &refusal.message,
                );
                return;
            }
        };
        let mut pending =
            PendingBrowserCreate::resolving(request, started_at, self.browser_create_host.io.board_fence());
        let profile = target_provider_profile(&target.remote, &target.name).cloned();
        let references = match (&profile, self.remote_browser_credentials.keychain_available()) {
            (Some(profile), true) => os_store_references(profile),
            // An unconfigured target is refused by the plan, with its own code
            // and text, and a computer without an OS store resolves the request
            // as it always did.
            _ => Vec::new(),
        };
        let read = split_first_read(references);
        let (Some(profile), Some((reference, remaining))) = (profile, read) else {
            if let Some(pending) =
                self.plan_and_lease_remote(pending, actor_panel, &target, &ReadCredentialStore::new())
            {
                self.browser_create_host.pending.push(pending);
            }
            return;
        };
        let reply = match self.remote_browser_credentials.request_value(&profile, &reference) {
            Ok(reply) => reply,
            Err(error) => {
                let refusal = credential_refusal(&profile, &reference, error);
                self.refuse_remote_create(&pending, &refusal);
                return;
            }
        };
        pending.stage = CreateStage::ReadingCredentials;
        pending.remote_credentials = Some(RemoteCredentialRead {
            actor: actor_panel,
            target,
            profile,
            reference,
            reply,
            remaining,
            values: ReadCredentialStore::new(),
        });
        self.browser_create_host.pending.push(pending);
    }

    /// Polls a remote create that waits for values from the workbench, and
    /// plans its request once they are at hand. The answer is polled without
    /// waiting, so a store the workbench holds never holds a frame, and the
    /// create deadline bounds the wait: a reply that never comes ends in the
    /// create timeout.
    pub(super) fn advance_remote_create(&mut self, mut pending: PendingBrowserCreate) -> RemoteCreateStep {
        // Nothing this create was asked for applies to a board that replaced
        // the one it was claimed on, or one that is being torn down: its value
        // opens no panel there, and the request is answered once.
        if !pending.board.holds() || self.browser_host_retiring() {
            let refusal = CreateRefusal {
                code: HOST_RETIRING.0,
                message: HOST_RETIRING.1.into(),
            };
            self.refuse_remote_create(&pending, &refusal);
            return None;
        }
        if pending.request.deadline_at_millis < manifest::now_millis() {
            let refusal = CreateRefusal {
                code: CREATE_TIMEOUT.0,
                message: CREATE_TIMEOUT.1.into(),
            };
            self.refuse_remote_create(&pending, &refusal);
            return None;
        }
        let Some(read) = pending.remote_credentials.as_mut() else {
            // Its slot lease runs on the coordination worker, which ends it.
            return Some(pending);
        };
        match self.poll_remote_credential(read) {
            RemoteCredentialStep::Waiting => Some(pending),
            RemoteCredentialStep::Refused(refusal) => {
                self.refuse_remote_create(&pending, &refusal);
                None
            }
            RemoteCredentialStep::Answered => {
                let Some(read) = pending.remote_credentials.take() else {
                    return Some(pending);
                };
                self.plan_and_lease_remote(pending, read.actor, &read.target, &read.values)
            }
        }
    }

    /// Reads the workbench's answer to the create's current read, and asks for
    /// the next value when one arrived.
    fn poll_remote_credential(&mut self, read: &mut RemoteCredentialRead) -> RemoteCredentialStep {
        let Some(value) = read.reply.poll() else {
            return RemoteCredentialStep::Waiting;
        };
        match value {
            Ok(value) => read.values.insert(read.reference.clone(), value),
            Err(error) => {
                return RemoteCredentialStep::Refused(credential_refusal(&read.profile, &read.reference, error));
            }
        }
        let Some(next) = read.remaining.pop_front() else {
            return RemoteCredentialStep::Answered;
        };
        match self.remote_browser_credentials.request_value(&read.profile, &next) {
            Ok(reply) => {
                read.reference = next;
                read.reply = reply;
                RemoteCredentialStep::Waiting
            }
            Err(error) => RemoteCredentialStep::Refused(credential_refusal(&read.profile, &next, error)),
        }
    }
}

/// The reference to read first, and the references to read after it, or `None`
/// when the request needs no value from the OS store.
fn split_first_read(
    references: Vec<CredentialReference>,
) -> Option<(CredentialReference, VecDeque<CredentialReference>)> {
    let mut references = references.into_iter();
    let first = references.next()?;
    Some((first, references.collect()))
}

/// The refusal a failed workbench read produces, named after the reference it
/// answers so its code and text match the plan's own credential refusals.
fn credential_refusal(
    profile: &RemoteProviderProfile,
    reference: &CredentialReference,
    error: RemoteCredentialError,
) -> CreateRefusal {
    let variable = profile
        .credential_bindings
        .get(reference)
        .and_then(horizon_core::browser::remote::CredentialBinding::environment_variable)
        .map(str::to_string);
    CreateRefusal::for_credential(&ResolveError {
        reference: reference.clone(),
        variable,
        error,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use horizon_core::Config;
    use horizon_core::browser::remote::{CredentialBinding, CredentialReference, CredentialStoreKind};
    use horizon_core::remote_browser_credential::{
        CredentialLocator, CredentialWorkbench, FakeCredentialStore, RemoteCredentialError, RemoteCredentialStore,
    };
    use horizon_core::{PanelId, WorkspaceId};

    use super::*;
    use crate::app::browser_remote_create::{plan_remote_create, tests::config};
    use crate::app::test_support::test_app;

    /// `config()` with the provider's key kept in the OS store, so a create of
    /// its target reads the value there.
    fn os_config() -> Config {
        let mut config = config();
        let profile = config.browser.remote.providers.get_mut("grid").expect("provider");
        profile.credential_bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: Some("remote-browser/grid/key".into()),
            },
        );
        config
    }

    /// The OS store the provider's key lives in, holding `value` for it.
    fn os_store(value: Option<&[u8]>) -> FakeCredentialStore {
        let mut store = FakeCredentialStore::new();
        let Some(value) = value else { return store };
        let profile = os_config().browser.remote.providers["grid"].clone();
        let key = CredentialReference::from("key");
        let locator = CredentialLocator::new(&profile.endpoint, &key, &profile.credential_bindings[&key]);
        store.put(&locator, value).expect("stored value");
        store
    }

    /// A workbench whose OS store answers what `store` holds, on its own thread.
    fn os_workbench(store: FakeCredentialStore) -> CredentialWorkbench {
        CredentialWorkbench::with_opener(Box::new(move || {
            Ok(Box::new(store) as Box<dyn RemoteCredentialStore + Send>)
        }))
    }

    fn wait_for_os_store(app: &mut HorizonApp) {
        for _ in 0..200 {
            app.remote_browser_credentials.poll();
            if app.remote_browser_credentials.keychain_available() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the workbench did not open its store");
    }

    /// A create of the configured target, and the agent panel it is asked
    /// beside.
    fn remote_request(app: &mut HorizonApp) -> (BrowserCreateRequest, ActorPanel) {
        let workspace: WorkspaceId = app.board.create_workspace("alpha");
        let mut request = BrowserCreateRequest::for_tests("agent");
        request.target = Some("ios_phone".into());
        (
            request,
            ActorPanel {
                panel_id: PanelId(1),
                workspace_id: workspace,
            },
        )
    }

    /// Polls the create as the frames of a running Horizon do, until `done`
    /// holds. The coordination worker is left to the test.
    fn pump_until(app: &mut HorizonApp, mut done: impl FnMut(&HorizonApp) -> bool) {
        for _ in 0..600 {
            app.remote_browser_credentials.poll();
            app.finish_pending_browser_creates();
            if done(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the create did not reach the expected state");
    }

    /// The read of the configured target's key, as a create holds it while it
    /// waits for the value.
    fn reading_key(app: &mut HorizonApp) -> RemoteCredentialRead {
        let profile = os_config().browser.remote.providers["grid"].clone();
        let reference = CredentialReference::from("key");
        let reply = app
            .remote_browser_credentials
            .request_value(&profile, &reference)
            .expect("the store is available");
        RemoteCredentialRead {
            actor: remote_request(app).1,
            target: app.remote_target("ios_phone").expect("configured target"),
            profile,
            reference,
            reply,
            remaining: VecDeque::new(),
            values: ReadCredentialStore::new(),
        }
    }

    /// Waits for the answer the workbench gives one read, as the frames of a
    /// running Horizon would.
    fn read_answer(reply: &mut CredentialValueReply) -> Result<zeroize::Zeroizing<Vec<u8>>, RemoteCredentialError> {
        for _ in 0..200 {
            if let Some(answer) = reply.poll() {
                return answer;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the workbench did not answer the read");
    }

    /// The locked store a test reads a value from without an unlock.
    fn locked_store() -> FakeCredentialStore {
        let mut store = os_store(Some(b"tok-en"));
        store.lock();
        store
    }

    #[test]
    fn a_read_the_store_cannot_answer_refuses_with_the_credential_reason() {
        for (store, reason) in [
            (os_store(None), RemoteCredentialError::Missing),
            (locked_store(), RemoteCredentialError::Locked),
        ] {
            let (_temp, mut app) = test_app();
            app.template_config = os_config();
            app.remote_browser_credentials = os_workbench(store);
            wait_for_os_store(&mut app);
            let mut read = reading_key(&mut app);

            let error = read_answer(&mut read.reply).expect_err("the store cannot answer this read");
            let refusal = credential_refusal(&read.profile, &read.reference, error);
            assert_eq!(refusal.code, "credentials_not_ready");
            assert!(
                refusal.message.contains("credential `key`"),
                "it names the reference: {}",
                refusal.message
            );
            assert!(
                refusal.message.contains(&reason.to_string()),
                "it names the reason: {}",
                refusal.message
            );
            assert!(
                !refusal.message.contains("tok-en"),
                "no value leaks: {}",
                refusal.message
            );
        }
    }

    #[test]
    fn a_read_value_takes_the_plans_own_credential_checks() {
        let (_temp, app) = test_app();
        let remote = os_config().browser.remote;
        let key = CredentialReference::from("key");
        let plan_with = |value: Option<&[u8]>| {
            let mut values = ReadCredentialStore::new();
            if let Some(value) = value {
                values.insert(key.clone(), zeroize::Zeroizing::new(value.to_vec()));
            }
            plan_remote_create(
                &remote,
                &app.remote_browser_credentials,
                (!values.is_empty()).then_some(&values),
                "ios_phone",
            )
        };

        let missing = plan_with(None).expect_err("no value was read");
        assert_eq!(missing.code, "credentials_not_ready");
        assert!(
            missing
                .message
                .contains(&RemoteCredentialError::StoreUnavailable.to_string())
        );
        let malformed = plan_with(Some(b"line\n")).expect_err("the value cannot form a header");
        assert_eq!(malformed.code, "credentials_invalid");
        assert!(
            malformed
                .message
                .contains(&RemoteCredentialError::NotHeaderSafe.to_string())
        );
        assert!(
            !malformed.message.contains("line"),
            "no value leaks: {}",
            malformed.message
        );
        plan_with(Some(b"tok-en")).expect("a header-safe value plans the request");
    }

    #[test]
    fn a_missing_or_locked_os_credential_ends_the_create_without_a_panel() {
        for store in [os_store(None), locked_store()] {
            let (_temp, mut app) = test_app();
            app.template_config = os_config();
            app.remote_browser_credentials = os_workbench(store);
            wait_for_os_store(&mut app);
            let (request, actor) = remote_request(&mut app);
            let panels = app.board.panels.len();

            app.begin_remote_create(request, actor, Instant::now());
            assert_eq!(
                app.browser_create_host.pending[0].stage,
                CreateStage::ReadingCredentials,
                "the value is read before any panel exists"
            );
            pump_until(&mut app, |app| app.browser_create_host.pending.is_empty());
            assert!(
                app.browser_create_host.io.busy(),
                "the agent's typed refusal is queued for the worker"
            );
            app.settle_browser_host_io();
            assert!(!app.browser_create_host.io.busy(), "the refusal was written");
            assert_eq!(app.board.panels.len(), panels, "a refused create opens no panel");
        }
    }

    /// A credentialed create for a provider that manages its own capacity
    /// plans inside the frame that advances it, and that frame must keep the
    /// pending entry of the audit the plan pushed: a frame restores the list
    /// it kept, so it must join that list instead of replacing it.
    #[test]
    fn a_create_that_plans_inside_the_frame_keeps_the_pending_entry_of_its_audit() {
        let (_temp, mut app) = test_app();
        let mut config = os_config();
        config
            .browser
            .remote
            .providers
            .get_mut("grid")
            .expect("provider")
            .adapter = horizon_core::browser::remote::RemoteAdapterKind::Browserstack;
        app.template_config = config;
        app.remote_browser_credentials = os_workbench(os_store(Some(b"tok-en")));
        wait_for_os_store(&mut app);
        let (request, actor) = remote_request(&mut app);
        let id = request.request_id.clone();
        let panels = app.board.panels.len();

        app.begin_remote_create(request, actor, Instant::now());
        pump_until(&mut app, |app| app.board.panels.len() > panels);
        assert!(
            app.browser_create_host
                .pending
                .iter()
                .any(|pending| pending.request.request_id == id),
            "the audit pending entry survives the frame that planned the create"
        );
        app.settle_browser_host_io();
        assert!(
            !app.browser_create_host.io.busy(),
            "the audit and its result run on the worker"
        );
    }

    #[test]
    fn a_store_that_never_opens_stalls_no_frame() {
        let (_temp, mut app) = test_app();
        app.template_config = os_config();
        let (open, opened) = channel::<()>();
        app.remote_browser_credentials = CredentialWorkbench::with_opener(Box::new(move || {
            // A store that waits for the user stands here until the test ends.
            let _ = opened.recv();
            Ok(Box::new(FakeCredentialStore::new()) as Box<dyn RemoteCredentialStore + Send>)
        }));
        let (request, actor) = remote_request(&mut app);
        let panels = app.board.panels.len();

        let started = Instant::now();
        app.begin_remote_create(request, actor, Instant::now());
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the credential store"
        );
        assert!(
            !app.remote_browser_credentials.keychain_available(),
            "the store is still opening"
        );

        pump_until(&mut app, |app| app.browser_create_host.pending.is_empty());
        app.settle_browser_host_io();
        assert_eq!(app.board.panels.len(), panels, "an unopened store opens no panel");
        open.send(()).expect("the parked opener ends with the test");
    }

    #[test]
    fn a_create_plans_from_the_value_the_workbench_read() {
        let (_temp, mut app) = test_app();
        app.template_config = os_config();
        app.remote_browser_credentials = os_workbench(os_store(Some(b"tok-en")));
        wait_for_os_store(&mut app);
        let (request, actor) = remote_request(&mut app);
        let panels = app.board.panels.len();
        // The lease is taken on the coordination worker; holding it keeps the
        // create at its slot instead of opening a browser panel the test
        // cannot run.
        let (release, released) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        app.begin_remote_create(request, actor, Instant::now());
        let pending = app.browser_create_host.pending.first().expect("the create waits");
        assert_eq!(pending.stage, CreateStage::ReadingCredentials);
        assert!(pending.remote_credentials.is_some(), "it waits for the read value");
        pump_until(&mut app, |app| {
            app.browser_create_host
                .pending
                .first()
                .is_some_and(|pending| pending.stage == CreateStage::Leasing)
        });
        let pending = app.browser_create_host.pending.first().expect("the create waits");
        assert!(
            pending.remote_credentials.is_none(),
            "the plan kept the value it read, not the read"
        );
        assert_eq!(
            app.board.panels.len(),
            panels,
            "no panel opens before its provider slot is leased"
        );

        // The create ends before its lease applies: the lease is dropped here
        // and no panel opens on a board that still stands.
        app.browser_create_host.pending.clear();
        release.send(()).expect("release the coordination worker");
        app.settle_browser_host_io();
        assert!(app.browser_create_host.pending.is_empty());
        assert_eq!(app.board.panels.len(), panels);
    }

    #[test]
    fn a_read_of_a_replaced_board_opens_no_panel_and_answers_once() {
        let (_temp, mut app) = test_app();
        app.template_config = os_config();
        app.remote_browser_credentials = os_workbench(os_store(Some(b"tok-en")));
        wait_for_os_store(&mut app);
        let (request, actor) = remote_request(&mut app);
        let panels = app.board.panels.len();
        app.begin_remote_create(request, actor, Instant::now());
        assert_eq!(app.browser_create_host.pending.len(), 1);

        // A session switch or a cloud prototype restore replaces the board.
        app.browser_create_host.io.replace_board();
        app.finish_pending_browser_creates();
        assert!(
            app.browser_create_host.pending.is_empty(),
            "the create of the replaced board ended with it"
        );
        assert!(app.browser_create_host.io.busy(), "its request is still answered, once");
        app.settle_browser_host_io();
        assert_eq!(
            app.board.panels.len(),
            panels,
            "a read of a replaced board opens no panel on the one that stands now"
        );
    }
}
