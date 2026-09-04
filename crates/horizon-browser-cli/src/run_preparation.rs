//! Cancellation ownership for detached durable preparation.

use horizon_browser_cli::{
    Plan,
    execution_control::{CancellationProbe, ExecutionControl, ExecutionStopReason},
    run_state::{DurablePreparationError, DurableRun},
};

use super::{CANCELLATION_FLUSH_GRACE, EXIT_CANCELLED};

pub(super) enum PreparationCompletion {
    Completed(Result<Box<DurableRun>, DurablePreparationError>),
    InfrastructureFailed(String),
}

enum WorkerCompletion<T> {
    Completed(T),
    Cancelled {
        result: T,
        flush_deadline: tokio::time::Instant,
    },
    InfrastructureFailed(String),
}

pub(super) async fn prepare_durable(
    control: &mut ExecutionControl,
    cancellation: &mut CancellationProbe,
    plan: Plan,
    timeout_seconds: u64,
    deadline_at_millis: u64,
) -> Result<PreparationCompletion, ExecutionStopReason> {
    match await_worker(control, cancellation, "horizon-browser-state-prepare", move || {
        DurableRun::prepare_cancellable(&plan, timeout_seconds, deadline_at_millis).map(Box::new)
    })
    .await?
    {
        WorkerCompletion::Completed(outcome) => Ok(PreparationCompletion::Completed(outcome)),
        WorkerCompletion::Cancelled { result, flush_deadline } => {
            persist_cancelled_preparation(result.map(|run| *run), flush_deadline).await;
            Err(ExecutionStopReason::Cancelled)
        }
        WorkerCompletion::InfrastructureFailed(error) => Ok(PreparationCompletion::InfrastructureFailed(error)),
    }
}

async fn await_worker<T>(
    control: &mut ExecutionControl,
    cancellation: &mut CancellationProbe,
    worker_name: &'static str,
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<WorkerCompletion<T>, ExecutionStopReason>
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
            return Ok(WorkerCompletion::InfrastructureFailed(format!(
                "could not start {worker_name}: {error}"
            )));
        }
    };
    let mut completion = Box::pin(async move {
        match receiver.await {
            Ok(result) => WorkerCompletion::Completed(result),
            Err(error) => {
                WorkerCompletion::InfrastructureFailed(format!("{worker_name} stopped without a result: {error}"))
            }
        }
    });
    match control.wait(completion.as_mut()).await {
        Ok(result) => return Ok(result),
        Err(ExecutionStopReason::Cancelled) => {}
        Err(ExecutionStopReason::DeadlineExceeded) => {
            // Atomic preparation remains owned after its action lease expires;
            // a later interrupt must still be able to bound that blocking I/O.
            tokio::select! {
                biased;
                () = cancellation.wait() => {}
                result = completion.as_mut() => return Ok(result),
            }
        }
    }
    let flush_deadline = tokio::time::Instant::now() + CANCELLATION_FLUSH_GRACE;
    match tokio::time::timeout_at(flush_deadline, completion.as_mut()).await {
        Ok(WorkerCompletion::Completed(result) | WorkerCompletion::Cancelled { result, .. }) => {
            Ok(WorkerCompletion::Cancelled { result, flush_deadline })
        }
        Ok(WorkerCompletion::InfrastructureFailed(_)) => Err(ExecutionStopReason::Cancelled),
        Err(_) => std::process::exit(i32::from(EXIT_CANCELLED)),
    }
}

async fn persist_cancelled_preparation(
    outcome: Result<DurableRun, DurablePreparationError>,
    flush_deadline: tokio::time::Instant,
) {
    let run = match outcome {
        Ok(run) => Some(run),
        Err(error) => error.into_parts().0,
    };
    let Some(mut run) = run else {
        return;
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("horizon-browser-state-cancel-preparation".to_string())
        .spawn(move || {
            let _ = sender.send(run.stop(ExecutionStopReason::Cancelled));
        });
    if let Err(error) = worker {
        tracing::warn!(%error, "could not start abandoned-preparation cancellation persistence");
        return;
    }
    match tokio::time::timeout_at(flush_deadline, receiver).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(%error, "could not persist abandoned preparation cancellation"),
        Ok(Err(error)) => tracing::warn!(%error, "abandoned-preparation cancellation worker lost its result"),
        Err(_) => std::process::exit(i32::from(EXIT_CANCELLED)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn cancellation_drains_a_late_worker_result_before_returning() {
        let (mut control, cancellation) = ExecutionControl::cancellable();
        let mut cancellation_probe = cancellation.probe();
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let worker_dropped = Arc::clone(&dropped);
        let worker = tokio::spawn(async move {
            await_worker(
                &mut control,
                &mut cancellation_probe,
                "late-preparation-test",
                move || {
                    started_sender.send(()).expect("signal worker start");
                    release_receiver.recv().expect("release worker result");
                    DropFlag(worker_dropped)
                },
            )
            .await
        });

        started_receiver.await.expect("worker start signal");
        cancellation.cancel();
        release_sender.send(()).expect("release worker result");
        let result = worker.await.expect("join cancellation test");
        let WorkerCompletion::Cancelled { result, .. } = result.expect("cancelled worker result") else {
            panic!("cancellation did not drain the late worker result");
        };
        drop(result);
        assert!(dropped.load(Ordering::Acquire));
    }
}
