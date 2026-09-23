use horizon_core::{Config, PanelKind, RuntimeState, SshConnection, StartupDecision, WorkspaceId};

use crate::app::test_support::{test_app_with_config_and_startup, test_app_with_startup};
use crate::remote_hosts_overlay::{RemoteConnectMode, WorkspaceChoice};

fn ephemeral() -> StartupDecision {
    StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    }
}

fn lab_connection() -> SshConnection {
    SshConnection {
        host: "lab".into(),
        user: Some("deploy".into()),
        ..SshConnection::default()
    }
}

#[test]
fn vnc_opens_a_tunnelled_device_panel_in_the_default_workspace() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let connection = lab_connection();

    let panel_id = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            connection.clone(),
            RemoteConnectMode::Vnc,
            &WorkspaceChoice::Default,
        )
        .expect("panel");

    let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
    assert_eq!(app.board.workspace(workspace_id).unwrap().name, "Remote Sessions");
    let panel = app.board.panel(panel_id).unwrap();
    assert_eq!(panel.kind, PanelKind::Device);
    assert_eq!(panel.title, "lab");
    let device = panel.device().expect("device state");
    assert_eq!(device.ssh_tunnel.as_ref(), Some(&connection));
    assert_eq!(device.target.address().to_string(), "127.0.0.1:5900");
    assert!(device.connect_on_start);
}

#[test]
fn the_configured_vnc_port_and_workspace_name_are_used() {
    let mut config = Config::default();
    config.remote_hosts.default_workspace = "Ops".into();
    config.remote_hosts.vnc_port = 5901;
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(&config, ephemeral());

    let panel_id = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Vnc,
            &WorkspaceChoice::Default,
        )
        .expect("panel");

    let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
    assert_eq!(app.board.workspace(workspace_id).unwrap().name, "Ops");
    let device = app.board.panel(panel_id).unwrap().device().expect("device state");
    assert_eq!(device.target.address().to_string(), "127.0.0.1:5901");
}

#[test]
fn ssh_opens_in_the_chosen_existing_workspace_and_reuses_the_default_one() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let ops = app.board.create_workspace("Ops");

    let first = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Ssh,
            &WorkspaceChoice::Existing(ops),
        )
        .expect("panel");
    assert_eq!(app.board.panel_workspace_id(first), Some(ops));
    let panel = app.board.panel(first).unwrap();
    assert_eq!(panel.kind, PanelKind::Ssh);
    assert_eq!(panel.ssh_connection.as_ref(), Some(&lab_connection()));

    let workspaces_before = app.board.workspaces.len();
    let second = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Ssh,
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    let third = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Vnc,
            &WorkspaceChoice::Existing(WorkspaceId(999)),
        )
        .expect("panel");
    let default_id = app.board.panel_workspace_id(second).expect("workspace");
    assert_eq!(app.board.workspace(default_id).unwrap().name, "Remote Sessions");
    assert_eq!(
        app.board.panel_workspace_id(third),
        Some(default_id),
        "a closed workspace choice falls back to the default"
    );
    assert_eq!(app.board.workspaces.len(), workspaces_before + 1);
}

#[test]
fn setting_the_default_workspace_rewrites_the_config_and_applies_it() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());

    assert!(app.set_remote_hosts_default_workspace("  Ops  "));
    assert!(!app.set_remote_hosts_default_workspace("   "));

    let saved = std::fs::read_to_string(&app.config_path).expect("config written");
    assert!(
        saved.contains("remote_hosts:\n  default_workspace: '  Ops  '\n  vnc_port: 5900\n"),
        "{saved}"
    );
    assert_eq!(
        app.template_config.remote_hosts.default_workspace_name(),
        "  Ops  ",
        "kept exact"
    );
    assert!(app.set_remote_hosts_default_workspace("Ops"));
    assert_eq!(
        Config::load(Some(&app.config_path))
            .unwrap()
            .remote_hosts
            .default_workspace_name(),
        "Ops"
    );

    let panel_id = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Vnc,
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
    assert_eq!(app.board.workspace(workspace_id).unwrap().name, "Ops");
}

#[test]
fn set_default_waits_for_unsaved_settings_edits_and_refreshes_a_clean_editor() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    std::fs::write(&app.config_path, Config::default().to_yaml().unwrap()).unwrap();
    app.toggle_settings();
    assert!(app.settings.is_some(), "settings editor open");

    // An unsaved edit in the editor blocks the write and stays intact.
    if let Some(editor) = app.settings.as_mut() {
        editor.buffer.push_str("\n# draft\n");
    }
    assert!(app.settings_has_unsaved_edits());
    assert!(!app.set_remote_hosts_default_workspace("Ops"));
    // A draft that already says the new name must not count as persisted.
    if let Some(editor) = app.settings.as_mut() {
        editor.buffer = editor
            .original
            .replace("default_workspace: Remote Sessions", "default_workspace: Ops");
    }
    assert!(app.settings_has_unsaved_edits());
    assert!(!app.set_remote_hosts_default_workspace("Ops"));
    if let Some(editor) = app.settings.as_mut() {
        editor.buffer.push_str("\n# draft\n");
    }
    assert_eq!(
        app.template_config.remote_hosts.default_workspace_name(),
        "Remote Sessions"
    );
    assert!(app.settings.as_ref().unwrap().buffer.ends_with("# draft\n"));

    // A clean editor is moved onto the rewritten file so a later Save keeps the default.
    if let Some(editor) = app.settings.as_mut() {
        editor.buffer.clone_from(&editor.original);
    }
    assert!(app.set_remote_hosts_default_workspace("Ops"));
    let editor = app.settings.as_ref().unwrap();
    assert_eq!(editor.buffer, editor.original);
    assert!(editor.buffer.contains("default_workspace: Ops"), "{}", editor.buffer);
    assert_eq!(
        editor
            .editing_config()
            .map(|config| config.remote_hosts.default_workspace_name()),
        Some("Ops"),
        "the GUI tabs' snapshot follows the file"
    );
    assert_eq!(
        std::fs::read_to_string(&app.config_path).unwrap(),
        editor.buffer,
        "editor text matches the file"
    );
}

#[test]
fn setting_the_default_workspace_patches_the_file_text_in_place() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    let source = "version: 11 # keep this comment\nremote_hosts:\n  vnc_port: 5901 # and this one\n  future_key: true\nworkspaces: []\n";
    std::fs::write(&app.config_path, source).unwrap();

    assert!(app.set_remote_hosts_default_workspace("Ops"));

    let saved = std::fs::read_to_string(&app.config_path).unwrap();
    assert_eq!(
        saved,
        "version: 11 # keep this comment\nremote_hosts:\n  default_workspace: Ops\n  vnc_port: 5901 # and this one\n  future_key: true\nworkspaces: []\n"
    );
    assert_eq!(app.template_config.remote_hosts.default_workspace_name(), "Ops");
}

#[test]
fn set_default_never_overwrites_a_file_it_cannot_patch_in_place() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());

    // A flow-style section is valid YAML the patcher does not rewrite.
    let flow = "version: 11 # keep\nremote_hosts: { vnc_port: 5901 }\nworkspaces: []\n";
    std::fs::write(&app.config_path, flow).unwrap();
    assert!(!app.set_remote_hosts_default_workspace("Ops"));
    assert_eq!(std::fs::read_to_string(&app.config_path).unwrap(), flow);

    // A file that is currently invalid keeps its external edits.
    let broken = "version: 11\nremote_hosts:\n  vnc_port: [\n";
    std::fs::write(&app.config_path, broken).unwrap();
    assert!(!app.set_remote_hosts_default_workspace("Ops"));
    assert_eq!(std::fs::read_to_string(&app.config_path).unwrap(), broken);
    assert_eq!(
        app.template_config.remote_hosts.default_workspace_name(),
        "Remote Sessions"
    );

    // The text written is what gets applied: a file edited since the last
    // reload keeps its other settings when one key is patched.
    let edited = "version: 11\nremote_hosts:\n  vnc_port: 5999\nworkspaces: []\n";
    std::fs::write(&app.config_path, edited).unwrap();
    assert_eq!(app.template_config.remote_hosts.vnc_port, 5900, "not reloaded yet");
    assert!(app.set_remote_hosts_default_workspace("Ops"));
    assert_eq!(app.template_config.remote_hosts.vnc_port, 5999, "the file's value wins");
    assert_eq!(app.template_config.remote_hosts.default_workspace_name(), "Ops");

    // Only a genuinely absent file is written from the config.
    std::fs::remove_file(&app.config_path).unwrap();
    assert!(app.set_remote_hosts_default_workspace("Fresh"));
    assert!(
        std::fs::read_to_string(&app.config_path)
            .unwrap()
            .contains("default_workspace: Fresh")
    );
}

#[test]
fn legacy_remote_workspaces_are_neither_listed_nor_used_as_destinations() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let legacy = app.board.create_workspace("Remote Sessions");
    app.board.workspace_mut(legacy).unwrap().remote_workspace = Some(
        horizon_core::RemoteWorkspaceReference::new(
            "8d8c366e-f07e-4dd1-9518-1e75b6eca205".into(),
            "remote-environment".into(),
        )
        .expect("reference"),
    );
    let ops = app.board.create_workspace("Ops");

    assert_eq!(
        app.destination_workspaces()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![ops],
        "the legacy workspace is not offered"
    );

    let picked = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Ssh,
            &WorkspaceChoice::Existing(legacy),
        )
        .expect("panel");
    let by_default = app
        .open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Vnc,
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    for panel_id in [picked, by_default] {
        let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
        assert_ne!(workspace_id, legacy, "never the legacy workspace");
        let workspace = app.board.workspace(workspace_id).unwrap();
        assert_eq!(workspace.name, "Remote Sessions");
        assert!(
            workspace.remote_workspace.is_none(),
            "a fresh local default was created"
        );
        assert!(
            app.board.panel(panel_id).unwrap().device().is_some()
                || app.board.panel(panel_id).unwrap().kind == PanelKind::Ssh
        );
    }
}

#[test]
fn saving_shortcuts_stores_presets_the_palette_can_create_anywhere() {
    let mut config = Config::default();
    config.remote_hosts.vnc_port = 5901;
    let (_temp, _ctx, mut app) = test_app_with_config_and_startup(&config, ephemeral());
    let presets_before = app.template_config.presets.len();

    assert_eq!(
        app.save_remote_host_shortcut("lab", lab_connection(), RemoteConnectMode::Vnc),
        Some("VNC: lab".to_string())
    );
    assert_eq!(
        app.save_remote_host_shortcut(" lab ", lab_connection(), RemoteConnectMode::Ssh),
        Some("SSH: lab".to_string())
    );
    let blank = SshConnection {
        host: " ".into(),
        ..SshConnection::default()
    };
    assert_eq!(
        app.save_remote_host_shortcut("lab", blank, RemoteConnectMode::Ssh),
        None
    );

    let saved = Config::load(Some(&app.config_path)).expect("config reloads");
    assert_eq!(saved.presets.len(), presets_before + 2);
    let vnc = saved
        .presets
        .iter()
        .find(|preset| preset.name == "VNC: lab")
        .expect("vnc preset");
    assert_eq!(vnc.kind, PanelKind::Device);
    assert_eq!(vnc.command.as_deref(), Some("127.0.0.1:5901"));
    assert_eq!(vnc.ssh_connection.as_ref(), Some(&lab_connection()));
    let ssh = saved
        .presets
        .iter()
        .find(|preset| preset.name == "SSH: lab")
        .expect("ssh preset");
    assert_eq!(ssh.kind, PanelKind::Ssh);
    assert_eq!(ssh.command, None);
    assert!(
        app.presets.iter().any(|preset| preset.name == "VNC: lab"),
        "applied live"
    );

    // Re-saving with another user replaces the preset instead of duplicating it.
    let as_root = SshConnection {
        user: Some("root".into()),
        ..lab_connection()
    };
    assert_eq!(
        app.save_remote_host_shortcut("lab", as_root.clone(), RemoteConnectMode::Vnc),
        Some("VNC: lab".to_string())
    );
    let saved = Config::load(Some(&app.config_path)).expect("config reloads");
    assert_eq!(saved.presets.len(), presets_before + 2);
    let vnc = saved
        .presets
        .iter()
        .find(|preset| preset.name == "VNC: lab")
        .expect("vnc preset");
    assert_eq!(vnc.ssh_connection.as_ref(), Some(&as_root));
}

mod picker_in_full_app {
    use horizon_core::{RemoteHost, RemoteHostCatalog, RemoteHostSources, RemoteHostStatus, SshConnection};

    use super::*;
    use crate::app::HorizonApp;
    use crate::app::test_support::{raw_input, run_app_frame_with_input};

    fn texts<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
        match shape {
            egui::Shape::Text(text) => out.push(text),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| texts(shape, out)),
            _ => {}
        }
    }

    fn label_center(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        let mut found = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut found);
        }
        found
            .iter()
            .find(|text| text.galley.text() == label)
            .map(|text| text.pos + text.galley.size() * 0.5)
    }

    fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut input = raw_input([1400.0, 900.0], None);
        input.events = events;
        run_app_frame_with_input(ctx, app, input)
    }

    fn click(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn destination_picker_opens_with_workspaces_and_panels_on_the_board() {
        let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let ops = app.board.create_workspace("Ops");
        app.open_remote_host(
            &ctx,
            "lab".into(),
            lab_connection(),
            RemoteConnectMode::Ssh,
            &WorkspaceChoice::Existing(ops),
        )
        .expect("ssh panel");
        app.remote_hosts_catalog = RemoteHostCatalog {
            hosts: vec![RemoteHost {
                label: "lab".into(),
                ssh_connection: SshConnection {
                    host: "lab".into(),
                    ..SshConnection::default()
                },
                sources: RemoteHostSources::default(),
                status: RemoteHostStatus::Unknown,
                last_seen_secs: None,
                os: None,
                hostname: None,
                tags: Vec::new(),
                ips: Vec::new(),
            }],
            refreshed_at: None,
        };
        // Keep discovery from replacing the fixture catalog mid-test.
        app.remote_hosts_refresh_in_flight = true;
        for _ in 0..3 {
            frame(&ctx, &mut app, Vec::new());
        }
        app.toggle_remote_hosts_overlay(&ctx);
        let mut output = frame(&ctx, &mut app, Vec::new());
        for _ in 0..3 {
            output = frame(&ctx, &mut app, Vec::new());
        }
        let picker = label_center(&output, "Remote Sessions (new)  \u{25be}").expect("picker button");
        frame(&ctx, &mut app, click(picker, true));
        frame(&ctx, &mut app, click(picker, false));
        let output = frame(&ctx, &mut app, Vec::new());
        assert!(
            label_center(&output, "Ops").is_some(),
            "picker popup did not open in the full app"
        );
        assert!(app.remote_hosts_overlay.is_some(), "overlay was dismissed by the click");
    }
}
