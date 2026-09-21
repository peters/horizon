//! Consume the existing public MCP host queues inside this one-cloud worker.
use super::browser::Host;
use horizon_browser_control::manifest::{self, AgentIdentity, BrowserCreateResult, CreateNavigation};
pub fn poll(host: &mut Host) {
    host_controls(host);
    super::remote::poll(&mut host.remote_allocations, &host.capabilities, &mut host.catalog);
    host.catalog.poll(&host.capabilities);
    host.retry_cleanup();
    device_viewer_requests();
    let membership = workspace();
    for browser in host.browsers.values() {
        let _ = manifest::sync_host_state(&browser.state.id, browser.state.visible, &membership);
    }
    create_requests(host);
    pending_requests(host);
}

fn create_requests(host: &mut Host) {
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
            if request.duplicate_from.is_some() {
                let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                    &request,
                    "unsupported_cloud_target",
                    "Duplicating a cloud browser is not supported",
                ));
                continue;
            }
            let id = format!("browser-{}", request.request_id);
            match start_before_deadline(&request, manifest::now_millis(), || {
                host.open(
                    &id,
                    request.url.clone(),
                    request.backend,
                    request.target.as_deref(),
                    &request.actor,
                )
            }) {
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
}

fn pending_requests(host: &mut Host) {
    host.pending.retain(|request| {
        let id = format!("browser-{}", request.request_id);
        if let Some(message) = pending_failure(
            request,
            host.browsers.get(&id).map(|browser| &browser.state),
            manifest::now_millis(),
        ) {
            host.pending_cleanup.insert(id);
            return manifest::complete_create_request(&BrowserCreateResult::failed(
                request,
                "browser_start_failed",
                message,
            ))
            .is_err();
        }
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
        } else {
            None
        };
        result.is_none_or(|result| manifest::complete_create_request(&result).is_err())
    });
}

fn start_before_deadline(
    request: &manifest::BrowserCreateRequest,
    now: i64,
    start: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    if now >= request.deadline_at_millis {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Browser creation deadline expired",
        ));
    }
    start()
}

fn pending_failure(
    request: &manifest::BrowserCreateRequest,
    state: Option<&horizon_browser_protocol::cloud_view::CloudViewState>,
    now: i64,
) -> Option<&'static str> {
    if now >= request.deadline_at_millis {
        Some("Browser creation deadline expired")
    } else if state.is_none_or(|state| state.lost) {
        Some("Worker browser did not become ready")
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser_protocol::cloud_view::CloudViewState;

    #[test]
    fn expired_create_never_invokes_the_allocator() {
        let mut request = manifest::BrowserCreateRequest::for_tests("cloud-agent");
        request.deadline_at_millis = 100;
        for now in [100, 101, i64::MAX] {
            let error = start_before_deadline(&request, now, || panic!("expired allocation started")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        }
        let mut started = false;
        start_before_deadline(&request, 99, || {
            started = true;
            Ok(())
        })
        .unwrap();
        assert!(started);
    }

    #[test]
    fn expired_pending_browser_is_retired_even_if_it_became_ready() {
        let mut request = manifest::BrowserCreateRequest::for_tests("cloud-agent");
        request.deadline_at_millis = 100;
        for ready in [false, true] {
            let state = CloudViewState {
                ready,
                ..Default::default()
            };
            assert_eq!(pending_failure(&request, Some(&state), 99), None);
            assert_eq!(
                pending_failure(&request, Some(&state), 100),
                Some("Browser creation deadline expired")
            );
        }
        assert!(pending_failure(&request, None, 99).is_some());
        assert!(
            pending_failure(
                &request,
                Some(&CloudViewState {
                    lost: true,
                    ..Default::default()
                }),
                99
            )
            .is_some()
        );
    }
}
