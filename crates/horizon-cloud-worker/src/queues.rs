//! Consume the existing public MCP host queues inside this one-cloud worker.
use super::browser::Host;
use horizon_browser_control::manifest::{self, AgentIdentity, BrowserCreateResult, CreateNavigation};
pub fn poll(host: &mut Host) {
    host_controls(host);
    device_viewer_requests();
    let membership = workspace();
    for browser in host.browsers.values() {
        let _ = manifest::sync_host_state(&browser.state.id, browser.state.visible, &membership);
    }
    if let Ok(requests) = manifest::list_create_requests() {
        for request in requests {
            if !request.actor.starts_with("horizon:cloud-") {
                continue;
            }
            let Ok(Some(request)) = manifest::claim_create_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) else {
                continue;
            };
            if request.target.is_some() || request.duplicate_from.is_some() {
                let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                    &request,
                    "unsupported_cloud_target",
                    "This worker supports its local browser runtime",
                ));
                continue;
            }
            let id = format!("browser-{}", request.request_id);
            match host.open(&id, request.url.clone(), request.backend) {
                Ok(()) => {
                    if let Some(browser) = host.browsers.get_mut(&id) {
                        browser.state.visible = request.visible;
                    }
                    host.pending.push(request);
                }
                Err(error) => {
                    let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                        &request,
                        "browser_start_failed",
                        &error.to_string(),
                    ));
                }
            }
        }
    }
    host.pending.retain(|request| {
        let id = format!("browser-{}", request.request_id);
        let Some(browser) = host.browsers.get(&id) else {
            return false;
        };
        let result = if browser.state.ready {
            if manifest::publish_requested_panel(
                &id,
                request.visible,
                &workspace(),
                AgentIdentity::new(&request.actor, Some(manifest::host_instance())),
            )
            .is_err()
            {
                return true;
            }
            let navigation = if request.url.is_none() {
                CreateNavigation::NotRequested
            } else if browser.state.url.is_empty() {
                CreateNavigation::Pending
            } else {
                CreateNavigation::Committed
            };
            Some(BrowserCreateResult::ready(
                request,
                id,
                navigation,
                None,
                manifest::now_millis()
                    .saturating_sub(request.requested_at_millis)
                    .unsigned_abs(),
            ))
        } else if browser.state.lost || manifest::now_millis() > request.deadline_at_millis {
            Some(BrowserCreateResult::failed(
                request,
                "browser_start_failed",
                "Worker browser did not become ready",
            ))
        } else {
            None
        };
        result.is_none_or(|result| manifest::complete_create_request(&result).is_err())
    });
}

fn device_viewer_requests() {
    if let Ok(requests) = manifest::device::claim(manifest::host_instance()) {
        for request in requests {
            let outcome = manifest::device::Outcome::failed(
                "viewer_requires_horizon",
                "This worker has no Horizon window. Use device_doctor, device_screenshot and device_act here; use Add desktop viewer in the attached Horizon cloud to view this desktop.",
            );
            let _ = manifest::device::complete(&request, outcome);
        }
    }
}

fn workspace() -> manifest::ManifestWorkspace {
    let actors = std::fs::read_dir("/workspace/sessions")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .map(|id| format!("horizon:cloud-{id}"))
        .collect();
    manifest::ManifestWorkspace::new(manifest::host_instance(), "cloud", actors)
}

fn host_controls(host: &mut Host) {
    if let Ok(requests) = manifest::list_visibility_requests() {
        for request in requests {
            let Ok(Some(request)) = manifest::claim_visibility_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) else {
                continue;
            };
            let result = if let Some(browser) = host.browsers.get_mut(&request.panel_local_id) {
                browser.state.visible = request.visible;
                let _ = manifest::sync_host_state(&request.panel_local_id, request.visible, &workspace());
                manifest::BrowserVisibilityResult::ready(&request)
            } else {
                manifest::BrowserVisibilityResult::failed(&request, "not_found", "Worker browser is no longer running")
            };
            let _ = manifest::complete_visibility_request(&result);
        }
    }
    if let Ok(requests) = manifest::list_close_requests() {
        for request in requests {
            let Ok(Some(request)) = manifest::claim_close_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) else {
                continue;
            };
            let result = match host.close(&request.panel_local_id) {
                Ok(()) => manifest::BrowserCloseResult::closed(&request),
                Err(_) => manifest::BrowserCloseResult::failed(
                    &request,
                    "shutdown_pending",
                    "Worker browser is still stopping",
                ),
            };
            let _ = manifest::complete_close_request(&result);
        }
    }
}
