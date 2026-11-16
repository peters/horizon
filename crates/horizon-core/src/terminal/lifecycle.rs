use std::os::unix::net::UnixStream;

use super::{
    Arc, Cow, Duration, Error, EventLoop, FairMutex, Instant, Msg, Ordering, OutputWake, OutputWakeState,
    PendingClipboard, PtyOptions, ReplayRestoreState, Result, Shell, Term, Terminal, TerminalDimensions,
    TerminalEventProxy, TerminalSpawnOptions, TerminalSshTrust, WindowSize, drain_replay_events, latency, mpsc,
    replay_terminal_bytes, term, tty,
};

use horizon_pty_tap::frames::{self, Frame, Hello};

use crate::panel_shutdown::PanelCompletion;

/// The time that an attach waits for the hello of its holder.
const HELLO_BOUND: Duration = Duration::from_secs(2);

/// The size of the terminal that `options` names.
fn window_size_of(options: &TerminalSpawnOptions) -> WindowSize {
    WindowSize {
        num_lines: options.rows.max(1),
        num_cols: options.cols.max(2),
        cell_width: options.cell_width.max(1),
        cell_height: options.cell_height.max(1),
    }
}

/// Reads the hello of a holder and the replay that follows it, which reaches the
/// grid before the live output of its program.
///
/// # Errors
///
/// Returns why the holder answers no hello, or no replay that its hello names.
fn holder_hello(socket: &mut UnixStream) -> Result<(Hello, Vec<u8>)> {
    socket
        .set_read_timeout(Some(HELLO_BOUND))
        .map_err(|error| Error::Pty(format!("the link of the holder takes no bound: {error}")))?;
    let answered =
        frames::read_one(socket).map_err(|error| Error::Pty(format!("the holder answered no hello: {error}")))?;
    let hello = match answered {
        Frame::Hello(hello) => hello,
        Frame::Refusal => return Err(Error::Pty("the holder of this panel serves another client".to_owned())),
        other => return Err(Error::Pty(format!("the holder answered the frame {}", other.kind()))),
    };
    if hello.ring_len == 0 {
        return Ok((hello, Vec::new()));
    }
    match frames::read_one(socket).map_err(|error| Error::Pty(format!("the holder answered no replay: {error}")))? {
        Frame::Replay(bytes) => Ok((hello, bytes)),
        other => Err(Error::Pty(format!(
            "the holder sent the frame {} instead of a replay",
            other.kind()
        ))),
    }
}

/// Opens the terminal of `program` with its arguments, its folder and its
/// environment, and starts it in that terminal.
///
/// The holder of a panel owns the same terminal without a `Terminal`, so both
/// users open and start it here instead of each writing the options of a PTY.
///
/// # Errors
///
/// Returns why the system cannot open the terminal or start the program.
pub(crate) fn open_pty(
    program: String,
    args: Vec<String>,
    cwd: Option<std::path::PathBuf>,
    env: std::collections::HashMap<String, String>,
    window_size: WindowSize,
    window_id: u64,
) -> Result<tty::Pty> {
    tty::setup_env();
    let pty_options = PtyOptions {
        shell: Some(Shell::new(program, args)),
        working_directory: cwd,
        drain_on_exit: true,
        env,
    };
    tty::new(&pty_options, window_size, window_id).map_err(|error| Error::Pty(error.to_string()))
}

impl Terminal {
    /// Spawn a terminal session backed by `alacritty_terminal`.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY or event loop cannot be created.
    pub fn spawn(options: TerminalSpawnOptions) -> Result<Self> {
        Self::spawn_guarded(options, TerminalSshTrust::default(), PtyOutput::Shown)
    }

    /// Spawn a terminal that shows `options.replay_bytes` and none of its process's output.
    ///
    /// A snapshot runs a process that exits at once only because a terminal needs a PTY.
    /// That output must not reach the snapshot: a console that clears its screen on the
    /// first paint would push the replayed text into scrollback and leave the panel
    /// blank, and one that names itself in the title would rename it.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY or event loop cannot be created.
    pub(crate) fn spawn_snapshot(options: TerminalSpawnOptions) -> Result<Self> {
        Self::spawn_guarded(options, TerminalSshTrust::default(), PtyOutput::Discarded)
    }

    /// Attaches a terminal to the holder of its panel, which `socket` reaches.
    ///
    /// The hello of the holder names the program and its start time, and the replay
    /// that follows it reaches the grid before the live output of the program.
    ///
    /// # Errors
    ///
    /// Returns why the holder answers no hello, or why its link cannot start.
    pub fn spawn_attached(mut options: TerminalSpawnOptions, mut socket: UnixStream) -> Result<Self> {
        // The hello of a holder and its replay wait in a socket, so only a worker
        // calls this: the guard of the UI thread reports a frame that waits here.
        let (hello, replay) = holder_hello(&mut socket)?;
        options.replay_bytes = replay;
        let pty = horizon_pty_tap::PanelPty::attached(socket, window_size_of(&options))
            .map_err(|error| Error::Pty(format!("failed to attach the pty of the panel: {error}")))?;
        let child_start_time = Arc::new(std::sync::OnceLock::new());
        let _ = child_start_time.set(hello.start_time);
        Self::on_pty(
            options,
            TerminalSshTrust::default(),
            PtyOutput::Shown,
            pty,
            Some(hello.pid),
            child_start_time,
        )
    }

    fn spawn_guarded(options: TerminalSpawnOptions, trust: TerminalSshTrust, output: PtyOutput) -> Result<Self> {
        // A terminal panel is a process, so its start is the one process that a
        // frame owns: the fork and the exec are the panel that the person asked
        // for, and no frame can exist without them.
        let _accepted = crate::ui_thread::exception("terminal spawn: a terminal panel is a process");
        let window_size = window_size_of(&options);
        let pty = open_pty(
            options.program.clone(),
            options.args.clone(),
            options.cwd.clone(),
            options.env.clone(),
            window_size,
            options.window_id,
        )?;
        let child_pid = Some(pty.child().id());
        let child_start_time = child_pid.map(crate::process::start_identity).unwrap_or_default();
        let pty = horizon_pty_tap::PanelPty::local(pty)
            .map_err(|error| Error::Pty(format!("failed to read the pty of the panel: {error}")))?;
        Self::on_pty(options, trust, output, pty, child_pid, child_start_time)
    }

    /// Starts the event loop of a terminal on `pty`, with the replay of `options`,
    /// and answers that terminal.
    fn on_pty(
        options: TerminalSpawnOptions,
        trust: TerminalSshTrust,
        output: PtyOutput,
        pty: horizon_pty_tap::PanelPty,
        child_pid: Option<u32>,
        child_start_time: Arc<std::sync::OnceLock<Option<String>>>,
    ) -> Result<Self> {
        let program_status = pty.inbox();
        let rows = options.rows.max(1);
        let cols = options.cols.max(2);
        let scrollback_limit = options.scrollback_limit.max(1);
        let cell_width = options.cell_width.max(1);
        let cell_height = options.cell_height.max(1);
        let dimensions = TerminalDimensions::new(rows, cols);
        let terminal_config = term::Config {
            scrolling_history: scrollback_limit,
            kitty_keyboard: options.kitty_keyboard,
            ..term::Config::default()
        };
        let replay_bytes = options.replay_bytes;
        let (event_tx, event_rx) = mpsc::channel();
        let latency = Arc::new(latency::Probe::new());
        let output_wake = Arc::new(OutputWakeState::default());
        let event_loop_proxy = TerminalEventProxy::new(event_tx, trust, Arc::clone(&latency), Arc::clone(&output_wake));
        let term_proxy = event_loop_proxy.clone();

        let term = Arc::new(FairMutex::new(Term::new(terminal_config, &dimensions, term_proxy)));
        let replay_restore = if replay_bytes.is_empty() {
            ReplayRestoreState::default()
        } else {
            replay_terminal_bytes(&term, &replay_bytes);
            drain_replay_events(&event_rx)
        };
        let pty_term = match output {
            PtyOutput::Shown => term.clone(),
            PtyOutput::Discarded => Arc::new(FairMutex::new(Term::new(
                term::Config {
                    scrolling_history: 0,
                    ..term::Config::default()
                },
                &dimensions,
                TerminalEventProxy::exit_only(event_loop_proxy.event_tx.clone()),
            ))),
        };
        let event_loop = EventLoop::new(pty_term, event_loop_proxy, pty, true, false)
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
            latency,
            output_wake,
            event_loop_handle,
            child_start_time,
            child_pid,
            rows,
            cols,
            cell_width,
            cell_height,
            scrollback_limit,
            title: replay_restore.title,
            program_status,
            pending_clipboard: PendingClipboard::default(),
            pending_pty_resize: None,
            pty_resized: false,
            child_exited: false,
            answered_query: false,
            child_exit_status: None,
            snapshot: matches!(output, PtyOutput::Discarded),
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

    /// Forward pointer motion without treating a hover as a user decision.
    pub fn write_pointer_motion(&self, bytes: &[u8]) {
        self.write_protocol(bytes);
    }

    /// Takes the callback that asks the window for the frame which shows new
    /// output of this terminal. The first callback of a panel stays, because a
    /// panel keeps the viewport that drew it.
    pub fn attach_output_wake(&self, wake: &OutputWake) {
        self.output_wake.attach(wake);
    }

    /// A person's key press waits for its echo, so the first output after it asks
    /// the window for the frame that paints the echo. The input of this panel
    /// arms the wake before it writes, so a local shell that echoes in
    /// microseconds is painted in the same frame.
    pub fn arm_output_wake(&self) {
        self.output_wake.arm();
    }

    /// Whether the frame that just ran drew this terminal. A terminal that no
    /// frame draws arms no wake, so the output of a panel that a person cannot
    /// see never makes the window repaint.
    pub fn note_drawn(&self, drawn: bool) {
        self.output_wake.set_drawn(drawn);
    }

    /// The time that this frame took a key event, while the latency lines of this
    /// terminal are on.
    #[must_use]
    pub fn press_clock(&self) -> Option<Instant> {
        self.latency.timed().then(Instant::now)
    }

    /// Records one key press that this frame wrote, for the latency lines.
    pub fn note_key_press(&self, event_at: Instant, bytes: usize) {
        self.latency.key_written(event_at, bytes);
    }

    /// Records that this frame painted the grid, which closes the lines of the
    /// key presses whose echo it shows.
    pub fn note_output_painted(&self) {
        self.latency.painted();
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

    /// The process that this panel owns, with the start time that tells it from
    /// a later process that took the same id.
    ///
    /// `None` while the identity is not known yet, which no caller can mistake
    /// for a proof of ownership. The caller that compares the value with the
    /// system runs that read off the UI thread.
    /// The process that this terminal started, while it still runs, with the
    /// value that tells it from a later process that took the same id.
    ///
    /// A caller that ends a panel closes the master of this terminal, which
    /// sends the hangup to the process group. This value reaches the program
    /// when the hangup does not end it.
    #[must_use]
    pub(crate) fn hanging_child(&self) -> Option<crate::process::HangingChild> {
        if self.child_exited || self.shutdown_complete.load(Ordering::Acquire) {
            return None;
        }
        Some(crate::process::HangingChild::new(
            self.child_pid?,
            std::sync::Arc::clone(&self.child_start_time),
        ))
    }

    pub(crate) fn owned_process(&self) -> Option<crate::process::OwnedProcess> {
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
        let start_time = self.child_start_time.get()?.clone()?;
        Some(crate::process::OwnedProcess { pid, start_time })
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.request_shutdown();
        self.wait_for_shutdown(timeout)
    }

    /// Preparation, cancellation and final evidence stay inside the existing
    /// asynchronous shutdown budget; the UI never waits for repository I/O.
    pub(crate) fn begin_work_shutdown(&mut self, completion: &PanelCompletion) -> bool {
        if self.event_loop_handle.is_none() {
            return false;
        }
        // A launch that only reports the state of its panel continues nothing:
        // it shuts down like any other terminal, without a cancellation and
        // without evidence of a continuation.
        let Some(owner) = self.work_owner.clone().filter(|owner| owner.continues_work()) else {
            return false;
        };
        let Some(handle) = self.event_loop_handle.take() else {
            return false;
        };
        let sender = self.event_sender.clone();
        let completion = completion.clone();
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
            completion.finish();
        });
        true
    }

    /// Releases the event loop and its PTY on a helper thread after loop exit.
    /// The grid stays available while cleanup waits for the child process.
    pub(crate) fn release_when_exited(&mut self) {
        let Some(handle) = self.event_loop_handle.take() else {
            return;
        };
        let terminal_done = Arc::clone(&self.shutdown_complete);
        std::thread::spawn(move || {
            // The joined event loop owns the PTY; dropping it here closes it.
            let _ = handle.join();
            terminal_done.store(true, Ordering::Release);
        });
    }

    /// Whether the event loop has ended and its PTY is closed.
    #[cfg(test)]
    pub(crate) fn pty_released(&self) -> bool {
        self.event_loop_handle.is_none() && self.shutdown_complete.load(Ordering::Acquire)
    }

    /// Spawns a detached thread that joins the event-loop handle and drops it,
    /// which releases the PTY and reaps the program of the panel.
    ///
    /// The thread counts no panel: a program that ended can hold that reaping
    /// without a bound (a Mac keeps it in its exiting state), so a shutdown
    /// never waits for this join. The panel is complete when its program ended.
    pub(crate) fn begin_async_join(&mut self) {
        let Some(handle) = self.event_loop_handle.take() else {
            return;
        };
        let terminal_done = Arc::clone(&self.shutdown_complete);
        std::thread::spawn(move || {
            // Join and drop on this helper thread so PTY teardown cannot block
            // the UI thread.
            let _ = handle.join();
            terminal_done.store(true, Ordering::Release);
        });
    }
}

/// Where the PTY output of a terminal goes.
#[derive(Clone, Copy)]
enum PtyOutput {
    /// Into the grid the panel shows.
    Shown,
    /// Into a scratch grid nobody sees; only the process's exit is reported.
    Discarded,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.request_shutdown();

        // A finished handle still owns the returned PTY. Dropping it here can
        // synchronously wait for the child, so join and destruction stay on a worker.
        self.release_when_exited();
    }
}

// The regression uses a Unix shell and PTY child that ignores SIGHUP.
#[cfg(all(test, unix))]
mod tests;

#[cfg(test)]
mod snapshot_tests;

// The latency lines and the wake of a key press run against a real PTY.
#[cfg(all(test, unix))]
mod latency_tests;

// The program status protocol and its feature detection query run against a PTY.
#[cfg(all(test, unix))]
mod tap_tests;
