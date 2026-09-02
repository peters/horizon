use super::{ApiPod, CreatePodRequest, RunPodApiKey, RunPodCleanup, RunPodError, Transport, valid_provider_id};
use std::{thread, time::Duration};
use url::Url;

const API_BASE: &str = "https://rest.runpod.io/v1/";
const GRAPHQL_URL: &str = "https://api.runpod.io/graphql";
const CREATE_MUTATION: &str =
    "mutation CreatePod($input: PodFindAndDeployOnDemandInput!) { podFindAndDeployOnDemand(input: $input) { id } }";
const RESPONSE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct RunPodHttp {
    agent: ureq::Agent,
    base: Url,
    authorization: String,
}

impl RunPodHttp {
    pub(super) fn new(api_key: &RunPodApiKey) -> Result<Self, RunPodError> {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("horizon/", env!("CARGO_PKG_VERSION")))
            .build();
        let base = Url::parse(API_BASE).map_err(|_| RunPodError::InvalidResponse {
            operation: "client initialization",
        })?;
        Ok(Self {
            agent: ureq::Agent::new_with_config(config),
            base,
            authorization: format!("Bearer {}", api_key.expose()),
        })
    }

    fn pods_url(&self) -> Result<Url, RunPodError> {
        self.base.join("pods").map_err(|_| RunPodError::InvalidResponse {
            operation: "URL construction",
        })
    }

    fn pod_url(&self, pod_id: &str) -> Result<Url, RunPodError> {
        self.base
            .join(&format!("pods/{pod_id}"))
            .map_err(|_| RunPodError::InvalidResponse {
                operation: "URL construction",
            })
    }
}

impl Transport for RunPodHttp {
    fn list_by_name(&self, name: &str) -> Result<Vec<ApiPod>, RunPodError> {
        let mut url = self.pods_url()?;
        url.query_pairs_mut().append_pair("name", name);
        let response = self
            .agent
            .get(url.as_str())
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod lookup",
            })?;
        decode_json(response, 200, "pod lookup")
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
            if envelope
                .pointer("/errors/0/extensions/code")
                .and_then(serde_json::Value::as_str)
                == Some("SUPPLY_CONSTRAINT")
            {
                return Err(RunPodError::CapacityUnavailable);
            }
            return Err(RunPodError::InvalidResponse {
                operation: "pod creation",
            });
        };
        for _ in 0..5 {
            if let Ok(Some(pod)) = self.get(&pod_id) {
                return Ok(pod);
            }
            thread::sleep(Duration::from_millis(250));
        }
        match self.delete(&pod_id) {
            Ok(_) => Err(RunPodError::CreationVerificationFailed { pod_id }),
            Err(_) => Err(RunPodError::CreationCleanupFailed { pod_id }),
        }
    }

    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError> {
        let url = self.pod_url(pod_id)?;
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
        decode_json(response, 200, "pod inspection").map(Some)
    }

    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError> {
        let url = self.pod_url(pod_id)?;
        let response = self
            .agent
            .delete(url.as_str())
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|_| RunPodError::RequestFailed {
                operation: "pod deletion",
            })?;
        match response.status().as_u16() {
            204 => Ok(RunPodCleanup::Deleted),
            404 => Ok(RunPodCleanup::AlreadyAbsent),
            status => Err(RunPodError::UnexpectedStatus {
                operation: "pod deletion",
                status,
            }),
        }
    }
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
