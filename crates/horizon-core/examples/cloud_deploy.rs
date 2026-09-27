//! Exercise the same deployment coordinator as the UI with private machine settings.
#![forbid(unsafe_code)]
#[path = "cloud_deploy/registry.rs"]
mod registry;
#[path = "cloud_deploy/registry_mcp.rs"]
mod registry_mcp;
use horizon_core::cloud_runtime::{
    self, Cancellation, Event,
    deployment::{self, replacement},
    repository,
    settings::Settings,
    state::Store,
};
use std::{path::PathBuf, process::ExitCode};
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> cloud_runtime::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|command| command == "registry-mcp") && args.len() == 2 {
        return registry_mcp::serve(PathBuf::from(&args[1]));
    }
    if args.first().is_some_and(|command| command.starts_with("registry")) {
        return registry::run(&args);
    }
    if args.first().is_some_and(|command| command == "offers") && (2..=3).contains(&args.len()) {
        return offers(&PathBuf::from(&args[1]), args.get(2).map(String::as_str));
    }
    if args.len() < 3 {
        return Err(cloud_runtime::Error::Invalid(
            "Usage: cloud_deploy offers SETTINGS [REQUIREMENTS_JSON] | deploy|prepare-image SETTINGS REPOSITORY PROFILE STATE_ROOT CLOUD_ID [REVISION] [--sibling ALIAS=PATH[@REVISION]]... (deploy only) | stop|resume|reconnect|endpoint|delete|idle-check|revoke-browserstack|continue-rebuild|cancel-rebuild SETTINGS STATE_ROOT | rebuild SETTINGS STATE_ROOT PROFILE | reconcile SETTINGS STATE_ROOT [WORKER_ID]. reconnect never creates a first worker; after a Hetzner resume it creates the new server on the same workspace volume.",
        ));
    }
    let settings = Settings::load(&PathBuf::from(&args[1]))?;
    let cancel = Cancellation::default();
    if args[0] == "reconcile" && (3..=4).contains(&args.len()) {
        return reconcile(&args, &settings, &cancel);
    }
    if args[0] == "delete" && args.len() == 3 {
        return deployment::terminate(&PathBuf::from(&args[2]), &settings, &cancel, &print_event);
    }
    if args[0] == "stop" && args.len() == 3 {
        cloud_runtime::lifecycle::stop(&PathBuf::from(&args[2]), &settings, &cancel)?;
        return Ok(());
    }
    if args[0] == "idle-check" && args.len() == 3 {
        return idle_check(&PathBuf::from(&args[2]), &settings, &cancel);
    }
    if args[0] == "revoke-browserstack" && args.len() == 3 {
        cloud_runtime::lifecycle::revoke_browserstack(&PathBuf::from(&args[2]), &settings, &cancel)?;
        return Ok(());
    }
    if args[0] == "resume" && args.len() == 3 {
        cloud_runtime::lifecycle::resume(&PathBuf::from(&args[2]), &settings, &cancel)?;
        // As on the cloud card, a resumed cloud is ready only after its reconnect.
        println!(
            "Resume requested. Next: cloud_deploy reconnect {} {} brings it to Ready and relaunches its sessions; for a Hetzner cloud it creates the new server on the same workspace volume.",
            args[1], args[2]
        );
        return Ok(());
    }
    if args[0] == "reconnect" && args.len() == 3 {
        deployment::reconnect(&PathBuf::from(&args[2]), settings, &cancel, &print_event)?;
        return Ok(());
    }
    if args[0] == "endpoint" && args.len() == 3 {
        return endpoint(&PathBuf::from(&args[2]), &settings, &cancel);
    }
    if matches!(args[0].as_str(), "rebuild" | "continue-rebuild" | "cancel-rebuild") {
        return rebuild(&args, settings, &cancel);
    }
    let (args, siblings) = sibling_bindings(&args)?;
    if !matches!(args[0].as_str(), "deploy" | "prepare-image")
        || !(6..=7).contains(&args.len())
        || (args[0] == "prepare-image" && !siblings.is_empty())
    {
        return Err(cloud_runtime::Error::Invalid("Invalid deployment arguments"));
    }
    let repository = PathBuf::from(&args[2]).canonicalize()?;
    let revision = repository::resolve(&repository, args.get(6).map_or("HEAD", String::as_str))?;
    let yaml = std::fs::read_to_string(repository.join(".horizon/cloud.yml"))?;
    let config = horizon_core::cloud_panel::CloudConfig::parse(&yaml)
        .map_err(|_| cloud_runtime::Error::Invalid("Invalid cloud profile file"))?;
    let profile = config
        .profiles
        .get(&args[3])
        .cloned()
        .ok_or(cloud_runtime::Error::Invalid("Profile does not exist"))?;
    let request = deployment::Request::new(
        args[5].clone(),
        repository,
        revision,
        profile,
        PathBuf::from(&args[4]),
        settings,
    );
    if args[0] == "prepare-image" {
        prepare_image(&request, &cancel, &print_event)?;
    } else {
        deployment::deploy_with_siblings(&request, &siblings, &cancel, &print_event)?;
    }
    Ok(())
}

/// Checks the recorded worker with the provider without creating, starting or deleting anything.
fn reconcile(args: &[String], settings: &Settings, cancel: &Cancellation) -> cloud_runtime::Result<()> {
    let recovered = cloud_runtime::lifecycle::reconcile(
        &PathBuf::from(&args[2]),
        settings,
        args.get(3).map(String::as_str),
        cancel,
    )?;
    println!(
        "{}",
        serde_json::to_string(&recovered.report).map_err(|_| cloud_runtime::Error::Json)?
    );
    if recovered.confirmed_stopped() {
        // As the cloud panel says it: what Resume does follows the provider.
        println!(
            "Stopped. {}",
            match recovered.stopped() {
                cloud_runtime::provider::StoppedCost::WorkerKept => "Resume starts the same worker again.",
                cloud_runtime::provider::StoppedCost::ServerDeleted => {
                    "Resume creates a new server that attaches the same workspace volume."
                }
            }
        );
    } else {
        println!("{}", recovered.report.outcome.explanation());
    }
    Ok(())
}

/// Separates `--sibling ALIAS=PATH[@REVISION]` options, in their order, from the other
/// arguments. A revision makes the deployment refuse a checkout whose `HEAD` moved from it.
fn sibling_bindings(args: &[String]) -> cloud_runtime::Result<(Vec<String>, Vec<cloud_runtime::siblings::Binding>)> {
    let mut positional = Vec::new();
    let mut siblings = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg != "--sibling" {
            positional.push(arg.clone());
            continue;
        }
        let (alias, path) = args
            .next()
            .and_then(|binding| binding.split_once('='))
            .ok_or(cloud_runtime::Error::Invalid("Use --sibling ALIAS=PATH[@REVISION]"))?;
        // Only a full commit ID after the last `@` is a revision, so paths may contain `@`.
        let (path, revision) = match path.rsplit_once('@') {
            Some((path, revision))
                if matches!(revision.len(), 40 | 64) && revision.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                (path, Some(revision.to_ascii_lowercase()))
            }
            _ => (path, None),
        };
        let local_repository = PathBuf::from(path).canonicalize().map_err(|error| {
            eprintln!("--sibling {alias}={path}: {error}");
            cloud_runtime::Error::Invalid("A --sibling checkout does not exist")
        })?;
        siblings.push(cloud_runtime::siblings::Binding {
            alias: alias.to_owned(),
            local_repository,
            revision,
        });
    }
    Ok((positional, siblings))
}

/// Prefixes each line with the seconds since this command started, for timing a deployment.
fn print_event(event: Event) {
    static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let at = STARTED.get_or_init(std::time::Instant::now).elapsed().as_secs_f64();
    match event {
        Event::Stage(stage, _) => println!("[{at:7.1}s] Stage: {}", stage.label()),
        Event::Progress(progress) => println!(
            "[{at:7.1}s] Progress: {} completed={} total={:?}",
            progress.detail, progress.completed, progress.total
        ),
        // One output event can carry a whole chunk of command output.
        Event::Output(chunk) => {
            for line in chunk.lines() {
                println!("[{at:7.1}s] {line}");
            }
        }
        Event::Ready(state, _) => {
            if let Some(worker) = &state.worker {
                println!(
                    "[{at:7.1}s] Ready: worker={} rate_per_hour={:?}",
                    worker.id, worker.cost_per_hr
                );
            }
            if let Some(timeline) = &state.timeline {
                for (phase, duration) in timeline.phases() {
                    println!(
                        "[{at:7.1}s]   {:>7.1}s  {}",
                        duration.as_secs_f64(),
                        timeline.label(phase)
                    );
                }
            }
        }
        _ => {}
    }
}

/// Rebuilds, continues or cancels an image replacement of the cloud saved under `STATE_ROOT`.
fn rebuild(args: &[String], settings: Settings, cancel: &Cancellation) -> cloud_runtime::Result<()> {
    let root = PathBuf::from(&args[2]);
    let state = Store::lock(&root)?
        .load()?
        .ok_or(cloud_runtime::Error::Invalid("No cloud deployment"))?;
    let request = deployment::Request::new(
        state.cloud_id,
        state.repository,
        state.revision,
        state.profile,
        root,
        settings,
    );
    match (args[0].as_str(), args.get(3)) {
        ("rebuild", Some(profile)) if args.len() == 4 => replacement::rebuild(&request, profile, cancel, &print_event),
        ("continue-rebuild", None) => replacement::continue_replacement(&request, cancel, &print_event),
        ("cancel-rebuild", None) => replacement::cancel_replacement(&request, cancel, &print_event),
        _ => Err(cloud_runtime::Error::Invalid("Invalid rebuild arguments")),
    }?;
    Ok(())
}

fn prepare_image(
    request: &deployment::Request,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> cloud_runtime::Result<()> {
    let mut registry = cloud_runtime::registry::Prepared::for_image(
        &request.settings,
        &request.profile.image,
        Some(&request.repository),
        request.profile.build.is_some(),
    )?;
    let git_auth =
        cloud_runtime::git_auth::Prepared::for_repository(&request.settings.git_credentials, &request.repository)?;
    let root = tempfile::tempdir()?;
    let runner = cloud_runtime::command::Runner {
        cancel,
        emit,
        secrets: registry
            .as_ref()
            .map_or_else(Vec::new, cloud_runtime::registry::Prepared::redactions),
    };
    repository::validate_tree(&request.repository, &request.revision, &runner)?;
    let snapshot = repository::snapshot(&request.repository, &request.revision, root.path(), &runner)?;
    let images = cloud_runtime::image::Images {
        docker_host: request.settings.docker_host.as_deref(),
        isolated_registry: registry.is_some(),
        docker_config: registry
            .as_ref()
            .map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(request.profile.build.is_some())
            }),
        runner: &runner,
    };
    let digest = images.prepare(&request.profile, &snapshot, &request.cloud_id)?;
    if git_auth.is_some() {
        images.validate_contract(&digest, &request.cloud_id, &request.profile, true)?;
    }
    if let Some(registry) = &mut registry {
        registry.verify_image(&digest, cancel)?;
    }
    println!("Prepared: {digest}");
    Ok(())
}

/// Ranks offers the way agents' `cloud_offers` does, from prices fetched now: `RunPod`'s
/// in `offers`, and each other configured provider in `other_providers`, in its own
/// currency.
fn offers(settings: &std::path::Path, requirements: Option<&str>) -> cloud_runtime::Result<()> {
    use horizon_cloud::offers::{Requirements, hetzner_section, offers};
    let requirements: Requirements = serde_json::from_str(requirements.unwrap_or("{}"))
        .map_err(|_| cloud_runtime::Error::Invalid("Invalid offer requirements"))?;
    requirements.validate().map_err(cloud_runtime::Error::Invalid)?;
    let settings = Settings::load(settings)?;
    let cancel = Cancellation::default();
    // A machine set up for Hetzner alone has no RunPod offers, and says why.
    let runpod = if settings.runpod_configured() {
        Some(cloud_runtime::prices::price_list(&settings, &cancel)?)
    } else {
        None
    };
    // A Hetzner failure is reported beside RunPod's offers, as agents' answers do.
    let other_providers: Vec<serde_json::Value> = match cloud_runtime::prices::hetzner_catalog(&settings, &cancel) {
        Ok(catalog) => catalog
            .map(|catalog| hetzner_section(&catalog, &requirements))
            .into_iter()
            .collect(),
        Err(error) => vec![serde_json::json!({"provider": "Hetzner", "error": error.to_string()})],
    };
    let answer = match &runpod {
        Some((list, preferences)) => serde_json::json!({
            "provider": list.provider,
            "offers": offers(list, preferences, &requirements),
            "other_providers": other_providers,
        }),
        None => serde_json::json!({
            "provider": "RunPod",
            "unavailable": cloud_runtime::settings::RUNPOD_KEY_MISSING,
            "offers": [],
            "other_providers": other_providers,
        }),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&answer).map_err(|_| cloud_runtime::Error::Json)?
    );
    Ok(())
}

/// Prints the running worker's SSH endpoint as the provider reports it now, with the
/// host key Horizon pinned, so external tools connect exactly as Horizon does.
fn endpoint(root: &std::path::Path, settings: &Settings, cancel: &Cancellation) -> cloud_runtime::Result<()> {
    let connection = cloud_runtime::lifecycle::endpoint(root, settings, cancel)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "host": connection.host,
            "port": connection.port,
            "user": cloud_runtime::ssh::USER,
            "host_key_alias": connection.host_key_alias,
            "known_hosts": std::path::absolute(&connection.known_hosts)?,
            "identity_file": connection.identity,
        }))
        .map_err(|_| cloud_runtime::Error::Json)?
    );
    Ok(())
}

/// Checks a Hetzner cloud's idle record once, stopping it after its whole idle period.
fn idle_check(root: &std::path::Path, settings: &Settings, cancel: &Cancellation) -> cloud_runtime::Result<()> {
    use cloud_runtime::lifecycle::IdleCheck;
    println!(
        "{}",
        match cloud_runtime::lifecycle::idle_check(root, settings, cancel)? {
            IdleCheck::NotWatched => "Not watched: no idle period, or not a running Hetzner cloud".to_owned(),
            IdleCheck::Active { idle, limit } => format!("Active: idle {}s of {}s", idle.as_secs(), limit.as_secs()),
            IdleCheck::Stopped { idle } => format!(
                "Stopped after {}s idle. Resume creates a new server that attaches the same workspace volume.",
                idle.as_secs()
            ),
        }
    );
    Ok(())
}
