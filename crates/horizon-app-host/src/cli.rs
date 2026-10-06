//! CLI adapter delegates all build, quota, action and cleanup decisions to the shared runner.
use crate::{
    Error, Result,
    archive::Archive,
    bootstrap::Host,
    runner::{self, Control},
};
use std::{sync::Arc, time::Duration};
mod output;
struct Cancel(Arc<Control>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// # Errors
/// Progress is NDJSON on stderr; stdout contains only the terminal retained report.
pub async fn execute(host: Host, lifetime: Duration) -> Result<()> {
    host.actor.arm()?;
    let views = crate::view::Registry::new(&host.actor);
    let control = Arc::new(Control::new(lifetime)?);
    let _cancel = Cancel(Arc::clone(&control));
    let retained = Arc::clone(&control);
    let progress = output::Output::new(std::io::stderr())?;
    let archive = Arc::new(Archive::new(&host.reports)?);
    let retained_archive = Arc::clone(&archive);
    #[cfg(unix)]
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| Error::Unavailable)?;
    let mut task = tokio::task::spawn_blocking(move || {
        let report = runner::run(
            &host.actor,
            &retained,
            |session, kind, bytes| retained_archive.capture(session, kind, bytes),
            |mut event| {
                if event.phase == "session_created"
                    && let Some(session) = event.session
                {
                    event.view = Some(views.open(session)?);
                }
                let mut bytes = serde_json::to_vec(&event).map_err(|_| Error::Cancelled)?;
                if bytes.len() > 8192 {
                    return Err(Error::Cancelled);
                }
                bytes.push(b'\n');
                progress.send(bytes, &retained)
            },
        );
        let failed = report.as_ref().map_or(true, |report| {
            report.cancelled
                || !report.upload_cleanup_errors.is_empty()
                || report.builds.iter().any(|build| build.error.is_some())
                || report.devices.iter().any(|device| {
                    device.error.is_some()
                        || !device.cleanup_confirmed
                        || device.steps.iter().any(|step| !step.passed)
                        || device.media.iter().any(|media| {
                            matches!(media.kind, horizon_app_provider::media::Kind::Video) && media.error.is_some()
                        })
                })
        });
        let saved = retained_archive.finish_result(report)?;
        Ok::<_, Error>((saved, failed))
    });
    let report;
    #[cfg(unix)]
    {
        report = tokio::select! {
            result=&mut task=>result.map_err(|_|Error::Unavailable)??,
            signal=tokio::signal::ctrl_c()=>{signal.map_err(|_|Error::Unavailable)?;control.cancel();task.await.map_err(|_|Error::Unavailable)??},
            _=terminate.recv()=>{control.cancel();task.await.map_err(|_|Error::Unavailable)??},
        };
    }
    #[cfg(not(unix))]
    {
        report = tokio::select! {
            result=&mut task=>result.map_err(|_|Error::Unavailable)??,
            signal=tokio::signal::ctrl_c()=>{signal.map_err(|_|Error::Unavailable)?;control.cancel();task.await.map_err(|_|Error::Unavailable)??},
        };
    }
    let (value, failed) = report;
    let mut bytes = serde_json::to_vec(&value).map_err(|_| Error::Unavailable)?;
    bytes.push(b'\n');
    output::Output::new(std::io::stdout())?.terminal(bytes)?;
    if failed {
        return Err(Error::RunFailed);
    }
    Ok(())
}
