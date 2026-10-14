//! Exercise the same deployment coordinator as the UI with private machine settings.
#![forbid(unsafe_code)]
#[path = "cloud_deploy/choice.rs"]
mod choice;
#[path = "cloud_deploy/registry.rs"]
mod registry;
#[path = "cloud_deploy/registry_mcp.rs"]
mod registry_mcp;
use horizon_core::cloud_runtime::{
    self, Cancellation, Event,
    deployment::{self, replacement},
    github::Prompt,
    repository::{self, uncommitted::Uncommitted},
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
            "Usage: cloud_deploy offers SETTINGS [REQUIREMENTS_JSON] | plan-deploy|deploy|prepare-image SETTINGS REPOSITORY PROFILE STATE_ROOT CLOUD_ID [REVISION] [--quick-start] [--worker-choice OFFER_JSON_FILE] [--plan-token PLAN_TOKEN] (deploy and prepare-image) [--sibling ALIAS=PATH[@REVISION]]... (deploy only) | stop|resume|reconnect|endpoint|delete|idle-check|revoke-browserstack|continue-rebuild|cancel-rebuild SETTINGS STATE_ROOT | plan-rebuild|rebuild SETTINGS STATE_ROOT PROFILE [PLAN_TOKEN] (rebuild only) | reconcile SETTINGS STATE_ROOT [WORKER_ID]. reconnect never creates a first worker; after a Hetzner resume it creates the new server on the same workspace volume. --quick-start uses the built-in quick-start profile on the public base image for a commit without .horizon/cloud.yml. On the checked-out commit the image builds from the working copy, as in New cloud: plan-deploy lists its uncommitted files and prints the token that deploy and prepare-image then need.",
        ));
    }
    let mut settings = Settings::load(&PathBuf::from(&args[1]))?;
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
    if matches!(
        args[0].as_str(),
        "plan-rebuild" | "rebuild" | "continue-rebuild" | "cancel-rebuild"
    ) {
        return rebuild(&args, settings, &cancel);
    }
    let (args, worker_choice) = choice::arguments(&args)?;
    let (args, siblings) = sibling_bindings(&args)?;
    let (args, token) = plan_token(&args)?;
    let quick_start = args.iter().any(|arg| arg == "--quick-start");
    let args: Vec<_> = args.into_iter().filter(|arg| arg != "--quick-start").collect();
    if !matches!(args[0].as_str(), "plan-deploy" | "deploy" | "prepare-image")
        || !(6..=7).contains(&args.len())
        || ((args[0] != "deploy" || quick_start) && !siblings.is_empty())
        || (args[0] == "plan-deploy" && token.is_some())
    {
        return Err(cloud_runtime::Error::Invalid("Invalid deployment arguments"));
    }
    let prepared = launch_config(&args[2], args.get(6), quick_start, &cancel)?;
    let profile = prepared
        .config
        .profiles
        .get(&args[3])
        .cloned()
        .ok_or(cloud_runtime::Error::Invalid("Profile does not exist"))?;
    // The plan token covers the profile as it is deployed, with the worker choice.
    let profile = if let Some(path) = worker_choice {
        choice::apply(&profile, &mut settings, &path, &cancel)?
    } else {
        profile
    };
    let files = uncommitted_files(&prepared, &profile)?;
    if args[0] == "plan-deploy" {
        print_plan(&prepared.notes, &profile, &settings, files.as_ref());
        return Ok(());
    }
    let working_copy = confirm(&profile, &settings, files, token.as_deref())?;
    let mut request = deployment::Request::new(
        args[5].clone(),
        prepared.repository,
        prepared.revision,
        profile,
        PathBuf::from(&args[4]),
        settings,
    );
    request.working_copy = working_copy;
    if args[0] == "prepare-image" {
        prepare_image(&request, &cancel, &print_event)?;
    } else {
        deployment::deploy_with_siblings(&request, &siblings, &cancel, &print_event)?;
    }
    Ok(())
}

/// The repository, revision and configuration a `deploy` or `prepare-image` launches, read
/// as New cloud reads them: on the checked-out commit the working copy's `.horizon/cloud.yml`
/// with the commit's source and companions, on another revision that commit's file, or
/// with `--quick-start` the built-in profile, which New cloud refuses as well for a commit
/// with its own configuration.
fn launch_config(
    directory: &str,
    revision: Option<&String>,
    quick_start: bool,
    cancel: &Cancellation,
) -> cloud_runtime::Result<repository::launch::Prepared> {
    let configuration = if quick_start {
        repository::launch::Configuration::QuickStart
    } else {
        repository::launch::Configuration::WorkingCopy
    };
    let prepared = repository::launch::prepare_with_configuration(
        directory,
        revision.map_or("HEAD", String::as_str),
        configuration,
        &cloud_runtime::command::Runner {
            cancel,
            emit: &print_event,
            secrets: Vec::new(),
        },
    )?;
    Ok(prepared)
}

/// The uncommitted files that the image of `profile` takes on the checked-out commit, as
/// New cloud reads them; `None` when it builds from the commit alone.
fn uncommitted_files(
    prepared: &repository::launch::Prepared,
    profile: &horizon_core::cloud_panel::Profile,
) -> cloud_runtime::Result<Option<Uncommitted>> {
    match (prepared.working_copy, &profile.build) {
        (true, Some(build)) => {
            repository::uncommitted::collect(&prepared.repository, &prepared.revision, build).map(Some)
        }
        _ => Ok(None),
    }
}

/// The plan token of a deployment: the profile, the uncommitted files and the worker
/// choice. An offer sets the place and GPU types in `settings` without changing the
/// profile, so another offer of the same size must not reuse the token.
fn deploy_token(
    profile: &horizon_core::cloud_panel::Profile,
    settings: &Settings,
    files: Option<&Uncommitted>,
) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let choice = serde_json::json!({
        "placement": settings.placement,
        "data_centers": settings.data_centers,
        "gpu_types": settings.gpu_types,
    });
    let mut hasher = sha2::Sha256::new();
    hasher.update(repository::uncommitted::token(
        profile,
        files.unwrap_or(&Uncommitted::default()),
    ));
    hasher.update([0]);
    hasher.update(choice.to_string());
    hasher.finalize().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// What `plan-deploy` prints: the notes, each uncommitted file and the plan token.
fn print_plan(
    notes: &[String],
    profile: &horizon_core::cloud_panel::Profile,
    settings: &Settings,
    files: Option<&Uncommitted>,
) {
    for note in notes {
        println!("{note}");
    }
    // An image can be pushed to a public registry, so each file is listed before a
    // person confirms the whole plan with its token.
    for file in files.map(Uncommitted::files).unwrap_or_default() {
        println!("Uncommitted file: {} {}", file.change.label(), file.path);
    }
    let plan = deploy_token(profile, settings, files);
    println!("To deploy with exactly these settings and files, pass --plan-token {plan}");
}

/// The uncommitted files that `token`, from `plan-deploy`, confirms. Without a token, only
/// a plan without uncommitted files may run.
fn confirm(
    profile: &horizon_core::cloud_panel::Profile,
    settings: &Settings,
    files: Option<Uncommitted>,
    token: Option<&str>,
) -> cloud_runtime::Result<Option<repository::uncommitted::Confirmed>> {
    let plan = deploy_token(profile, settings, files.as_ref());
    let confirmed = token.map_or_else(
        || files.as_ref().is_none_or(Uncommitted::is_empty),
        |token| token == plan,
    );
    if !confirmed {
        return Err(cloud_runtime::Error::Invalid(
            "The checkout differs from the printed plan, or has uncommitted files in the image's build context and no token was given. Run plan-deploy, check what it prints, and pass the token it prints",
        ));
    }
    Ok(files.map(|files| repository::uncommitted::Confirmed { hash: files.hash() }))
}

/// Separates `--plan-token TOKEN`, the token `plan-deploy` printed, from the other arguments.
fn plan_token(args: &[String]) -> cloud_runtime::Result<(Vec<String>, Option<String>)> {
    let mut positional = Vec::new();
    let mut token = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg != "--plan-token" {
            positional.push(arg.clone());
            continue;
        }
        let value = args
            .next()
            .ok_or(cloud_runtime::Error::Invalid("Use --plan-token PLAN_TOKEN"))?;
        if token.replace(value.clone()).is_some() {
            return Err(cloud_runtime::Error::Invalid("Give --plan-token once"));
        }
    }
    Ok((positional, token))
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
        // The deployment waits for this sign-in, so the person needs its code or page here.
        Event::GitHub(prompt) => match prompt {
            Prompt::Device {
                user_code,
                verification_uri,
                ..
            } => println!("[{at:7.1}s] GitHub: enter {user_code} at {verification_uri}"),
            Prompt::Web { url } => {
                println!("[{at:7.1}s] GitHub: approve this cloud at {url}");
                if let Err(error) = horizon_core::open_url(&url) {
                    println!("[{at:7.1}s] GitHub: could not open the page: {error}");
                }
            }
            Prompt::Connected {
                login, repositories, ..
            } => println!(
                "[{at:7.1}s] GitHub: signed in as {login} for {}",
                repositories.join(", ")
            ),
            Prompt::Ended { reason, .. } => println!("[{at:7.1}s] GitHub: {reason}"),
            Prompt::Publish {
                user_code,
                verification_uri,
                ..
            } => println!("[{at:7.1}s] GitHub: to publish the image, enter {user_code} at {verification_uri}"),
            Prompt::Published { allowed } => {
                println!(
                    "[{at:7.1}s] GitHub: image publishing {}",
                    if allowed { "allowed" } else { "not allowed" }
                );
            }
        },
        _ => {}
    }
}

/// Lists what a rebuild would change, or rebuilds, continues or cancels an image
/// replacement of the cloud saved under `STATE_ROOT`.
fn rebuild(args: &[String], settings: Settings, cancel: &Cancellation) -> cloud_runtime::Result<()> {
    let root = PathBuf::from(&args[2]);
    let state = Store::lock(&root)?
        .load()?
        .ok_or(cloud_runtime::Error::Invalid("No cloud deployment"))?;
    if let ("plan-rebuild", Some(profile), 4) = (args[0].as_str(), args.get(3), args.len()) {
        let plan = replacement::plan(&state.repository, &state.profile, profile, cancel)?;
        for note in &plan.notes {
            println!("{note}");
        }
        if plan.changes.is_empty() {
            println!("The rebuild keeps this cloud's settings");
        }
        for change in &plan.changes {
            println!("{}{change}", replacement::PROFILE_CHANGE);
        }
        // An image can be pushed to a public registry, so each file is listed before a
        // person confirms the whole plan with its token.
        for file in plan.uncommitted.files() {
            println!("Uncommitted file: {} {}", file.change.label(), file.path);
        }
        println!(
            "To rebuild with exactly these settings and files, pass {} to rebuild",
            plan.token()
        );
        return Ok(());
    }
    let request = deployment::Request::new(
        state.cloud_id,
        state.repository,
        state.revision,
        state.profile,
        root,
        settings,
    );
    match (args[0].as_str(), args.get(3)) {
        ("rebuild", Some(profile)) if matches!(args.len(), 4 | 5) => {
            // The token confirms what plan-rebuild printed: the settings and the uncommitted
            // files. Without one, only a plan without uncommitted files may run.
            let plan = replacement::plan(&request.repository, &request.profile, profile, cancel)?;
            let confirmed = args
                .get(4)
                .map_or(plan.uncommitted.is_empty(), |token| *token == plan.token());
            if !confirmed {
                return Err(cloud_runtime::Error::Invalid(
                    "The checkout differs from the printed plan, or has uncommitted files and no token was given. Run plan-rebuild, check what it prints, and pass the token it prints",
                ));
            }
            replacement::rebuild(&request, profile, Some(&plan), cancel, &print_event)
        }
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
    if let Some(registry) = &registry {
        registry.preflight(&runner)?;
    }
    repository::validate_tree(&request.repository, &request.revision, &runner)?;
    let snapshot = repository::snapshot(&request.repository, &request.revision, root.path(), &runner)?;
    // As a deployment builds it: the confirmed uncommitted files laid over the commit.
    let uncommitted = request.working_copy.as_ref().and_then(|confirmed| {
        confirmed
            .hash
            .clone()
            .map(|hash| cloud_runtime::state::UncommittedImage {
                revision: request.revision.clone(),
                hash,
            })
    });
    if let (Some(confirmed), Some(build)) = (&request.working_copy, &request.profile.build) {
        repository::uncommitted::overlay(
            &request.repository,
            &request.revision,
            build,
            confirmed.hash.as_deref(),
            &snapshot,
        )
        .map_err(|error| {
            if repository::uncommitted::changed(&error) {
                cloud_runtime::Error::Invalid(
                    "The uncommitted files in the image's build context changed after plan-deploy printed them. Run plan-deploy again and pass the token it prints",
                )
            } else {
                error
            }
        })?;
    }
    let images = cloud_runtime::image::Images {
        docker_host: request.settings.docker_host.as_deref(),
        isolated_registry: registry.is_some(),
        docker_config: registry
            .as_ref()
            .map_or(request.settings.docker_config.as_path(), |registry| {
                registry.docker_config(request.profile.build.is_some())
            }),
        runner: &runner,
        publisher: registry.is_none().then_some(cloud_runtime::github::publish::Publisher {
            docker_config: &request.settings.docker_config,
            cloud_id: &request.cloud_id,
        }),
    };
    let context = cloud_runtime::image::Context {
        source: &snapshot,
        uncommitted: uncommitted.as_ref(),
    };
    let tag = cloud_runtime::image::default_tag(&request.cloud_id);
    let digest = images.prepare_layered(&request.profile, context, &[], &request.cloud_id, &tag, false)?;
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
    let runpod = settings
        .runpod_configured()
        .then(|| cloud_runtime::prices::price_list(&settings, &cancel));
    // A Hetzner failure is reported beside RunPod's offers, as agents' answers do.
    let other_providers: Vec<serde_json::Value> = match if requirements.gpu {
        Ok(None)
    } else {
        cloud_runtime::prices::hetzner_catalog(&settings, &cancel)
    } {
        Ok(catalog) => catalog
            .map(|catalog| hetzner_section(&catalog, &requirements))
            .into_iter()
            .collect(),
        Err(error) => vec![serde_json::json!({"provider": "Hetzner", "error": error.to_string()})],
    };
    let mut answer = match &runpod {
        Some(Ok((list, preferences))) => serde_json::json!({
            "provider": list.provider,
            "offers": offers(list, preferences, &requirements),
            "other_providers": other_providers,
        }),
        Some(Err(error)) => {
            serde_json::json!({"provider":"RunPod", "error": error.to_string(), "offers":[], "other_providers":other_providers})
        }
        None => serde_json::json!({
            "provider": "RunPod",
            "unavailable": cloud_runtime::settings::RUNPOD_KEY_MISSING,
            "offers": [],
            "other_providers": other_providers,
        }),
    };
    horizon_cloud::offers::comparison::fetch_append(&mut answer);
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
            "proxy_command": connection.proxy_command,
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
