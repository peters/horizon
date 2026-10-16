use super::*;
use horizon_core::cloud_runtime::{claude_token::ClaudeToken, settings::Settings, setup::Agent};

#[test]
fn registry_edit_preserves_agent_bindings_and_defers_profile_authentication_until_start() {
    for token in [false, true] {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let root = temp.path().join("cloud");
        let mut settings = bound_registry_draft(&root).settings;
        settings.default_agents = vec![Agent::Codex];
        let absent = root.join("absent-agent-credential");
        if token {
            settings.claude_token = Some(ClaudeToken {
                file: absent.clone(),
                saved_at: 1,
            });
        } else {
            settings.anthropic_api_key_file = Some(absent.clone());
        }
        std::fs::write(root.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        app.cloud_prototype.root = Some(root.clone());
        let form = &mut app.cloud_prototype.production;
        form.profiles = Some(horizon_core::cloud_panel::CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    capabilities:\n      agents: [claude]\n").unwrap());
        form.selected_profile = "dev".into();

        app.edit_creation_registry(&ctx, Some("registry.example/another/worker".into()));
        wait(&mut app, &ctx);
        let state = &mut app.cloud_prototype.production.setup;
        assert!(state.open);
        state
            .draft
            .as_mut()
            .unwrap()
            .registries
            .push(horizon_core::cloud_runtime::registry::draft::Draft {
                repository: "registry.example/another/worker".into(),
                pull_username: "reader".into(),
                pull_secret: zeroize::Zeroizing::new("another-synthetic-pull".into()),
                read_only_confirmed: true,
                ..Default::default()
            });
        app.save_cloud_accounts(&ctx);
        wait(&mut app, &ctx);
        let form = &app.cloud_prototype.production;
        assert!(form.setup.error.is_none(), "{:?}", form.setup.error);
        assert!(!form.setup.open);
        assert!(form.creating);
        assert!(!form.launch.accounts_checked);
        assert!(!form.launch.submitted);
        assert!(app.cloud_prototype.groups.0.is_empty());
        let saved = Settings::load(&root.join("settings.json")).unwrap();
        assert_eq!(saved.default_agents, settings.default_agents);
        assert_eq!(saved.claude_token, settings.claude_token);
        assert_eq!(saved.anthropic_api_key_file, settings.anthropic_api_key_file);
        assert_eq!(saved.runpod_key_file, settings.runpod_key_file);
        assert_eq!(saved.registries.unwrap().bindings.len(), 2);
        assert!(!absent.exists());

        app.cloud_prototype.production.launch.submitted = true;
        app.poll_cloud_launch(&ctx);
        wait(&mut app, &ctx);
        let state = &app.cloud_prototype.production.setup;
        assert!(state.open, "Start must still require the profile's missing credential");
        let draft = state.draft.as_ref().unwrap();
        assert_eq!(draft.selected_agents(), &[Agent::Claude]);
        assert!(draft.validate().is_err());
        assert!(app.cloud_prototype.groups.0.is_empty());
    }
}
