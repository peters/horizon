use serde_json::{Map, Value};

pub(super) fn horizon_opencode_mcp_overlay() -> Option<String> {
    let command = crate::browser_mcp_executable()?.into_os_string().into_string().ok()?;
    merge_opencode_mcp_overlay(std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref(), &command)
}

fn merge_opencode_mcp_overlay(existing: Option<&str>, command: &str) -> Option<String> {
    let mut root = match existing.map(str::trim).filter(|value| !value.is_empty()) {
        None => Value::Object(Map::new()),
        Some(raw) => parse_opencode_config_object(raw)?,
    };
    let object = root.as_object_mut()?;
    match object.get("mcp") {
        None => {
            object.insert("mcp".to_string(), Value::Object(Map::new()));
        }
        Some(Value::Object(_)) => {}
        Some(_) => {
            tracing::warn!("OPENCODE_CONFIG_CONTENT.mcp is not a JSON object; skipping Horizon browser MCP overlay");
            return None;
        }
    }
    let mcp = object.get_mut("mcp")?.as_object_mut()?;
    mcp.insert(
        "horizon-browser".to_string(),
        serde_json::json!({
            "type": "local",
            "command": [command, "--browser-mcp"],
            "enabled": true,
            "environment": {
                "HORIZON_BROWSER_ACTOR": "{env:HORIZON_BROWSER_ACTOR}",
                "HORIZON_BROWSER_HOST_INSTANCE": "{env:HORIZON_BROWSER_HOST_INSTANCE}",
            }
        }),
    );
    serde_json::to_string(&root).ok()
}

fn parse_opencode_config_object(raw: &str) -> Option<Value> {
    let mut jsonc = raw.to_string();
    if let Err(error) = json_strip_comments::strip(&mut jsonc) {
        tracing::warn!(
            %error,
            "OPENCODE_CONFIG_CONTENT is not valid JSONC; skipping Horizon browser MCP overlay"
        );
        return None;
    }
    match serde_json::from_str::<Value>(jsonc.trim()) {
        Ok(Value::Object(map)) => Some(Value::Object(map)),
        Ok(_) => {
            tracing::warn!("OPENCODE_CONFIG_CONTENT is not a JSON object; skipping Horizon browser MCP overlay");
            None
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "OPENCODE_CONFIG_CONTENT is not valid JSONC; skipping Horizon browser MCP overlay"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::merge_opencode_mcp_overlay;

    #[test]
    fn opencode_overlay_merges_into_existing_inline_config() {
        let overlay = merge_opencode_mcp_overlay(
            Some(r#"{"model":"x","mcp":{"github":{"type":"remote","url":"https://example"}}}"#),
            "/opt/horizon",
        )
        .expect("merge overlay");
        assert!(overlay.contains("\"model\":\"x\"") || overlay.contains("\"model\": \"x\""));
        assert!(overlay.contains("github"));
        assert!(overlay.contains("https://example"));
        assert!(overlay.contains("horizon-browser"));
        assert!(overlay.contains("/opt/horizon"));
    }

    #[test]
    fn opencode_overlay_skips_non_object_inline_config() {
        assert!(merge_opencode_mcp_overlay(Some("[1,2]"), "/opt/horizon").is_none());
        assert!(merge_opencode_mcp_overlay(Some("not-json"), "/opt/horizon").is_none());
        assert!(merge_opencode_mcp_overlay(Some(r#"{"mcp":[]}"#), "/opt/horizon").is_none());
    }

    #[test]
    fn opencode_overlay_merges_jsonc_inline_config() {
        let overlay = merge_opencode_mcp_overlay(
            Some(
                r#"{
  // default model
  "model": "x",
  "mcp": {
    "github": {
      "type": "remote",
      "url": "https://example",
    },
  },
}"#,
            ),
            "/opt/horizon",
        )
        .expect("merge jsonc overlay");
        assert!(overlay.contains("\"model\":\"x\"") || overlay.contains("\"model\": \"x\""));
        assert!(overlay.contains("github"));
        assert!(overlay.contains("https://example"));
        assert!(overlay.contains("horizon-browser"));
        assert!(overlay.contains("/opt/horizon"));
    }
}
