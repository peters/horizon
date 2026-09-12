//! Bounded, narrowly typed Azure Resource Manager requests. Every URL is built from
//! validated segments with an explicit API version; responses are size-limited,
//! decoded into small typed views, and never echoed into errors.
use super::{
    AzureCredentialSource, AzureError, COMPUTE_API_VERSION, DEPLOYMENT_API_VERSION, MANAGEMENT_ENDPOINT,
    REQUEST_TIMEOUT, RESOURCE_GROUP_API_VERSION, RESPONSE_LIMIT_BYTES, valid_resource_group_name,
    valid_subscription_id,
};
use std::{collections::BTreeMap, time::Duration};

mod power;
mod run_command;

pub use run_command::AzureRunCommand;

/// Resource group as observed from ARM.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzureGroupInfo {
    /// Group-level ARM resource ID as returned, verified against the requested
    /// subscription and group; the provider persists exactly this value.
    pub id: String,
    pub name: String,
    pub location: String,
    pub provisioning_state: String,
    pub tags: BTreeMap<String, String>,
}

/// Deployment state plus its string outputs (for example the public IP).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzureDeploymentState {
    pub provisioning_state: String,
    pub outputs: BTreeMap<String, String>,
}

impl AzureDeploymentState {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.provisioning_state.as_str(), "Succeeded" | "Failed" | "Canceled")
    }
}

/// The subset of a VM resource plus instance view that the lifecycle needs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzureVmView {
    pub id: String,
    pub name: String,
    pub location: String,
    pub vm_size: String,
    pub provisioning_state: String,
    pub power_state: Option<String>,
    pub tags: BTreeMap<String, String>,
}

/// Outcome of a request that ARM may complete asynchronously.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AzureLongRunningState {
    Accepted,
    Completed,
}

/// Narrow set of management operations the adapter is allowed to perform.
pub trait AzureManagementTransport: Send + Sync {
    /// The single subscription every request of this transport addresses.
    fn subscription_id(&self) -> &str;
    /// # Errors
    fn get_resource_group(&self, name: &str) -> Result<Option<AzureGroupInfo>, AzureError>;
    /// [`Self::get_resource_group`] for a poll under an absolute deadline: the whole
    /// request must finish within `budget`, which is never zero.
    /// # Errors
    fn get_resource_group_within(&self, name: &str, budget: Duration) -> Result<Option<AzureGroupInfo>, AzureError>;
    /// # Errors
    fn create_resource_group(
        &self,
        name: &str,
        location: &str,
        tags: &BTreeMap<String, String>,
    ) -> Result<AzureGroupInfo, AzureError>;
    /// # Errors
    fn delete_resource_group(&self, name: &str) -> Result<AzureLongRunningState, AzureError>;
    /// # Errors
    fn put_deployment(
        &self,
        group: &str,
        name: &str,
        template: &serde_json::Value,
        parameters: &serde_json::Value,
    ) -> Result<AzureDeploymentState, AzureError>;
    /// # Errors
    fn get_deployment(&self, group: &str, name: &str) -> Result<Option<AzureDeploymentState>, AzureError>;
    /// # Errors
    fn get_vm(&self, group: &str, name: &str) -> Result<Option<AzureVmView>, AzureError>;
    /// [`Self::get_vm`] for a poll under an absolute deadline: the whole request must
    /// finish within `budget`, which is never zero.
    /// # Errors
    fn get_vm_within(&self, group: &str, name: &str, budget: Duration) -> Result<Option<AzureVmView>, AzureError>;
    /// Release the VM's compute while retaining its disks (`PowerState/deallocated`);
    /// `None` when the VM is absent.
    /// # Errors
    fn deallocate_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError>;
    /// Start a deallocated or guest-stopped VM's compute (`PowerState/running`); `None`
    /// when the VM is absent.
    /// # Errors
    fn start_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError>;
    /// Run one of the closed set of commands inside the VM through the ARM run-command
    /// channel and return its standard output, or `None` when the VM is absent.
    /// # Errors
    fn run_command(&self, group: &str, name: &str, command: AzureRunCommand) -> Result<Option<String>, AzureError>;
}

impl<T: AzureManagementTransport + ?Sized> AzureManagementTransport for std::sync::Arc<T> {
    fn subscription_id(&self) -> &str {
        (**self).subscription_id()
    }

    fn get_resource_group(&self, name: &str) -> Result<Option<AzureGroupInfo>, AzureError> {
        (**self).get_resource_group(name)
    }

    fn get_resource_group_within(&self, name: &str, budget: Duration) -> Result<Option<AzureGroupInfo>, AzureError> {
        (**self).get_resource_group_within(name, budget)
    }

    fn create_resource_group(
        &self,
        name: &str,
        location: &str,
        tags: &BTreeMap<String, String>,
    ) -> Result<AzureGroupInfo, AzureError> {
        (**self).create_resource_group(name, location, tags)
    }

    fn delete_resource_group(&self, name: &str) -> Result<AzureLongRunningState, AzureError> {
        (**self).delete_resource_group(name)
    }

    fn put_deployment(
        &self,
        group: &str,
        name: &str,
        template: &serde_json::Value,
        parameters: &serde_json::Value,
    ) -> Result<AzureDeploymentState, AzureError> {
        (**self).put_deployment(group, name, template, parameters)
    }

    fn get_deployment(&self, group: &str, name: &str) -> Result<Option<AzureDeploymentState>, AzureError> {
        (**self).get_deployment(group, name)
    }

    fn get_vm(&self, group: &str, name: &str) -> Result<Option<AzureVmView>, AzureError> {
        (**self).get_vm(group, name)
    }

    fn get_vm_within(&self, group: &str, name: &str, budget: Duration) -> Result<Option<AzureVmView>, AzureError> {
        (**self).get_vm_within(group, name, budget)
    }

    fn deallocate_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        (**self).deallocate_vm(group, name)
    }

    fn start_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        (**self).start_vm(group, name)
    }

    fn run_command(&self, group: &str, name: &str, command: AzureRunCommand) -> Result<Option<String>, AzureError> {
        (**self).run_command(group, name, command)
    }
}

/// HTTPS transport pinned to the public Azure Resource Manager endpoint.
pub struct AzureArmHttp {
    agent: ureq::Agent,
    credential: Box<dyn AzureCredentialSource>,
    subscription_id: String,
}

impl AzureArmHttp {
    /// # Errors
    /// Rejects a malformed subscription identifier.
    pub fn new(
        subscription_id: impl Into<String>,
        credential: impl AzureCredentialSource + 'static,
    ) -> Result<Self, AzureError> {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("horizon/", env!("CARGO_PKG_VERSION")))
            .build();
        Self::with_agent(ureq::Agent::new_with_config(config), subscription_id, credential)
    }

    pub(crate) fn with_agent(
        agent: ureq::Agent,
        subscription_id: impl Into<String>,
        credential: impl AzureCredentialSource + 'static,
    ) -> Result<Self, AzureError> {
        let subscription_id = subscription_id.into();
        if !valid_subscription_id(&subscription_id) {
            return Err(AzureError::InvalidProfile);
        }
        Ok(Self {
            agent,
            credential: Box::new(credential),
            subscription_id,
        })
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    fn group_url(&self, name: &str, api_version: &str) -> Result<String, AzureError> {
        valid_resource_group_name(name)
            .then(|| {
                format!(
                    "{MANAGEMENT_ENDPOINT}/subscriptions/{}/resourcegroups/{name}?api-version={api_version}",
                    self.subscription_id
                )
            })
            .ok_or(AzureError::ResourceIdentityMismatch)
    }

    fn resource_url(
        &self,
        group: &str,
        provider_path: &str,
        name: &str,
        api_version: &str,
        suffix: &str,
    ) -> Result<String, AzureError> {
        (valid_resource_group_name(group) && super::valid_resource_name(name))
            .then(|| {
                format!(
                    "{MANAGEMENT_ENDPOINT}/subscriptions/{}/resourcegroups/{group}/providers/{provider_path}/{name}{suffix}?api-version={api_version}",
                    self.subscription_id
                )
            })
            .ok_or(AzureError::ResourceIdentityMismatch)
    }

    fn authorization(&self) -> Result<String, AzureError> {
        self.credential.token().map(|token| token.authorization_header())
    }

    fn authorization_within(&self, budget: Duration) -> Result<String, AzureError> {
        self.credential
            .token_within(budget)
            .map(|token| token.authorization_header())
    }

    fn get_json(&self, url: &str, operation: &'static str) -> Result<Option<serde_json::Value>, AzureError> {
        self.get_json_within(url, operation, REQUEST_TIMEOUT)
    }

    /// A GET whose whole exchange is capped at `budget` (never above the request
    /// timeout), so a poll loop can hand each request only what is left of its bound.
    pub(super) fn get_json_within(
        &self,
        url: &str,
        operation: &'static str,
        budget: Duration,
    ) -> Result<Option<serde_json::Value>, AzureError> {
        let response = self.get_within(url, operation, budget)?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        decode_json(response, &[200], operation).map(Some)
    }

    /// The raw GET behind [`Self::get_json_within`], for callers that read headers.
    /// Obtaining the token counts against the budget: the credential itself is asked to
    /// answer within it (a CLI refresh is bounded by it), the HTTP exchange gets only
    /// what that step left, and none at all once the budget is spent.
    pub(super) fn get_within(
        &self,
        url: &str,
        operation: &'static str,
        budget: Duration,
    ) -> Result<ureq::http::Response<ureq::Body>, AzureError> {
        let started = std::time::Instant::now();
        let authorization = self.authorization_within(budget)?;
        let budget = budget.saturating_sub(started.elapsed());
        if budget.is_zero() {
            return Err(AzureError::OperationTimedOut { operation });
        }
        self.agent
            .get(url)
            .config()
            .timeout_global(Some(budget.min(REQUEST_TIMEOUT)))
            .build()
            .header("Authorization", &authorization)
            .call()
            .map_err(|_| AzureError::RequestFailed { operation })
    }

    /// Send a mutating request. Long-running operations answer with a status and an
    /// empty or unused body, so callers say whether the body must be decoded.
    fn send_json(
        &self,
        method: &'static str,
        url: &str,
        body: Option<&serde_json::Value>,
        accepted: &[u16],
        decode_body: bool,
        operation: &'static str,
    ) -> Result<(u16, serde_json::Value), AzureError> {
        let authorization = self.authorization()?;
        let failed = |_| AzureError::RequestFailed { operation };
        let response = match (method, body) {
            ("DELETE", _) => self
                .agent
                .delete(url)
                .header("Authorization", &authorization)
                .call()
                .map_err(failed)?,
            ("PUT", Some(body)) => self
                .agent
                .put(url)
                .header("Authorization", &authorization)
                .send_json(body)
                .map_err(failed)?,
            ("PUT", None) => self
                .agent
                .put(url)
                .header("Authorization", &authorization)
                .send_empty()
                .map_err(failed)?,
            (_, Some(body)) => self
                .agent
                .post(url)
                .header("Authorization", &authorization)
                .send_json(body)
                .map_err(failed)?,
            (_, None) => self
                .agent
                .post(url)
                .header("Authorization", &authorization)
                .send_empty()
                .map_err(failed)?,
        };
        let status = response.status().as_u16();
        if !accepted.contains(&status) {
            return Err(AzureError::UnexpectedStatus { operation, status });
        }
        if !decode_body {
            return Ok((status, serde_json::Value::Null));
        }
        // An asynchronous acceptance may carry no body at all; the caller treats Null as
        // accepted. A body that is present must still be well-formed JSON within bounds.
        if status == 202 {
            return decode_json_or_empty(response, operation).map(|value| (status, value));
        }
        decode_json(response, accepted, operation).map(|value| (status, value))
    }
}

/// Bounded read of a 202 body: empty is `Null`, anything else must parse as JSON.
fn decode_json_or_empty(
    mut response: ureq::http::Response<ureq::Body>,
    operation: &'static str,
) -> Result<serde_json::Value, AzureError> {
    let text = response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT_BYTES)
        .read_to_string()
        .map_err(|_| AzureError::InvalidResponse { operation })?;
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|_| AzureError::InvalidResponse { operation })
}

fn decode_json(
    mut response: ureq::http::Response<ureq::Body>,
    accepted: &[u16],
    operation: &'static str,
) -> Result<serde_json::Value, AzureError> {
    let status = response.status().as_u16();
    if !accepted.contains(&status) {
        return Err(AzureError::UnexpectedStatus { operation, status });
    }
    response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT_BYTES)
        .read_json()
        .map_err(|_| AzureError::InvalidResponse { operation })
}

/// Time left before an absolute deadline, or `None` once it has passed.
pub(super) fn remaining(deadline: std::time::Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    (!left.is_zero()).then_some(left)
}

fn long_running(status: u16) -> AzureLongRunningState {
    if status == 202 {
        AzureLongRunningState::Accepted
    } else {
        AzureLongRunningState::Completed
    }
}

fn text(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn tags(value: &serde_json::Value) -> BTreeMap<String, String> {
    value
        .get("tags")
        .and_then(serde_json::Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn group_info(
    value: &serde_json::Value,
    subscription_id: &str,
    expected_name: &str,
    operation: &'static str,
) -> Result<AzureGroupInfo, AzureError> {
    let name = text(value, "/name").ok_or(AzureError::InvalidResponse { operation })?;
    let id = text(value, "/id").ok_or(AzureError::InvalidResponse { operation })?;
    if !name.eq_ignore_ascii_case(expected_name) || !super::valid_resource_group_id(&id, subscription_id, expected_name)
    {
        return Err(AzureError::ResourceIdentityMismatch);
    }
    Ok(AzureGroupInfo {
        id,
        name,
        location: text(value, "/location").unwrap_or_default(),
        provisioning_state: text(value, "/properties/provisioningState").unwrap_or_default(),
        tags: tags(value),
    })
}

fn deployment_state(value: &serde_json::Value, operation: &'static str) -> Result<AzureDeploymentState, AzureError> {
    let provisioning_state =
        text(value, "/properties/provisioningState").ok_or(AzureError::InvalidResponse { operation })?;
    let outputs = value
        .pointer("/properties/outputs")
        .and_then(serde_json::Value::as_object)
        .map(|outputs| {
            outputs
                .iter()
                .filter_map(|(key, output)| text(output, "/value").map(|value| (key.clone(), value)))
                .collect()
        })
        .unwrap_or_default();
    Ok(AzureDeploymentState {
        provisioning_state,
        outputs,
    })
}

impl AzureManagementTransport for AzureArmHttp {
    fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    fn get_resource_group(&self, name: &str) -> Result<Option<AzureGroupInfo>, AzureError> {
        self.get_resource_group_within(name, REQUEST_TIMEOUT)
    }

    fn get_resource_group_within(&self, name: &str, budget: Duration) -> Result<Option<AzureGroupInfo>, AzureError> {
        let operation = "resource group lookup";
        self.get_json_within(&self.group_url(name, RESOURCE_GROUP_API_VERSION)?, operation, budget)?
            .map(|value| group_info(&value, &self.subscription_id, name, operation))
            .transpose()
    }

    fn create_resource_group(
        &self,
        name: &str,
        location: &str,
        tags: &BTreeMap<String, String>,
    ) -> Result<AzureGroupInfo, AzureError> {
        let operation = "resource group creation";
        if !super::valid_location(location) {
            return Err(AzureError::InvalidProfile);
        }
        let body = serde_json::json!({ "location": location, "tags": tags });
        let (_, value) = self.send_json(
            "PUT",
            &self.group_url(name, RESOURCE_GROUP_API_VERSION)?,
            Some(&body),
            &[200, 201],
            true,
            operation,
        )?;
        group_info(&value, &self.subscription_id, name, operation)
    }

    fn delete_resource_group(&self, name: &str) -> Result<AzureLongRunningState, AzureError> {
        let operation = "resource group deletion";
        let (status, _) = self.send_json(
            "DELETE",
            &self.group_url(name, RESOURCE_GROUP_API_VERSION)?,
            None,
            &[200, 202, 204],
            false,
            operation,
        )?;
        Ok(long_running(status))
    }

    fn put_deployment(
        &self,
        group: &str,
        name: &str,
        template: &serde_json::Value,
        parameters: &serde_json::Value,
    ) -> Result<AzureDeploymentState, AzureError> {
        let operation = "deployment submission";
        let url = self.resource_url(
            group,
            "Microsoft.Resources/deployments",
            name,
            DEPLOYMENT_API_VERSION,
            "",
        )?;
        let body = serde_json::json!({ "properties": { "mode": "Incremental", "template": template, "parameters": parameters } });
        let (status, value) = self.send_json("PUT", &url, Some(&body), &[200, 201, 202], true, operation)?;
        // An asynchronous acceptance is an acceptance whatever its body carries; the
        // deployment shape is parsed only when the body has it.
        if status == 202 && value.pointer("/properties/provisioningState").is_none() {
            return Ok(AzureDeploymentState {
                provisioning_state: "Accepted".into(),
                outputs: BTreeMap::new(),
            });
        }
        deployment_state(&value, operation)
    }

    fn get_deployment(&self, group: &str, name: &str) -> Result<Option<AzureDeploymentState>, AzureError> {
        let operation = "deployment lookup";
        let url = self.resource_url(
            group,
            "Microsoft.Resources/deployments",
            name,
            DEPLOYMENT_API_VERSION,
            "",
        )?;
        self.get_json(&url, operation)?
            .map(|value| deployment_state(&value, operation))
            .transpose()
    }

    fn get_vm(&self, group: &str, name: &str) -> Result<Option<AzureVmView>, AzureError> {
        self.get_vm_within(group, name, REQUEST_TIMEOUT)
    }

    fn get_vm_within(&self, group: &str, name: &str, budget: Duration) -> Result<Option<AzureVmView>, AzureError> {
        let operation = "virtual machine lookup";
        let url = self.resource_url(
            group,
            "Microsoft.Compute/virtualMachines",
            name,
            COMPUTE_API_VERSION,
            "",
        )?;
        let Some(value) = self.get_json_within(&format!("{url}&$expand=instanceView"), operation, budget)? else {
            return Ok(None);
        };
        let id = text(&value, "/id").ok_or(AzureError::InvalidResponse { operation })?;
        let observed_name = text(&value, "/name").ok_or(AzureError::InvalidResponse { operation })?;
        if !observed_name.eq_ignore_ascii_case(name)
            || !super::valid_vm_resource_id(&id, &self.subscription_id, group, name)
        {
            return Err(AzureError::ResourceIdentityMismatch);
        }
        let power_state = value
            .pointer("/properties/instanceView/statuses")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|status| text(status, "/code"))
            .find(|code| code.starts_with("PowerState/"));
        Ok(Some(AzureVmView {
            id,
            name: observed_name,
            location: text(&value, "/location").unwrap_or_default(),
            vm_size: text(&value, "/properties/hardwareProfile/vmSize").unwrap_or_default(),
            provisioning_state: text(&value, "/properties/provisioningState").unwrap_or_default(),
            power_state,
            tags: tags(&value),
        }))
    }

    fn deallocate_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        self.power_operation(group, name, "/deallocate", "virtual machine deallocation")
    }

    fn start_vm(&self, group: &str, name: &str) -> Result<Option<AzureLongRunningState>, AzureError> {
        self.power_operation(group, name, "/start", "virtual machine start")
    }

    fn run_command(&self, group: &str, name: &str, command: AzureRunCommand) -> Result<Option<String>, AzureError> {
        self.submit_run_command(group, name, command)
    }
}
