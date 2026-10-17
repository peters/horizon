use std::time::Duration;

use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::Instant,
};

use crate::{Cancellation, Completion, Credentials, Error, Event, ExecRequest, protocol};

pub(crate) const CHUNK: usize = 32 * 1024;
pub(crate) const QUEUE: usize = 8;
pub(crate) const SHUTDOWN: Duration = Duration::from_secs(2);

pub(crate) enum Input {
    Data(Vec<u8>),
    Eof,
}

/// Independently owned bounded stdin. Dropping it sends EOF after already queued data.
pub struct CommandInput {
    sender: mpsc::Sender<Input>,
    cancelled: watch::Receiver<bool>,
    deadline: Instant,
    remaining: usize,
}

impl CommandInput {
    /// Queue at most 32 KiB, applying backpressure without blocking a runtime thread.
    ///
    /// # Errors
    /// Reports data limits, cancellation, deadline or a finished command.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > CHUNK || bytes.len() > self.remaining {
            return Err(Error::DataLimit);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.send(Input::Data(bytes.to_vec())).await?;
        self.remaining -= bytes.len();
        Ok(())
    }

    /// Send EOF after queued input. Output continues to drain independently.
    ///
    /// # Errors
    /// Reports cancellation, deadline or a finished command.
    pub async fn eof(mut self) -> Result<(), Error> {
        self.send(Input::Eof).await
    }

    async fn send(&mut self, message: Input) -> Result<(), Error> {
        tokio::select! {
            biased;
            _ = self.cancelled.wait_for(|cancelled| *cancelled) => Err(Error::Cancelled),
            () = tokio::time::sleep_until(self.deadline) => Err(Error::Deadline),
            result = self.sender.send(message) => result.map_err(|_| Error::Finished),
        }
    }
}

/// One SSH exec request, with its own connection and finite event queue.
pub struct Command {
    events: mpsc::Receiver<Event>,
    input: Option<CommandInput>,
    cancellation: Cancellation,
    task: Option<JoinHandle<Result<Completion, Error>>>,
}

impl Drop for Command {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl Command {
    /// Take the sole input owner, allowing input and events to be driven concurrently.
    pub fn take_input(&mut self) -> Option<CommandInput> {
        self.input.take()
    }

    /// Obtain independent synchronous cancellation authority.
    #[must_use]
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    /// Read the next observed channel event. None means no more events will follow.
    /// Consume events until None before calling completion to avoid output backpressure.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Await the terminal result after draining events. Exit status alone is not completion.
    ///
    /// # Errors
    /// Reports the session's failure or that its result was already taken.
    pub async fn completion(&mut self) -> Result<Completion, Error> {
        let result = self.task.as_mut().ok_or(Error::Finished)?.await;
        self.task.take();
        result.map_err(|_| Error::Transport)?
    }

    /// Cancel physically and await retirement, including when no events are consumed.
    ///
    /// # Errors
    /// Returns Cancelled for a cancelled operation, or its prior terminal result/failure.
    pub async fn shutdown(mut self) -> Result<Completion, Error> {
        self.cancellation.cancel();
        self.events.close();
        self.input.take();
        self.completion().await
    }
}

/// Start a pinned non-PTY SSH command on the caller's Tokio runtime.
/// `ExecAccepted` is the SSH server's reply, not remote application readiness.
///
/// # Errors
/// Rejects an elapsed deadline or missing runtime. Network failures are terminal Command results.
pub fn execute(request: ExecRequest, credentials: Credentials) -> Result<Command, Error> {
    if request.deadline <= Instant::now() {
        return Err(Error::Deadline);
    }
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::NoRuntime)?;
    let cancellation = Cancellation::default();
    let (input_tx, input_rx) = mpsc::channel(QUEUE);
    let (event_tx, events) = mpsc::channel(QUEUE);
    let input = CommandInput {
        sender: input_tx,
        cancelled: cancellation.subscribe(),
        deadline: request.deadline,
        remaining: request.limits.stdin,
    };
    let control = cancellation.clone();
    let task = runtime.spawn(async move { protocol::run(request, credentials, control, input_rx, event_tx).await });
    Ok(Command {
        events,
        input: Some(input),
        cancellation,
        task: Some(task),
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
