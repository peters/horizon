#![forbid(unsafe_code)]

mod app;
mod branding;
mod command_palette;
mod command_registry;
mod dir_picker;
mod editor_widget;
mod git_changes_widget;
mod input;
mod loading_spinner;
mod plugin_install;
mod remote_hosts_overlay;
mod terminal_widget;
mod theme;
mod usage_widget;

use std::fmt::Write as _;
use std::path::PathBuf;

use app::HorizonApp;
use eframe::wgpu;
use horizon_core::{
    Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser, StartupDecision,
    WindowConfig,
};
use tracing_subscriber::fmt::format::FmtSpan;

fn main() -> eframe::Result {
    init_tracing();

    let horizon_home = HorizonHome::resolve();
    plugin_install::install_agent_plugins(&horizon_home);

    let cli_args = parse_cli_args();
    let resolved_config_path =
        Config::resolve_path(cli_args.config_path.as_deref()).unwrap_or_else(|| horizon_home.config_path());
    let config = load_config_or_default(&resolved_config_path);
    let session_store = SessionStore::new(horizon_home.clone(), resolved_config_path.clone());
    let startup = prepare_startup(&session_store, &config, &cli_args);

    let window = startup_window_config(&startup, &config.window);
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
            log_graphics_adapter(cc);
            Ok(Box::new(HorizonApp::new(
                cc,
                &config,
                resolved_config_path.clone(),
                session_store.clone(),
                startup.clone(),
            )))
        }),
    )
}

fn log_graphics_adapter(cc: &eframe::CreationContext<'_>) {
    let Some(render_state) = cc.wgpu_render_state.as_ref() else {
        tracing::warn!("wgpu render state unavailable during startup");
        return;
    };

    let adapter_info = render_state.adapter.get_info();
    let vendor_id = format!("{:#06x}", adapter_info.vendor);
    let device_id = format!("{:#06x}", adapter_info.device);

    tracing::info!(
        backend = ?adapter_info.backend,
        device_type = ?adapter_info.device_type,
        adapter_name = %adapter_info.name,
        vendor_id = %vendor_id,
        device_id = %device_id,
        driver = %adapter_info.driver,
        driver_info = %adapter_info.driver_info,
        target_format = ?render_state.target_format,
        "graphics adapter selected"
    );

    #[cfg(not(target_arch = "wasm32"))]
    if render_state.available_adapters.len() > 1 {
        let available = render_state
            .available_adapters
            .iter()
            .map(|adapter| summarize_adapter(&adapter.get_info()))
            .collect::<Vec<_>>()
            .join("; ");
        tracing::info!(adapters = %available, "graphics adapters available");
    }
}

fn summarize_adapter(info: &wgpu::AdapterInfo) -> String {
    let mut summary = format!("{} [{:?}/{:?}]", info.name, info.backend, info.device_type);
    if !info.driver.is_empty() {
        let _ = write!(summary, " driver={}", info.driver);
    }
    if !info.driver_info.is_empty() && info.driver_info != info.driver {
        let _ = write!(summary, " ({})", info.driver_info);
    }
    summary
}
fn startup_window_config(startup: &StartupDecision, fallback: &WindowConfig) -> WindowConfig {
    match startup {
        StartupDecision::Open { session, .. } => session.runtime_state.window_or(fallback).clone(),
        StartupDecision::Ephemeral { runtime_state } => runtime_state.window_or(fallback).clone(),
        StartupDecision::Choose(chooser) => startup_chooser_window_config(chooser),
    }
}

fn startup_chooser_window_config(chooser: &StartupChooser) -> WindowConfig {
    const STARTUP_CHOOSER_WIDTH: f32 = 880.0;
    const STARTUP_CHOOSER_MIN_HEIGHT: f32 = 420.0;
    const STARTUP_CHOOSER_MAX_HEIGHT: f32 = 680.0;
    const STARTUP_CHOOSER_BASE_HEIGHT: f32 = 290.0;
    const STARTUP_CHOOSER_CARD_HEIGHT: f32 = 82.0;

    let visible_sessions = match chooser.sessions.len() {
        0 | 1 => 1.0,
        2 => 2.0,
        3 => 3.0,
        _ => 4.0,
    };
    let height = (STARTUP_CHOOSER_BASE_HEIGHT + visible_sessions * STARTUP_CHOOSER_CARD_HEIGHT)
        .clamp(STARTUP_CHOOSER_MIN_HEIGHT, STARTUP_CHOOSER_MAX_HEIGHT);

    WindowConfig {
        width: STARTUP_CHOOSER_WIDTH,
        height,
        x: None,
        y: None,
    }
}
fn load_config_or_default(config_path: &std::path::Path) -> Config {
    if !config_path.exists() {
        tracing::info!("no config found at {}, using defaults", config_path.display());
        return Config::default();
    }

    Config::load(Some(config_path)).unwrap_or_else(|error| {
        tracing::error!("failed to load config: {error}");
        Config::default()
    })
}

fn prepare_startup(session_store: &SessionStore, config: &Config, cli_args: &CliArgs) -> StartupDecision {
    if cli_args.ephemeral || cli_args.new_session || cli_args.blank {
        let runtime_state = if cli_args.blank {
            RuntimeState::default()
        } else {
            RuntimeState::from_config(config)
        };

        if cli_args.ephemeral {
            return StartupDecision::Ephemeral {
                runtime_state: Box::new(runtime_state),
            };
        }

        return match session_store.create_session_from_runtime(runtime_state) {
            Ok(session) => StartupDecision::Open {
                disposition: SessionOpenDisposition::New,
                session: Box::new(session),
            },
            Err(error) => {
                eprintln!("fatal: failed to create Horizon session: {error}");
                std::process::exit(1);
            }
        };
    }

    match session_store.prepare_startup(config) {
        Ok(startup) => startup,
        Err(error) => {
            tracing::error!("failed to prepare startup session: {error}");
            match session_store.create_new_session(config) {
                Ok(session) => StartupDecision::Open {
                    disposition: SessionOpenDisposition::New,
                    session: Box::new(session),
                },
                Err(create_error) => {
                    eprintln!("fatal: failed to create Horizon session: {create_error}");
                    std::process::exit(1);
                }
            }
        }
    }
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

struct CliArgs {
    config_path: Option<PathBuf>,
    new_session: bool,
    ephemeral: bool,
    blank: bool,
}

fn parse_cli_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = None;
    let mut new_session = false;
    let mut ephemeral = false;
    let mut blank = false;

    for (i, arg) in args.iter().enumerate() {
        if (arg == "--config" || arg == "-c") && i + 1 < args.len() {
            config_path = Some(PathBuf::from(&args[i + 1]));
        } else if arg == "--new-session" {
            new_session = true;
        } else if arg == "--ephemeral" {
            ephemeral = true;
        } else if arg == "--blank" {
            blank = true;
        }
    }

    CliArgs {
        config_path,
        new_session,
        ephemeral,
        blank,
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{SessionSummary, StartupChooser, StartupPromptReason};

    use super::{startup_chooser_window_config, summarize_adapter, wgpu};

    fn chooser_with_sessions(session_count: usize) -> StartupChooser {
        StartupChooser {
            reason: StartupPromptReason::LiveConflict,
            config_path: "/tmp/horizon.yaml".to_string(),
            sessions: (0..session_count)
                .map(|index| SessionSummary {
                    session_id: format!("session-{index}"),
                    label: format!("Session {index}"),
                    workspace_count: 1,
                    panel_count: 1,
                    last_active_at: 0,
                    config_path: "/tmp/horizon.yaml".to_string(),
                    is_live: index == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn startup_chooser_window_is_compact_and_centered() {
        let window = startup_chooser_window_config(&chooser_with_sessions(1));

        assert!((window.width - 880.0).abs() < f32::EPSILON);
        assert!((window.height - 420.0).abs() < f32::EPSILON);
        assert_eq!(window.x, None);
        assert_eq!(window.y, None);
    }

    #[test]
    fn startup_chooser_window_caps_visible_session_growth() {
        let window = startup_chooser_window_config(&chooser_with_sessions(8));

        assert!((window.height - 618.0).abs() < f32::EPSILON);
    }

    #[test]
    fn summarize_adapter_includes_backend_and_device_type() {
        let summary = summarize_adapter(&wgpu::AdapterInfo {
            name: "Apple M3 Max".to_string(),
            vendor: 0x106b,
            device: 0x0001,
            device_type: wgpu::DeviceType::IntegratedGpu,
            driver: "metal".to_string(),
            driver_info: "Apple GPU".to_string(),
            backend: wgpu::Backend::Metal,
        });

        assert!(summary.contains("Apple M3 Max"));
        assert!(summary.contains("Metal"));
        assert!(summary.contains("IntegratedGpu"));
        assert!(summary.contains("driver=metal"));
        assert!(summary.contains("(Apple GPU)"));
    }

    #[test]
    fn summarize_adapter_omits_duplicate_driver_info() {
        let summary = summarize_adapter(&wgpu::AdapterInfo {
            name: "Adapter".to_string(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::Other,
            driver: "same".to_string(),
            driver_info: "same".to_string(),
            backend: wgpu::Backend::Metal,
        });

        assert_eq!(summary.matches("same").count(), 1);
    }
}
