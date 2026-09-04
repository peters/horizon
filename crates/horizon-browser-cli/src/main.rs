#![forbid(unsafe_code)]

mod run_finalization;
mod run_preparation;

use std::convert::Infallible;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::future::pending;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use horizon_browser::BackendKind;
use horizon_browser_cli::{
    Plan, PlanResume,
    checkpoint::{ResumeError, ResumeSelection, UncertainPolicy, valid_job_id},
    execute_plan_with_resume,
    execution_control::{
        BlockingIoError, BlockingIoMode, CancellationHandle, CancellationProbe, ExecutionControl, ExecutionStopReason,
    },
    job::JobOptions,
    run_state::{DurableExecutionReport, DurablePreparationError, DurableRun},
    standalone::StandaloneOptions,
};
use run_preparation::PreparationCompletion;

const MAX_PLAN_BYTES: u64 = 1024 * 1024;
const DEFAULT_RUN_TIMEOUT_SECONDS: u64 = 30 * 60;
const MAX_RUN_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
const EXIT_TIMED_OUT: u8 = 124;
const EXIT_CANCELLED: u8 = 130;
const CANCELLATION_FLUSH_GRACE: Duration = Duration::from_secs(1);
const HELP: &str = r#"horizon-browser — run browser jobs through one MCP contract

USAGE:
    horizon-browser "<GOAL>" [--backend <auto|chromium|firefox|safari>] [--visible] [--json]
    horizon-browser do "<GOAL>" [OPTIONS]
    horizon-browser run <PLAN.json|-> [--output <REPORT.json|->] [--timeout <SECONDS>]
    horizon-browser resume <JOB-ID> [--output <REPORT.json|->] [--timeout <SECONDS>] [--on-uncertain fail|skip]
    horizon-browser mcp [--standalone|--connect] [--backend <BACKEND>] [--visible]

COMMANDS:
    do     Ask an optional local agent to complete a goal through Horizon MCP.
           This is the default when the first argument is a quoted goal.
    run    Execute a fail-fast JSON plan through the existing MCP tools.
           Saves durable job state; reads stdin when PLAN is '-' and writes
           JSON to stdout by default. Ctrl-C preserves progress and exits 130.
    resume Continue a cancelled, timed-out, or failed job from verified
           checkpoints. Uncertain in-flight calls are never replayed.
           --on-uncertain skip continues later steps after audit inspection.
    mcp    Serve the browser MCP contract over stdio. Outside Horizon it owns
           a standalone browser; --connect uses existing Horizon panels only.

OPTIONS:
    --backend <BACKEND>   Select a backend; prompt/MCP jobs default to auto.
    --visible             Show a native browser window; jobs default headless.
    --json                Emit stable JSONL job progress and completion events.
    -o, --output <PATH>    Write the JSON report to PATH; '-' means stdout.
    --timeout <SECONDS>    Bound durable preparation and MCP work (default 1800, max 86400).
    --on-uncertain <MODE>  resume: fail (default) or continue after the
                           uncertain step without replaying it.
    -h, --help             Print this help.
    -V, --version          Print the version.
"#;

enum Command {
    Run {
        plan: PathBuf,
        output: Option<PathBuf>,
        timeout: Duration,
    },
    Resume {
        job_id: String,
        output: Option<PathBuf>,
        timeout: Duration,
        on_uncertain: UncertainPolicy,
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
        Ok(Command::Resume {
            job_id,
            output,
            timeout,
            on_uncertain,
        }) => resume(job_id, output.as_deref(), timeout, on_uncertain).await,
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

struct PreparedResume {
    durable: DurableRun,
    plan: Plan,
    selection: ResumeSelection,
}

enum ResumePreparation {
    Completed(Box<PreparedResume>),
    Rejected(ResumeError),
    RearmFailed { durable: Box<DurableRun>, error: String },
}

enum BlockingCompletion<T> {
    Completed(T),
    InfrastructureFailed(String),
}

async fn controlled_blocking<T>(
    control: &mut ExecutionControl,
    worker_name: &'static str,
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<BlockingCompletion<T>, ExecutionStopReason>
where
    T: Send + 'static,
{
    control.check()?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let _worker = match std::thread::Builder::new()
        .name(worker_name.to_string())
        .spawn(move || {
            let _ = sender.send(operation());
        }) {
        Ok(worker) => worker,
        Err(error) => {
            control.check()?;
            return Ok(BlockingCompletion::InfrastructureFailed(format!(
                "could not start {worker_name}: {error}"
            )));
        }
    };
    control
        .wait(async move {
            match receiver.await {
                Ok(result) => BlockingCompletion::Completed(result),
                Err(error) => {
                    BlockingCompletion::InfrastructureFailed(format!("{worker_name} stopped without a result: {error}"))
                }
            }
        })
        .await
}

async fn prepare_run(
    plan_path: PathBuf,
    timeout: Duration,
    mut control: ExecutionControl,
    cancellation: &mut CancellationProbe,
) -> Result<PreparedRun, ExitCode> {
    let plan = match controlled_blocking(&mut control, "horizon-browser-plan-load", move || {
        let bytes = read_bounded(&plan_path)?;
        Plan::from_slice(&bytes).map_err(|error| error.to_string())
    })
    .await
    {
        Ok(BlockingCompletion::Completed(Ok(plan))) => plan,
        Ok(BlockingCompletion::Completed(Err(error))) => {
            eprintln!("error: {error}");
            return Err(ExitCode::from(2));
        }
        Ok(BlockingCompletion::InfrastructureFailed(error)) => {
            eprintln!("error: {error}");
            return Err(ExitCode::FAILURE);
        }
        Err(reason) => {
            eprintln!("error: {}", reason.message());
            return Err(stop_exit_code(reason));
        }
    };

    let deadline = control.start_timeout(timeout);
    let prepared = match run_preparation::prepare_durable(
        &mut control,
        cancellation,
        plan.clone(),
        timeout.as_secs(),
        deadline.unix_millis(),
    )
    .await
    {
        Ok(PreparationCompletion::Completed(prepared)) => prepared.map(|run| *run),
        Ok(PreparationCompletion::InfrastructureFailed(error)) => {
            eprintln!("error: {error}");
            return Err(ExitCode::FAILURE);
        }
        Err(reason) => {
            eprintln!("error: {}", reason.message());
            return Err(stop_exit_code(reason));
        }
    };
    let durable = match prepared {
        Ok(durable) => durable,
        Err(error) => return Err(persist_preparation_failure(error, cancellation).await),
    };
    if let Err(reason) = control.check() {
        return Err(persist_stopped(&durable, reason, cancellation).await);
    }

    Ok(PreparedRun { plan, durable, control })
}

async fn run_controlled(
    plan_path: PathBuf,
    output_path: Option<&Path>,
    timeout: Duration,
    control: ExecutionControl,
    mut preparation_cancellation: CancellationProbe,
    mut finalization_cancellation: CancellationProbe,
) -> ExitCode {
    let PreparedRun {
        plan,
        mut durable,
        mut control,
    } = match prepare_run(plan_path, timeout, control, &mut preparation_cancellation).await {
        Ok(prepared) => prepared,
        Err(exit) => return exit,
    };
    let report = match execute_plan_with_resume(
        &plan,
        &mut control,
        PlanResume {
            completed: Vec::new(),
            start_index: 0,
            checkpoint: Some(&mut durable),
        },
    )
    .await
    {
        Ok(report) => report,
        Err(horizon_browser_cli::RunError::Stopped(reason)) => {
            return persist_stopped(&durable, reason, &mut finalization_cancellation).await;
        }
        Err(error) => {
            return persist_failed(&durable, error.to_string(), &mut finalization_cancellation).await;
        }
    };
    let post_process_error_exit = report.stop_reason.map_or(ExitCode::FAILURE, stop_exit_code);
    let finalization =
        run_finalization::finalize_report(&durable, &report, output_path, &mut finalization_cancellation).await;
    if let Err(error) = finalization.result {
        eprintln!("error: {error}");
        return if finalization.interrupted {
            ExitCode::from(EXIT_CANCELLED)
        } else {
            post_process_error_exit
        };
    }
    if finalization.interrupted {
        return ExitCode::from(EXIT_CANCELLED);
    }
    if let Some(reason) = report.stop_reason {
        stop_exit_code(reason)
    } else if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn resume(job_id: String, output_path: Option<&Path>, timeout: Duration, policy: UncertainPolicy) -> ExitCode {
    let (control, cancellation) = ExecutionControl::cancellable();
    let finalization_cancellation = cancellation.probe();
    let execution = resume_controlled(job_id, output_path, timeout, policy, control, finalization_cancellation);
    let interrupts = forward_interrupts(cancellation);
    tokio::pin!(execution);
    tokio::pin!(interrupts);
    tokio::select! {
        biased;
        never = &mut interrupts => match never {},
        result = &mut execution => result,
    }
}

async fn resume_controlled(
    job_id: String,
    output_path: Option<&Path>,
    timeout: Duration,
    policy: UncertainPolicy,
    mut control: ExecutionControl,
    mut finalization_cancellation: CancellationProbe,
) -> ExitCode {
    let deadline = control.start_timeout(timeout);
    let timeout_secs = timeout.as_secs();
    let deadline_millis = deadline.unix_millis();
    let prepared = match control
        .wait_owned_blocking("horizon-browser-resume-prepare", BlockingIoMode::Bound, move || {
            let (durable, plan, selection) = match DurableRun::prepare_resume(&job_id, policy) {
                Ok(prepared) => prepared,
                Err(error) => return ResumePreparation::Rejected(error),
            };
            match durable.rearm(timeout_secs, deadline_millis, selection.skipped.clone()) {
                Ok(durable) => ResumePreparation::Completed(Box::new(PreparedResume {
                    durable,
                    plan,
                    selection,
                })),
                Err(error) => {
                    let (durable, source) = error.into_parts();
                    ResumePreparation::RearmFailed {
                        durable: Box::new(durable),
                        error: source.to_string(),
                    }
                }
            }
        })
        .await
    {
        Ok(ResumePreparation::Completed(prepared)) => *prepared,
        Ok(ResumePreparation::Rejected(error)) => {
            if let Some(reason) = pending_stop_reason(&control) {
                return stop_exit_code(reason);
            }
            eprintln!("error: {error}");
            return resume_error_exit(&error);
        }
        Ok(ResumePreparation::RearmFailed { durable, error }) => {
            if let Some(reason) = pending_stop_reason(&control) {
                return persist_stopped(&durable, reason, &mut finalization_cancellation).await;
            }
            return persist_failed(&durable, error, &mut finalization_cancellation).await;
        }
        Err(BlockingIoError::Stopped(reason)) => return stop_exit_code(reason),
        Err(BlockingIoError::Failed(error)) => {
            if let Some(reason) = pending_stop_reason(&control) {
                return stop_exit_code(reason);
            }
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let PreparedResume {
        mut durable,
        plan,
        selection,
    } = prepared;
    if let Err(reason) = control.check() {
        return persist_stopped(&durable, reason, &mut finalization_cancellation).await;
    }
    let report = match execute_plan_with_resume(
        &plan,
        &mut control,
        PlanResume {
            completed: selection.completed,
            start_index: selection.start_index,
            checkpoint: Some(&mut durable),
        },
    )
    .await
    {
        Ok(report) => report,
        Err(horizon_browser_cli::RunError::Stopped(reason)) => {
            return persist_stopped(&durable, reason, &mut finalization_cancellation).await;
        }
        Err(error) => {
            return persist_failed(&durable, error.to_string(), &mut finalization_cancellation).await;
        }
    };
    let post_process_error_exit = report.stop_reason.map_or(ExitCode::FAILURE, stop_exit_code);
    let finalization =
        run_finalization::finalize_report(&durable, &report, output_path, &mut finalization_cancellation).await;
    if let Err(error) = finalization.result {
        eprintln!("error: {error}");
        return if finalization.interrupted {
            ExitCode::from(EXIT_CANCELLED)
        } else {
            post_process_error_exit
        };
    }
    if finalization.interrupted {
        return ExitCode::from(EXIT_CANCELLED);
    }
    if let Some(reason) = report.stop_reason {
        stop_exit_code(reason)
    } else if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn resume_error_exit(error: &ResumeError) -> ExitCode {
    match error {
        ResumeError::InvalidJobId(_) | ResumeError::Uncertain { .. } => ExitCode::from(2),
        _ => ExitCode::FAILURE,
    }
}

fn pending_stop_reason(control: &ExecutionControl) -> Option<ExecutionStopReason> {
    control.check().err()
}

async fn run(plan_path: PathBuf, output_path: Option<&Path>, timeout: Duration) -> ExitCode {
    let (control, cancellation) = ExecutionControl::cancellable();
    let preparation_cancellation = cancellation.probe();
    let finalization_cancellation = cancellation.probe();
    let execution = run_controlled(
        plan_path,
        output_path,
        timeout,
        control,
        preparation_cancellation,
        finalization_cancellation,
    );
    let interrupts = forward_interrupts(cancellation);
    tokio::pin!(execution);
    tokio::pin!(interrupts);
    tokio::select! {
        biased;
        never = &mut interrupts => match never {},
        result = &mut execution => result,
    }
}

async fn forward_interrupts(cancellation: CancellationHandle) -> Infallible {
    loop {
        match tokio::signal::ctrl_c().await {
            Ok(()) => cancellation.cancel(),
            Err(error) => {
                tracing::warn!(%error, "could not listen for browser-job cancellation");
                pending::<()>().await;
            }
        }
    }
}

async fn persist_stopped(
    durable: &DurableRun,
    reason: ExecutionStopReason,
    cancellation: &mut CancellationProbe,
) -> ExitCode {
    let persistence = run_finalization::persist_stop(durable, reason, cancellation).await;
    if let Err(state_error) = persistence.result {
        eprintln!(
            "error: {}; additionally could not persist stopped state: {state_error}",
            reason.message()
        );
    } else {
        eprintln!(
            "error: {}; durable job {}: {}",
            reason.message(),
            durable.job_id(),
            durable.state_path().display()
        );
    }
    if persistence.interrupted {
        ExitCode::from(EXIT_CANCELLED)
    } else {
        stop_exit_code(reason)
    }
}

async fn persist_failed(durable: &DurableRun, error: String, cancellation: &mut CancellationProbe) -> ExitCode {
    let persistence = run_finalization::persist_failure(durable, error.clone(), cancellation).await;
    if let Err(state_error) = persistence.result {
        eprintln!("error: {error}; additionally could not persist failure state: {state_error}");
    } else {
        eprintln!(
            "error: {error}; durable job {}: {}",
            durable.job_id(),
            durable.state_path().display()
        );
    }
    if persistence.interrupted {
        ExitCode::from(EXIT_CANCELLED)
    } else {
        ExitCode::FAILURE
    }
}

async fn persist_preparation_failure(error: DurablePreparationError, cancellation: &mut CancellationProbe) -> ExitCode {
    let (durable, source) = error.into_parts();
    let message = source.to_string();
    let Some(durable) = durable else {
        if cancellation.is_cancelled() {
            eprintln!("error: {}", ExecutionStopReason::Cancelled.message());
            return ExitCode::from(EXIT_CANCELLED);
        }
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    };
    let persistence = run_finalization::persist_failure(&durable, message.clone(), cancellation).await;
    if let Err(state_error) = persistence.result {
        eprintln!("error: {message}; additionally could not persist failure state: {state_error}");
    } else {
        eprintln!(
            "error: {message}; durable job {}: {}",
            durable.job_id(),
            durable.state_path().display()
        );
    }
    if persistence.interrupted {
        ExitCode::from(EXIT_CANCELLED)
    } else {
        ExitCode::FAILURE
    }
}

fn stop_exit_code(reason: ExecutionStopReason) -> ExitCode {
    ExitCode::from(match reason {
        ExecutionStopReason::Cancelled => EXIT_CANCELLED,
        ExecutionStopReason::DeadlineExceeded => EXIT_TIMED_OUT,
    })
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
        Some("resume") => parse_resume(args),
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

fn parse_resume(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let job_id = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| "resume requires a job id".to_string())?;
    if !valid_job_id(&job_id) {
        return Err(format!("invalid job id `{job_id}`"));
    }
    let mut output = None;
    let mut timeout = Duration::from_secs(DEFAULT_RUN_TIMEOUT_SECONDS);
    let mut timeout_seen = false;
    let mut on_uncertain = UncertainPolicy::Fail;
    let mut on_uncertain_seen = false;
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
            Some("--on-uncertain") if !on_uncertain_seen => {
                on_uncertain_seen = true;
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| "--on-uncertain requires fail or skip".to_string())?;
                on_uncertain = UncertainPolicy::parse(&value)?;
            }
            Some("--on-uncertain") => return Err("--on-uncertain may be specified only once".to_string()),
            Some(argument) => return Err(format!("unexpected resume argument `{argument}`")),
            None => return Err("resume argument is not valid UTF-8".to_string()),
        }
    }
    Ok(Command::Resume {
        job_id,
        output,
        timeout,
        on_uncertain,
    })
}

fn parse_run_timeout(value: Option<&OsString>) -> Result<Duration, String> {
    let seconds = value
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=MAX_RUN_TIMEOUT_SECONDS).contains(seconds))
        .ok_or_else(|| format!("--timeout requires whole seconds from 1 through {MAX_RUN_TIMEOUT_SECONDS}"))?;
    Ok(Duration::from_secs(seconds))
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
        let Command::Resume {
            job_id, on_uncertain, ..
        } = parse_args(
            [
                "resume",
                "job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4",
                "--on-uncertain",
                "skip",
            ]
            .map(OsString::from),
        )
        .expect("valid resume command")
        else {
            panic!("expected resume command");
        };
        assert_eq!(job_id, "job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4");
        assert_eq!(on_uncertain, UncertainPolicy::Skip);
        assert!(parse_args(["resume", "../job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4"].map(OsString::from)).is_err());
        assert!(
            parse_args(
                [
                    "resume",
                    "job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4",
                    "--on-uncertain",
                    "retry"
                ]
                .map(OsString::from)
            )
            .is_err()
        );
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
    fn pending_stop_reason_wins_over_a_late_resume_preparation_error() {
        let (cancelled, cancellation) = ExecutionControl::cancellable();
        cancellation.cancel();
        assert_eq!(pending_stop_reason(&cancelled), Some(ExecutionStopReason::Cancelled));

        let expired = ExecutionControl::with_timeout(Duration::ZERO);
        assert_eq!(
            pending_stop_reason(&expired),
            Some(ExecutionStopReason::DeadlineExceeded)
        );
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
