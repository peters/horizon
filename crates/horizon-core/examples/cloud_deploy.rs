//! Exercise the same deployment coordinator as the UI with private machine settings.
#![forbid(unsafe_code)]
use horizon_core::cloud_runtime::{self, Cancellation, Event, deployment, repository, settings::Settings};
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
    if args.len() < 3 {
        return Err(cloud_runtime::Error::Invalid(
            "Usage: cloud_deploy deploy|prepare-image SETTINGS REPOSITORY PROFILE STATE_ROOT CLOUD_ID [REVISION] | stop|resume|delete|revoke-browserstack SETTINGS STATE_ROOT | reconcile SETTINGS STATE_ROOT [WORKER_ID]",
        ));
    }
    let settings = Settings::load(&PathBuf::from(&args[1]))?;
    let cancel = Cancellation::default();
    if args[0] == "reconcile" && (3..=4).contains(&args.len()) {
        let recovered = cloud_runtime::lifecycle::reconcile(
            &PathBuf::from(&args[2]),
            &settings,
            args.get(3).map(String::as_str),
            &cancel,
        )?;
        println!(
            "{}",
            serde_json::to_string(&recovered.report).map_err(|_| cloud_runtime::Error::Json)?
        );
        println!("{}", recovered.report.outcome.explanation());
        return Ok(());
    }
    if args[0] == "delete" && args.len() == 3 {
        return deployment::terminate(&PathBuf::from(&args[2]), &settings, &cancel);
    }
    if args[0] == "stop" && args.len() == 3 {
        cloud_runtime::lifecycle::stop(&PathBuf::from(&args[2]), &settings, &cancel)?;
        return Ok(());
    }
    if args[0] == "revoke-browserstack" && args.len() == 3 {
        cloud_runtime::lifecycle::revoke_browserstack(&PathBuf::from(&args[2]), &settings, &cancel)?;
        return Ok(());
    }
    if args[0] == "resume" && args.len() == 3 {
        return cloud_runtime::lifecycle::resume(&PathBuf::from(&args[2]), &settings, &cancel);
    }
    if !matches!(args[0].as_str(), "deploy" | "prepare-image") || !(6..=7).contains(&args.len()) {
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
    let request = deployment::Request {
        cloud_id: args[5].clone(),
        repository,
        revision,
        profile,
        state_root: PathBuf::from(&args[4]),
        settings,
    };
    let emit = |event| match event {
        Event::Stage(stage, _) => println!("Stage: {}", stage.label()),
        Event::Progress(progress) => println!(
            "Progress: {} completed={} total={:?}",
            progress.detail, progress.completed, progress.total
        ),
        Event::Output(line) => println!("{line}"),
        Event::Ready(state, _) => {
            if let Some(worker) = state.worker {
                println!("Ready: worker={} rate_per_hour={:?}", worker.id, worker.cost_per_hr);
            }
        }
        _ => {}
    };
    if args[0] == "prepare-image" {
        let git_auth =
            cloud_runtime::git_auth::Prepared::for_repository(&request.settings.git_credentials, &request.repository)?;
        let root = tempfile::tempdir()?;
        let runner = cloud_runtime::command::Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        repository::validate_tree(&request.repository, &request.revision, &runner)?;
        let snapshot = repository::snapshot(&request.repository, &request.revision, root.path(), &runner)?;
        let images = cloud_runtime::image::Images {
            docker_host: request.settings.docker_host.as_deref(),
            docker_config: &request.settings.docker_config,
            runner: &runner,
        };
        let digest = images.prepare(&request.profile, &snapshot, &request.cloud_id)?;
        if git_auth.is_some() {
            images.validate_contract(&digest, &request.cloud_id, &request.profile.capabilities, true)?;
        }
        println!("Prepared: {digest}");
    } else {
        deployment::deploy(&request, &cancel, &emit)?;
    }
    Ok(())
}
