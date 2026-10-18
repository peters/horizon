//! A standalone host for configured remote targets.
//!
//! `horizon-browser mcp --standalone --config <path>` serves the MCP contract
//! over stdio for the remote targets of one host file, without Horizon: a
//! server or CI host can open, drive and release remote device sessions. The
//! MCP server and the host run in this process. The host claims only the
//! requests of this process's own agent identity and host instance, starts
//! no local browser, and releases every session when MCP stdin closes or the
//! process gets an interrupt or a termination signal. A host that was killed
//! leaves its holds on disk; the next host on the same runtime root checks
//! each of them at the original provider.

mod config;
mod holds;
mod host;
mod usage;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use horizon_browser_control::{BrowserRuntimePaths, manifest};

pub use config::HostConfig;

use super::StandaloneError;

/// Environment variable that names the host file when `--config` is absent.
/// It holds a path, never a credential value.
pub const CONFIG_ENV: &str = "HORIZON_BROWSER_CONFIG";
const TICK: Duration = Duration::from_millis(50);

/// Serve the MCP contract for the remote targets in the host file at `path`
/// until MCP stdin closes or the process is interrupted or terminated.
///
/// # Errors
/// The host file is invalid, the host cannot start, MCP fails, or a remote
/// session could not be released before the exit deadline.
pub async fn serve(path: &Path) -> Result<(), StandaloneError> {
    let config = HostConfig::load(path).map_err(|error| StandaloneError::Startup(error.to_string()))?;
    let actor = format!("horizon:standalone-{}", std::process::id());
    let root = BrowserRuntimePaths::resolve().root().to_path_buf();
    let host = host::RemoteHost::open(config, actor.clone(), root)
        .map_err(|error| StandaloneError::Startup(format!("could not open the remote hold folder: {error}")))?;
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::clone(&stop);
    let worker = std::thread::Builder::new()
        .name("standalone-remote-host".into())
        .spawn(move || {
            let mut host = host;
            while !running.load(Ordering::Acquire) {
                host.tick();
                std::thread::sleep(TICK);
            }
            host.shutdown()
        })
        .map_err(|error| StandaloneError::Startup(format!("could not start the remote host: {error}")))?;
    let mcp = horizon_browser_mcp::serve_stdio_for_host(&actor, manifest::host_instance());
    let result = tokio::select! {
        result = mcp => result.map_err(StandaloneError::from),
        () = super::wait_for_interrupt() => Ok(()),
    };
    stop.store(true, Ordering::Release);
    let released = tokio::task::spawn_blocking(move || worker.join().unwrap_or(false))
        .await
        .unwrap_or(false);
    result?;
    if released {
        Ok(())
    } else {
        Err(StandaloneError::Release)
    }
}

#[cfg(test)]
mod tests;
