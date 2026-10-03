use super::CastStatus;

pub(crate) struct Progress {
    pub(crate) state: CastStatus,
    frames: u64,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            state: CastStatus::Connecting,
            frames: 0,
        }
    }
}

impl Progress {
    pub(crate) fn frames_sent(&self) -> u64 {
        self.frames
    }

    pub(crate) fn record_transmission(&mut self) {
        self.frames = self.frames.saturating_add(1);
        // A send completing during cancellation must not undo the stopping state.
        if matches!(self.state, CastStatus::Streaming { .. }) {
            self.state = CastStatus::Streaming { frames: self.frames };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_keep_transmitted_frames_and_late_sends_cannot_restart_them() {
        for terminal in [
            CastStatus::Stopping,
            CastStatus::Stopped,
            CastStatus::Failed("receiver ended".into()),
        ] {
            let mut progress = Progress {
                state: CastStatus::Streaming { frames: 0 },
                frames: 0,
            };
            progress.record_transmission();
            assert_eq!(progress.state, CastStatus::Streaming { frames: 1 });
            progress.state = terminal.clone();
            assert_eq!(progress.frames_sent(), 1);
            progress.record_transmission();
            assert_eq!(progress.frames_sent(), 2);
            assert_eq!(progress.state, terminal);
        }
    }
}
