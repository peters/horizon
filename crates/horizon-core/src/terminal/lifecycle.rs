use super::{
    Arc, AtomicUsize, Cow, Duration, Error, EventLoop, FairMutex, Msg, Ordering, PtyOptions, ReplayRestoreState,
    Result, Shell, Term, Terminal, TerminalDimensions, TerminalEventProxy, TerminalSpawnOptions, TerminalSshTrust,
    WindowSize, drain_replay_events, mpsc, replay_terminal_bytes, term, tty,
};

impl Terminal {
    /// Spawn a terminal session backed by `alacritty_terminal`.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY or event loop cannot be created.
    pub fn spawn(options: TerminalSpawnOptions) -> Result<Self> {
        Self::spawn_guarded(options, TerminalSshTrust::default())
    }

    fn spawn_guarded(options: TerminalSpawnOptions, trust: TerminalSshTrust) -> Result<Self> {
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
        let event_loop_proxy = TerminalEventProxy::new(event_tx, trust);
        let term_proxy = event_loop_proxy.clone();

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
        #[cfg(target_os = "linux")]
        let child_start_time = child_pid.and_then(process_start_time);
        let event_loop = EventLoop::new(term.clone(), event_loop_proxy, pty, true, false)
            .map_err(|error| Error::Pty(format!("failed to initialize terminal event loop: {error}")))?;
        let event_sender = event_loop.channel();
        let event_loop_handle = Some(event_loop.spawn());

        let mut terminal = Self {
            work_owner: None,
            shutdown_complete: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            work_continuation: crate::agent_work::WorkContinuation::default(),
            term,
            event_sender,
            event_rx,
            event_loop_handle,
            #[cfg(target_os = "linux")]
            child_start_time,
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

    #[must_use]
    pub fn pending_work_resume(&self) -> Option<crate::agent_work::AskReason> {
        self.work_continuation.pending()
    }

    pub fn write_input(&self, bytes: &[u8]) {
        self.work_continuation.note_input(bytes);
        if let Some(owner) = &self.work_owner {
            owner.note_input(bytes);
        }
        self.write_protocol(bytes);
    }

    pub(super) fn write_protocol(&self, bytes: &[u8]) {
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
        enum JoinStatus {
            Complete,
            Panicked,
        }

        let Some(event_loop_handle) = self.event_loop_handle.take() else {
            return self.shutdown_complete.load(Ordering::Acquire);
        };

        let (shutdown_tx, shutdown_rx) = mpsc::sync_channel(1);
        let completed = Arc::clone(&self.shutdown_complete);
        std::thread::spawn(move || {
            // Drop the joined event loop on this helper thread so PTY teardown
            // cannot block the UI thread in `Pty::drop`.
            let status = match event_loop_handle.join() {
                Ok(_) => JoinStatus::Complete,
                Err(_) => JoinStatus::Panicked,
            };
            completed.store(true, Ordering::Release);
            let _ = shutdown_tx.send(status);
        });

        match shutdown_rx.recv_timeout(timeout) {
            Ok(JoinStatus::Complete) => true,
            Ok(JoinStatus::Panicked) => {
                tracing::warn!("terminal event loop panicked during shutdown");
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => false,
        }
    }

    pub(crate) fn owned_process_id(&self) -> Option<u32> {
        if !self.work_continuation.owns_process
            || self.child_exited
            || self.shutdown_complete.load(Ordering::Acquire)
            || self
                .event_loop_handle
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return None;
        }
        let pid = self.child_pid?;
        #[cfg(target_os = "linux")]
        if self.child_start_time.is_none() || process_start_time(pid) != self.child_start_time {
            return None;
        }
        Some(pid)
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.request_shutdown();
        self.wait_for_shutdown(timeout)
    }

    /// Preparation, cancellation and final evidence stay inside the existing
    /// asynchronous shutdown budget; the UI never waits for repository I/O.
    pub(crate) fn begin_work_shutdown(&mut self, completed: &Arc<AtomicUsize>) -> bool {
        if self.work_owner.is_none() || self.event_loop_handle.is_none() {
            return false;
        }
        let Some(owner) = self.work_owner.clone() else {
            return false;
        };
        let Some(handle) = self.event_loop_handle.take() else {
            return false;
        };
        let sender = self.event_sender.clone();
        let done = Arc::clone(completed);
        let terminal_done = Arc::clone(&self.shutdown_complete);
        std::thread::spawn(move || {
            let prepared = owner.prepare_suspend();
            if prepared {
                let _ = sender.send(Msg::Input(Cow::Borrowed(b"\x1b")));
                // Separate Input and Shutdown dispatch so the event loop can
                // flush cancellation instead of dropping queued input on exit.
                std::thread::sleep(Duration::from_millis(250));
            }
            let _ = sender.send(Msg::Shutdown);
            if let Ok(joined) = handle.join() {
                // Drop the PTY on this worker, before sealing the transcript.
                drop(joined);
                if prepared {
                    owner.finish_suspend();
                }
            }
            terminal_done.store(true, Ordering::Release);
            done.fetch_add(1, Ordering::Relaxed);
        });
        true
    }

    /// Spawns a background thread to join the event-loop handle, incrementing
    /// `completed` when done. Returns `true` if a join thread was spawned.
    pub(crate) fn begin_async_join(&mut self, completed: &Arc<AtomicUsize>) -> bool {
        let Some(handle) = self.event_loop_handle.take() else {
            return false;
        };
        let done = Arc::clone(completed);
        let terminal_done = Arc::clone(&self.shutdown_complete);
        std::thread::spawn(move || {
            // Join and drop on this helper thread so PTY teardown cannot block
            // the UI thread.
            let _ = handle.join();
            terminal_done.store(true, Ordering::Release);
            done.fetch_add(1, Ordering::Relaxed);
        });
        true
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.request_shutdown();

        // Detach the event loop thread instead of joining it; joining can
        // block the main thread indefinitely if the child process is still
        // starting up or the PTY is stuck on I/O.
        drop(self.event_loop_handle.take());
    }
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
