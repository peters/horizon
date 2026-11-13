#![deny(unsafe_code)]

mod agent_integrations;
mod agent_work_hook;
mod app;
mod badge;
mod block_widget;
mod branding;
mod browser_widget;
mod cli;
mod clipboard;
mod clipboard_image;
mod command_palette;
mod command_registry;
mod device_widget;
mod dir_picker;
mod editor_widget;
mod icon_button;
mod input;
#[cfg(any(test, target_os = "linux"))]
mod linux_desktop;
mod loading_spinner;
mod native_window;
mod plugin_install;
mod primary_selection;
mod remote_hosts_overlay;
mod screenshot;
mod search_overlay;
mod terminal_widget;
#[cfg(test)]
mod test_egui;
mod text;
mod theme;
mod typeface;
mod zoom_text;

use std::fmt::Write as _;

use cli::{
    CLI_NAME, CLI_USAGE, CliArgs, Command, RemoteProfileCommand, Start, SyncMode, parse_start, prepare_home_or_exit,
    starts_from_a_build_folder,
};

use agent_integrations::AgentIntegrations;
use app::HorizonApp;
use std::sync::Arc;

use eframe::{egui_wgpu, wgpu};
use horizon_core::{
    Config, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser, StartupDecision, WindowConfig,
};
use tracing_subscriber::fmt::format::FmtSpan;

fn main() -> eframe::Result {
    // Every argument of this start is judged before anything of this computer is
    // touched: no Horizon home, no tracing writer, no plugin, no settings, no
    // session and no lock. An argument that this build does not know ends the
    // start with code 2 instead of falling through to the window, so a start by
    // mistake cannot rewrite the settings or the skills of the person.
    let start = match parse_start(std::env::args_os().skip(1)) {
        Ok(start) => start,
        Err(problem) => {
            eprintln!("error: {problem}");
            eprintln!("{CLI_USAGE}");
            std::process::exit(2);
        }
    };
    let cli_args = match start {
        Start::Help => {
            println!("{CLI_USAGE}");
            std::process::exit(0);
        }
        Start::Version => {
            println!("{CLI_NAME} {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        // The agent work hook writes only to HORIZON_WORK_ROOT and keeps its own
        // exit codes, which steer the agent's turn.
        Start::AgentWorkHook => {
            agent_work_hook::run();
            return Ok(());
        }
        Start::BrowserMcp => {
            let _home = prepare_home_or_exit();
            run_browser_mcp_server();
            return Ok(());
        }
        // The driver of a browser host runs on that machine and speaks on the
        // standard input and output of one `ssh` child. It owns nothing of this
        // computer, so it comes before the home, the settings and the window.
        Start::BrowserHostDriver(options) => {
            init_tracing();
            std::process::exit(horizon_core::browser::host_driver::run(&options));
        }
        // The server and its link are the same binary, and they touch one
        // directory of their own. They come before the Horizon home, the settings
        // and the agent integration of the desktop, because a server must never
        // read or write the home of the person. The exit codes steer a service
        // manager, so they leave here and not through the window.
        //
        // The exit is also what ends the link mode: its relay returns when the
        // socket of the server ends, and the thread that reads its standard input
        // stays in a read that nothing of this process ends. A return through the
        // window would wait for that thread, so a stopped server would leave one
        // child of every idle computer alive for ever.
        Start::Sync(mode) => {
            init_tracing();
            std::process::exit(run_sync_mode(&mode));
        }
        Start::Desktop(cli_args) => cli_args,
    };
    // The copy of this binary that cargo keeps in `deps` sits beside the test
    // binaries, and a start of it cannot tell a person from a test runner. Every
    // mode that ends early has already left, so this refuses only a start that
    // would open the window, and it refuses it before the home is prepared.
    if starts_from_a_build_folder() {
        eprintln!("error: this file is a build artifact; start target/<profile>/horizon");
        std::process::exit(2);
    }
    init_tracing();
    run_window(&prepare_home_or_exit(), &cli_args)
}

/// The window of the person, or the command that ends without one.
fn run_window(horizon_home: &horizon_core::HorizonHome, cli_args: &CliArgs) -> eframe::Result {
    let resolved_config_path =
        Config::resolve_path(cli_args.config_path.as_deref()).unwrap_or_else(|| horizon_home.config_path());

    let agent_integrations = AgentIntegrations::from_environment();
    // The machine list of this start follows one server, or stays on this
    // computer. The browser host needs no value here: the app reads the entry of
    // the settings that it holds, so a change of it serves the next panel.
    horizon_core::configure_server(plugin_install::server_setting(&resolved_config_path));
    let _agent_plugin_host = agent_integrations.install_plugins(horizon_home);
    if let Some(command) = cli_args.command.as_ref() {
        run_command(command, &resolved_config_path);
    }
    #[cfg(target_os = "linux")]
    agent_integrations.install_desktop_entry();
    let config = load_config_or_exit(&resolved_config_path, cli_args.config_path.is_some());
    let session_store = SessionStore::new(horizon_home.clone(), resolved_config_path.clone());
    let startup = prepare_startup(&session_store, &config, cli_args);

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
    let device_requests = app::DeviceRequestBridge::new();
    let device_requests_for_app = Arc::clone(&device_requests);
    native_window::run_native_with_keyboard_observer(
        branding::APP_NAME,
        options,
        Box::new(move |cc| {
            log_graphics_adapter(cc);
            let host = HorizonApp::new(
                cc,
                &config,
                resolved_config_path.clone(),
                session_store.clone(),
                startup.clone(),
                app_keyboard_inputs.clone(),
            );
            device_requests_for_app.install(host, cc.egui_ctx.clone());
            Ok(Box::new(app::BridgeApp::new(device_requests_for_app)))
        }),
        observed_keyboard_inputs,
        device_requests,
    )
}

/// Runs the mode and answers its exit code: zero when it ended cleanly, the fixed
/// code of a link that found no server, and one for any other failure. Every
/// reason is one line on the standard error, which is the log of a service.
fn run_sync_mode(mode: &SyncMode) -> i32 {
    let home = match horizon_sync::ServerHome::resolve(None) {
        Ok(home) => home,
        Err(problem) => {
            eprintln!("error: {problem}");
            return problem.exit_code();
        }
    };
    let result = match mode {
        SyncMode::Server(options) => horizon_sync::terminate_flag()
            .map_err(|error| horizon_sync::HostError::Io {
                path: home.root().to_path_buf(),
                error,
            })
            .and_then(|stop| horizon_sync::serve_with_options(&home, &stop, options)),
        SyncMode::Link => horizon_sync::link_stdio(&home),
        SyncMode::Invite => return print_first_invitation(&home),
    };
    match result {
        Ok(()) => 0,
        Err(problem) => {
            eprintln!("error: {problem}");
            problem.exit_code()
        }
    }
}

/// Asks the server of this home for the first invitation, and prints the link, its
/// code and the QR code of the link.
///
/// The mode starts no server and writes no file: the server of the home answers,
/// or one typed line says that no server answers.
fn print_first_invitation(home: &horizon_sync::ServerHome) -> i32 {
    match horizon_sync::first_invitation(home, horizon_sync::SERVER_DEVICE) {
        Ok(invited) => match horizon_sync::invitation_text(&invited) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(problem) => {
                eprintln!("error: {problem}");
                1
            }
        },
        Err(problem) => {
            eprintln!("error: {problem}");
            1
        }
    }
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

/// Run the one command of this launch and exit with its status.
fn run_command(command: &Command, config_path: &std::path::Path) -> ! {
    let code = match command {
        Command::RemoteProfile(profile) => run_remote_profile_command(config_path, profile),
    };
    plugin_install::exit_after_releasing_plugins(code)
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

/// Read the settings of this start, or stop with the reason when they cannot be
/// read.
fn load_config_or_exit(config_path: &std::path::Path, named: bool) -> Config {
    match load_config(config_path, named) {
        Ok(config) => config,
        Err(problem) => {
            eprintln!("error: {problem}");
            plugin_install::exit_after_releasing_plugins(2);
        }
    }
}

/// Read the settings of this start, or the defaults when the run has no
/// settings yet.
///
/// `named` says that the run names its settings file, so the file must exist and
/// a YAML file is imported only from that exact name. A run with no settings
/// argument reads the store, the settings file, or the YAML settings file that
/// an older Horizon left in the same folder.
///
/// # Errors
///
/// Returns an error when a setting exists and cannot be read. The defaults never
/// replace the settings of a person, so the start stops and names the file
/// instead.
fn load_config(config_path: &std::path::Path, named: bool) -> Result<Config, String> {
    let loaded = if named {
        let store = horizon_core::config_store::ConfigStore::resolve(config_path);
        if !config_path.exists() && !store.path().exists() {
            tracing::info!("no config found at {}, using defaults", config_path.display());
            return Ok(Config::default());
        }
        Config::load(Some(config_path))
    } else {
        Config::load_from_home(config_path)
    };

    loaded.map_err(|error| {
        tracing::error!("failed to load config: {error}");
        match error {
            // The message of an old settings file already names the file and the
            // line of its fault, so it is not wrapped in the settings path of this
            // build.
            horizon_core::Error::OldSettingsFile { .. } => {
                format!("{error}\nMove the file aside to start with the default settings.")
            }
            error => format!(
                "cannot read the settings of {}: {error}\n\
                 Move the file aside to start with the default settings.",
                config_path.display()
            ),
        }
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

/// The log filter from `RUST_LOG`, or Horizon's default.
fn log_filter(configured: Option<&str>) -> tracing_subscriber::EnvFilter {
    configured
        .and_then(|directives| tracing_subscriber::EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new("horizon=info,horizon_core=info"))
}

/// The filter of the window: the directive of every latency module whose switch is
/// on, added to the filter of the person.
///
/// A latency switch is the only setting that its lines need, on both switches that
/// the building has: `HORIZON_TERMINAL_LATENCY` for the key press of a terminal and
/// `HORIZON_BROWSER_LATENCY` for an action of a browser panel. The default filter
/// allows `horizon_core`, which covers the line of a key press; a browser line has
/// a module of another crate, so the window names it here. A switch that is on wins
/// over a `RUST_LOG` that silences its module, because the person asked for those
/// lines and for nothing else.
fn filter_with_latency(configured: Option<&str>, terminal: bool, browser: bool) -> tracing_subscriber::EnvFilter {
    let mut directives: Vec<&str> = Vec::new();
    if terminal {
        directives.push("horizon_core::terminal::latency=info");
    }
    if browser {
        directives.push("horizon_browser::latency=info");
    }
    let mut filter = log_filter(configured);
    for directive in directives {
        if let Ok(directive) = directive.parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// Whether an event or span may be recorded at all. Raw HTTP from the provider clients
/// never is: at trace level `ureq_proto` dumps request bytes, and with them the API
/// tokens and registry logins in their headers. Applied beside the `RUST_LOG` filter,
/// so no directive, however specific, turns it back on.
fn recordable(metadata: &tracing::Metadata<'_>) -> bool {
    let target = metadata.target();
    !(target == "ureq_proto" || target.starts_with("ureq_proto::"))
}

fn init_tracing() {
    use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};
    let env_filter = filter_with_latency(
        std::env::var("RUST_LOG").ok().as_deref(),
        horizon_core::latency::enabled(),
        horizon_browser::latency::enabled(),
    );
    let recordable = tracing_subscriber::filter::filter_fn(recordable);

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
            .finish()
            .with(recordable)
            .init();
    } else {
        subscriber.finish().with(recordable).init();
    }
}

#[cfg(test)]
mod log_filter_tests {
    use tracing_subscriber::layer::SubscriberExt as _;

    /// Whether `target` records at trace level under the filters built from `configured`.
    fn traces(configured: Option<&str>, target: &str) -> bool {
        let subscriber = tracing_subscriber::registry()
            .with(super::log_filter(configured))
            .with(tracing_subscriber::filter::filter_fn(super::recordable));
        tracing::subscriber::with_default(subscriber, || match target {
            "ureq_proto::util" => tracing::enabled!(target: "ureq_proto::util", tracing::Level::TRACE),
            "ureq_proto" => tracing::enabled!(target: "ureq_proto", tracing::Level::TRACE),
            _ => tracing::enabled!(target: "horizon_core", tracing::Level::TRACE),
        })
    }

    #[test]
    fn raw_http_is_never_logged_whatever_rust_log_says() {
        for configured in [
            None,
            Some("trace"),
            Some("ureq_proto=trace"),
            Some("ureq_proto::util=trace"),
            Some("trace,ureq_proto=trace,ureq_proto::util=trace"),
            Some("not a filter ["),
        ] {
            for target in ["ureq_proto", "ureq_proto::util"] {
                assert!(!traces(configured, target), "{configured:?} {target}");
            }
        }
        assert!(traces(Some("trace"), "horizon_core"), "other targets keep their level");
        assert!(!traces(None, "horizon_core"), "the default stays at info");
    }

    /// Whether one module records an INFO line under `filter_with_latency`.
    fn records(configured: Option<&str>, terminal: bool, browser: bool, module: &str) -> bool {
        let subscriber = tracing_subscriber::registry()
            .with(super::filter_with_latency(configured, terminal, browser))
            .with(tracing_subscriber::filter::filter_fn(super::recordable));
        tracing::subscriber::with_default(subscriber, || match module {
            "horizon_core::terminal::latency" => {
                tracing::enabled!(target: "horizon_core::terminal::latency", tracing::Level::INFO)
            }
            "horizon_core::terminal" => tracing::enabled!(target: "horizon_core::terminal", tracing::Level::INFO),
            "horizon_browser" => tracing::enabled!(target: "horizon_browser", tracing::Level::INFO),
            _ => tracing::enabled!(target: "horizon_browser::latency", tracing::Level::INFO),
        })
    }

    #[test]
    fn a_latency_switch_is_the_only_setting_that_its_lines_need() {
        let terminal = "horizon_core::terminal::latency";
        let browser = "horizon_browser::latency";

        // Each switch is the only setting that its lines need, whatever filter the
        // person configured, because the window adds the one module of a switch
        // that is on.
        for configured in [None, Some("warn"), Some("error")] {
            assert!(
                records(configured, true, true, terminal),
                "the terminal switch {configured:?}"
            );
            assert!(
                records(configured, true, true, browser),
                "the browser switch {configured:?}"
            );
        }
        // Without that, a filter of the person silences the module of another
        // crate, and the switch would need a second variable.
        assert!(
            !records(Some("warn"), false, false, browser),
            "a browser line under a filter that silences it"
        );
        // A switch adds its one module and nothing else.
        assert!(
            !records(Some("warn"), true, true, "horizon_core::terminal"),
            "the switch adds no whole module"
        );
        assert!(
            !records(Some("warn"), false, false, terminal),
            "a switch that is off adds nothing"
        );
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{SessionSummary, StartupChooser, StartupPromptReason};

    use super::{startup_chooser_window_config, summarize_adapter, wgpu};
    use crate::cli::parse_browser_mcp_args;

    fn browser_mcp_mode(arguments: &[&str]) -> Result<bool, String> {
        parse_browser_mcp_args(arguments.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn the_browser_mcp_mode_takes_the_entry_of_the_local_server_alone() {
        assert_eq!(
            browser_mcp_mode(&["--browser-mcp"]),
            Ok(true),
            "the entry of the local browser starts the local server"
        );
        assert_eq!(
            browser_mcp_mode(&["--config", "/etc/horizon.json"]),
            Ok(false),
            "another start is not this mode"
        );
        for refused in [
            &["--browser-mcp", "--verbose"][..],
            &["--browser-mcp", "--remote", "browser-vm"],
            &["--browser-mcp", "--remote"],
            &["--browser-mcp", "--profile", "mail"],
        ] {
            assert!(browser_mcp_mode(refused).is_err(), "{refused:?}");
        }
    }

    fn chooser_with_sessions(session_count: usize) -> StartupChooser {
        StartupChooser {
            reason: StartupPromptReason::LiveConflict,
            config_path: "/tmp/horizon.json".to_string(),
            sessions: (0..session_count)
                .map(|index| SessionSummary {
                    session_id: format!("session-{index}"),
                    label: format!("Session {index}"),
                    workspace_count: 1,
                    panel_count: 1,
                    last_active_at: 0,
                    config_path: "/tmp/horizon.json".to_string(),
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

#[cfg(test)]
mod config_start_tests {
    use super::load_config;

    /// A store that no build can read, and no copy to carry the settings: the
    /// start must stop instead of running with the defaults.
    #[test]
    fn settings_that_cannot_be_read_stop_the_start() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.json");
        std::fs::write(dir.path().join("settings.sqlite"), b"not a database").expect("write");

        let error = load_config(&path, true).expect_err("a damaged store with no copy is an error");
        assert!(error.contains("cannot read the settings of"), "{error}");
        assert!(error.contains("config.json"), "{error}");
    }

    /// A YAML settings file of an older Horizon that cannot be read stops the
    /// start, and the sentence names the file and the line.
    #[test]
    fn a_broken_yaml_settings_file_names_its_file_and_its_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join(".horizon");
        std::fs::create_dir_all(&home).expect("the Horizon home");
        let yaml = home.join("config.yaml");
        std::fs::write(&yaml, "version: 8\nappearance:\n  theme: dark\n   theme: light\n").expect("write");

        let error = load_config(&home.join("config.json"), false).expect_err("a broken file stops the start");

        assert!(
            error.contains(&yaml.display().to_string()),
            "the sentence names the file: {error}"
        );
        assert!(error.contains("line"), "the sentence names the line: {error}");
        assert!(
            !home.join("settings.sqlite").exists(),
            "a file that cannot be read writes no store"
        );
    }

    /// A run with no settings at all starts with the defaults, and writes
    /// nothing.
    #[test]
    fn a_run_with_no_settings_starts_with_the_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.json");

        let config = load_config(&path, true).expect("defaults");
        assert_eq!(config.version, horizon_core::Config::default().version);
        assert!(!path.exists(), "the start writes no copy");
        assert!(
            !horizon_core::config_store::ConfigStore::resolve(&path).path().exists(),
            "the start writes no store"
        );
    }
}
