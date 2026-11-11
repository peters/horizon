//! The hosts of `~/.ssh/config`, read on a worker thread.
//!
//! The presets of the window carry one entry for each alias of that file, so the
//! start of Horizon and a settings frame that applies a draft need the hosts. The
//! file is a file of the person's home, so one worker reads it and the frame reads
//! the result that the worker keeps. A read that fails keeps the hosts of the last
//! good read, and no frame waits for the disk.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use egui::Context;
use horizon_core::DiscoveredSshHost;

use super::HorizonApp;

/// The hosts of the last read, and the read that runs now.
#[derive(Default)]
pub(in crate::app) struct SshHostsRead {
    hosts: Vec<DiscoveredSshHost>,
    answer: Option<Receiver<horizon_core::Result<Vec<DiscoveredSshHost>>>>,
    /// Whether a frame asked for the first read of this run.
    asked: bool,
}

impl SshHostsRead {
    /// The hosts of the last read that answered.
    pub(in crate::app) fn hosts(&self) -> &[DiscoveredSshHost] {
        &self.hosts
    }

    /// Whether a read runs now.
    pub(in crate::app) fn is_reading(&self) -> bool {
        self.answer.is_some()
    }

    /// Asks for a read, unless one runs. The worker asks for a frame after it
    /// answers.
    pub(in crate::app) fn refresh(&mut self, ctx: &Context) {
        self.asked = true;
        if self.is_reading() {
            return;
        }
        self.answer = Some(spawn_read(ctx));
    }

    /// Asks for the first read of this run, from the first frame. A start that
    /// read the file before its window exists would ask for a frame that no
    /// window reads yet.
    fn read_once(&mut self, ctx: &Context) {
        if !self.asked {
            self.refresh(ctx);
        }
    }

    /// Applies the answer of the worker, and answers whether the hosts changed.
    fn poll(&mut self) -> bool {
        let Some(answer) = &self.answer else {
            return false;
        };
        match answer.try_recv() {
            Ok(Ok(hosts)) => {
                self.answer = None;
                let changed = hosts != self.hosts;
                self.hosts = hosts;
                changed
            }
            Ok(Err(error)) => {
                self.answer = None;
                tracing::warn!(%error, "the hosts of the ssh config of this computer were not read");
                false
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.answer = None;
                tracing::warn!("the worker of the ssh config answered nothing");
                false
            }
        }
    }
}

impl HorizonApp {
    /// Reads `~/.ssh/config` on a worker thread, so the presets of the window
    /// list its hosts.
    pub(in crate::app) fn refresh_ssh_hosts(&mut self, ctx: &Context) {
        self.ssh_hosts.refresh(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// Applies the hosts that the worker read: a preset of the window lists an
    /// alias of the file, so the presets follow the hosts. The first frame of an
    /// open Settings window reads the file again, and it never reads it twice.
    pub(in crate::app) fn poll_ssh_hosts(&mut self, ctx: &Context) {
        if self.ssh_hosts.poll() {
            self.presets = self.template_config.ssh_presets(self.ssh_hosts.hosts());
        }
        self.ssh_hosts.read_once(ctx);
        if self.settings.is_some() && !self.settings_hosts_refreshed {
            self.settings_hosts_refreshed = true;
            self.refresh_ssh_hosts(ctx);
        }
    }
}

/// Starts one read of `~/.ssh/config`, and wakes the window when it ends.
fn spawn_read(ctx: &Context) -> Receiver<horizon_core::Result<Vec<DiscoveredSshHost>>> {
    let (answers, answer) = channel();
    let waking = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("horizon-ssh-hosts".to_owned())
        .spawn(move || {
            let _ = answers.send(horizon_core::discover_ssh_hosts(None));
            waking.request_repaint();
        });
    if let Err(error) = spawned {
        // The dropped body closed the channel, so the frame reads the disconnect
        // and keeps the hosts of the last read.
        tracing::error!("cannot start the worker of the ssh config: {error}");
    }
    answer
}

#[cfg(test)]
mod tests;
