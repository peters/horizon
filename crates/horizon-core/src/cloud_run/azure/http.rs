use super::{
    ApiExecution, ApiJob, AzureAccessToken, AzureCleanup, AzureError, AzureProfile, CreateJobRequest, CreateResult,
    Transport,
};
use std::time::Duration;
use url::Url;
const API_VERSION: &str = "2025-07-01";
const API_BASE: &str = "https://management.azure.com/";
const RESPONSE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) struct AzureHttp {
    agent: ureq::Agent,
    authorization: String,
    subscription_id: String,
    resource_group: String,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionList {
    value: Vec<ApiExecution>,
    next_link: Option<String>,
}
impl AzureHttp {
    pub(super) fn new(token: &AzureAccessToken, profile: &AzureProfile) -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("horizon/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            authorization: format!("Bearer {}", token.expose()),
            subscription_id: profile.subscription_id.clone(),
            resource_group: profile.resource_group.clone(),
        }
    }
    fn job_url(&self, name: &str, suffix: Option<&str>) -> Result<Url, AzureError> {
        let suffix = suffix.map_or_else(String::new, |value| format!("/{value}"));
        Url::parse(&format!(
            "{API_BASE}subscriptions/{}/resourceGroups/{}/providers/Microsoft.App/jobs/{name}{suffix}?api-version={API_VERSION}",
            self.subscription_id, self.resource_group
        ))
        .map_err(|_| AzureError::InvalidResponse {
            operation: "URL construction",
        })
    }
    fn authorize<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request.header("Authorization", &self.authorization)
    }
}
impl Transport for AzureHttp {
    fn get(&self, name: &str) -> Result<Option<ApiJob>, AzureError> {
        let url = self.job_url(name, None)?;
        let response = self
            .authorize(self.agent.get(url.as_str()))
            .call()
            .map_err(request_failed("job inspection"))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        decode_json(response, &[200], "job inspection").map(Some)
    }
    fn create(&self, name: &str, request: &CreateJobRequest) -> Result<CreateResult, AzureError> {
        let url = self.job_url(name, None)?;
        let response = self
            .authorize(self.agent.put(url.as_str()))
            .send_json(request)
            .map_err(request_failed("job creation"))?;
        let created = response.status().as_u16() == 201;
        decode_json(response, &[200, 201], "job creation").map(|job| CreateResult { job, created })
    }
    fn executions(&self, name: &str) -> Result<Vec<ApiExecution>, AzureError> {
        let url = self.job_url(name, Some("executions"))?;
        let response = self
            .authorize(self.agent.get(url.as_str()))
            .call()
            .map_err(request_failed("execution lookup"))?;
        let result: ExecutionList = decode_json(response, &[200], "execution lookup")?;
        if result.next_link.is_some() {
            return Err(AzureError::InvalidResponse {
                operation: "execution lookup",
            });
        }
        Ok(result.value)
    }
    fn start(&self, name: &str) -> Result<Option<ApiExecution>, AzureError> {
        let url = self.job_url(name, Some("start"))?;
        let response = self
            .authorize(self.agent.post(url.as_str()))
            .send_empty()
            .map_err(request_failed("job start"))?;
        if response.status().as_u16() == 202 {
            return Ok(None);
        }
        decode_json(response, &[200], "job start").map(Some)
    }
    fn delete(&self, name: &str) -> Result<AzureCleanup, AzureError> {
        let url = self.job_url(name, None)?;
        let response = self
            .authorize(self.agent.delete(url.as_str()))
            .call()
            .map_err(request_failed("job deletion"))?;
        match response.status().as_u16() {
            200 | 204 => Ok(AzureCleanup::Deleted),
            202 => Ok(AzureCleanup::DeletionPending),
            404 => Ok(AzureCleanup::AlreadyAbsent),
            status => Err(AzureError::UnexpectedStatus {
                operation: "job deletion",
                status,
            }),
        }
    }
}
fn request_failed(operation: &'static str) -> impl FnOnce(ureq::Error) -> AzureError {
    move |_| AzureError::RequestFailed { operation }
}
fn decode_json<T>(
    mut response: ureq::http::Response<ureq::Body>,
    expected: &[u16],
    operation: &'static str,
) -> Result<T, AzureError>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status().as_u16();
    if !expected.contains(&status) {
        return Err(AzureError::UnexpectedStatus { operation, status });
    }
    response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT_BYTES)
        .read_json()
        .map_err(|_| AzureError::InvalidResponse { operation })
}
