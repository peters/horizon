#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use horizon_browser::BackendKind;
use horizon_browser_cli::{ExecutionReport, Plan, standalone::StandaloneOptions};

const MAX_PLAN_BYTES: u64 = 1024 * 1024;
const HELP: &str = r"horizon-browser — run browser jobs through one MCP contract

USAGE:
    horizon-browser run <PLAN.json|-> [--output <REPORT.json|->]
    horizon-browser mcp [--standalone|--connect] [--backend <BACKEND>] [--visible]

COMMANDS:
    run    Execute a fail-fast JSON plan through the existing MCP tools.
           Reads stdin when PLAN is '-' and writes JSON to stdout by default.
    mcp    Serve the browser MCP contract over stdio. Outside Horizon it owns
           a standalone browser; --connect uses existing Horizon panels only.

OPTIONS:
    --backend <BACKEND>   Select a standalone backend; defaults to auto.
    --visible             Show the standalone native browser window.
    -o, --output <PATH>    Write the JSON report to PATH; '-' means stdout.
    -h, --help             Print this help.
    -V, --version          Print the version.
";

enum Command {
    Run {
        plan: PathBuf,
        output: Option<PathBuf>,
    },
    Mcp {
        standalone: bool,
        options: StandaloneOptions,
    },
    Help,
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    initialize_tracing();
    match parse_args(std::env::args_os().skip(1)) {
        Ok(Command::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("horizon-browser {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Mcp { standalone, options }) => serve_mcp(standalone, options).await,
        Ok(Command::Run { plan, output }) => run(plan, output.as_deref()).await,
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn initialize_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(false)
        .with_writer(io::stderr)
        .try_init();
}

async fn serve_mcp(standalone: bool, options: StandaloneOptions) -> ExitCode {
    let result = if standalone {
        horizon_browser_cli::standalone::serve(options)
            .await
            .map_err(|error| error.to_string())
    } else {
        horizon_browser_mcp::serve_stdio()
            .await
            .map_err(|error| error.to_string())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "Horizon browser MCP server stopped with an error");
            ExitCode::FAILURE
        }
    }
}

async fn run(plan_path: PathBuf, output_path: Option<&Path>) -> ExitCode {
    let bytes = match read_bounded(&plan_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let plan = match Plan::from_slice(&bytes) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let report = match horizon_browser_cli::execute_plan(&plan).await {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = write_report(&report, output_path) {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Command::Help);
    };
    match command.to_str() {
        Some("-h" | "--help") => no_more(args, Command::Help),
        Some("-V" | "--version") => no_more(args, Command::Version),
        Some("mcp") => parse_mcp(args),
        Some("run") => parse_run(args),
        Some(command) => Err(format!("unknown command `{command}`")),
        None => Err("command is not valid UTF-8".to_string()),
    }
}

fn parse_mcp(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let mut standalone = std::env::var_os("HORIZON_BROWSER_ACTOR").is_none();
    let mut mode_seen = false;
    let mut backend = None;
    let mut backend_seen = false;
    let mut visible = false;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--standalone") if !mode_seen => {
                standalone = true;
                mode_seen = true;
            }
            Some("--connect") if !mode_seen => {
                standalone = false;
                mode_seen = true;
            }
            Some("--standalone" | "--connect") => {
                return Err("choose only one of --standalone or --connect".to_string());
            }
            Some("--backend") if !backend_seen => {
                backend_seen = true;
                backend = parse_backend(args.next().as_ref())?;
            }
            Some("--backend") => return Err("--backend may be specified only once".to_string()),
            Some("--visible") if !visible => visible = true,
            Some(argument) => return Err(format!("unexpected mcp argument `{argument}`")),
            None => return Err("mcp argument is not valid UTF-8".to_string()),
        }
    }
    if !standalone && (backend_seen || visible) {
        return Err("--backend and --visible require standalone MCP mode".to_string());
    }
    Ok(Command::Mcp {
        standalone,
        options: StandaloneOptions { backend, visible },
    })
}

fn parse_backend(value: Option<&OsString>) -> Result<Option<BackendKind>, String> {
    match value.and_then(|value| value.to_str()) {
        Some("auto") => Ok(None),
        Some("chromium") => Ok(Some(BackendKind::ChromiumCdp)),
        Some("firefox" | "firefox-bidi") => Ok(Some(BackendKind::FirefoxBidi)),
        Some("safari") => Ok(Some(BackendKind::SafariWebDriver)),
        Some(value) => Err(format!("unknown backend `{value}`")),
        None => Err("--backend requires auto, chromium, firefox, or safari".to_string()),
    }
}

fn no_more(mut args: impl Iterator<Item = OsString>, command: Command) -> Result<Command, String> {
    if let Some(extra) = args.next() {
        return Err(format!("unexpected argument `{}`", extra.to_string_lossy()));
    }
    Ok(command)
}

fn parse_run(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let plan = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "run requires a plan path or '-'".to_string())?;
    let mut output = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("-o" | "--output") if output.is_none() => {
                output = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--output requires a path or '-'".to_string())?,
                ));
            }
            Some("-o" | "--output") => return Err("--output may be specified only once".to_string()),
            Some(argument) => return Err(format!("unexpected run argument `{argument}`")),
            None => return Err("run argument is not valid UTF-8".to_string()),
        }
    }
    Ok(Command::Run { plan, output })
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        io::stdin()
            .take(MAX_PLAN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read plan from stdin: {error}"))?;
    } else {
        std::fs::File::open(path)
            .map_err(|error| format!("could not open plan {}: {error}", path.display()))?
            .take(MAX_PLAN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read plan {}: {error}", path.display()))?;
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PLAN_BYTES {
        return Err(format!("plan exceeds the {MAX_PLAN_BYTES}-byte limit"));
    }
    Ok(bytes)
}

fn write_report(report: &ExecutionReport, output: Option<&Path>) -> Result<(), String> {
    if output.is_none_or(|path| path == Path::new("-")) {
        let mut stdout = io::stdout().lock();
        encode_report(report, &mut stdout, "stdout")?;
        stdout
            .flush()
            .map_err(|error| format!("could not flush report to stdout: {error}"))
    } else if let Some(path) = output {
        write_private_file(path, report)
    } else {
        unreachable!()
    }
}

fn write_private_file(path: &Path, report: &ExecutionReport) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not open report {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = file
            .metadata()
            .map_err(|error| format!("could not inspect report {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)
            .map_err(|error| format!("could not secure report {}: {error}", path.display()))?;
    }
    encode_report(report, &mut file, &path.display().to_string())?;
    file.sync_all()
        .map_err(|error| format!("could not write report {}: {error}", path.display()))
}

fn encode_report(report: &ExecutionReport, writer: &mut impl io::Write, destination: &str) -> Result<(), String> {
    serde_json::to_writer_pretty(&mut *writer, report)
        .map_err(|error| format!("could not encode report to {destination}: {error}"))?;
    writer
        .write_all(b"\n")
        .map_err(|error| format!("could not finish report to {destination}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parser_keeps_run_surface_small() {
        let command =
            parse_args(["run", "plan.json", "--output", "report.json"].map(OsString::from)).expect("valid run command");
        let Command::Run { plan, output } = command else {
            panic!("expected run command");
        };
        assert_eq!(plan, Path::new("plan.json"));
        assert_eq!(output.as_deref(), Some(Path::new("report.json")));
        assert!(parse_args(["run", "plan.json", "--actor", "spoof"].map(OsString::from)).is_err());
        assert!(parse_args(["mcp", "extra"].map(OsString::from)).is_err());

        let Command::Mcp { standalone, options } =
            parse_args(["mcp", "--standalone", "--backend", "firefox", "--visible"].map(OsString::from))
                .expect("standalone MCP")
        else {
            panic!("expected MCP command");
        };
        assert!(standalone);
        assert_eq!(options.backend, Some(BackendKind::FirefoxBidi));
        assert!(options.visible);
        assert!(parse_args(["mcp", "--connect", "--visible"].map(OsString::from)).is_err());
    }
}
