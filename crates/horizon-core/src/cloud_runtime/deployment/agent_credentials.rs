//! Agent API credentials validated before allocation and installed on the worker.
use super::{Connection, Result, Runner, Settings};
use std::time::Duration;

pub(super) fn validate_agent_auth(settings: &Settings, capabilities: &horizon_cloud::Capabilities) -> Result<()> {
    for path in [
        ("claude", &settings.anthropic_api_key_file),
        ("codex", &settings.openai_api_key_file),
    ]
    .into_iter()
    .filter(|(agent, _)| capabilities.permits_agent(agent))
    .filter_map(|(_, path)| path.as_ref())
    {
        crate::cloud_runtime::settings::validate_private_key_file(path)?;
    }
    Ok(())
}
pub(super) fn configure_agent_auth(
    connection: &Connection,
    settings: &Settings,
    capabilities: &horizon_cloud::Capabilities,
    runner: &Runner<'_>,
) -> Result<()> {
    let clear = format!(
        "python3 - {} {} {} <<'HORIZON_AUTH_CLEANUP'\n{}\nHORIZON_AUTH_CLEANUP",
        u8::from(!capabilities.permits_agent("claude") || settings.anthropic_api_key_file.is_none()),
        u8::from(!capabilities.permits_agent("codex") || settings.openai_api_key_file.is_none()),
        u8::from(!capabilities.permits_agent("claude") || settings.anthropic_workspace_id.is_none()),
        include_str!("clear_agent_auth.py"),
    );
    runner.run(
        "Removed agent credential bindings",
        &mut connection.command(&clear),
        Duration::from_secs(20),
    )?;
    if capabilities.permits_agent("claude")
        && let Some(path) = &settings.anthropic_api_key_file
    {
        runner.private_input(&mut connection.command(
            "umask 077; mkdir -p /workspace/credentials && cat > /workspace/credentials/anthropic-api-key.new && mv /workspace/credentials/anthropic-api-key.new /workspace/credentials/anthropic-api-key"
        ), path)?;
    }
    if capabilities.permits_agent("codex")
        && let Some(path) = &settings.openai_api_key_file
    {
        runner.private_input(
            &mut connection.command("umask 077; HOME=/workspace/home codex login --with-api-key"),
            path,
        )?;
    }
    if capabilities.permits_agent("claude")
        && let Some(workspace) = &settings.anthropic_workspace_id
    {
        runner.run("Agent workspace binding", &mut connection.command(&format!(
            "umask 077; mkdir -p /workspace/credentials && printf '%s' '{workspace}' > /workspace/credentials/anthropic-workspace"
        )), Duration::from_secs(20))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_agent_credentials_must_be_nonempty_before_deployment() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused", "ssh_identity_file":"unused", "docker_config":"unused",
            "registry_pull_auth_id":null, "cpu_flavors":[], "gpu_types":[]
        }))
        .unwrap();
        for agent in [horizon_cloud::Agent::Codex, horizon_cloud::Agent::Claude] {
            settings.openai_api_key_file = (agent == horizon_cloud::Agent::Codex).then(|| file.path().into());
            settings.anthropic_api_key_file = (agent == horizon_cloud::Agent::Claude).then(|| file.path().into());
            let selected: horizon_cloud::Capabilities =
                serde_json::from_value(serde_json::json!({"agents":[agent]})).unwrap();
            let disabled: horizon_cloud::Capabilities = serde_json::from_str("{}").unwrap();
            for content in ["", " \n\t\r", "fixture-credential\n"] {
                std::fs::write(file.path(), content).unwrap();
                assert_eq!(
                    validate_agent_auth(&settings, &selected).is_ok(),
                    !content.trim().is_empty()
                );
                assert!(validate_agent_auth(&settings, &disabled).is_ok());
            }
        }
    }
}
