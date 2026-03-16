#![forbid(unsafe_code)]

mod app;
mod branding;
mod dir_picker;
mod editor_widget;
mod git_changes_widget;
mod input;
mod quick_nav;
mod terminal_widget;
mod theme;
mod usage_widget;

use std::path::PathBuf;

use app::HorizonApp;
use horizon_core::{Config, RuntimeState, runtime_state_path_for_config};
use tracing_subscriber::fmt::format::FmtSpan;

fn main() -> eframe::Result {
    init_tracing();

    install_agent_plugins();

    // Parse --config <path> from CLI args
    let config_path = parse_config_arg();
    let config = Config::load(config_path.as_deref()).unwrap_or_else(|e| {
        tracing::error!("failed to load config: {e}");
        Config::default()
    });
    let resolved_config_path =
        config_path.unwrap_or_else(|| Config::default_path().unwrap_or_else(|| PathBuf::from("horizon.yaml")));
    let runtime_state_path =
        runtime_state_path_for_config(&resolved_config_path).unwrap_or_else(|| PathBuf::from(".horizon-runtime.yaml"));
    let runtime_state = RuntimeState::load(&runtime_state_path)
        .unwrap_or_else(|error| {
            tracing::warn!("failed to load runtime state {}: {error}", runtime_state_path.display());
            None
        })
        .unwrap_or_else(|| RuntimeState::from_config(&config));

    let window = runtime_state.window_or(&config.window);
    // Clamp to reasonable bounds so we don't open larger than the screen.
    let width = window.width.clamp(800.0, 7680.0);
    let height = window.height.clamp(600.0, 4320.0);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(branding::APP_NAME)
        .with_icon(branding::app_icon())
        .with_decorations(true)
        .with_transparent(false)
        .with_inner_size([width, height])
        .with_min_inner_size([800.0, 600.0])
        .with_resizable(true);

    if let (Some(x), Some(y)) = (window.x, window.y) {
        viewport = viewport.with_position([x, y]);
    }

    if cfg!(target_os = "linux") {
        viewport = viewport.with_app_id(branding::APP_ID);
    }

    let has_saved_position = window.x.is_some();
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: !has_saved_position,
        run_and_return: false,
        ..Default::default()
    };

    eframe::run_native(
        branding::APP_NAME,
        options,
        Box::new(move |cc| {
            Ok(Box::new(HorizonApp::new(
                cc,
                &config,
                resolved_config_path.clone(),
                runtime_state_path.clone(),
                runtime_state.clone(),
            )))
        }),
    )
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("horizon=info,horizon_core=info"));

    let subscriber = tracing_subscriber::fmt().with_env_filter(env_filter);

    if std::env::var_os("HORIZON_TRACE_SPANS").is_some() {
        subscriber
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .compact()
            .init();
    } else {
        subscriber.init();
    }
}

fn parse_config_arg() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if (arg == "--config" || arg == "-c") && i + 1 < args.len() {
            return Some(PathBuf::from(&args[i + 1]));
        }
    }
    None
}

fn install_agent_plugins() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = std::path::PathBuf::from(home);

    // Claude Code plugin: extract to ~/.config/horizon/plugins/claude-code/
    // so we can point --plugin-dir at it.
    let claude_plugin_dir = home.join(".config").join("horizon").join("plugins").join("claude-code");
    install_plugin_files(
        &claude_plugin_dir,
        &[
            (
                ".claude-plugin/plugin.json",
                include_str!("../../../assets/plugins/claude-code/.claude-plugin/plugin.json"),
            ),
            (
                "skills/horizon-notify/SKILL.md",
                include_str!("../../../assets/plugins/claude-code/skills/horizon-notify/SKILL.md"),
            ),
        ],
    );

    // Codex skill: install to ~/.agents/skills/ (Codex auto-discovers this).
    let codex_skill_dir = home.join(".agents").join("skills").join("horizon-notify");
    install_plugin_files(
        &codex_skill_dir,
        &[(
            "SKILL.md",
            include_str!("../../../assets/plugins/codex/skills/horizon-notify/SKILL.md"),
        )],
    );
}

fn install_plugin_files(base: &std::path::Path, files: &[(&str, &str)]) {
    for (relative_path, content) in files {
        let path = base.join(relative_path);
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        let _ = std::fs::write(&path, content);
    }
}
