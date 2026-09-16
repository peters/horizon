use horizon_browser::{BrowserControlAction, BrowserHttpAuthOperation, SecretString};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum HttpAuthOperation {
    Set,
    Clear,
}

impl From<HttpAuthOperation> for BrowserHttpAuthOperation {
    fn from(value: HttpAuthOperation) -> Self {
        match value {
            HttpAuthOperation::Set => Self::Set,
            HttpAuthOperation::Clear => Self::Clear,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct HttpAuthInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Set credentials before navigating to a protected page, or clear them.
    operation: HttpAuthOperation,
    /// Username for HTTP Basic or Digest. Set only.
    username: Option<String>,
    /// Password for HTTP Basic or Digest. Set only. Never written to Horizon's audit log.
    password: Option<String>,
    /// Optional `http://host[:port]` or `https://host[:port]` origin that may use the credentials. Set only.
    origin: Option<String>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

impl HttpAuthInput {
    pub(crate) fn build_action(&self) -> Result<BrowserControlAction, String> {
        if matches!(self.operation, HttpAuthOperation::Clear)
            && (self.username.is_some() || self.password.is_some() || self.origin.is_some())
        {
            return Err("HTTP auth clear does not accept credentials".to_string());
        }
        if matches!(self.operation, HttpAuthOperation::Set) && (self.username.is_none() || self.password.is_none()) {
            return Err("HTTP auth set requires username and password".to_string());
        }
        Ok(BrowserControlAction::HttpAuth {
            operation: self.operation.into(),
            username: self.username.clone(),
            password: self.password.clone().map(SecretString::new),
            origin: self.origin.clone(),
        })
    }

    pub(crate) fn output(&self, action_id: String) -> HttpAuthOutput {
        let active = matches!(self.operation, HttpAuthOperation::Set);
        HttpAuthOutput {
            panel_id: self.panel_id.clone(),
            action_id,
            operation: match self.operation {
                HttpAuthOperation::Set => "set",
                HttpAuthOperation::Clear => "clear",
            }
            .to_string(),
            origin: if active { self.origin.clone() } else { None },
            active,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct HttpAuthOutput {
    panel_id: String,
    action_id: String,
    operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_requires_username_and_password_and_clear_rejects_them() {
        let set = HttpAuthInput {
            panel_id: "panel".into(),
            operation: HttpAuthOperation::Set,
            username: Some("smoke-user".into()),
            password: Some("smoke-pass-zephyr".into()),
            origin: Some("http://127.0.0.1:8080".into()),
            timeout_millis: None,
        };
        let BrowserControlAction::HttpAuth { password, .. } = set.build_action().expect("set") else {
            panic!("expected http auth");
        };
        assert_eq!(format!("{:?}", password.expect("password")), "SecretString(<redacted>)");
        let clear = HttpAuthInput {
            panel_id: "panel".into(),
            operation: HttpAuthOperation::Clear,
            username: Some("smoke-user".into()),
            password: None,
            origin: None,
            timeout_millis: None,
        };
        assert!(clear.build_action().is_err());
    }
}
