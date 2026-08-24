use super::{
    Arc, AtomicUsize, Cow, Duration, Error, EventLoop, FairMutex, JoinHandle, Msg, Ordering, PtyOptions,
    ReplayRestoreState, Result, Shell, Term, Terminal, TerminalDimensions, TerminalEventLoop, TerminalEventLoopState,
    TerminalEventProxy, TerminalSpawnOptions, WindowSize, drain_replay_events, mpsc, replay_terminal_bytes, term, tty,
};

use std::sync::Mutex;

type TerminalJoinHandle = JoinHandle<(TerminalEventLoop, TerminalEventLoopState)>;

enum JoinStatus {
    Complete,
    Panicked,
}

enum TerminalJoinCompletion {
    Notify(mpsc::SyncSender<JoinStatus>),
    Count(Arc<AtomicUsize>),
    Detached,
}

struct TerminalJoinTask {
    handle: TerminalJoinHandle,
    completion: TerminalJoinCompletion,
}

impl TerminalJoinTask {
    fn run(self) {
        // The successful join value owns the event loop and PTY, so evaluating
        // this on the worker also keeps their destructors off the UI thread.
        let status = if self.handle.join().is_ok() {
            JoinStatus::Complete
        } else {
            JoinStatus::Panicked
        };

        match self.completion {
            TerminalJoinCompletion::Notify(sender) => {
                let _ = sender.send(status);
            }
            TerminalJoinCompletion::Count(completed) => {
                if matches!(status, JoinStatus::Panicked) {
                    tracing::warn!("terminal event loop panicked during asynchronous shutdown");
                }
                completed.fetch_add(1, Ordering::Relaxed);
            }
            TerminalJoinCompletion::Detached => {
                if matches!(status, JoinStatus::Panicked) {
                    tracing::warn!("terminal event loop panicked during detached shutdown");
                }
            }
        }
    }
}

// Static storage is intentionally never dropped. If the OS refuses to create
// a join worker, keeping the task here prevents caller-thread PTY teardown; a
// later successful worker spawn can still drain it.
static PENDING_TERMINAL_JOINS: Mutex<Vec<TerminalJoinTask>> = Mutex::new(Vec::new());

fn enqueue_before_worker_spawn<T>(
    queue: &Mutex<Vec<T>>,
    task: T,
    spawn_worker: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(task);
    spawn_worker()
}

fn schedule_terminal_join(task: TerminalJoinTask) -> std::io::Result<()> {
    enqueue_before_worker_spawn(&PENDING_TERMINAL_JOINS, task, || {
        std::thread::Builder::new()
            .name("terminal-join".to_string())
            .spawn(drain_pending_terminal_joins)
            .map(drop)
    })
}

fn drain_pending_terminal_joins() {
    loop {
        let task = PENDING_TERMINAL_JOINS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        let Some(task) = task else {
            return;
        };
        task.run();
    }
}

impl Terminal {
    /// Spawn a terminal session backed by `alacritty_terminal`.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY or event loop cannot be created.
    pub fn spawn(options: TerminalSpawnOptions) -> Result<Self> {
        let rows = options.rows.max(1);
        let cols = options.cols.max(2);
        let scrollback_limit = options.scrollback_limit.max(1);
        let cell_width = options.cell_width.max(1);
        let cell_height = options.cell_height.max(1);
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };
        let dimensions = TerminalDimensions::new(rows, cols);
        let terminal_config = term::Config {
            scrolling_history: scrollback_limit,
            kitty_keyboard: options.kitty_keyboard,
            ..term::Config::default()
        };
        let replay_bytes = options.replay_bytes;
        let (event_tx, event_rx) = mpsc::channel();
        let term_proxy = TerminalEventProxy {
            event_tx: event_tx.clone(),
        };
        let event_loop_proxy = TerminalEventProxy { event_tx };

        tty::setup_env();

        let pty_options = PtyOptions {
            shell: Some(Shell::new(options.program, options.args)),
            working_directory: options.cwd,
            drain_on_exit: true,
            env: options.env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let term = Arc::new(FairMutex::new(Term::new(terminal_config, &dimensions, term_proxy)));
        let replay_restore = if replay_bytes.is_empty() {
            ReplayRestoreState::default()
        } else {
            replay_terminal_bytes(&term, &replay_bytes);
            drain_replay_events(&event_rx)
        };
        let pty =
            tty::new(&pty_options, window_size, options.window_id).map_err(|error| Error::Pty(error.to_string()))?;
        #[cfg(not(windows))]
        let child_pid = Some(pty.child().id());
        #[cfg(windows)]
        let child_pid = None;
        let event_loop = EventLoop::new(term.clone(), event_loop_proxy, pty, true, false)
            .map_err(|error| Error::Pty(format!("failed to initialize terminal event loop: {error}")))?;
        let event_sender = event_loop.channel();
        let event_loop_handle = Some(event_loop.spawn());

        let mut terminal = Self {
            term,
            event_sender,
            event_rx,
            event_loop_handle,
            child_pid,
            rows,
            cols,
            cell_width,
            cell_height,
            scrollback_limit,
            title: replay_restore.title,
            clipboard_contents: String::new(),
            selection_contents: String::new(),
            pending_pty_resize: None,
            pty_resized: false,
            child_exited: false,
            child_exit_status: None,
            bell_pending: false,
            pending_notification: None,
        };
        terminal.process_events();
        Ok(terminal)
    }

    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let _ = self
            .event_sender
            .send(Msg::Input(Cow::Owned(bytes.to_vec())))
            .map_err(|error| tracing::debug!("failed to forward terminal input: {error}"));
    }

    pub fn request_shutdown(&mut self) {
        if self.event_loop_handle.is_none() {
            return;
        }

        let _ = self
            .event_sender
            .send(Msg::Shutdown)
            .map_err(|error| tracing::debug!("failed to stop terminal event loop: {error}"));
    }

    #[must_use]
    pub fn wait_for_shutdown(&mut self, timeout: Duration) -> bool {
        let Some(event_loop_handle) = self.event_loop_handle.take() else {
            return true;
        };

        let (shutdown_tx, shutdown_rx) = mpsc::sync_channel(1);
        if let Err(error) = schedule_terminal_join(TerminalJoinTask {
            handle: event_loop_handle,
            completion: TerminalJoinCompletion::Notify(shutdown_tx),
        }) {
            tracing::warn!("failed to start terminal join worker; retained task for later cleanup: {error}");
        }

        match shutdown_rx.recv_timeout(timeout) {
            Ok(JoinStatus::Complete) => true,
            Ok(JoinStatus::Panicked) => {
                tracing::warn!("terminal event loop panicked during shutdown");
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => false,
        }
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.request_shutdown();
        self.wait_for_shutdown(timeout)
    }

    /// Queues the event-loop handle for a background join, incrementing
    /// `completed` when done. Returns `true` if a handle was queued.
    pub(crate) fn begin_async_join(&mut self, completed: &Arc<AtomicUsize>) -> bool {
        let Some(handle) = self.event_loop_handle.take() else {
            return false;
        };
        if let Err(error) = schedule_terminal_join(TerminalJoinTask {
            handle,
            completion: TerminalJoinCompletion::Count(Arc::clone(completed)),
        }) {
            tracing::warn!("failed to start terminal join worker; retained task for later cleanup: {error}");
        }
        true
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.request_shutdown();

        let Some(handle) = self.event_loop_handle.take() else {
            return;
        };

        // Queue ownership before the fallible worker spawn. A finished handle
        // can own the returned event loop, whose PTY destructor may otherwise
        // block this thread indefinitely while waiting for the child.
        if let Err(error) = schedule_terminal_join(TerminalJoinTask {
            handle,
            completion: TerminalJoinCompletion::Detached,
        }) {
            tracing::warn!("failed to start terminal join worker; retained task for later cleanup: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::enqueue_before_worker_spawn;

    #[test]
    fn failed_worker_spawn_retains_queued_ownership() {
        let queue = Mutex::new(Vec::new());

        let result = enqueue_before_worker_spawn(&queue, 42, || {
            Err(std::io::Error::other("injected worker spawn failure"))
        });

        assert!(result.is_err());
        assert_eq!(
            *queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![42]
        );
    }
}
