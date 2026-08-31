//! Cancellation-safe durable persistence and requested report finalization.

use std::path::Path;

use horizon_browser_cli::{
    ExecutionReport,
    execution_control::{CancellationProbe, ExecutionStopReason},
    run_state::DurableRun,
};

use super::{CANCELLATION_FLUSH_GRACE, EXIT_CANCELLED, write_report};

pub(super) struct PersistenceResult {
    pub(super) interrupted: bool,
    pub(super) result: Result<(), String>,
}

pub(super) async fn finalize_report(
    durable: &DurableRun,
    report: &ExecutionReport,
    output_path: Option<&Path>,
    cancellation: &mut CancellationProbe,
) -> PersistenceResult {
    let mut processor = durable.postprocessor();
    let worker_report = report.clone();
    let worker_output = output_path.map(Path::to_path_buf);
    let worker_cancellation = cancellation.clone();
    run_io_worker("horizon-browser-report-finalize", cancellation, move || {
        let mut result = processor.finish(&worker_report).map_err(|error| error.to_string());
        if result.is_ok() {
            let public_report = processor.report(&worker_report);
            result = write_report(&public_report, worker_output.as_deref());
        }
        if worker_cancellation.is_cancelled()
            && worker_report.stop_reason.is_none()
            && let Err(error) = processor.stop(ExecutionStopReason::Cancelled)
        {
            let message = format!("could not persist finalization cancellation: {error}");
            result = Err(result
                .err()
                .map_or(message.clone(), |existing| format!("{existing}; {message}")));
        }
        result
    })
    .await
}

pub(super) async fn persist_stop(
    durable: &DurableRun,
    reason: ExecutionStopReason,
    cancellation: &mut CancellationProbe,
) -> PersistenceResult {
    let mut processor = durable.postprocessor();
    let worker_cancellation = cancellation.clone();
    run_io_worker("horizon-browser-state-stop", cancellation, move || {
        let mut result = processor.stop(reason).map_err(|error| error.to_string());
        if reason != ExecutionStopReason::Cancelled
            && worker_cancellation.is_cancelled()
            && let Err(error) = processor.stop(ExecutionStopReason::Cancelled)
        {
            let message = format!("could not persist cancellation after {reason:?}: {error}");
            result = Err(result
                .err()
                .map_or(message.clone(), |existing| format!("{existing}; {message}")));
        }
        result
    })
    .await
}

pub(super) async fn persist_failure(
    durable: &DurableRun,
    message: String,
    cancellation: &mut CancellationProbe,
) -> PersistenceResult {
    let mut processor = durable.postprocessor();
    let worker_cancellation = cancellation.clone();
    run_io_worker("horizon-browser-state-fail", cancellation, move || {
        let mut result = processor.fail(&message).map_err(|error| error.to_string());
        if worker_cancellation.is_cancelled()
            && let Err(error) = processor.stop(ExecutionStopReason::Cancelled)
        {
            let cancellation_error = format!("could not persist cancellation after failure: {error}");
            result = Err(result.err().map_or(cancellation_error.clone(), |existing| {
                format!("{existing}; {cancellation_error}")
            }));
        }
        result
    })
    .await
}

async fn run_io_worker(
    worker_name: &'static str,
    cancellation: &mut CancellationProbe,
    operation: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> PersistenceResult {
    let (sender, mut completion_rx) = tokio::sync::oneshot::channel();
    let worker = std::thread::Builder::new()
        .name(worker_name.to_string())
        .spawn(move || {
            let _ = sender.send(operation());
        });
    let _worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            return PersistenceResult {
                interrupted: cancellation.is_cancelled(),
                result: Err(format!("could not start {worker_name}: {error}")),
            };
        }
    };
    let mut interrupted = false;
    let delivery = tokio::select! {
        biased;
        () = cancellation.wait() => {
            interrupted = true;
            match tokio::time::timeout(CANCELLATION_FLUSH_GRACE, &mut completion_rx).await {
                Ok(delivery) => delivery,
                Err(_) => std::process::exit(i32::from(EXIT_CANCELLED)),
            }
        }
        delivery = &mut completion_rx => delivery,
    };
    let result = match delivery {
        Ok(result) => result,
        Err(error) => Err(format!("{worker_name} stopped without a result: {error}")),
    };
    PersistenceResult { interrupted, result }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use horizon_browser_cli::execution_control::{ExecutionControl, JobDeadline};

    use super::*;

    #[tokio::test]
    async fn cancellation_is_observed_while_persistence_worker_is_blocked() {
        let (_control, cancellation) = ExecutionControl::until(JobDeadline::after(Duration::from_secs(5)));
        let mut probe = cancellation.probe();
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let interrupt = std::thread::spawn(move || {
            started_receiver.recv().expect("persistence start signal");
            cancellation.cancel();
            release_sender.send(()).expect("release persistence worker");
        });

        let persistence = run_io_worker("blocked-persistence-test", &mut probe, move || {
            started_sender.send(()).expect("signal persistence start");
            release_receiver.recv().expect("persistence release signal");
            Ok(())
        })
        .await;

        interrupt.join().expect("interrupt thread");
        assert!(persistence.interrupted);
        assert!(persistence.result.is_ok());
    }
}
