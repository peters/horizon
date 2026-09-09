use super::{ApiPod, CreatePodRequest, RunPodApiKey, RunPodCleanup, RunPodError, Transport, valid_provider_id};
use std::{thread, time::Duration};
const PODS_URL: &str = "https://api.runpod.io/v2/pods";
const GRAPHQL_URL: &str = "https://api.runpod.io/graphql";
const CREATE_MUTATION: &str =
    "mutation CreatePod($input: PodFindAndDeployOnDemandInput!) { podFindAndDeployOnDemand(input: $input) { id } }";
pub(super) const RESPONSE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const PROPAGATION_BACKOFF_MS: [u64; 8] = [0, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000];
const CAPACITY_ERROR_MARKERS: [&str; 8] = [
    "no longer any instances available",
    "no instances currently available",
    "please refresh and try again",
    "does not have the resources",
    "try a different machine",
    "insufficient capacity",
    "out of stock",
    "sold out",
];
pub(super) struct RunPodHttp {
    agent: ureq::Agent,
    authorization: String,
}
#[derive(serde::Deserialize)]
struct ListPodsResponse {
    pods: Vec<ApiPod>,
}
impl RunPodHttp {
    pub(super) fn new(api_key: &RunPodApiKey) -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("horizon/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            authorization: format!("Bearer {}", api_key.expose()),
        }
    }
    fn pod_url(pod_id: &str) -> Result<String, RunPodError> {
        valid_provider_id(pod_id)
            .then(|| format!("{PODS_URL}/{pod_id}"))
            .ok_or(RunPodError::ResourceIdentityMismatch)
    }

    pub(super) fn host_key_sample(&self, pod_id: &str) -> Result<Vec<u8>, RunPodError> {
        let url = format!("{}/logs?source=container&tail=5000", Self::pod_url(pod_id)?);
        let mut response = self
            .agent
            .get(url.as_str())
            .header("Authorization", &self.authorization)
            .config()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "host-key bootstrap",
            })?;
        if response.status().as_u16() != 200 {
            return Err(RunPodError::UnexpectedStatus {
                operation: "host-key bootstrap",
                status: response.status().as_u16(),
            });
        }
        if response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            != Some("text/event-stream")
        {
            return Err(RunPodError::InvalidResponse {
                operation: "host-key bootstrap",
            });
        }
        super::host_key::sample::read(response.body_mut().as_reader())
    }

    #[cfg(test)]
    pub(super) fn mock(agent: ureq::Agent) -> Self {
        Self {
            agent,
            authorization: "Bearer synthetic-credential".into(),
        }
    }
}
impl Transport for RunPodHttp {
    fn list_by_name(&self, name: &str) -> Result<Vec<ApiPod>, RunPodError> {
        let response = self
            .agent
            .get(PODS_URL)
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod lookup",
            })?;
        let response: ListPodsResponse = decode_json(response, 200, "pod lookup")?;
        Ok(response.pods.into_iter().filter(|pod| pod.name == name).collect())
    }
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        let response = self
            .agent
            .post(GRAPHQL_URL)
            .header("Authorization", &self.authorization)
            .send_json(serde_json::json!({
                "query": CREATE_MUTATION,
                "variables": { "input": request },
            }))
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod creation",
            })?;
        let envelope: serde_json::Value = decode_json(response, 200, "pod creation")?;
        let pod_id = envelope
            .pointer("/data/podFindAndDeployOnDemand/id")
            .and_then(serde_json::Value::as_str)
            .filter(|pod_id| valid_provider_id(pod_id))
            .map(str::to_string);
        let Some(pod_id) = pod_id else {
            return Err(if capacity_unavailable(&envelope) {
                RunPodError::CapacityUnavailable
            } else {
                RunPodError::InvalidResponse {
                    operation: "pod creation",
                }
            });
        };
        reconcile_creation(self, request, pod_id)
    }
    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError> {
        let url = Self::pod_url(pod_id)?;
        let response = self
            .agent
            .get(url.as_str())
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod inspection",
            })?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let pod: ApiPod = decode_json(response, 200, "pod inspection")?;
        (pod.id == pod_id)
            .then_some(Some(pod))
            .ok_or(RunPodError::ResourceIdentityMismatch)
    }
    fn stop(&self, pod_id: &str) -> Result<(), RunPodError> {
        let url = format!("{}/action", Self::pod_url(pod_id)?);
        let response = self
            .agent
            .post(url.as_str())
            .header("Authorization", &self.authorization)
            .send_json(serde_json::json!({"action": "stop"}))
            .map_err(|_| RunPodError::RequestFailed { operation: "pod Stop" })?;
        let pod: ApiPod = decode_json(response, 200, "pod Stop")?;
        (pod.id == pod_id)
            .then_some(())
            .ok_or(RunPodError::ResourceIdentityMismatch)
    }
    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError> {
        let url = Self::pod_url(pod_id)?;
        let response = self
            .agent
            .delete(url.as_str())
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod deletion",
            })?;
        if response.status().as_u16() != 204 {
            return Err(RunPodError::UnexpectedStatus {
                operation: "pod deletion",
                status: response.status().as_u16(),
            });
        }
        for delay_ms in PROPAGATION_BACKOFF_MS {
            thread::sleep(Duration::from_millis(delay_ms));
            if matches!(self.get(pod_id), Ok(None)) {
                return Ok(RunPodCleanup::Deleted);
            }
        }
        Err(RunPodError::DeletionVerificationFailed {
            pod_id: pod_id.to_string(),
        })
    }
}
pub(super) fn reconcile_creation(
    transport: &dyn Transport,
    request: &CreatePodRequest,
    pod_id: String,
) -> Result<ApiPod, RunPodError> {
    for delay_ms in PROPAGATION_BACKOFF_MS {
        if !cfg!(test) {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        if let Ok(Some(pod)) = transport.get(&pod_id) {
            return Ok(pod);
        }
    }
    if request.terminate_after.is_none() {
        return Err(RunPodError::PersistentCreationReconciliationRequired {
            name: request.name.clone(),
            pod_id,
        });
    }
    if transport.delete(&pod_id) != Ok(RunPodCleanup::Deleted) || !matches!(transport.get(&pod_id), Ok(None)) {
        return Err(RunPodError::CreationCleanupFailed { pod_id });
    }
    Err(RunPodError::CreationVerificationFailed { pod_id })
}
pub(super) fn capacity_unavailable(envelope: &serde_json::Value) -> bool {
    envelope
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|error| error.get("message").and_then(serde_json::Value::as_str))
        .any(|message| {
            let normalized = message.to_ascii_lowercase();
            CAPACITY_ERROR_MARKERS.iter().any(|marker| normalized.contains(marker))
        })
}
fn decode_json<T>(
    mut response: ureq::http::Response<ureq::Body>,
    expected: u16,
    operation: &'static str,
) -> Result<T, RunPodError>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status().as_u16();
    if status != expected {
        return Err(RunPodError::UnexpectedStatus { operation, status });
    }
    response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT_BYTES)
        .read_json()
        .map_err(|_| RunPodError::InvalidResponse { operation })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read as _,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use ureq::{
        Body, SendBody,
        http::{Request, Response},
        middleware::MiddlewareNext,
    };

    #[test]
    fn stop_http_uses_only_exact_fixed_action_and_bounded_redacted_responses() {
        for (status, body, expected) in [
            (200, r#"{"id":"pod_stop"}"#.to_string(), Ok(())),
            (
                200,
                r#"{"id":"foreign"}"#.to_string(),
                Err(RunPodError::ResourceIdentityMismatch),
            ),
            (
                200,
                "private-malformed-payload".into(),
                Err(RunPodError::InvalidResponse { operation: "pod Stop" }),
            ),
            (
                200,
                format!(r#"{{"id":"pod_stop","padding":"{}"}}"#, "x".repeat(2 * 1024 * 1024)),
                Err(RunPodError::InvalidResponse { operation: "pod Stop" }),
            ),
            (
                403,
                "private-forbidden-payload".into(),
                Err(RunPodError::UnexpectedStatus {
                    operation: "pod Stop",
                    status: 403,
                }),
            ),
            (
                204,
                String::new(),
                Err(RunPodError::UnexpectedStatus {
                    operation: "pod Stop",
                    status: 204,
                }),
            ),
            (
                302,
                "private-redirect-payload".into(),
                Err(RunPodError::UnexpectedStatus {
                    operation: "pod Stop",
                    status: 302,
                }),
            ),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&calls);
            let agent = ureq::Agent::config_builder()
                .middleware(move |request: Request<SendBody>, _: MiddlewareNext| {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(request.method(), "POST");
                    assert_eq!(request.uri(), "https://api.runpod.io/v2/pods/pod_stop/action");
                    assert_eq!(request.headers()["Authorization"], "Bearer synthetic-credential");
                    assert_eq!(request.headers()["Content-Type"], "application/json; charset=utf-8");
                    let mut payload = String::new();
                    request
                        .into_body()
                        .into_reader()
                        .read_to_string(&mut payload)
                        .expect("payload");
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&payload).expect("request JSON"),
                        serde_json::json!({"action": "stop"})
                    );
                    Ok(Response::builder()
                        .status(status)
                        .body(Body::builder().data(body.clone()))
                        .expect("response"))
                })
                .build()
                .new_agent();
            let http = RunPodHttp {
                agent,
                authorization: "Bearer synthetic-credential".into(),
            };
            assert_eq!(http.stop("pod_stop"), expected);
            for invalid in ["", "../other", "pod_stop/action", "pod_stop?terminate=true"] {
                assert_eq!(http.stop(invalid), Err(RunPodError::ResourceIdentityMismatch));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            if let Err(error) = expected {
                assert!(!format!("{error:?} {error}").contains("private-"));
                assert!(!format!("{error:?} {error}").contains("synthetic-credential"));
            }
        }
        let production = RunPodHttp::new(&RunPodApiKey::new("synthetic-credential").expect("key"));
        assert!(production.agent.config().https_only());
        assert_eq!(production.agent.config().max_redirects(), 0);
        assert_eq!(production.agent.config().timeouts().global, Some(REQUEST_TIMEOUT));
    }
}
