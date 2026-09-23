//! Host bridge for read-only provider usage requests from MCP and CLI plans.
use std::collections::BTreeMap;

use horizon_core::browser::manifest::{
    self,
    provider_usage::{ProviderUsageSummary, UsageQueue, UsageRequest},
};
use horizon_core::browser::remote::RemoteProviderProfile;
use horizon_core::browser::remote_usage::ProviderUsageMonitor;

use super::{HorizonApp, browser_requests::actor_panel};

#[derive(Default)]
pub(super) struct UsageHostState {
    pending: Vec<PendingUsage>,
}

struct PendingUsage {
    request: UsageRequest,
    providers: BTreeMap<String, ProviderUsageMonitor>,
}

impl HorizonApp {
    pub(super) fn poll_provider_usage(&mut self) -> bool {
        self.poll_provider_usage_queue(&UsageQueue::default())
    }

    fn poll_provider_usage_queue(&mut self, queue: &UsageQueue) -> bool {
        let requests = match queue.claim(manifest::host_instance()) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(kind = ?error.kind(), "could not poll provider usage requests");
                Vec::new()
            }
        };
        for request in requests {
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

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::browser::manifest::AgentIdentity;
    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialReference, RemoteAdapterKind, RemoteAuthentication, RemoteSessionLimits,
    };
    use horizon_core::{PanelKind, PanelOptions, browser_actor};

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn authorized_agents_can_query_multiple_profiles_without_allocating_or_opening_settings() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("usage");
        let (command, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".into(), "exit 0".into()])
        } else {
            ("/bin/sh", vec!["-c".into(), "exit 0".into()])
        };
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
        app.poll_provider_usage_queue(&queue);
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
        app.poll_provider_usage_queue(&queue);
        let result = queue.take(identity, &id).expect("result").expect("completed");
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].provider, "cloud-b");
        let id = queue
            .enqueue(identity, Some("unknown".into()))
            .expect("unknown provider");
        app.poll_provider_usage_queue(&queue);
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
        app.poll_provider_usage_queue(&queue);
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
        app.poll_provider_usage_queue(queue);
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
        app.poll_provider_usage_queue(queue);
        let result = queue.take(identity, &id).expect("result").expect("partial completion");
        assert!(result.error.is_none());
        assert_eq!(result.providers[0].running, Some(2));
        assert_eq!(result.providers[1].error.as_deref(), Some("provider_usage_timed_out"));
    }
}
