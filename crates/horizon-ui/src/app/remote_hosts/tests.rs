use horizon_core::{Config, PanelKind, RuntimeState, SshConnection, StartupDecision, WorkspaceId};

use super::launch::RemoteLaunch;
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

fn launch(label: &str, connection: SshConnection, mode: RemoteConnectMode) -> RemoteLaunch {
    RemoteLaunch {
        label: label.into(),
        connection,
        mode,
        vnc_port: None,
    }
}

#[test]
fn vnc_opens_a_tunnelled_device_panel_in_the_default_workspace() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let connection = lab_connection();

    let panel_id = app
        .open_remote_host(
            &ctx,
            launch("lab", connection.clone(), RemoteConnectMode::Vnc),
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
            launch("lab", lab_connection(), RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");

    let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
    assert_eq!(app.board.workspace(workspace_id).unwrap().name, "Ops");
    let device = app.board.panel(panel_id).unwrap().device().expect("device state");
    assert_eq!(device.target.address().to_string(), "127.0.0.1:5901");
}

#[test]
fn per_host_ports_and_a_typed_port_override_the_global_vnc_port() {
    let mut config = Config::default();
    config.remote_hosts.vnc_port = 5901;
    config.remote_hosts.vnc_ports.insert("lab".into(), 5902);
    config.remote_hosts.vnc_ports.insert("db.example".into(), 5903);
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(&config, ephemeral());
    let target = |app: &crate::app::HorizonApp, panel_id| {
        let panel = app.board.panel(panel_id).unwrap();
        panel.device().expect("device state").target.address().to_string()
    };

    let by_label = app
        .open_remote_host(
            &ctx,
            launch("lab", lab_connection(), RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    assert_eq!(target(&app, by_label), "127.0.0.1:5902", "the label's entry wins");

    let db = SshConnection {
        host: "db.example".into(),
        ..SshConnection::default()
    };
    let by_host = app
        .open_remote_host(
            &ctx,
            launch("db", db.clone(), RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    assert_eq!(
        target(&app, by_host),
        "127.0.0.1:5903",
        "the SSH host name is the fallback key"
    );

    let mut typed = launch("lab", lab_connection(), RemoteConnectMode::Vnc);
    typed.vnc_port = Some(5999);
    let overridden = app
        .open_remote_host(&ctx, typed, &WorkspaceChoice::Default)
        .expect("panel");
    assert_eq!(
        target(&app, overridden),
        "127.0.0.1:5999",
        "a typed :port beats the map"
    );

    let unlisted = app
        .open_remote_host(
            &ctx,
            launch("web", db, RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    assert_eq!(target(&app, unlisted), "127.0.0.1:5903");
    let other = SshConnection {
        host: "web.example".into(),
        ..SshConnection::default()
    };
    let global = app
        .open_remote_host(
            &ctx,
            launch("web", other, RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    assert_eq!(
        target(&app, global),
        "127.0.0.1:5901",
        "no entry falls back to vnc_port"
    );
}

#[test]
fn ssh_opens_in_the_chosen_existing_workspace_and_reuses_the_default_one() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let ops = app.board.create_workspace("Ops");

    let first = app
        .open_remote_host(
            &ctx,
            launch("lab", lab_connection(), RemoteConnectMode::Ssh),
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
            launch("lab", lab_connection(), RemoteConnectMode::Ssh),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    let third = app
        .open_remote_host(
            &ctx,
            launch("lab", lab_connection(), RemoteConnectMode::Vnc),
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
    app.wait_for_settings_io();

    let saved = Config::load(Some(&app.config_path)).expect("settings reload");
    assert_eq!(
        saved.remote_hosts.default_workspace_name(),
        "  Ops  ",
        "the store holds the default workspace"
    );
    assert!(!app.config_path.exists(), "no YAML copy is written next to the store");
    assert_eq!(
        app.template_config.remote_hosts.default_workspace_name(),
        "  Ops  ",
        "kept exact"
    );
    assert!(app.set_remote_hosts_default_workspace("Ops"));
    app.wait_for_settings_io();
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
            launch("lab", lab_connection(), RemoteConnectMode::Vnc),
            &WorkspaceChoice::Default,
        )
        .expect("panel");
    let workspace_id = app.board.panel_workspace_id(panel_id).expect("workspace");
    assert_eq!(app.board.workspace(workspace_id).unwrap().name, "Ops");
}

#[test]
fn set_default_waits_for_unsaved_settings_edits_and_refreshes_a_clean_editor() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    Config::save(&app.config_path, &app.template_config).expect("write the store");
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
    app.wait_for_settings_io();
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
        Config::load(Some(&app.config_path))
            .expect("reload")
            .remote_hosts
            .default_workspace_name(),
        editor
            .editing_config()
            .map(|config| config.remote_hosts.default_workspace_name())
            .unwrap_or_default(),
        "the editor and the store hold the same settings"
    );
}

#[test]
fn setting_the_default_workspace_leaves_an_exported_copy_alone() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    Config::save(&app.config_path, &app.template_config).expect("write the store");
    Config::export(&app.config_path, &app.config_path).expect("export a copy");
    let exported = std::fs::read_to_string(&app.config_path).expect("read the export");

    assert!(app.set_remote_hosts_default_workspace("Ops"));
    app.wait_for_settings_io();

    assert_eq!(
        std::fs::read_to_string(&app.config_path).expect("read the export again"),
        exported,
        "Horizon writes only the store, so the export keeps its bytes"
    );
    assert_eq!(
        Config::load(Some(&app.config_path))
            .expect("reload")
            .remote_hosts
            .default_workspace_name(),
        "Ops"
    );
}

#[test]
fn set_default_uses_the_settings_of_the_run_with_no_file_to_patch() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());

    // A store of another version refuses the change instead of overwriting it.
    Config::save(&app.config_path, &app.template_config).expect("write the store");
    let store = horizon_core::config_store::ConfigStore::resolve(&app.config_path);
    {
        let connection = rusqlite::Connection::open(store.path()).expect("store");
        connection
            .execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '99') \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )
            .expect("write a newer schema version");
    }
    let before = std::fs::read(store.path()).expect("read the store");
    assert!(app.set_remote_hosts_default_workspace("Ops"));
    app.wait_for_settings_io();
    assert_eq!(
        std::fs::read(store.path()).expect("reread the store"),
        before,
        "a store of another version is never rewritten"
    );

    // A fresh run with no settings writes the default workspace to the store.
    let (_second_temp, _second_ctx, mut fresh) = test_app_with_startup(ephemeral());
    assert!(fresh.set_remote_hosts_default_workspace("Fresh"));
    fresh.wait_for_settings_io();
    assert_eq!(
        Config::load(Some(&fresh.config_path))
            .expect("reload")
            .remote_hosts
            .default_workspace_name(),
        "Fresh"
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
            launch("lab", lab_connection(), RemoteConnectMode::Ssh),
            &WorkspaceChoice::Existing(legacy),
        )
        .expect("panel");
    let by_default = app
        .open_remote_host(
            &ctx,
            launch("lab", lab_connection(), RemoteConnectMode::Vnc),
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
        app.save_remote_host_shortcut(launch("lab", lab_connection(), RemoteConnectMode::Vnc)),
        Some("VNC: lab".to_string())
    );
    assert_eq!(
        app.save_remote_host_shortcut(launch(" lab ", lab_connection(), RemoteConnectMode::Ssh)),
        Some("SSH: lab".to_string())
    );
    let blank = SshConnection {
        host: " ".into(),
        ..SshConnection::default()
    };
    assert_eq!(
        app.save_remote_host_shortcut(launch("lab", blank, RemoteConnectMode::Ssh)),
        None
    );

    app.wait_for_settings_io();
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

    let mut typed = launch("lab", lab_connection(), RemoteConnectMode::Vnc);
    typed.vnc_port = Some(5999);
    assert_eq!(app.save_remote_host_shortcut(typed), Some("VNC: lab".to_string()));
    app.wait_for_settings_io();
    let saved = Config::load(Some(&app.config_path)).expect("config reloads");
    let vnc = saved
        .presets
        .iter()
        .find(|preset| preset.name == "VNC: lab")
        .expect("vnc preset");
    assert_eq!(
        vnc.command.as_deref(),
        Some("127.0.0.1:5999"),
        "a typed :port is frozen into the re-saved shortcut"
    );
    assert_eq!(saved.presets.len(), presets_before + 2, "re-saving replaces the preset");
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
        app.save_remote_host_shortcut(launch("lab", as_root.clone(), RemoteConnectMode::Vnc)),
        Some("VNC: lab".to_string())
    );
    app.wait_for_settings_io();
    let saved = Config::load(Some(&app.config_path)).expect("config reloads");
    assert_eq!(saved.presets.len(), presets_before + 2);
    let vnc = saved
        .presets
        .iter()
        .find(|preset| preset.name == "VNC: lab")
        .expect("vnc preset");
    assert_eq!(vnc.ssh_connection.as_ref(), Some(&as_root));
}

#[test]
fn saving_a_shortcut_keeps_the_settings_of_the_run() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    app.template_config.remote_hosts.vnc_port = 5999;

    assert_eq!(
        app.save_remote_host_shortcut(launch("lab", lab_connection(), RemoteConnectMode::Vnc)),
        Some("VNC: lab".to_string())
    );
    app.wait_for_settings_io();
    let saved = Config::load(Some(&app.config_path)).expect("settings reload");
    assert_eq!(saved.remote_hosts.vnc_port, 5999, "the earlier setting survives");
    let vnc = saved
        .presets
        .iter()
        .find(|preset| preset.name == "VNC: lab")
        .expect("vnc preset");
    assert_eq!(vnc.command.as_deref(), Some("127.0.0.1:5999"));
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
            launch("lab", lab_connection(), RemoteConnectMode::Ssh),
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
