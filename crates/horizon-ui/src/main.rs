#![forbid(unsafe_code)]

mod agent_work_hook;
mod app;
mod badge;
mod branding;
mod browser_widget;
mod command_palette;
mod command_registry;
mod dir_picker;
mod editor_widget;
mod git_changes_widget;
mod input;
mod loading_spinner;
mod native_app;
mod plugin_install;
mod primary_selection;
mod remote_hosts_overlay;
mod search_overlay;
mod terminal_widget;
#[cfg(test)]
mod test_egui;
mod text;
mod theme;
mod usage_widget;

use std::fmt::Write as _;
use std::path::PathBuf;

use app::HorizonApp;
use std::sync::Arc;

use eframe::{egui_wgpu, wgpu};
use horizon_core::{
    Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser, StartupDecision,
    WindowConfig,
};
use tracing_subscriber::fmt::format::FmtSpan;

fn main() -> eframe::Result {
    if agent_work_hook::run_if_requested() {
        return Ok(());
    }
    init_tracing();

    if browser_mcp_mode_requested() {
        run_browser_mcp_server();
        return Ok(());
    }

    let horizon_home = HorizonHome::resolve();
    let _agent_plugin_host = plugin_install::install_agent_plugins(&horizon_home);

    let cli_args = match parse_cli_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(problem) => {
            eprintln!("error: {problem}");
            eprintln!("{CLI_USAGE}");
            plugin_install::exit_after_releasing_plugins(2);
        }
    };
    let resolved_config_path =
        Config::resolve_path(cli_args.config_path.as_deref()).unwrap_or_else(|| horizon_home.config_path());
    if let Some(profile_path) = cli_args.remote_profile.as_ref() {
        let code = run_remote_profile_command(&resolved_config_path, profile_path);
        plugin_install::exit_after_releasing_plugins(code);
    }
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
        wgpu_options: egui_wgpu::WgpuConfiguration {
            surface: egui_wgpu::SurfaceConfig {
                present_mode: wgpu::PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: Some(1),
            },
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                native_adapter_selector: Some(Arc::new(select_adapter)),
                ..egui_wgpu::WgpuSetupCreateNew::without_display_handle()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let observed_keyboard_inputs = input::ObservedKeyboardInputs::default();
    let app_keyboard_inputs = observed_keyboard_inputs.clone();
    native_app::run_native_with_keyboard_observer(
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
                app_keyboard_inputs.clone(),
            )))
        }),
        observed_keyboard_inputs,
    )
}

fn browser_mcp_mode_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    args.next().is_some_and(|argument| argument == "--browser-mcp") && args.next().is_none()
}

fn run_browser_mcp_server() {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build();
    let result = match runtime {
        Ok(runtime) => runtime
            .block_on(horizon_browser_mcp::serve_stdio())
            .map_err(|error| error.to_string()),
        Err(error) => Err(format!("could not start browser MCP runtime: {error}")),
    };
    if let Err(error) = result {
        tracing::error!(%error, "Horizon browser MCP server stopped with an error");
        std::process::exit(1);
    }
}

/// Select a wgpu adapter that can actually present to the given surface.
///
/// On VMs and headless machines the default DX12 adapter may enumerate
/// successfully but fail at `Surface::configure` because the Hyper-V
/// Video driver does not expose a usable swapchain.  We score each
/// adapter by device type (discrete > integrated > software > other)
/// and only consider adapters whose surface capabilities include at
/// least one texture format — which rules out the broken DX12 path
/// on virtual display adapters.
fn select_adapter(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter, String> {
    let mut candidates: Vec<(&wgpu::Adapter, u8)> = adapters
        .iter()
        .filter(|adapter| {
            let Some(surface) = surface else {
                return true;
            };
            let caps = surface.get_capabilities(adapter);
            let usable = !caps.formats.is_empty();
            if !usable {
                let info = adapter.get_info();
                tracing::warn!(
                    adapter = %info.name,
                    backend = ?info.backend,
                    "skipping adapter: surface reports no usable formats"
                );
            }
            usable
        })
        .map(|adapter| {
            let score = match adapter.get_info().device_type {
                wgpu::DeviceType::DiscreteGpu => 4,
                wgpu::DeviceType::IntegratedGpu => 3,
                wgpu::DeviceType::VirtualGpu => 2,
                wgpu::DeviceType::Cpu => 1,
                wgpu::DeviceType::Other => 0,
            };
            (adapter, score)
        })
        .collect();

    candidates.sort_by_key(|(_, score)| std::cmp::Reverse(*score));

    if let Some((best, _)) = candidates.into_iter().next() {
        let info = best.get_info();
        tracing::info!(
            adapter = %info.name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            "selected graphics adapter"
        );
        Ok(best.clone())
    } else {
        Err("no graphics adapter with a usable surface was found".to_string())
    }
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

/// `--export-remote-profile PATH` writes the shareable half of
/// `browser.remote` (no credential bindings or values) and
/// `--import-remote-profile PATH` merges such a file into the configuration
/// file, so a second computer without a pointer can take a profile from the
/// command line. Both finish without opening a window.
fn run_remote_profile_command(config_path: &std::path::Path, command: &RemoteProfileCommand) -> i32 {
    use horizon_core::browser::remote_profile;

    match command {
        RemoteProfileCommand::Export(path) => match remote_profile::export_portable_file_from_config(config_path, path)
        {
            Ok(remote) => {
                println!(
                    "exported {} provider(s) and {} target(s) to {} without credentials",
                    remote.providers.len(),
                    remote.targets.len(),
                    path.display()
                );
                0
            }
            Err(error) => {
                eprintln!("error: could not export the remote browser profile: {error}");
                2
            }
        },
        RemoteProfileCommand::Import(path) => match remote_profile::import_portable_file_into_config(config_path, path)
        {
            Ok(summary) => {
                println!(
                    "imported {} into {}: {}; enter this computer's credentials in Settings > Remote browsers",
                    path.display(),
                    config_path.display(),
                    remote_profile::summary_line(&summary)
                );
                0
            }
            Err(error) => {
                eprintln!("error: could not import the remote browser profile: {error}");
                2
            }
        },
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
                plugin_install::exit_after_releasing_plugins(1);
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
                    plugin_install::exit_after_releasing_plugins(1);
                }
            }
        }
    }
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("horizon=info,horizon_core=info"));

    // Stdout is an MCP protocol channel in `--browser-mcp` mode. Keep every
    // tracing event on stderr so a coordination warning can never corrupt a
    // JSON-RPC response.
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr);

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

const CLI_USAGE: &str = "usage: horizon [--config <path>] [--ephemeral] [--new-session] [--blank]
       horizon [--config <path>] --export-remote-profile <path>
       horizon [--config <path>] --import-remote-profile <path>";

#[derive(Debug, PartialEq, Eq)]
enum RemoteProfileCommand {
    Export(PathBuf),
    Import(PathBuf),
}

struct CliArgs {
    config_path: Option<PathBuf>,
    new_session: bool,
    ephemeral: bool,
    blank: bool,
    remote_profile: Option<RemoteProfileCommand>,
}

/// Parse the launch flags. The remote-profile commands take exactly one
/// operand that is not itself a flag, and at most one of them may be given,
/// so a typo can never fall through to an ordinary launch or run the other
/// command.
fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, String> {
    let mut config_path = None;
    let mut new_session = false;
    let mut ephemeral = false;
    let mut blank = false;
    let mut remote_profile: Option<RemoteProfileCommand> = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut operand = |flag: &str| -> Result<PathBuf, String> {
            match args.next() {
                Some(value) if !value.starts_with('-') && !value.is_empty() => Ok(PathBuf::from(value)),
                _ => Err(format!("{flag} needs a path")),
            }
        };
        match arg.as_str() {
            "--config" | "-c" => config_path = Some(operand(&arg)?),
            "--export-remote-profile" | "--import-remote-profile" => {
                let path = operand(&arg)?;
                let command = if arg == "--export-remote-profile" {
                    RemoteProfileCommand::Export(path)
                } else {
                    RemoteProfileCommand::Import(path)
                };
                if remote_profile.replace(command).is_some() {
                    return Err("give one of --export-remote-profile and --import-remote-profile, once".to_string());
                }
            }
            "--new-session" => new_session = true,
            "--ephemeral" => ephemeral = true,
            "--blank" => blank = true,
            _ => {}
        }
    }

    Ok(CliArgs {
        config_path,
        new_session,
        ephemeral,
        blank,
        remote_profile,
    })
}

#[cfg(test)]
mod cli_args_tests {
    use super::{RemoteProfileCommand, parse_cli_args};

    fn parse(args: &[&str]) -> Result<super::CliArgs, String> {
        parse_cli_args(args.iter().map(|arg| (*arg).to_string()))
    }

    #[test]
    fn remote_profile_commands_take_one_real_operand_and_exclude_each_other() {
        let args = parse(&["--config", "c.yaml", "--export-remote-profile", "p.yaml"]).expect("export");
        assert_eq!(args.remote_profile, Some(RemoteProfileCommand::Export("p.yaml".into())));
        assert_eq!(args.config_path.as_deref(), Some(std::path::Path::new("c.yaml")));
        let args = parse(&["--import-remote-profile", "p.yaml", "--ephemeral"]).expect("import");
        assert_eq!(args.remote_profile, Some(RemoteProfileCommand::Import("p.yaml".into())));
        assert!(args.ephemeral);

        for bad in [
            &["--import-remote-profile"][..],
            &["--import-remote-profile", "--ephemeral"],
            &["--export-remote-profile", ""],
            &["--config"],
            &["--export-remote-profile", "a.yaml", "--import-remote-profile", "b.yaml"],
            &["--export-remote-profile", "a.yaml", "--export-remote-profile", "b.yaml"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            parse(&["--blank", "--new-session"])
                .expect("plain launch")
                .remote_profile
                .is_none()
        );
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
            driver: "metal".to_string(),
            driver_info: "Apple GPU".to_string(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Metal)
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
            driver: "same".to_string(),
            driver_info: "same".to_string(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::Other, wgpu::Backend::Metal)
        });

        assert_eq!(summary.matches("same").count(), 1);
    }
}
