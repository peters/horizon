//! Docker and scp expose live counters only on a terminal. Reuse the existing PTY.
mod counters;
use super::super::{Error, Event, Result, progress::Progress};
use super::Runner;
use crate::terminal::{Terminal, TerminalSpawnOptions};
use std::{
    collections::HashMap,
    process::Command,
    time::{Duration, Instant},
};

pub enum Transfer {
    Image,
    Pull,
    File(u64),
}

impl Runner<'_> {
    /// # Errors
    /// Keeps CLI authentication/context behavior while measuring its terminal output.
    pub fn transfer(&self, name: &'static str, command: &Command, kind: Transfer, timeout: Duration) -> Result<()> {
        self.cancel.check()?;
        let options = options(command)?;
        let mut terminal = Terminal::spawn(options).map_err(|_| Error::Invalid("Cannot start transfer terminal"))?;
        // This direct command and its process group belong solely to this operation.
        terminal.work_continuation.owns_process = true;
        let mut deadline = TransferDeadline::new(matches!(kind, Transfer::File(_)), timeout);
        let mut counters = counters::Counters::new(kind);
        let mut previous = HashMap::new();
        (self.emit)(Event::Progress(Progress::activity(name)));
        loop {
            terminal.process_events();
            let lines = terminal.full_text_lines(1024).0;
            for line in &lines {
                let line = self.redact(line.clone());
                if line.trim().is_empty() {
                    continue;
                }
                counters.observe(&line);
                let key = line.split_once(':').map_or(line.as_str(), |(key, _)| key).to_owned();
                if previous.get(&key) != Some(&line) {
                    (self.emit)(Event::Output(line.clone()));
                    if previous.len() < 2048 {
                        previous.insert(key, line);
                    }
                }
            }
            let progress = counters.snapshot(name);
            let timed_out = deadline.expired(Instant::now(), counters.activity());
            (self.emit)(Event::Progress(progress));
            if terminal.child_exited() {
                let success = terminal.child_exit_status().is_some_and(|status| status.success());
                let _ = terminal.shutdown_with_timeout(Duration::from_secs(2));
                return if success { Ok(()) } else { Err(Error::Command(name)) };
            }
            if self.cancel.is_cancelled() || timed_out {
                stop(&mut terminal);
                self.cancel.check()?;
                return Err(Error::Invalid("Transfer timed out"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

struct TransferDeadline {
    file: bool,
    timeout: Duration,
    started: Instant,
    advanced: Instant,
    completed: u64,
}
impl TransferDeadline {
    const MAX_FILE_DURATION: Duration = Duration::from_hours(6);

    fn new(file: bool, timeout: Duration) -> Self {
        let started = Instant::now();
        Self {
            file,
            timeout,
            started,
            advanced: started,
            completed: 0,
        }
    }

    fn expired(&mut self, now: Instant, completed: u64) -> bool {
        if completed > self.completed {
            self.completed = completed;
            self.advanced = now;
        }
        if self.file {
            // Large LFS archives may exceed ten minutes while still making progress.
            now.duration_since(self.advanced) > self.timeout
                || now.duration_since(self.started) > Self::MAX_FILE_DURATION
        } else {
            now.duration_since(self.started) > self.timeout
        }
    }
}

fn options(command: &Command) -> Result<TerminalSpawnOptions> {
    let text = |value: &std::ffi::OsStr| {
        value
            .to_str()
            .map(str::to_owned)
            .ok_or(Error::Invalid("Transfer command is not UTF-8"))
    };
    let mut env = HashMap::from([("TERM".into(), "xterm-256color".into())]);
    for (key, value) in command.get_envs() {
        let value = value.ok_or(Error::Invalid("Transfer environment removal is unsupported"))?;
        env.insert(text(key)?, text(value)?);
    }
    Ok(TerminalSpawnOptions {
        program: text(command.get_program())?,
        args: command.get_args().map(text).collect::<Result<Vec<_>>>()?,
        cwd: command.get_current_dir().map(std::path::Path::to_path_buf),
        rows: 128,
        cols: 200,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 1024,
        window_id: 0,
        replay_bytes: Vec::new(),
        env,
        kitty_keyboard: false,
    })
}

fn stop(terminal: &mut Terminal) {
    #[cfg(unix)]
    if let Some(id) = terminal
        .owned_process_id()
        .and_then(|pid| rustix::process::Pid::from_raw(pid.cast_signed()))
    {
        let _ = rustix::process::kill_process_group(id, rustix::process::Signal::KILL);
    }
    let _ = terminal.shutdown_with_timeout(Duration::from_secs(2));
}

#[cfg(all(test, unix))]
mod tests;
