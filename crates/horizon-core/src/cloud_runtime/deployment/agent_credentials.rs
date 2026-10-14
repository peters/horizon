//! Agent API credentials validated before allocation and installed on the worker, and
//! this computer's `ChatGPT` login for Codex given to it.
use super::{Connection, Error, Result, Runner, Settings};
use std::time::Duration;

/// Refuses the first agent of `capabilities` whose configured API-key file is unusable,
/// naming the agent and the file; for a `rebuild`, the refusal says a reconnect needs it too.
pub(super) fn validate_agent_auth(
    settings: &Settings,
    capabilities: &horizon_cloud::Capabilities,
    rebuild: bool,
) -> Result<()> {
    for (agent, path) in [
        ("claude", &settings.anthropic_api_key_file),
        ("codex", &settings.openai_api_key_file),
    ]
    .into_iter()
    .filter(|(agent, _)| capabilities.permits_agent(agent))
    .filter_map(|(agent, path)| Some((agent, path.as_ref()?)))
    {
        crate::cloud_runtime::settings::validate_private_key_file(path).map_err(|error| Error::AgentCredential {
            agent,
            path: path.clone(),
            problem: match error {
                Error::Io(error) if error.kind() == std::io::ErrorKind::NotFound => "the file is missing".into(),
                Error::Io(error) => error.to_string(),
                error => error.to_string(),
            },
            rebuild,
        })?;
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
        "launcher=python3; if [ -f /run/horizon-tailnet/agent-isolation ]; then launcher='horizon-worker-tailnet agent python3'; fi; $launcher - {} {} {} <<'HORIZON_AUTH_CLEANUP'\n{}\nHORIZON_AUTH_CLEANUP",
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
        runner.private_input(&mut connection.command(&write_command("anthropic-api-key", None)), path)?;
    }
    if capabilities.permits_agent("codex")
        && let Some(path) = &settings.openai_api_key_file
    {
        runner.private_input(
            &mut connection.command("umask 077; if [ -f /run/horizon-tailnet/agent-isolation ]; then exec horizon-worker-tailnet agent codex login --with-api-key; else HOME=/workspace/home codex login --with-api-key; fi"),
            path,
        )?;
    } else if capabilities.permits_agent("codex")
        && let Some(home) = &settings.codex_home
    {
        crate::cloud_runtime::codex_login::give(home, connection, runner)?;
    }
    if capabilities.permits_agent("claude")
        && let Some(workspace) = &settings.anthropic_workspace_id
    {
        runner.run(
            "Agent workspace binding",
            &mut connection.command(&write_command("anthropic-workspace", Some(workspace))),
            Duration::from_secs(20),
        )?;
    }
    Ok(())
}

fn write_command(name: &str, value: Option<&str>) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\"'\"'"));
    let command = format!(
        "python3 -c {} {} {}",
        quote(include_str!("write_agent_auth.py")),
        quote(name),
        value.map_or_else(String::new, quote),
    );
    format!(
        "if [ -f /run/horizon-tailnet/agent-isolation ]; then exec horizon-worker-tailnet agent /bin/sh -c {}; else exec /bin/sh -c {}; fi",
        quote(&command),
        quote(&command),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "exports nonsecret commands for an explicitly isolated worker smoke"]
    fn export_protected_commands_for_isolated_smoke() {
        let output = std::env::var_os("HORIZON_TEST_AUTH_COMMANDS").unwrap();
        let commands = serde_json::json!({
            "key": write_command("anthropic-api-key", None),
            "workspace": write_command("anthropic-workspace", Some("synthetic-workspace")),
        });
        std::fs::write(output, serde_json::to_vec(&commands).unwrap()).unwrap();
    }

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
                    validate_agent_auth(&settings, &selected, false).is_ok(),
                    !content.trim().is_empty()
                );
                assert!(validate_agent_auth(&settings, &disabled, false).is_ok());
            }
            // A deployment names the agent and the file; only a rebuild adds the reconnect.
            std::fs::write(file.path(), "").unwrap();
            let name = serde_json::to_value(agent).unwrap();
            let error = validate_agent_auth(&settings, &selected, false)
                .unwrap_err()
                .to_string();
            assert!(
                error.starts_with(&format!(
                    "Agent {} cannot use its API-key file {}: ",
                    name.as_str().unwrap(),
                    file.path().display()
                )) && !error.contains("reconnect"),
                "{error}"
            );
        }
    }
}
