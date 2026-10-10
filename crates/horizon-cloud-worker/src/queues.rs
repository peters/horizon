//! Consume the existing public MCP host queues inside this one-cloud worker.
use super::browser::Host;
use horizon_browser_control::manifest::{self, AgentIdentity, BrowserCreateResult, CreateNavigation};
pub(crate) struct PendingCreate {
    request: manifest::BrowserCreateRequest,
    failure: Option<BrowserCreateResult>,
}

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
            match start_before_deadline(&request, &workspace(), manifest::now_millis(), || {
                host.open_oriented(
                    &id,
                    request.url.clone(),
                    request.backend,
                    request.target.as_deref(),
                    &request.actor,
                    request.orientation,
                )
            }) {
                Ok(()) => {
                    if let Some(browser) = host.browsers.get_mut(&id) {
                        browser.state.visible = request.visible;
                    }
                    host.pending.push(PendingCreate { request, failure: None });
                }
                Err(error) => {
                    let result = create_failure(&request, &error, "browser_start_failed");
                    if manifest::complete_create_request(&result).is_err() {
                        host.pending.push(PendingCreate {
                            request,
                            failure: Some(result),
                        });
                    }
                }
            }
        }
    }
}

fn pending_requests(host: &mut Host) {
    host.pending.retain_mut(|pending| {
        let request = &pending.request;
        let id = format!("browser-{}", request.request_id);
        if let Some(result) = &pending.failure {
            return complete_failure(
                &mut host.pending_cleanup,
                host.browsers.contains_key(&id).then_some(id),
                result,
                manifest::complete_create_request,
            );
        }
        if let Some(horizon_browser::RemoteStartFailure::OrientationRejected { code, released }) = host
            .browsers
            .get(&id)
            .and_then(|browser| browser.start_orientation_failure.as_ref())
        {
            let result = orientation_create_failure(request, code, released);
            return complete_orientation_failure(
                &mut host.pending_cleanup,
                id,
                &result,
                manifest::complete_create_request,
            );
        }
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
            if let Err(error) = manifest::publish_requested_panel(
                &id,
                request.visible,
                &workspace(),
                AgentIdentity::new(&request.actor, Some(manifest::host_instance())),
            ) {
                return publication_failure(
                    pending,
                    &mut host.pending_cleanup,
                    id,
                    &error,
                    manifest::complete_create_request,
                );
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

fn complete_orientation_failure(
    cleanup: &mut std::collections::BTreeSet<String>,
    id: String,
    result: &BrowserCreateResult,
    complete: impl FnOnce(&BrowserCreateResult) -> std::io::Result<()>,
) -> bool {
    complete_failure(cleanup, Some(id), result, complete)
}

fn complete_failure(
    cleanup: &mut std::collections::BTreeSet<String>,
    id: Option<String>,
    result: &BrowserCreateResult,
    complete: impl FnOnce(&BrowserCreateResult) -> std::io::Result<()>,
) -> bool {
    // Cleanup removes the browser's typed startup failure, so keep both it and
    // the claimed request until publishing the result and retiring the request succeed.
    if complete(result).is_err() {
        return true;
    }
    if let Some(id) = id {
        cleanup.insert(id);
    }
    false
}

fn publication_failure(
    pending: &mut PendingCreate,
    cleanup: &mut std::collections::BTreeSet<String>,
    id: String,
    error: &std::io::Error,
    complete: impl FnOnce(&BrowserCreateResult) -> std::io::Result<()>,
) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted | std::io::ErrorKind::TimedOut
    ) {
        return true;
    }
    let result = pending
        .failure
        .get_or_insert_with(|| create_failure(&pending.request, error, "browser_publish_failed"));
    complete_failure(cleanup, Some(id), result, complete)
}

fn create_failure(
    request: &manifest::BrowserCreateRequest,
    error: &std::io::Error,
    fallback_code: &str,
) -> BrowserCreateResult {
    let message = error.to_string();
    let code = if error.kind() == std::io::ErrorKind::PermissionDenied && message == manifest::OUTSIDE_WORKSPACE_MESSAGE
    {
        "panel_outside_workspace"
    } else {
        fallback_code
    };
    BrowserCreateResult::failed(request, code, &message)
}

fn orientation_create_failure(
    request: &manifest::BrowserCreateRequest,
    code: &str,
    released: &horizon_browser::RemoteReleaseOutcome,
) -> BrowserCreateResult {
    use horizon_browser::RemoteReleaseOutcome;
    let message = match released {
        RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone => {
            "start orientation could not be verified; the provider confirmed session release"
        }
        RemoteReleaseOutcome::NeverAllocated => {
            "start orientation could not be verified; no remote session was allocated"
        }
        RemoteReleaseOutcome::ReleaseUnknown { .. } | RemoteReleaseOutcome::Failed { .. } => {
            "start orientation could not be verified; release is unconfirmed and capacity remains held, reconcile the retained allocation before creating again"
        }
    };
    BrowserCreateResult::failed(request, code, message)
}

fn start_before_deadline(
    request: &manifest::BrowserCreateRequest,
    membership: &manifest::ManifestWorkspace,
    now: i64,
    start: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    if !membership.authorizes(AgentIdentity::new(&request.actor, request.host_instance.as_deref())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            manifest::OUTSIDE_WORKSPACE_MESSAGE,
        ));
    }
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
    fn orientation_failure_retries_the_same_outcome_before_cleanup_can_remove_its_state() {
        use horizon_browser::RemoteReleaseOutcome;
        let request = manifest::BrowserCreateRequest::for_tests("cloud-agent");
        for released in [
            RemoteReleaseOutcome::Released,
            RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "private detail".into(),
            },
        ] {
            let expected = orientation_create_failure(&request, "remote_orientation_mismatch", &released);
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("result.json");
            let mut cleanup = std::collections::BTreeSet::new();
            let mut pending = vec![request.clone()];
            let mut attempted = Vec::new();
            for attempt in 0..4 {
                pending.retain(|request| {
                    assert_eq!(request.request_id, expected.request_id);
                    complete_orientation_failure(&mut cleanup, "tablet".into(), &expected, |result| {
                        attempted.push(result.clone());
                        if attempt == 0 {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                "result not writable",
                            ));
                        }
                        std::fs::write(&path, serde_json::to_vec(result)?)?;
                        if attempt < 3 {
                            return Err(std::io::Error::other("request retirement failed after result write"));
                        }
                        Ok(())
                    })
                });
                assert_eq!(pending.len(), usize::from(attempt < 3));
                assert_eq!(cleanup.contains("tablet"), attempt == 3);
                if attempt > 0 {
                    let persisted: BrowserCreateResult =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    assert_eq!(persisted, expected);
                }
            }
            assert_eq!(attempted, vec![expected; 4]);
            assert_eq!(cleanup.len(), 1);
        }
    }

    #[test]
    fn orientation_create_failure_preserves_code_and_only_warns_for_unconfirmed_release() {
        use horizon_browser::RemoteReleaseOutcome;
        use manifest::BrowserCreateOutcome;
        let request = manifest::BrowserCreateRequest::for_tests("cloud-agent");
        for (released, held) in [
            (RemoteReleaseOutcome::Released, false),
            (RemoteReleaseOutcome::AlreadyGone, false),
            (RemoteReleaseOutcome::NeverAllocated, false),
            (
                RemoteReleaseOutcome::ReleaseUnknown {
                    attempts: 3,
                    reason: "private provider detail".into(),
                },
                true,
            ),
            (
                RemoteReleaseOutcome::Failed {
                    error: "private provider error".into(),
                    message: "private provider detail".into(),
                },
                true,
            ),
        ] {
            for code in [
                "orientation_unsupported",
                "orientation_unverified",
                "browser_unavailable",
                "remote_orientation_mismatch",
            ] {
                let result = orientation_create_failure(&request, code, &released);
                let BrowserCreateOutcome::Failed { code: actual, message } = result.outcome else {
                    panic!("failed startup reported ready")
                };
                assert_eq!(actual, code);
                assert_eq!(result.request_id, request.request_id);
                assert_eq!(result.actor, request.actor);
                assert_eq!(message.contains("reconcile"), held);
                assert_eq!(message.contains("capacity remains held"), held);
                assert!(!message.contains("private provider"));
                if matches!(
                    released,
                    RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone
                ) {
                    assert!(message.contains("confirmed session release"));
                }
            }
        }
    }

    #[test]
    fn workspace_preflight_refuses_unknown_actors_and_hosts_before_starting() {
        let mut request = manifest::BrowserCreateRequest::for_tests("cloud-registered");
        request.host_instance = Some("worker".into());
        let membership = manifest::ManifestWorkspace::new("worker", "cloud", vec![request.actor.clone()]);
        for (actor, host) in [
            ("horizon:cloud-missing", Some("worker")),
            ("horizon:cloud-registered", Some("other-worker")),
            ("horizon:cloud-registered", None),
        ] {
            request.actor = actor.into();
            request.host_instance = host.map(str::to_owned);
            let error =
                start_before_deadline(&request, &membership, 0, || panic!("unauthorized browser started")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            let result = create_failure(&request, &error, "browser_start_failed");
            assert_eq!(
                result.outcome,
                manifest::BrowserCreateOutcome::Failed {
                    code: "panel_outside_workspace".into(),
                    message: manifest::OUTSIDE_WORKSPACE_MESSAGE.into(),
                }
            );
        }
        request.actor = "horizon:cloud-registered".into();
        request.host_instance = Some("worker".into());
        let mut starts = 0;
        start_before_deadline(&request, &membership, 0, || {
            starts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(starts, 1);
    }

    #[test]
    fn terminal_publication_failure_is_retained_until_result_completion_and_cleanup() {
        let request = manifest::BrowserCreateRequest::for_tests("cloud-registered");
        let mut pending = PendingCreate { request, failure: None };
        let mut cleanup = std::collections::BTreeSet::new();
        let error = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            manifest::OUTSIDE_WORKSPACE_MESSAGE,
        );
        let mut attempted = Vec::new();
        assert!(publication_failure(
            &mut pending,
            &mut cleanup,
            "browser".into(),
            &error,
            |result| {
                attempted.push(result.clone());
                Err(std::io::Error::other("result write failed"))
            }
        ));
        let result = pending.failure.as_ref().unwrap();
        assert_eq!(
            result.outcome,
            manifest::BrowserCreateOutcome::Failed {
                code: "panel_outside_workspace".into(),
                message: manifest::OUTSIDE_WORKSPACE_MESSAGE.into(),
            }
        );
        assert!(cleanup.is_empty());
        // The remembered refusal wins over a later deadline and does not retry publication.
        pending.request.deadline_at_millis = 0;
        for fails in [true, false] {
            assert_eq!(
                complete_failure(&mut cleanup, Some("browser".into()), result, |result| {
                    attempted.push(result.clone());
                    if fails {
                        Err(std::io::Error::other("request retirement failed"))
                    } else {
                        Ok(())
                    }
                }),
                fails
            );
            assert_eq!(cleanup.contains("browser"), !fails);
        }
        assert_eq!(attempted, vec![result.clone(); 3]);
    }

    #[test]
    fn transient_publication_failure_retries_without_retiring_the_browser() {
        let mut pending = PendingCreate {
            request: manifest::BrowserCreateRequest::for_tests("cloud-registered"),
            failure: None,
        };
        let mut cleanup = std::collections::BTreeSet::new();
        for kind in [
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::TimedOut,
        ] {
            assert!(publication_failure(
                &mut pending,
                &mut cleanup,
                "browser".into(),
                &std::io::Error::from(kind),
                |_| panic!("transient failure must not complete the request"),
            ));
            assert!(pending.failure.is_none());
            assert!(cleanup.is_empty());
        }
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::StorageFull,
        ] {
            pending.failure = None;
            cleanup.clear();
            let error = std::io::Error::new(kind, "manifest unavailable");
            assert!(!publication_failure(
                &mut pending,
                &mut cleanup,
                "browser".into(),
                &error,
                |result| {
                    assert!(matches!(
                        &result.outcome,
                        manifest::BrowserCreateOutcome::Failed { code, .. } if code == "browser_publish_failed"
                    ));
                    Ok(())
                }
            ));
            assert!(cleanup.contains("browser"));
        }
    }

    #[test]
    fn preflight_failure_completion_does_not_schedule_browser_cleanup() {
        let request = manifest::BrowserCreateRequest::for_tests("cloud-missing");
        let result =
            BrowserCreateResult::failed(&request, "panel_outside_workspace", manifest::OUTSIDE_WORKSPACE_MESSAGE);
        let mut cleanup = std::collections::BTreeSet::new();
        for fails in [true, false] {
            assert_eq!(
                complete_failure(&mut cleanup, None, &result, |_| {
                    if fails {
                        Err(std::io::Error::other("result write failed"))
                    } else {
                        Ok(())
                    }
                }),
                fails
            );
            assert!(cleanup.is_empty());
        }
    }

    #[test]
    fn expired_create_never_invokes_the_allocator() {
        let mut request = manifest::BrowserCreateRequest::for_tests("cloud-agent");
        request.deadline_at_millis = 100;
        request.host_instance = Some("worker".into());
        let membership = manifest::ManifestWorkspace::new("worker", "cloud", vec![request.actor.clone()]);
        for now in [100, 101, i64::MAX] {
            let error =
                start_before_deadline(&request, &membership, now, || panic!("expired allocation started")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        }
        let mut started = false;
        start_before_deadline(&request, &membership, 99, || {
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
