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
mod native_app;
mod plugin_install;
mod primary_selection;
mod remote_hosts_overlay;
mod search_overlay;
mod terminal_widget;
mod theme;
mod usage_widget;

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use app::HorizonApp;
use std::sync::Arc;

use eframe::{egui_wgpu, wgpu};
use horizon_core::{
    AGENT_LAUNCH_HELPER_ARG, Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser,
    StartupDecision, WindowConfig,
};
use tracing_subscriber::fmt::format::FmtSpan;

fn main() -> eframe::Result {
    if let Some(exit_code) = run_agent_launch_helper() {
        std::process::exit(exit_code);
    }

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
        vsync: false,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: Some(1),
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                native_adapter_selector: Some(Arc::new(select_adapter)),
                ..Default::default()
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

fn run_agent_launch_helper() -> Option<i32> {
    let request = parse_agent_launch_helper(std::env::args_os().skip(1))?;
    let (program, args) = match request {
        Ok(request) => request,
        Err(error) => {
            eprintln!("Horizon agent launch helper: {error}");
            return Some(2);
        }
    };

    let status = agent_launch_command(program, args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    Some(match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("Horizon agent launch helper failed: {error}");
            1
        }
    })
}

fn agent_launch_command(program: OsString, args: Vec<OsString>) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    command
}

fn parse_agent_launch_helper(
    args: impl IntoIterator<Item = OsString>,
) -> Option<Result<(OsString, Vec<OsString>), &'static str>> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(OsStr::new(AGENT_LAUNCH_HELPER_ARG)) {
        return None;
    }
    let Some(program) = args.next() else {
        return Some(Err("missing target executable"));
    };
    if program.is_empty() {
        return Some(Err("target executable is empty"));
    }

    Some(Ok((program, args.map(normalize_agent_launch_helper_arg).collect())))
}

fn normalize_agent_launch_helper_arg(argument: OsString) -> OsString {
    #[cfg(windows)]
    {
        normalize_windows_agent_launch_helper_arg(&argument)
    }
    #[cfg(not(windows))]
    {
        argument
    }
}

#[cfg(any(windows, test))]
fn normalize_windows_agent_launch_helper_arg(argument: &OsStr) -> OsString {
    argument.to_string_lossy().replace(['\r', '\n'], " ").into()
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
    use std::ffi::{OsStr, OsString};

    #[cfg(windows)]
    use std::fs;

    use horizon_core::{SessionSummary, StartupChooser, StartupPromptReason};

    #[cfg(windows)]
    use super::agent_launch_command;
    use super::{
        AGENT_LAUNCH_HELPER_ARG, normalize_windows_agent_launch_helper_arg, parse_agent_launch_helper,
        startup_chooser_window_config, summarize_adapter, wgpu,
    };

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

    #[test]
    fn internal_agent_launch_helper_requires_marker_and_target() {
        assert!(parse_agent_launch_helper([OsString::from("--blank")]).is_none());
        let missing_target = parse_agent_launch_helper([OsString::from(AGENT_LAUNCH_HELPER_ARG)])
            .expect("helper marker")
            .expect_err("target is required");
        assert_eq!(missing_target, "missing target executable");

        let (program, args) = parse_agent_launch_helper([
            OsString::from(AGENT_LAUNCH_HELPER_ARG),
            OsString::from(r"C:\Tools\agent.cmd"),
            OsString::from("--safe-flag"),
            OsString::from("setup prompt"),
        ])
        .expect("helper marker")
        .expect("valid helper request");
        assert_eq!(program, OsStr::new(r"C:\Tools\agent.cmd"));
        assert_eq!(args, ["--safe-flag", "setup prompt"]);
    }

    #[test]
    fn windows_helper_flattens_batch_unsafe_line_breaks_only() {
        let prompt = OsStr::new("first line\r\nsecond & third %PATH%");

        assert_eq!(
            normalize_windows_agent_launch_helper_arg(prompt),
            OsStr::new("first line  second & third %PATH%")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_batch_helper_round_trips_arguments_without_command_injection() {
        let directory = tempfile::tempdir().expect("temporary batch helper directory");
        let sentinel = directory.path().join("command-injection-sentinel");
        let raw_arguments = [
            "value with spaces".to_string(),
            r#"quotes "inside" and backslashes C:\Program Files\Agent\"#.to_string(),
            r#"operators &|<>^%! stay literal"#.to_string(),
            r#"environment token %PATH% stays literal"#.to_string(),
            "Unicode: Blåbær Ω Привет 漢字".to_string(),
            "first line\r\nsecond line\nthird line\rfourth line".to_string(),
            r#"& echo injected>"%HORIZON_AGENT_HELPER_SENTINEL%" & rem"#.to_string(),
        ];
        let expected_arguments = [
            raw_arguments[0].clone(),
            raw_arguments[1].clone(),
            raw_arguments[2].clone(),
            raw_arguments[3].clone(),
            raw_arguments[4].clone(),
            "first line  second line third line fourth line".to_string(),
            raw_arguments[6].clone(),
        ];

        for extension in ["cmd", "bat"] {
            let script = directory.path().join(format!("agent shim.{extension}"));
            let capture = directory.path().join(format!("capture-{extension}.txt"));
            fs::write(
                &script,
                "@echo off\r\n\
                 setlocal DisableDelayedExpansion\r\n\
                 chcp 65001 >nul\r\n\
                 set \"HORIZON_CAPTURE_ARG_1=%~1\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_2=%~2\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_3=%~3\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_4=%~4\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_5=%~5\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_6=%~6\"\r\n\
                 set \"HORIZON_CAPTURE_ARG_7=%~7\"\r\n\
                 set HORIZON_CAPTURE_ARG_ > \"%HORIZON_AGENT_HELPER_CAPTURE%\"\r\n\
                 exit /b 0\r\n",
            )
            .expect("write batch helper");

            let mut helper_arguments = vec![OsString::from(AGENT_LAUNCH_HELPER_ARG), script.clone().into_os_string()];
            helper_arguments.extend(raw_arguments.iter().map(OsString::from));
            let (program, arguments) = parse_agent_launch_helper(helper_arguments)
                .expect("helper marker")
                .expect("valid helper request");
            let status = agent_launch_command(program, arguments)
                .env("HORIZON_AGENT_HELPER_CAPTURE", &capture)
                .env("HORIZON_AGENT_HELPER_SENTINEL", &sentinel)
                .status()
                .expect("execute batch helper");

            assert!(status.success(), "{extension} helper failed with {status}");
            assert!(
                !sentinel.exists(),
                "{extension} helper executed argument text as a command"
            );
            let captured = fs::read_to_string(&capture).expect("read captured arguments");
            let expected = expected_arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| format!("HORIZON_CAPTURE_ARG_{}={argument}", index + 1))
                .collect::<Vec<_>>();
            assert_eq!(captured.lines().collect::<Vec<_>>(), expected);
        }
    }
}
