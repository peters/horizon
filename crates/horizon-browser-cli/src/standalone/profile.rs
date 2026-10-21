//! A profile host: named persistent browser profiles for `--connect` clients.
//!
//! `horizon-browser host --config <path> [--visible]` keeps one persistent
//! Chromium or Firefox profile for each entry of `browser.profiles` in the
//! host file, under `<root>/browser-profiles/profile-<name>`. It runs no MCP
//! server itself. MCP clients start `horizon-browser mcp --connect` with
//! `HORIZON_BROWSER_ACTOR` (a `horizon:` identity) and
//! `HORIZON_BROWSER_HOST_INSTANCE` (the instance in `profile-host.json`), and
//! call `browser_create` with a profile name as the target. The host opens
//! each page in the shared browser of its profile, so the pages of one
//! profile share its cookies, and it stamps each page for the actor that
//! opened it only: each client lists and controls only its own pages.
//!
//! The host keeps one stable host instance for its runtime root in
//! `profile-host.json` (mode 600), and only one host runs on a root. A page
//! with no action for 60 minutes closes, and at most 16 pages run. An
//! interrupt or a termination signal closes every page and keeps every
//! profile: the host never deletes a profile.

mod config;
mod host;
mod instance;
mod rules;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use horizon_browser_control::BrowserRuntimePaths;

pub use config::{Pref, Profile, ProfileConfig};
pub use instance::{RECORD, Record};

use super::StandaloneError;

const TICK: Duration = Duration::from_millis(50);

/// Serve the profiles of the host file at `path` until an interrupt or a
/// termination signal. `headed` shows each page as a window on the display.
///
/// # Errors
/// The host file is invalid, another host runs on this root, the host
/// instance cannot be kept, or a browser did not stop before the deadline.
pub async fn serve(path: &Path, headed: bool) -> Result<(), StandaloneError> {
    let config = ProfileConfig::load(path).map_err(|error| StandaloneError::Startup(error.to_string()))?;
    let root = BrowserRuntimePaths::resolve().root().to_path_buf();
    let claim = instance::Claim::take(&root).map_err(|error| StandaloneError::Startup(error.to_string()))?;
    horizon_browser_control::manifest::configure_host_instance(claim.host_instance())
        .map_err(|error| StandaloneError::Startup(format!("could not use the host instance: {error}")))?;
    let names = config.profiles.len();
    let host = host::ProfileHost::open(
        config,
        claim.host_instance().to_owned(),
        root.clone(),
        headed,
        host::IDLE_CLOSE,
    )
    .map_err(|error| StandaloneError::Startup(format!("could not prepare the profiles: {error}")))?;
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::clone(&stop);
    let worker = std::thread::Builder::new()
        .name("standalone-profile-host".into())
        .spawn(move || {
            let mut host = host;
            while !running.load(Ordering::Acquire) {
                host.tick();
                std::thread::sleep(TICK);
            }
            host.shutdown()
        })
        .map_err(|error| StandaloneError::Startup(format!("could not start the profile host: {error}")))?;
    // A client can stop the host as soon as it sees the record, so listen first.
    let interrupted = listen();
    let ready = claim.ready(&root);
    if let Err(error) = &ready {
        tracing::error!(%error, "could not write the host record");
    } else {
        tracing::info!(profiles = names, root = %root.display(), "profile host ready");
    }
    if ready.is_ok() {
        interrupted.await;
    }
    stop.store(true, Ordering::Release);
    let stopped = tokio::task::spawn_blocking(move || worker.join().unwrap_or(false))
        .await
        .unwrap_or(false);
    drop(claim);
    ready.map_err(|error| StandaloneError::Startup(format!("could not write {RECORD}: {error}")))?;
    if stopped {
        Ok(())
    } else {
        Err(StandaloneError::Shutdown)
    }
}

/// Wait for an interrupt or a termination signal. On Unix the host listens
/// from this call on, not from the first poll of the result.
#[cfg(unix)]
fn listen() -> impl Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};
    let signals = signal(SignalKind::interrupt())
        .and_then(|interrupt| signal(SignalKind::terminate()).map(|terminate| (interrupt, terminate)));
    async move {
        match signals {
            Ok((mut interrupt, mut terminate)) => {
                tokio::select! {
                    _ = interrupt.recv() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => super::wait_for_interrupt().await,
        }
    }
}

#[cfg(not(unix))]
fn listen() -> impl Future<Output = ()> {
    super::wait_for_interrupt()
}

#[cfg(test)]
mod tests;
