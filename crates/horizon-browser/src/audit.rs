//! Browser audit values re-exported from the lightweight protocol crate.

use std::time::{Duration, Instant};

use crate::{BrowserCommand, BrowserInput};

pub(crate) use horizon_browser_protocol::redact_url;
pub use horizon_browser_protocol::{
    BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id,
};

const POINTER_AUDIT_INTERVAL: Duration = Duration::from_millis(100);

/// Bounds durable audit work for high-rate, already-coalesced pointer motion.
/// Presses, releases, wheel input, keys, and page actions are always recorded.
#[derive(Debug, Default)]
pub(crate) struct BrowserAuditSampler {
    last_pointer_move: Option<Instant>,
}

impl BrowserAuditSampler {
    pub(crate) fn should_record(&mut self, command: &BrowserCommand) -> bool {
        self.should_record_at(command, Instant::now())
    }

    fn should_record_at(&mut self, command: &BrowserCommand, now: Instant) -> bool {
        if !matches!(command, BrowserCommand::Input(BrowserInput::MouseMove { .. })) {
            return true;
        }
        if self
            .last_pointer_move
            .is_some_and(|last| now.saturating_duration_since(last) < POINTER_AUDIT_INTERVAL)
        {
            return false;
        }
        self.last_pointer_move = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserButton, BrowserModifiers};

    #[test]
    fn high_rate_pointer_motion_is_sampled_but_state_changes_are_not() {
        let started = Instant::now();
        let movement = BrowserCommand::Input(BrowserInput::MouseMove {
            x: 10.0,
            y: 20.0,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        });
        let release = BrowserCommand::Input(BrowserInput::MouseRelease {
            x: 10.0,
            y: 20.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        });
        let mut sampler = BrowserAuditSampler::default();

        assert!(sampler.should_record_at(&movement, started));
        assert!(!sampler.should_record_at(&movement, started + Duration::from_millis(99)));
        assert!(sampler.should_record_at(&release, started + Duration::from_millis(99)));
        assert!(sampler.should_record_at(&movement, started + POINTER_AUDIT_INTERVAL));
    }
}
