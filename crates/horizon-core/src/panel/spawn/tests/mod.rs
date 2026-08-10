use super::*;

mod general;
mod setup_prompt;
mod transcript_session;

fn fresh_launch_context(resume: &PanelResume) -> AgentLaunchContext<'_> {
    AgentLaunchContext {
        resume,
        session_binding: None,
        should_resume_binding: false,
        initial_agent_prompt: None,
        agent_login_shell: false,
        is_restore: false,
    }
}

fn absolute_agent_command() -> String {
    if cfg!(windows) {
        r"C:\Program Files\Setup Agent\agent.exe".to_string()
    } else {
        "/opt/tools/setup-agent".to_string()
    }
}
