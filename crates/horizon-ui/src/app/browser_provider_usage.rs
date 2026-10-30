//! Host bridge for read-only provider usage requests from MCP and CLI plans.
//! The claim of the agents' requests runs on the coordination worker, and so
//! does every answer, so a slow disk never stalls a frame.
use std::collections::BTreeMap;

use horizon_core::browser::manifest::{
    self,
    provider_usage::{ProviderUsageSummary, UsageQueue, UsageRequest, UsageResult},
};
use horizon_core::browser::remote::RemoteProviderProfile;
use horizon_core::browser::remote_usage::ProviderUsageMonitor;

use super::answer_retries::{RefusedAnswer, RefusedAnswers};
use super::{HorizonApp, browser_requests::actor_panel};

#[derive(Default)]
pub(super) struct UsageHostState {
    pending: Vec<PendingUsage>,
    /// A claim of the agents' requests runs on the coordination worker.
    claiming: bool,
    /// Answers the coordination worker could not write, for a later frame.
    refused: RefusedAnswers<UsageResult>,
    /// The queue this host claims its requests from and answers them on. A
    /// test polls its own root, and the answers follow it.
    queue: UsageQueue,
    #[cfg(feature = "cloud-workspaces")]
    pub(in crate::app) image_destinations: horizon_core::cloud_runtime::publishing::operations::Manager,
    #[cfg(feature = "cloud-workspaces")]
    pub(in crate::app) tailnet_credentials: super::tailnets::requests::Requests,
    #[cfg(feature = "cloud-workspaces")]
    pub(in crate::app) browser_vm: super::browser_vm_requests::State,
}

struct PendingUsage {
    request: UsageRequest,
    providers: BTreeMap<String, ProviderUsageMonitor>,
}

impl HorizonApp {
    /// Answers `result` on the coordination worker, on the queue the request
    /// was claimed from. A write that fails keeps the result, which a later
    /// frame writes again until `deadline_at_millis`. A request is answered
    /// once: a successful write removes the request file, so a later write of
    /// the same answer finds no request file and writes nothing.
    pub(in crate::app) fn publish_host_usage_result(&mut self, result: UsageResult, deadline_at_millis: i64) {
        let request_id = result.request_id.clone();
        self.write_host_usage_result(RefusedAnswer::new(result, request_id, deadline_at_millis));
    }

    /// Writes one answer on the coordination worker. A write that fails, or a
    /// job that panics, keeps the answer: the request stays claimed, so the
    /// host writes the same result again rather than claim the request again.
    fn write_host_usage_result(&mut self, answer: RefusedAnswer<UsageResult>) {
        let queue = self.browser_create_host.provider_usage.queue.clone();
        let writing = answer.result.clone();
        self.browser_create_host.io.then(
            move || queue.complete(&writing),
            move |app, outcome| {
                if matches!(outcome, Some(Ok(()))) {
                    return;
                }
                tracing::debug!(
                    request_id = %answer.request_id,
                    "the coordination worker could not answer a host metadata request yet"
                );
                app.browser_create_host.provider_usage.refused.keep(answer);
            },
        );
    }

    /// Writes the answers that the coordination worker could not write again,
    /// at most every `RETRY_INTERVAL` and until each request's deadline.
    fn retry_host_usage_answers(&mut self) -> bool {
        let due = self
            .browser_create_host
            .provider_usage
            .refused
            .due(manifest::now_millis());
        let changed = !due.is_empty();
        for answer in due {
            self.write_host_usage_result(answer);
        }
        changed
    }

    #[cfg(feature = "cloud-workspaces")]
    fn poll_tailnet_credential_requests(&mut self) {
        self.browser_create_host
            .provider_usage
            .tailnet_credentials
            .poll(manifest::now_millis());
        if let Some(editor) = &mut self.settings {
            editor
                .tailnet_status_state()
                .discard_cancelled_api_draft(&self.browser_create_host.provider_usage.tailnet_credentials);
        }
    }

    pub(super) fn poll_provider_usage(&mut self, ctx: &egui::Context) -> bool {
        self.poll_provider_usage_queue(&UsageQueue::default(), ctx)
    }

    fn poll_provider_usage_queue(&mut self, queue: &UsageQueue, ctx: &egui::Context) -> bool {
        self.browser_create_host.provider_usage.queue = queue.clone();
        #[cfg(feature = "cloud-workspaces")]
        self.poll_tailnet_credential_requests();
        #[cfg(feature = "cloud-workspaces")]
        self.poll_image_destination_operations(ctx);
        #[cfg(feature = "cloud-workspaces")]
        self.poll_browser_vm(ctx);
        self.claim_provider_usage(queue, ctx);
        let mut changed = self.finish_provider_usage();
        changed |= self.retry_host_usage_answers();
        changed
    }

    /// Claims the agents' usage, catalog and cloud offer requests on the
    /// coordination worker, then starts what they ask for. One claim runs at a
    /// time, so a slow disk never queues one per tick.
    fn claim_provider_usage(&mut self, queue: &UsageQueue, ctx: &egui::Context) {
        if self.browser_create_host.provider_usage.claiming {
            return;
        }
        self.browser_create_host.provider_usage.claiming = true;
        let claiming = queue.clone();
        let host = manifest::host_instance().to_string();
        let ctx = ctx.clone();
        self.browser_create_host.io.then(
            move || claim_usage(&claiming, &host),
            move |app, requests| {
                app.browser_create_host.provider_usage.claiming = false;
                app.dispatch_usage_requests(requests.unwrap_or_default(), &ctx);
                app.finish_provider_usage();
                // The catalog answers what the claim took on this frame, not on the next tick.
                app.poll_provider_catalog();
            },
        );
    }

    /// Starts what each claimed request asks for, on this frame.
    fn dispatch_usage_requests(&mut self, requests: Vec<UsageRequest>, ctx: &egui::Context) {
        for request in requests {
            if request.image_destination.is_some() {
                self.answer_image_destination(&request, ctx);
                continue;
            }
            if request.tailnet_credential.is_some() {
                self.answer_tailnet_credential(&request);
                continue;
            }
            if request.cloud_companion.is_some() {
                self.queue_cloud_companion(request);
                continue;
            }
            if request.cloud_list.is_some() {
                self.queue_cloud_list(request);
                continue;
            }
            if request.cloud_create.is_some() {
                self.queue_cloud_create(request);
                continue;
            }
            if request.cloud_delete.is_some() {
                self.queue_cloud_delete(request);
                continue;
            }
            if request.browser_vm.is_some() {
                self.queue_browser_vm(request, ctx);
                continue;
            }
            if request.cloud_exec.is_some() {
                self.queue_cloud_exec(request, ctx);
                continue;
            }
            if request.cloud_offers.is_some() {
                self.browser_create_host.cloud_offers.push(request);
                continue;
            }
            if request.catalog.is_some() {
                self.browser_create_host.catalog.pending.push(request);
                continue;
            }
            let providers = self
                .template_config
                .browser
                .remote
                .providers
                .keys()
                .filter(|name| request.provider.as_ref().is_none_or(|selected| selected == *name))
                .map(|name| (name.clone(), ProviderUsageMonitor::default()))
                .collect();
            self.browser_create_host
                .provider_usage
                .pending
                .push(PendingUsage { request, providers });
        }
    }

    fn finish_provider_usage(&mut self) -> bool {
        let pending = std::mem::take(&mut self.browser_create_host.provider_usage.pending);
        let mut changed = false;
        for mut pending in pending {
            let timed_out = manifest::now_millis() >= pending.request.deadline_at_millis.saturating_sub(3_000);
            let error = if pending.request.host_instance != manifest::host_instance()
                || actor_panel(&self.board, &pending.request.actor).is_none()
            {
                Some("provider_usage_unavailable")
            } else if pending
                .request
                .provider
                .as_ref()
                .is_some_and(|name| !self.template_config.browser.remote.providers.contains_key(name))
            {
                Some("provider_unknown")
            } else {
                None
            };
            let mut summaries = Vec::new();
            let mut refreshing = false;
            if error.is_none() {
                for (name, monitor) in &mut pending.providers {
                    if let Some(profile) = self.template_config.browser.remote.providers.get(name) {
                        if timed_out {
                            monitor.poll(profile);
                        } else {
                            monitor.update(profile, &self.remote_browser_credentials, false);
                        }
                        refreshing |= monitor.refreshing();
                        let mut summary = usage_summary(name, profile, monitor);
                        if timed_out && (monitor.refreshing() || (monitor.sample.is_none() && monitor.error.is_none()))
                        {
                            summary.error = Some("provider_usage_timed_out".into());
                        }
                        summaries.push(summary);
                    }
                }
            }
            if error.is_none() && refreshing && !timed_out {
                self.browser_create_host.provider_usage.pending.push(pending);
                continue;
            }
            self.publish_host_usage_result(
                pending.request.result(summaries, error.map(str::to_string)),
                pending.request.deadline_at_millis,
            );
            changed = true;
        }
        changed
    }
}

impl HorizonApp {
    #[cfg(not(feature = "cloud-workspaces"))]
    fn answer_image_destination(&mut self, request: &UsageRequest, _ctx: &egui::Context) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("image_destination_unavailable: cloud support is disabled".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    fn answer_tailnet_credential(&mut self, request: &UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("tailnet_credential_unavailable: cloud support is disabled".into()),
        );
        self.publish_host_usage_result(result, request.deadline_at_millis);
    }
}

fn usage_summary(name: &str, profile: &RemoteProviderProfile, monitor: &ProviderUsageMonitor) -> ProviderUsageSummary {
    ProviderUsageSummary {
        provider: name.to_string(),
        supported: ProviderUsageMonitor::supported(profile),
        local_session_limit: profile.local_session_limit(),
        running: monitor.sample.map(|(usage, _)| usage.running),
        allowed: monitor.sample.map(|(usage, _)| usage.allowed),
        queued: monitor.sample.map(|(usage, _)| usage.queued),
        sampled_at_millis: monitor.sample.map(|(_, at)| {
            manifest::now_millis().saturating_sub(i64::try_from(at.elapsed().as_millis()).unwrap_or(i64::MAX))
        }),
        error: monitor.error.map(|error| error.to_string()),
    }
}

impl HorizonApp {
    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_companion(&mut self, request: UsageRequest) {
        self.queue_cloud_companion_request(request);
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_list(&mut self, request: UsageRequest) {
        self.queue_cloud_list_request(request);
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_exec(&mut self, request: UsageRequest, ctx: &egui::Context) {
        self.start_cloud_exec(request, ctx);
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_cloud_exec(&mut self, request: UsageRequest, _ctx: &egui::Context) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("cloud_exec_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_cloud_list(&mut self, request: UsageRequest) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("cloud_list_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_browser_vm(&mut self, request: UsageRequest, _ctx: &egui::Context) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("browser_vm_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_create(&mut self, request: UsageRequest) {
        self.queue_cloud_create_request(request);
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_cloud_create(&mut self, request: UsageRequest) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("cloud_create_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_delete(&mut self, request: UsageRequest) {
        self.queue_cloud_delete_request(request);
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_cloud_delete(&mut self, request: UsageRequest) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("cloud_delete_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::needless_pass_by_value)]
    fn queue_cloud_companion(&mut self, request: UsageRequest) {
        self.publish_host_usage_result(
            request.result(
                Vec::new(),
                Some("cloud_companion_unavailable: this Horizon build has no cloud support".into()),
            ),
            request.deadline_at_millis,
        );
    }
}

/// The requests the claim took, or none with a warning: a queue that cannot be
/// read leaves its requests for a later tick.
fn claim_usage(queue: &UsageQueue, host: &str) -> Vec<UsageRequest> {
    match queue.claim(host) {
        Ok(requests) => requests,
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), "could not poll provider usage requests");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::answer_retries::RETRY_INTERVAL;
    use horizon_core::browser::manifest::AgentIdentity;
    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialReference, RemoteAdapterKind, RemoteAuthentication, RemoteSessionLimits,
    };
    use horizon_core::{PanelKind, PanelOptions, browser_actor};

    /// Polls the queue and applies what the coordination worker answered, as a
    /// later frame does.
    fn poll_usage(app: &mut HorizonApp, queue: &UsageQueue) {
        app.poll_provider_usage_queue(queue, &egui::Context::default());
        app.settle_browser_host_io();
    }

    #[test]
    fn authorized_agents_can_query_multiple_profiles_without_allocating_or_opening_settings() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("usage");
        let (command, args) = ("/bin/sh", vec!["-c".into(), "exit 0".into()]);
        let agent = app
            .board
            .create_panel(
                PanelOptions {
                    command: Some(command.into()),
                    args,
                    kind: PanelKind::Codex,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("agent");
        let actor = browser_actor(&app.board.panel(agent).expect("panel").local_id);
        for (name, adapter) in [
            ("cloud-a", RemoteAdapterKind::Browserstack),
            ("cloud-b", RemoteAdapterKind::Browserstack),
            ("grid", RemoteAdapterKind::Webdriver),
        ] {
            app.template_config.browser.remote.providers.insert(
                name.into(),
                RemoteProviderProfile {
                    adapter,
                    endpoint: ControlEndpoint::parse("https://grid.example.test").expect("endpoint"),
                    authentication: RemoteAuthentication::Bearer {
                        token_ref: CredentialReference::from(name),
                    },
                    credential_bindings: BTreeMap::new(),
                    limits: RemoteSessionLimits::default(),
                },
            );
        }
        let queue = UsageQueue::new(temp.path().to_path_buf());
        let identity = AgentIdentity::new(&actor, Some(manifest::host_instance()));
        let id = queue.enqueue(identity, None).expect("all providers");
        poll_usage(&mut app, &queue);
        let result = queue.take(identity, &id).expect("result").expect("completed");
        assert!(result.error.is_none());
        assert_eq!(result.providers.len(), 3);
        assert!(result.providers[0].supported);
        assert!(
            result.providers[0].running.is_none(),
            "missing credentials are not zero usage"
        );
        assert!(result.providers[0].error.is_some());
        assert_eq!(result.providers[2].local_session_limit, Some(1));
        assert!(!result.providers[2].supported);
        assert!(app.settings.is_none());
        assert!(app.browser_create_host.remote_allocations.summaries(None).is_empty());
        let id = queue.enqueue(identity, Some("cloud-b".into())).expect("one provider");
        poll_usage(&mut app, &queue);
        let result = queue.take(identity, &id).expect("result").expect("completed");
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].provider, "cloud-b");
        let id = queue
            .enqueue(identity, Some("unknown".into()))
            .expect("unknown provider");
        poll_usage(&mut app, &queue);
        assert_eq!(
            queue
                .take(identity, &id)
                .expect("result")
                .expect("completed")
                .error
                .as_deref(),
            Some("provider_unknown")
        );
        assert_partial_timeout(&mut app, &queue, identity);
        assert_removed_profile(&mut app, &queue, identity);
        let foreign = AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
        let id = queue.enqueue(foreign, None).expect("stale agent");
        poll_usage(&mut app, &queue);
        let result = queue.take(foreign, &id).expect("result").expect("refused");
        assert_eq!(result.error.as_deref(), Some("provider_usage_unavailable"));
        assert!(result.providers.is_empty());
    }
    fn assert_removed_profile(app: &mut HorizonApp, queue: &UsageQueue, identity: AgentIdentity<'_>) {
        let id = queue
            .enqueue(identity, Some("cloud-b".into()))
            .expect("selected profile");
        let request = queue.claim(manifest::host_instance()).expect("claim").remove(0);
        app.browser_create_host.provider_usage.pending.push(PendingUsage {
            request,
            providers: [("cloud-b".into(), ProviderUsageMonitor::default())].into(),
        });
        app.template_config.browser.remote.providers.remove("cloud-b");
        poll_usage(app, queue);
        let result = queue.take(identity, &id).expect("result").expect("completed");
        assert_eq!(result.error.as_deref(), Some("provider_unknown"));
        assert!(result.providers.is_empty());
    }
    fn assert_partial_timeout(app: &mut HorizonApp, queue: &UsageQueue, identity: AgentIdentity<'_>) {
        let id = queue.enqueue(identity, None).expect("deadline request");
        let mut request = queue.claim(manifest::host_instance()).expect("claim").remove(0);
        request.deadline_at_millis = manifest::now_millis() + 1_000;
        let sample = horizon_core::browser::remote_usage::ProviderUsage {
            running: 2,
            allowed: 5,
            queued: 0,
        };
        let mut completed = ProviderUsageMonitor::default();
        completed.update(
            &app.template_config.browser.remote.providers["cloud-a"],
            &app.remote_browser_credentials,
            false,
        );
        completed.sample = Some((sample, std::time::Instant::now()));
        completed.error = None;
        app.browser_create_host.provider_usage.pending.push(PendingUsage {
            request,
            providers: [
                ("cloud-a".into(), completed),
                ("cloud-b".into(), ProviderUsageMonitor::default()),
            ]
            .into(),
        });
        poll_usage(app, queue);
        let result = queue.take(identity, &id).expect("result").expect("partial completion");
        assert!(result.error.is_none());
        assert_eq!(result.providers[0].running, Some(2));
        assert_eq!(result.providers[1].error.as_deref(), Some("provider_usage_timed_out"));
    }

    /// Holds `path` with an OS file lock from a helper thread until the test
    /// signals it, as a slow disk does.
    #[cfg(unix)]
    fn hold_lock(path: &std::path::Path) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        std::fs::create_dir_all(path.parent().expect("lock directory")).expect("lock directory");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .expect("lock file");
        file.lock().expect("hold the lock");
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _held = file;
            // The bound keeps a failing assertion from leaving the lock held.
            let _ = released.recv_timeout(std::time::Duration::from_secs(10));
        });
        (release, holder)
    }

    #[cfg(unix)]
    #[test]
    fn a_held_queue_lock_never_stalls_the_frame_that_claims() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        let identity = AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
        let id = queue.enqueue(identity, None).expect("usage request");
        let (release, holder) = hold_lock(&temp.path().join("usage/runtime/browser-provider-usage/queue.json.lock"));

        let started = std::time::Instant::now();
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "the claim stalled the frame: {elapsed:?}"
        );
        assert!(
            app.browser_create_host.provider_usage.claiming,
            "the claim guards the frames that follow it"
        );
        // The claim waits for the lock on the worker, and the frame goes on.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let waiting = app.browser_create_host.io.busy();
        release.send(()).expect("the lock holder waits");
        assert!(waiting, "the claim waited on the worker while the lock was held");

        let answer = loop {
            poll_usage(&mut app, &queue);
            if let Some(answer) = queue.take(identity, &id).expect("result") {
                break answer;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "the request was never answered"
            );
        };
        holder.join().expect("the lock holder ends");
        assert_eq!(
            answer.error.as_deref(),
            Some("provider_usage_unavailable"),
            "the claimed request was answered on the coordination worker"
        );
    }

    #[test]
    fn a_claim_in_flight_is_not_queued_again() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        // The claim of an earlier frame still waits on the worker.
        app.browser_create_host.provider_usage.claiming = true;

        app.poll_provider_usage_queue(&queue, &egui::Context::default());

        assert!(
            !app.browser_create_host.io.busy(),
            "a slow claim is not queued again on the next tick"
        );
        assert!(
            app.browser_create_host.provider_usage.claiming,
            "a frame that queued no claim keeps the guard of the claim it found"
        );
    }

    #[test]
    fn an_answer_is_written_once_so_a_later_answer_for_the_request_writes_nothing() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        app.browser_create_host.provider_usage.queue = queue.clone();
        let identity = AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
        let id = queue.enqueue(identity, None).expect("usage request");
        let request = queue.claim(manifest::host_instance()).expect("claim").remove(0);

        app.publish_host_usage_result(
            request.result(Vec::new(), Some("the first answer".into())),
            request.deadline_at_millis,
        );
        app.settle_browser_host_io();
        assert_eq!(
            queue
                .take(identity, &id)
                .expect("result")
                .expect("answered")
                .error
                .as_deref(),
            Some("the first answer")
        );

        // The answered request file is gone, so a later answer writes nothing.
        app.publish_host_usage_result(
            request.result(Vec::new(), Some("a later answer".into())),
            request.deadline_at_millis,
        );
        app.settle_browser_host_io();
        assert!(
            queue.take(identity, &id).expect("result").is_none(),
            "a request is answered once"
        );
    }

    /// A queue whose root is a file: a write to it fails at once, as a disk
    /// that refuses the coordination file does.
    fn unavailable_queue(root: &std::path::Path) -> UsageQueue {
        std::fs::create_dir_all(root.join("runtime")).expect("runtime");
        std::fs::write(root.join("runtime/browser-provider-usage"), b"fixture").expect("fixture");
        UsageQueue::new(root.to_path_buf())
    }

    /// A lock that outlives the wait of a write makes the write fail. The
    /// request stays claimed, so the host writes the same answer again while
    /// the request is still waiting.
    #[cfg(unix)]
    #[test]
    fn a_refused_answer_is_written_after_the_lock_is_released() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        app.browser_create_host.provider_usage.queue = queue.clone();
        let identity = AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
        let id = queue.enqueue(identity, None).expect("usage request");
        let request = queue.claim(manifest::host_instance()).expect("claim").remove(0);
        let (release, holder) = hold_lock(&temp.path().join("usage/runtime/browser-provider-usage/queue.json.lock"));

        app.publish_host_usage_result(
            request.result(Vec::new(), Some("the answer".into())),
            request.deadline_at_millis,
        );
        // The write waits out the lock's bound and then fails.
        app.settle_browser_host_io();
        assert!(
            queue.take(identity, &id).expect("result").is_none(),
            "the answer waits for the lock"
        );

        release.send(()).expect("the lock holder waits");
        holder.join().expect("the lock holder ends");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let answer = loop {
            assert!(
                std::time::Instant::now() < deadline,
                "the refused answer was never written"
            );
            std::thread::sleep(RETRY_INTERVAL);
            poll_usage(&mut app, &queue);
            if let Some(answer) = queue.take(identity, &id).expect("result") {
                break answer;
            }
        };
        assert_eq!(answer.error.as_deref(), Some("the answer"));
    }

    /// A retry after the request deadline writes nothing: the agent has its own
    /// timeout then, and the request is not answered a second time.
    #[test]
    fn a_refused_answer_is_not_written_after_its_deadline() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        let identity = AgentIdentity::new("horizon:missing-agent", Some(manifest::host_instance()));
        let id = queue.enqueue(identity, None).expect("usage request");
        let mut request = queue.claim(manifest::host_instance()).expect("claim").remove(0);
        request.deadline_at_millis = manifest::now_millis();
        // The first write cannot reach the queue, so the answer is kept.
        app.browser_create_host.provider_usage.queue = unavailable_queue(&temp.path().join("refused"));
        app.publish_host_usage_result(
            request.result(Vec::new(), Some("the answer".into())),
            request.deadline_at_millis,
        );
        app.settle_browser_host_io();

        // The retry finds the deadline past and drops the answer instead of
        // writing it, although the queue can be written now.
        app.browser_create_host.provider_usage.queue = queue.clone();
        poll_usage(&mut app, &queue);
        assert!(
            queue.take(identity, &id).expect("result").is_none(),
            "a retry after the request deadline writes nothing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_held_usage_lock_never_stalls_the_cloud_offer_poll() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let queue = UsageQueue::new(temp.path().join("usage"));
        let ctx = egui::Context::default();
        // The host answers on the queue the offers are claimed from.
        app.poll_provider_usage_queue(&queue, &ctx);
        app.settle_browser_host_io();
        let request: UsageRequest = serde_json::from_value(serde_json::json!({
            "request_id": "offers", "actor": "horizon:missing-agent",
            "host_instance": manifest::host_instance(), "deadline_at_millis": i64::MAX,
            "cloud_offers": {}, "claimed": true
        }))
        .expect("offer request");
        app.browser_create_host.cloud_offers.push(request);
        let (release, holder) = hold_lock(&temp.path().join("usage/runtime/browser-provider-usage/queue.json.lock"));

        let started = std::time::Instant::now();
        let answered = app.poll_cloud_offers(&ctx);
        let elapsed = started.elapsed();
        let writing = app.browser_create_host.io.busy();
        release.send(()).expect("the lock holder waits");
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "the offer poll stalled the frame: {elapsed:?}"
        );
        assert!(writing, "the answer is written on the coordination worker");
        assert!(answered, "the offer was answered");

        app.settle_browser_host_io();
        holder.join().expect("the lock holder ends");
        assert!(
            app.browser_create_host.cloud_offers.is_empty(),
            "the answered offer is not polled again"
        );
    }

    #[test]
    #[cfg(feature = "cloud-workspaces")]
    fn destination_operations_are_polled_even_when_the_usage_queue_is_unavailable() {
        use horizon_core::cloud_runtime::publishing::operations::LaunchOwner;
        let (temp, mut app) = crate::app::test_support::test_app();
        let (notify, received) = std::sync::mpsc::sync_channel(1);
        app.browser_create_host
            .provider_usage
            .image_destinations
            .dispatch_launch(
                LaunchOwner {
                    actor: "horizon:owned-poll".into(),
                    host: manifest::host_instance().into(),
                    root: temp.path().join("cloud"),
                    directory: temp.path().join("missing-checkout"),
                },
                "owned-queue-failure",
                Some("cpu".into()),
                serde_json::json!({"operation":"list"}),
                std::sync::Arc::new(move || {
                    let _ = notify.try_send(());
                }),
            )
            .unwrap();
        received.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(
            app.browser_create_host.provider_usage.image_destinations.running(),
            "the worker report still needs polling"
        );
        let unavailable = temp.path().join("not-a-directory");
        std::fs::create_dir_all(unavailable.join("runtime")).unwrap();
        std::fs::write(unavailable.join("runtime/browser-provider-usage"), b"fixture").unwrap();
        let queue = UsageQueue::new(unavailable);
        assert!(queue.claim(manifest::host_instance()).is_err());
        poll_usage(&mut app, &queue);
        assert!(
            !app.browser_create_host.provider_usage.image_destinations.running(),
            "queue failure must not suppress operation completion, scope or expiry polling"
        );
    }
}
