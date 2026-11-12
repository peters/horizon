#![deny(unsafe_code)]

mod agent_integrations;
mod agent_work_hook;
mod app;
mod badge;
mod block_widget;
mod branding;
mod browser_widget;
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

use std::fmt::Write as _;
use std::path::PathBuf;

use agent_integrations::AgentIntegrations;
use app::HorizonApp;
use std::sync::Arc;

use eframe::{egui_wgpu, wgpu};
use horizon_core::{
    Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupChooser, StartupDecision,
    WindowConfig,
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
    init_tracing();
    let horizon_home = prepare_home_or_exit();
    let resolved_config_path =
        Config::resolve_path(cli_args.config_path.as_deref()).unwrap_or_else(|| horizon_home.config_path());

    let agent_integrations = AgentIntegrations::from_environment();
    let browser_host = plugin_install::browser_host_setting(&resolved_config_path);
    horizon_core::configure_browser_host(browser_host.clone());
    // The machine list of this start follows one server, or stays on this
    // computer. The entry travels to the app beside the browser host.
    horizon_core::configure_server(plugin_install::server_setting(&resolved_config_path));
    let _agent_plugin_host = agent_integrations.install_plugins(&horizon_home);
    if let Some(command) = cli_args.command.as_ref() {
        run_command(command, &resolved_config_path);
    }
    #[cfg(target_os = "linux")]
    agent_integrations.install_desktop_entry();
    let config = load_config_or_exit(&resolved_config_path, cli_args.config_path.is_some());
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

/// The name of this program on a command line, as the usage writes it.
const CLI_NAME: &str = "horizon";

/// One start of this binary, judged from its arguments alone.
#[derive(Debug)]
enum Start {
    /// `--help` or `-h`: the usage of this build.
    Help,
    /// `--version` or `-V`: the name and the version of this build.
    Version,
    /// `--agent-work-hook`: one lifecycle answer of an opted-in panel.
    AgentWorkHook,
    /// One of the three modes of the server.
    Sync(SyncMode),
    /// `--browser-mcp`: the browser MCP server on the standard input and output.
    BrowserMcp,
    /// The window of the person, or a command that ends without one.
    Desktop(CliArgs),
}

/// Reads every argument of this start and answers its mode.
///
/// Each mode takes its own arguments alone, and an argument that this build does
/// not know is an error: a start that no mode serves never falls through to the
/// window of the person.
///
/// # Errors
///
/// Returns the reason that the arguments of this start cannot continue.
fn parse_start(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Start, String> {
    let args: Vec<std::ffi::OsString> = args.into_iter().collect();
    match args.first().and_then(|first| first.to_str()) {
        Some("--help" | "-h") => alone(&args, Start::Help),
        Some("--version" | "-V") => alone(&args, Start::Version),
        Some("--agent-work-hook") => alone(&args, Start::AgentWorkHook),
        Some("--browser-mcp") => {
            parse_browser_mcp_args(args.iter().cloned())?;
            Ok(Start::BrowserMcp)
        }
        _ => match parse_sync_mode(args.iter().cloned())? {
            Some(mode) => Ok(Start::Sync(mode)),
            None => Ok(Start::Desktop(parse_cli_args(args)?)),
        },
    }
}

/// A mode that takes no other argument.
fn alone(args: &[std::ffi::OsString], start: Start) -> Result<Start, String> {
    match args.get(1) {
        Some(extra) => Err(unknown_argument(extra)),
        None => Ok(start),
    }
}

/// The reason of one argument that this build does not know.
fn unknown_argument(argument: &std::ffi::OsStr) -> String {
    format!("unknown argument {}", argument.to_string_lossy())
}

/// The Horizon home of this start, or the end of the start with code 2.
fn prepare_home_or_exit() -> HorizonHome {
    match HorizonHome::prepare() {
        Ok(home) => home,
        Err(problem) => {
            eprintln!("error: {problem}");
            std::process::exit(2);
        }
    }
}

/// One of the three modes of the server, whose work needs no window and no home of
/// the person.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SyncMode {
    /// `horizon --server`: answers the links of the computers of one server home,
    /// and opens the TCP listener when `--listen` names one.
    Server(horizon_sync::ServerOptions),
    /// `horizon --server-link`: relays the standard input and output of one `ssh`
    /// child to the socket of that server.
    Link,
    /// `horizon --server-invite`: asks the server of this home for the first
    /// invitation, and prints the link, the code and its QR code.
    Invite,
}

/// Whether these arguments ask for the server, for its link or for its first
/// invitation.
///
/// # Errors
/// Returns why an argument that starts one of the three modes cannot continue.
fn parse_sync_mode(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Option<SyncMode>, String> {
    let mut options = horizon_sync::ServerOptions::default();
    let mode = match args.next().as_deref() {
        Some(argument) if argument == "--server" => {
            loop {
                match args.next() {
                    None => break,
                    Some(flag) if flag == "--listen" => options.listen = Some(argument_of(&mut args, "--listen")?),
                    Some(flag) if flag == "--name" => options.name = Some(argument_of(&mut args, "--name")?),
                    Some(argument) => {
                        return Err(format!(
                            "`{}` is not part of the server mode",
                            argument.to_string_lossy()
                        ));
                    }
                }
            }
            return Ok(Some(SyncMode::Server(options)));
        }
        Some(argument) if argument == "--server-link" => SyncMode::Link,
        Some(argument) if argument == "--server-invite" => SyncMode::Invite,
        _ => return Ok(None),
    };
    if let Some(argument) = args.next() {
        return Err(format!(
            "`{}` is not part of the server mode",
            argument.to_string_lossy()
        ));
    }
    Ok(Some(mode))
}

/// The value of one flag of the server mode.
fn argument_of(args: &mut impl Iterator<Item = std::ffi::OsString>, flag: &str) -> Result<String, String> {
    let value = args.next().ok_or_else(|| format!("`{flag}` needs one value"))?;
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("the value of `{flag}` is not text"))
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

/// Whether these arguments ask for the browser MCP server of this computer,
/// whose entry in a client is `--browser-mcp` alone.
///
/// A further argument is an error: an entry that names something this mode does
/// not serve must never fall back to another browser.
fn parse_browser_mcp_args(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<bool, String> {
    if args.next().is_none_or(|argument| argument != "--browser-mcp") {
        return Ok(false);
    }
    if let Some(argument) = args.next() {
        return Err(format!(
            "`{}` is not part of the browser MCP mode",
            argument.to_string_lossy()
        ));
    }
    Ok(true)
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
    let env_filter = log_filter(std::env::var("RUST_LOG").ok().as_deref());
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

const CLI_USAGE: &str = "usage: horizon [--config <path>] [--ephemeral] [--new-session] [--blank]
       horizon [--config <path>] --export-remote-profile <path>
       horizon [--config <path>] --import-remote-profile <path>
       horizon --server [--listen <ip>:<port>] [--name <dns-name>]
       horizon --server-link
       horizon --server-invite";

#[derive(Debug, PartialEq, Eq)]
enum RemoteProfileCommand {
    Export(PathBuf),
    Import(PathBuf),
}

#[derive(Debug)]
struct CliArgs {
    config_path: Option<PathBuf>,
    new_session: bool,
    ephemeral: bool,
    blank: bool,
    command: Option<Command>,
}

/// A command that finishes without opening a window.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    RemoteProfile(RemoteProfileCommand),
}

/// Parse the launch flags. The remote-profile commands take exactly one
/// operand that is not itself a flag, and at most one of them may be given. An
/// argument that this build does not know is an error, so a typo can never fall
/// through to an ordinary launch or run another command.
fn parse_cli_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<CliArgs, String> {
    let mut config_path = None;
    let mut new_session = false;
    let mut ephemeral = false;
    let mut blank = false;
    let mut command: Option<Command> = None;

    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str() else {
            return Err(unknown_argument(&arg));
        };
        let mut operand = |flag: &str| -> Result<PathBuf, String> {
            match args.next() {
                Some(value) if !value.is_empty() && !value.to_string_lossy().starts_with('-') => {
                    Ok(PathBuf::from(value))
                }
                _ => Err(format!("{flag} needs a path")),
            }
        };
        match flag {
            "--config" | "-c" => config_path = Some(operand(flag)?),
            "--export-config" => {
                return Err(
                    "--export-config is gone: the settings live in settings.sqlite, and the Settings window writes them"
                        .to_string(),
                );
            }
            "--export-remote-profile" | "--import-remote-profile" => {
                let path = operand(flag)?;
                let profile = if flag == "--export-remote-profile" {
                    RemoteProfileCommand::Export(path)
                } else {
                    RemoteProfileCommand::Import(path)
                };
                set_command(&mut command, Command::RemoteProfile(profile))?;
            }
            "--new-session" => new_session = true,
            "--ephemeral" => ephemeral = true,
            "--blank" => blank = true,
            unknown => return Err(unknown_argument(std::ffi::OsStr::new(unknown))),
        }
    }

    Ok(CliArgs {
        config_path,
        new_session,
        ephemeral,
        blank,
        command,
    })
}

/// Keep one command, so a second one is an error instead of a silent winner.
fn set_command(command: &mut Option<Command>, next: Command) -> Result<(), String> {
    if command.replace(next).is_some() {
        return Err("give one command only, once: --export-remote-profile or --import-remote-profile".to_string());
    }
    Ok(())
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
}

#[cfg(test)]
mod cli_args_tests {
    use std::ffi::OsString;

    use super::{Command, RemoteProfileCommand, Start, SyncMode, parse_cli_args, parse_start, parse_sync_mode};

    fn parse(args: &[&str]) -> Result<super::CliArgs, String> {
        parse_cli_args(args.iter().map(|arg| OsString::from(*arg)))
    }

    fn start(args: &[&str]) -> Result<Start, String> {
        parse_start(args.iter().map(|arg| OsString::from(*arg)))
    }

    /// The usage, the version and the hook take no other argument, and an
    /// argument that this build does not know ends the start instead of opening
    /// the window of the person.
    #[test]
    fn a_mode_takes_its_own_arguments_alone() {
        for (arguments, help, version, hook) in [
            (&["--help"][..], true, false, false),
            (&["-h"][..], true, false, false),
            (&["--version"][..], false, true, false),
            (&["-V"][..], false, true, false),
            (&["--agent-work-hook"][..], false, false, true),
        ] {
            let mode = start(arguments).expect("the mode");
            assert_eq!(matches!(mode, Start::Help), help, "{arguments:?}");
            assert_eq!(matches!(mode, Start::Version), version, "{arguments:?}");
            assert_eq!(matches!(mode, Start::AgentWorkHook), hook, "{arguments:?}");
            let extra = [arguments[0], "--list"];
            let error = start(&extra).err().expect("a mode takes no other argument");
            assert_eq!(error, "unknown argument --list", "{extra:?}");
        }
        assert!(matches!(
            start(&["--browser-mcp"]).expect("the browser MCP mode"),
            Start::BrowserMcp
        ));
        assert!(
            matches!(start(&[]).expect("a start with no argument"), Start::Desktop(_)),
            "no argument is the window of the person"
        );
    }

    #[test]
    fn an_argument_that_this_build_does_not_know_ends_the_start() {
        for arguments in [
            &["--list"][..],
            &["--nocapture"],
            &["--test-threads", "4"],
            &["--config", "c.json", "--list"],
            &["stray"],
        ] {
            let error = start(arguments).err().expect("the argument is refused");
            assert!(error.starts_with("unknown argument "), "{arguments:?}: {error}");
            assert!(
                !error.contains("--blank") && error.len() < 80,
                "one line names only the argument: {error}"
            );
        }
        let error = start(&["--definitely-unknown"]).err().expect("refused");
        assert_eq!(error, "unknown argument --definitely-unknown");
    }

    #[test]
    fn remote_profile_commands_take_one_real_operand_and_exclude_each_other() {
        let args = parse(&["--config", "c.json", "--export-remote-profile", "p.json"]).expect("export");
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Export("p.json".into())))
        );
        assert_eq!(args.config_path.as_deref(), Some(std::path::Path::new("c.json")));
        let args = parse(&["--import-remote-profile", "p.json", "--ephemeral"]).expect("import");
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Import("p.json".into())))
        );
        assert!(args.ephemeral);

        for bad in [
            &["--import-remote-profile"][..],
            &["--import-remote-profile", "--ephemeral"],
            &["--export-remote-profile", ""],
            &["--config"],
            &["--export-remote-profile", "a.json", "--import-remote-profile", "b.json"],
            &["--export-remote-profile", "a.json", "--export-remote-profile", "b.json"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            parse(&["--blank", "--new-session"])
                .expect("plain launch")
                .command
                .is_none()
        );
    }

    #[test]
    fn the_server_and_its_link_are_one_flag_alone() {
        assert_eq!(
            parse_sync_mode(["--server"].map(Into::into).into_iter()).expect("server"),
            Some(SyncMode::Server(horizon_sync::ServerOptions::default()))
        );
        assert_eq!(
            parse_sync_mode(["--server-link"].map(Into::into).into_iter()).expect("link"),
            Some(SyncMode::Link)
        );
        assert_eq!(
            parse_sync_mode(["--server-invite"].map(Into::into).into_iter()).expect("invite"),
            Some(SyncMode::Invite)
        );
        assert_eq!(
            parse_sync_mode(["--blank"].map(Into::into).into_iter()).expect("the desktop"),
            None
        );
        assert_eq!(parse_sync_mode(std::iter::empty()).expect("no argument"), None);
        for bad in [
            &["--server", "--config"][..],
            &["--server-link", "extra"],
            &["--server-invite", "extra"],
            &["--server", "--listen"],
            &["--server", "--port", "7000"],
            &["--server", "--listen", "127.0.0.1:7000", "extra"],
        ] {
            assert!(
                parse_sync_mode(bad.iter().map(Into::into)).is_err(),
                "{bad:?} must not fall through to the desktop"
            );
        }
    }

    #[test]
    fn the_server_takes_the_address_of_its_listener_and_the_name_of_its_links() {
        let mode = parse_sync_mode(
            ["--server", "--listen", "127.0.0.1:7000", "--name", "server-computer"]
                .map(Into::into)
                .into_iter(),
        )
        .expect("a server with a listener")
        .expect("the server mode");
        let SyncMode::Server(options) = mode else {
            panic!("the server mode");
        };
        assert_eq!(options.listen.as_deref(), Some("127.0.0.1:7000"));
        assert_eq!(options.name.as_deref(), Some("server-computer"));
        let info = options
            .listener()
            .expect("the listener of the mode")
            .expect("one listener");
        assert_eq!(info.address().to_string(), "127.0.0.1:7000");
        assert_eq!(
            info.join_link("ABCD-EFGH-JKMN").expect("a link"),
            "horizon://server-computer:7000/join#ABCD-EFGH-JKMN",
            "the name of the server is the host of a link"
        );

        // A person never writes a wildcard address, and the rule says so.
        let wildcard = parse_sync_mode(["--server", "--listen", "0.0.0.0:7000"].map(Into::into).into_iter())
            .expect("the mode parses")
            .expect("the server mode");
        let SyncMode::Server(options) = wildcard else {
            panic!("the server mode");
        };
        let error = options.listener().expect_err("a wildcard address is refused");
        assert!(error.to_string().contains("100.64.0.0/10"), "{error}");
        assert!(
            horizon_sync::ServerOptions::default()
                .listener()
                .expect("no listener")
                .is_none(),
            "the listener is off without --listen"
        );
    }

    #[test]
    fn a_profile_command_takes_its_path_and_runs_alone() {
        let args = parse(&["--config", "c.json", "--export-remote-profile", "out.json"]).expect("one profile command");
        assert_eq!(args.config_path.as_deref(), Some(std::path::Path::new("c.json")));
        assert_eq!(
            args.command,
            Some(Command::RemoteProfile(RemoteProfileCommand::Export("out.json".into())))
        );

        for bad in [
            &["--export-remote-profile", "a.json", "--import-remote-profile", "b.json"][..],
            &["--export-remote-profile", "a.json", "--export-remote-profile", "b.json"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let error = parse(&["--export-config"])
            .err()
            .expect("the removed export is refused");
        assert!(error.contains("settings.sqlite"), "{error}");
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{SessionSummary, StartupChooser, StartupPromptReason};

    use super::{parse_browser_mcp_args, startup_chooser_window_config, summarize_adapter, wgpu};

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
