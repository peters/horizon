//! A real local agent can prepare missing repository settings before any allocation.
use super::{HorizonApp, PanelKind, PanelOptions, Production};
use crate::theme;

fn prompt(agents: &[horizon_core::cloud_runtime::setup::Agent]) -> String {
    let selection = agents.iter().map(|agent| agent.as_str()).collect::<Vec<_>>().join(", ");
    format!(
        "{PROMPT}\nMachine account preference for new profiles: agents [{selection}]. Preserve existing repository profile choices unless I explicitly ask to change them."
    )
}

pub(super) fn render(ui: &mut egui::Ui, form: &mut Production) -> bool {
    ui.add_space(12.0);
    ui.collapsing("No cloud configuration yet?", |ui| {
        ui.label("Let a local agent inspect this repository and prepare its worker image and cloud.yml. Review and commit the files, then return here and reload.");
        ui.small("This setup terminal uses your local agent login. Cloud settings configure authentication on remote workers.");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut form.setup_agent, Some(PanelKind::Codex), "Codex");
            ui.selectable_value(&mut form.setup_agent, Some(PanelKind::Claude), "Claude");
        });
        ui.add_enabled(!form.repository.trim().is_empty() && form.setup_agent.is_some(),
            egui::Button::new("Open setup agent").fill(theme::PANEL_BG_ALT())).clicked()
    }).body_returned.unwrap_or(false)
}

impl HorizonApp {
    pub(super) fn start_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let form = &self.cloud_prototype.production;
        let Some(kind) = form.setup_agent else { return };
        let repository = match std::fs::canonicalize(&form.repository) {
            Ok(path) if path.is_dir() => path,
            _ => {
                self.cloud_prototype.error = Some("Choose an existing local repository directory".into());
                return;
            }
        };
        let workspace = self.board.ensure_workspace();
        // Deliberately local: config preparation must not inherit a focused cloud.
        let result = self.board.create_panel(
            PanelOptions {
                name: Some("Cloud repository setup".into()),
                name_is_custom: Some(true),
                kind,
                cwd: Some(repository),
                args: vec![prompt(&form.setup_agents)],
                transcript_root: self.transcript_root.clone(),
                ..PanelOptions::default()
            },
            workspace,
        );
        match result {
            Ok(panel) => {
                self.cloud_prototype.production.creating = false;
                self.cloud_prototype.error = None;
                self.reveal_new_panel(ctx, workspace, panel);
            }
            Err(error) => self.cloud_prototype.error = Some(error.to_string()),
        }
    }
}

const PROMPT: &str = "Prepare this repository for Horizon Cloud Workspaces. First inspect AGENTS.md, its build/test requirements and existing Dockerfiles. Explain your proposed setup before editing. Create or update .horizon/cloud.yml version 1 with a named default profile, provider: runpod, image, optional build {context, dockerfile, platform: linux/amd64}, cpu, memory_gb, gpu and explicit capabilities {agents, browsers, desktop}. Preserve required GPU/runtime dependencies; never silently choose a CPU fallback. Ask which coding agents and tools are wanted if not known. Native desktop/VNC does not need browsers. A worker image must satisfy Horizon's SSH, Git, tmux and enabled agent/tool contract; use the documented Horizon worker examples, not an arbitrary application image. Keep all credentials in machine-local bindings, never YAML, source, images or build caches. Do not allocate compute, push images, commit, or open a PR automatically. Run applicable config/build checks, explain prerequisites and ask me to review and commit the setup. Then tell me to return to Cloud > New cloud and reload .horizon/cloud.yml.";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_agent_preference_is_passed_without_machine_bindings() {
        let text = prompt(&[horizon_core::cloud_runtime::setup::Agent::Claude]);
        assert!(text.contains("agents [claude]"));
        assert!(!text.contains("agents [codex"));
        assert!(text.contains("Preserve existing repository profile choices"));
        assert!(!text.contains("settings.json"));
    }
}
