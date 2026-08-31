#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use horizon_browser::BackendKind;
use horizon_browser_cli::{
    Plan,
    execution_control::{ExecutionControl, ExecutionStopReason, JobDeadline},
    job::JobOptions,
    run_state::{DurableExecutionReport, DurableRun},
    standalone::StandaloneOptions,
};

const MAX_PLAN_BYTES: u64 = 1024 * 1024;
const DEFAULT_RUN_TIMEOUT_SECONDS: u64 = 30 * 60;
const MAX_RUN_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
const EXIT_TIMED_OUT: u8 = 124;
const HELP: &str = r#"horizon-browser — run browser jobs through one MCP contract

USAGE:
    horizon-browser "<GOAL>" [--backend <auto|chromium|firefox|safari>] [--visible] [--json]
    horizon-browser do "<GOAL>" [OPTIONS]
    horizon-browser run <PLAN.json|-> [--output <REPORT.json|->] [--timeout <SECONDS>]
    horizon-browser mcp [--standalone|--connect] [--backend <BACKEND>] [--visible]

COMMANDS:
    do     Ask an optional local agent to complete a goal through Horizon MCP.
           This is the default when the first argument is a quoted goal.
    run    Execute a fail-fast JSON plan through the existing MCP tools.
           Saves durable job state; reads stdin when PLAN is '-' and writes
           JSON to stdout by default.
    mcp    Serve the browser MCP contract over stdio. Outside Horizon it owns
           a standalone browser; --connect uses existing Horizon panels only.

OPTIONS:
    --backend <BACKEND>   Select a backend; prompt/MCP jobs default to auto.
    --visible             Show a native browser window; jobs default headless.
    --json                Emit stable JSONL job progress and completion events.
    -o, --output <PATH>    Write the JSON report to PATH; '-' means stdout.
    --timeout <SECONDS>    Bound the whole deterministic run (default 1800, max 86400).
    -h, --help             Print this help.
    -V, --version          Print the version.
"#;

enum Command {
    Run {
        plan: PathBuf,
        output: Option<PathBuf>,
        timeout: Duration,
    },
    Do(JobOptions),
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
        Ok(Command::Do(options)) => run_job(&options),
        Ok(Command::Mcp { standalone, options }) => serve_mcp(standalone, options).await,
        Ok(Command::Run { plan, output, timeout }) => run(plan, output.as_deref(), timeout).await,
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn run_job(options: &JobOptions) -> ExitCode {
    match horizon_browser_cli::job::run(options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
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

struct PreparedRun {
    plan: Plan,
    durable: DurableRun,
    control: ExecutionControl,
}

async fn prepare_run(plan_path: PathBuf, timeout: Duration) -> Result<PreparedRun, ExitCode> {
    let deadline = JobDeadline::after(timeout);
    let mut control = ExecutionControl::until(deadline);
    let plan = match deadline_bound_blocking(&mut control, "horizon-browser-plan-load", move || {
        let bytes = read_bounded(&plan_path)?;
        Plan::from_slice(&bytes).map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(plan)) => plan,
        Ok(Err(error)) => {
            eprintln!("error: {error}");
            return Err(ExitCode::from(2));
        }
        Err(_) => {
            eprintln!("error: {}", ExecutionStopReason::MESSAGE);
            return Err(ExitCode::from(EXIT_TIMED_OUT));
        }
    };
    let durable_plan = plan.clone();
    let timeout_seconds = timeout.as_secs();
    let deadline_at_millis = deadline.unix_millis();
    let mut durable = match deadline_bound_blocking(&mut control, "horizon-browser-state-prepare", move || {
        DurableRun::prepare(&durable_plan, timeout_seconds, deadline_at_millis).map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(durable)) => durable,
        Ok(Err(error)) => {
            eprintln!("error: {error}");
            return Err(ExitCode::FAILURE);
        }
        Err(_) => {
            eprintln!("error: {}", ExecutionStopReason::MESSAGE);
            return Err(ExitCode::from(EXIT_TIMED_OUT));
        }
    };
    if deadline.check().is_err() {
        eprintln!(
            "error: {}; durable job {}: {}",
            ExecutionStopReason::MESSAGE,
            durable.job_id(),
            durable.state_path().display()
        );
        return Err(ExitCode::from(EXIT_TIMED_OUT));
    }
    if let Err(error) = durable.activate() {
        if let Err(state_error) = durable.fail(&error.to_string()) {
            eprintln!("error: {error}; additionally could not persist failure state: {state_error}");
        } else {
            eprintln!(
                "error: {error}; durable job {}: {}",
                durable.job_id(),
                durable.state_path().display()
            );
        }
        return Err(ExitCode::FAILURE);
    }
    if let Err(reason) = deadline.check() {
        if let Err(state_error) = durable.stop(reason) {
            eprintln!(
                "error: {}; additionally could not persist stopped state: {state_error}",
                ExecutionStopReason::MESSAGE
            );
        } else {
            eprintln!(
                "error: {}; durable job {}: {}",
                ExecutionStopReason::MESSAGE,
                durable.job_id(),
                durable.state_path().display()
            );
        }
        return Err(ExitCode::from(EXIT_TIMED_OUT));
    }

    Ok(PreparedRun { plan, durable, control })
}

async fn run(plan_path: PathBuf, output_path: Option<&Path>, timeout: Duration) -> ExitCode {
    let PreparedRun {
        plan,
        mut durable,
        mut control,
    } = match prepare_run(plan_path, timeout).await {
        Ok(prepared) => prepared,
        Err(exit) => return exit,
    };
    let report = match horizon_browser_cli::execute_plan_with_control(&plan, &mut control).await {
        Ok(report) => report,
        Err(horizon_browser_cli::RunError::Stopped(reason)) => {
            if let Err(state_error) = durable.stop(reason) {
                eprintln!(
                    "error: {}; additionally could not persist stopped state: {state_error}",
                    ExecutionStopReason::MESSAGE
                );
                return ExitCode::from(EXIT_TIMED_OUT);
            }
            eprintln!(
                "error: {}; durable job {}: {}",
                ExecutionStopReason::MESSAGE,
                durable.job_id(),
                durable.state_path().display()
            );
            return ExitCode::from(EXIT_TIMED_OUT);
        }
        Err(error) => {
            if let Err(state_error) = durable.fail(&error.to_string()) {
                eprintln!("error: {error}; additionally could not persist failure state: {state_error}");
                return ExitCode::FAILURE;
            }
            eprintln!(
                "error: {error}; durable job {}: {}",
                durable.job_id(),
                durable.state_path().display()
            );
            return ExitCode::FAILURE;
        }
    };
    let post_process_error_exit = report
        .stop_reason
        .map_or(ExitCode::FAILURE, |_| ExitCode::from(EXIT_TIMED_OUT));
    if let Err(error) = durable.finish(&report) {
        eprintln!("error: {error}");
        return post_process_error_exit;
    }
    let report = durable.report(&report);
    if let Err(error) = write_report(&report, output_path) {
        eprintln!("error: {error}");
        return post_process_error_exit;
    }
    if report.execution.stop_reason.is_some() {
        ExitCode::from(EXIT_TIMED_OUT)
    } else if report.execution.ok {
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
        Some("do") => parse_do(args),
        Some("run") => parse_run(args),
        Some(prompt) if !prompt.starts_with('-') => parse_job(prompt.to_string(), args),
        Some(command) => Err(format!("unknown command or option `{command}`")),
        None => Err("command is not valid UTF-8".to_string()),
    }
}

fn parse_do(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let prompt = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| "do requires one quoted goal".to_string())?;
    parse_job(prompt, args)
}

fn parse_job(prompt: String, mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let mut backend = None;
    let mut backend_seen = false;
    let mut visible = false;
    let mut json = false;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--backend") if !backend_seen => {
                backend_seen = true;
                backend = parse_backend(args.next().as_ref())?;
            }
            Some("--backend") => return Err("--backend may be specified only once".to_string()),
            Some("--visible") if !visible => visible = true,
            Some("--json") if !json => json = true,
            Some(argument) => return Err(format!("unexpected job argument `{argument}`")),
            None => return Err("job argument is not valid UTF-8".to_string()),
        }
    }
    Ok(Command::Do(JobOptions {
        prompt,
        backend,
        visible,
        json,
    }))
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
    let mut timeout = Duration::from_secs(DEFAULT_RUN_TIMEOUT_SECONDS);
    let mut timeout_seen = false;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("-o" | "--output") if output.is_none() => {
                output = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--output requires a path or '-'".to_string())?,
                ));
            }
            Some("-o" | "--output") => return Err("--output may be specified only once".to_string()),
            Some("--timeout") if !timeout_seen => {
                timeout_seen = true;
                timeout = parse_run_timeout(args.next().as_ref())?;
            }
            Some("--timeout") => return Err("--timeout may be specified only once".to_string()),
            Some(argument) => return Err(format!("unexpected run argument `{argument}`")),
            None => return Err("run argument is not valid UTF-8".to_string()),
        }
    }
    Ok(Command::Run { plan, output, timeout })
}

fn parse_run_timeout(value: Option<&OsString>) -> Result<Duration, String> {
    let seconds = value
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=MAX_RUN_TIMEOUT_SECONDS).contains(seconds))
        .ok_or_else(|| format!("--timeout requires whole seconds from 1 through {MAX_RUN_TIMEOUT_SECONDS}"))?;
    Ok(Duration::from_secs(seconds))
}

async fn deadline_bound_blocking<T>(
    control: &mut ExecutionControl,
    worker_name: &'static str,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<Result<T, String>, ExecutionStopReason>
where
    T: Send + 'static,
{
    control
        .wait(async move {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let _worker = std::thread::Builder::new()
                .name(worker_name.to_string())
                .spawn(move || {
                    let _ = sender.send(operation());
                })
                .map_err(|error| format!("could not start {worker_name}: {error}"))?;
            receiver
                .await
                .map_err(|error| format!("{worker_name} stopped without a result: {error}"))?
        })
        .await
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

fn write_report(report: &DurableExecutionReport<'_>, output: Option<&Path>) -> Result<(), String> {
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

fn write_private_file(path: &Path, report: &DurableExecutionReport<'_>) -> Result<(), String> {
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

fn encode_report(
    report: &DurableExecutionReport<'_>,
    writer: &mut impl io::Write,
    destination: &str,
) -> Result<(), String> {
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
        let Command::Run { plan, output, timeout } = command else {
            panic!("expected run command");
        };
        assert_eq!(plan, Path::new("plan.json"));
        assert_eq!(output.as_deref(), Some(Path::new("report.json")));
        assert_eq!(timeout, Duration::from_secs(DEFAULT_RUN_TIMEOUT_SECONDS));
        assert!(parse_args(["run", "plan.json", "--timeout", "0"].map(OsString::from)).is_err());
        assert!(parse_args(["run", "plan.json", "--timeout", "86401"].map(OsString::from)).is_err());
        assert!(parse_args(["run", "plan.json", "--timeout", "1", "--timeout", "2"].map(OsString::from)).is_err());
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

    #[test]
    fn quoted_goal_is_the_default_and_hidden_auto_is_the_default() {
        let Command::Do(options) = parse_args(["visit example.com"].map(OsString::from)).expect("default job") else {
            panic!("expected job command");
        };
        assert_eq!(options.prompt, "visit example.com");
        assert_eq!((options.backend, options.visible, options.json), (None, false, false));
    }
}
