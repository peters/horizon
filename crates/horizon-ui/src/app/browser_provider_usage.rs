//! Host bridge for read-only provider usage requests from MCP and CLI plans.
use std::collections::BTreeMap;

use horizon_core::browser::manifest::{
    self,
    provider_usage::{ProviderUsageSummary, UsageQueue, UsageRequest, UsageResult},
};
use horizon_core::browser::remote::RemoteProviderProfile;
use horizon_core::browser::remote_usage::ProviderUsageMonitor;

use super::{HorizonApp, browser_requests::actor_panel};

#[derive(Default)]
pub(super) struct UsageHostState {
    pending: Vec<PendingUsage>,
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
    pub(in crate::app) fn publish_host_usage_result(&mut self, result: UsageResult) {
        self.browser_create_host.io.write(move || {
            if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
                tracing::warn!(kind = ?error.kind(), "could not answer a host metadata request");
            }
        });
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
        let requests = match queue.claim(manifest::host_instance()) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(kind = ?error.kind(), "could not poll provider usage requests");
                Vec::new()
            }
        };
        #[cfg(feature = "cloud-workspaces")]
        self.poll_tailnet_credential_requests();
        #[cfg(feature = "cloud-workspaces")]
        self.poll_image_destination_operations(ctx);
        #[cfg(feature = "cloud-workspaces")]
        self.poll_browser_vm(ctx);
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
        self.finish_provider_usage(queue)
    }

    fn finish_provider_usage(&mut self, queue: &UsageQueue) -> bool {
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
            if queue
                .complete(&pending.request.result(summaries, error.map(str::to_string)))
                .is_err()
            {
                self.browser_create_host.provider_usage.pending.push(pending);
            } else {
                changed = true;
            }
        }
        changed
    }
}

impl HorizonApp {
    #[cfg(not(feature = "cloud-workspaces"))]
    fn answer_image_destination(&mut self, request: &UsageRequest, _ctx: &egui::Context) {
        self.publish_host_usage_result(request.result(
            Vec::new(),
            Some("image_destination_unavailable: cloud support is disabled".into()),
        ));
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    fn answer_tailnet_credential(&mut self, request: &UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("tailnet_credential_unavailable: cloud support is disabled".into()),
        );
        self.publish_host_usage_result(result);
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
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_cloud_exec(&mut self, request: UsageRequest, _ctx: &egui::Context) {
        let result = request.result(
            Vec::new(),
            Some("cloud_exec_unavailable: this Horizon build has no cloud support".into()),
        );
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a cloud command request");
        }
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_cloud_list(&mut self, request: UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("cloud_list_unavailable: this Horizon build has no cloud support".into()),
        );
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a cloud list request");
        }
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_browser_vm(&mut self, request: UsageRequest, _ctx: &egui::Context) {
        let result = request.result(Vec::new(), Some(super::settings::BROWSER_VM_UNAVAILABLE.into()));
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a browser VM request");
        }
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_create(&mut self, request: UsageRequest) {
        self.queue_cloud_create_request(request);
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_cloud_create(&mut self, request: UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("cloud_create_unavailable: this Horizon build has no cloud support".into()),
        );
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a cloud create request");
        }
    }

    #[cfg(feature = "cloud-workspaces")]
    fn queue_cloud_delete(&mut self, request: UsageRequest) {
        self.queue_cloud_delete_request(request);
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_cloud_delete(&mut self, request: UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("cloud_delete_unavailable: this Horizon build has no cloud support".into()),
        );
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a cloud delete request");
        }
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    fn queue_cloud_companion(&mut self, request: UsageRequest) {
        let result = request.result(
            Vec::new(),
            Some("cloud_companion_unavailable: this Horizon build has no cloud support".into()),
        );
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer a cloud companion request");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::browser::manifest::AgentIdentity;
    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialReference, RemoteAdapterKind, RemoteAuthentication, RemoteSessionLimits,
    };
    use horizon_core::{PanelKind, PanelOptions, browser_actor};

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
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
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
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
        let result = queue.take(identity, &id).expect("result").expect("completed");
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].provider, "cloud-b");
        let id = queue
            .enqueue(identity, Some("unknown".into()))
            .expect("unknown provider");
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
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
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
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
        app.poll_provider_usage_queue(queue, &egui::Context::default());
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
        app.poll_provider_usage_queue(queue, &egui::Context::default());
        let result = queue.take(identity, &id).expect("result").expect("partial completion");
        assert!(result.error.is_none());
        assert_eq!(result.providers[0].running, Some(2));
        assert_eq!(result.providers[1].error.as_deref(), Some("provider_usage_timed_out"));
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
        app.poll_provider_usage_queue(&queue, &egui::Context::default());
        assert!(
            !app.browser_create_host.provider_usage.image_destinations.running(),
            "queue failure must not suppress operation completion, scope or expiry polling"
        );
    }
}
