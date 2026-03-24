use std::path::PathBuf;
use std::sync::Arc;

use horizon_core::ManagedInstall;
use surge_core::context::{Context as SurgeContext, StorageProvider};
use surge_core::update::manager::UpdateManager;

struct Args {
    app_exe: PathBuf,
    apply: bool,
}

fn usage() -> &'static str {
    "Usage: cargo run -p horizon-ui --example surge-update-smoke -- --app-exe <path> [--apply]"
}

fn parse_args() -> Result<Args, String> {
    let mut app_exe = None;
    let mut apply = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--app-exe" => {
                let value = args.next().ok_or_else(|| "--app-exe requires a path".to_string())?;
                app_exe = Some(PathBuf::from(value));
            }
            "--apply" => apply = true,
            "-h" | "--help" => return Err(usage().to_string()),
            other => return Err(format!("unknown argument: {other}\n{}", usage())),
        }
    }

    let app_exe = app_exe.ok_or_else(|| format!("--app-exe is required\n{}", usage()))?;
    Ok(Args { app_exe, apply })
}

fn parse_storage_provider(raw: &str) -> Result<StorageProvider, String> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "filesystem" | "fs" => Ok(StorageProvider::Filesystem),
        "github" | "github_releases" | "githubreleases" => Ok(StorageProvider::GitHubReleases),
        other => Err(format!("unsupported storage provider for smoke helper: {other}")),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let managed_install = ManagedInstall::discover(&args.app_exe)
        .ok_or_else(|| format!("failed to discover a managed install from '{}'", args.app_exe.display()))?;
    let provider = parse_storage_provider(&managed_install.provider)?;
    let ctx = Arc::new(SurgeContext::new());
    ctx.set_storage(
        provider,
        &managed_install.bucket,
        &managed_install.region,
        "",
        "",
        &managed_install.endpoint,
    );

    let install_root = managed_install.install_root.to_string_lossy().into_owned();
    let mut manager = UpdateManager::new(
        Arc::clone(&ctx),
        &managed_install.app_id,
        &managed_install.version,
        &managed_install.channel,
        &install_root,
    )?;

    match manager.check_for_updates().await? {
        Some(info) => {
            println!(
                "update available: current={} latest={} channel={}",
                managed_install.version, info.latest_version, managed_install.channel
            );
            if args.apply {
                manager.download_and_apply(&info, None::<fn(_)>).await?;
                println!("update applied: {}", info.latest_version);
            }
        }
        None => {
            println!(
                "no update available: current={} channel={}",
                managed_install.version, managed_install.channel
            );
        }
    }

    Ok(())
}
