use serde::{Deserialize, Serialize};

use crate::PanelKind;

use super::{TranscriptSnapshot, TurnLedger, TurnState};

/// Separate from conversation binding: restoring history does not opt a panel
/// into unattended tool execution.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ResumePolicy {
    pub enabled: bool,
    pub max_downtime_seconds: u32,
}

impl Default for ResumePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_downtime_seconds: 3 * 60 * 60,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SuspendRecord {
    pub kind: PanelKind,
    /// Snapshot taken before cancellation; an existing user interrupt is a veto.
    pub before_cancel: Option<TranscriptSnapshot>,
    pub panel_local_id: String,
    pub session_id: String,
    pub prompt_id: String,
    pub generation: u64,
    pub suspended_at_millis: i64,
    pub cwd: String,
    pub repo_fingerprint: Option<String>,
    /// Filled only after the provider has exited. Without this final snapshot
    /// a crash during shutdown takes the manual path.
    pub final_transcript: Option<TranscriptSnapshot>,
    /// True only when Horizon recorded the same working turn before sending Esc.
    pub cancelled_by_horizon: bool,
}

impl SuspendRecord {
    pub(super) fn has_working_snapshot_before_cancel(&self) -> bool {
        self.before_cancel
            .as_ref()
            .is_some_and(|before| before.state == TurnState::Working)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskReason {
    UncleanShutdown,
    MissingEvidence,
    WaitingForUser,
    Downtime,
    RepositoryChanged,
    BatchLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartDecision {
    NotResumable,
    Ask(AskReason),
    Resume,
}

pub struct RestartEvidence<'a> {
    pub kind: PanelKind,
    pub panel_local_id: &'a str,
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub policy: &'a ResumePolicy,
    pub handoff: Option<&'a SuspendRecord>,
    pub ledger: Option<&'a TurnLedger>,
    pub transcript: Option<&'a TranscriptSnapshot>,
    pub repo_fingerprint: Option<&'a str>,
    pub session_live_elsewhere: bool,
    pub stale_live_session: bool,
    pub now_millis: i64,
}

impl RestartEvidence<'_> {
    fn without_handoff(&self, transcript: &TranscriptSnapshot) -> RestartDecision {
        if self.ledger.is_some_and(|ledger| {
            ledger.session_id == self.session_id
                && (ledger.deliberate_exit
                    || matches!(
                        ledger.state,
                        TurnState::Finished | TurnState::Failed | TurnState::Interrupted
                    ))
        }) {
            return RestartDecision::NotResumable;
        }
        if self
            .ledger
            .is_some_and(|ledger| ledger.session_id == self.session_id && ledger.state == TurnState::Blocked)
        {
            return RestartDecision::Ask(AskReason::WaitingForUser);
        }
        match (transcript.state, self.stale_live_session) {
            (TurnState::Working, true) => RestartDecision::Ask(AskReason::UncleanShutdown),
            (TurnState::Blocked, _) => RestartDecision::Ask(AskReason::WaitingForUser),
            _ => RestartDecision::NotResumable,
        }
    }

    /// Fail closed on identity drift, completed/interrupted work, or a changed
    /// transcript. Only a fully attested clean shutdown can auto-continue.
    #[must_use]
    pub fn classify(&self, remaining_slots: usize) -> RestartDecision {
        if !self.policy.enabled || self.session_live_elsewhere || self.session_id.is_empty() {
            return RestartDecision::NotResumable;
        }
        let Some(transcript) = self.transcript else {
            return RestartDecision::NotResumable;
        };
        if matches!(
            transcript.state,
            TurnState::Finished | TurnState::Failed | TurnState::Unknown
        ) {
            return RestartDecision::NotResumable;
        }
        let Some(record) = self.handoff else {
            return self.without_handoff(transcript);
        };
        if record.kind != self.kind
            || record.panel_local_id != self.panel_local_id
            || record.session_id != self.session_id
            || record.cwd != self.cwd
        {
            return RestartDecision::NotResumable;
        }
        if record
            .final_transcript
            .as_ref()
            .is_some_and(|saved| saved != transcript)
        {
            return RestartDecision::NotResumable;
        }
        if (transcript.state == TurnState::Interrupted
            || self.ledger.is_some_and(|ledger| ledger.state == TurnState::Interrupted)
            || record.cancelled_by_horizon)
            && (!record.cancelled_by_horizon
                || !record
                    .before_cancel
                    .as_ref()
                    .is_some_and(|before| before.state == TurnState::Working && before.bytes < transcript.bytes))
        {
            return RestartDecision::NotResumable;
        }
        let Some(ledger) = self.ledger else {
            return RestartDecision::Ask(AskReason::MissingEvidence);
        };
        if ledger.session_id != self.session_id
            || ledger.prompt_id.as_deref() != Some(&record.prompt_id)
            || ledger.generation != record.generation
            || ledger.deliberate_exit
            || matches!(ledger.state, TurnState::Finished | TurnState::Failed)
            || (ledger.state == TurnState::Interrupted && !record.cancelled_by_horizon)
        {
            return RestartDecision::NotResumable;
        }
        if ledger.state == TurnState::Blocked || transcript.state == TurnState::Blocked {
            return RestartDecision::Ask(AskReason::WaitingForUser);
        }
        // Only this lifecycle has been verified through PTY teardown and seeded resume.
        if self.kind != PanelKind::Claude {
            return RestartDecision::Ask(AskReason::MissingEvidence);
        }
        if record.prompt_id.is_empty()
            || record.generation == 0
            || !record.has_working_snapshot_before_cancel()
            || !matches!(ledger.state, TurnState::Working | TurnState::Interrupted)
            || record.final_transcript.is_none()
            || !ledger.ended_at_millis.is_some_and(|ended| {
                ended >= record.suspended_at_millis
                    && ended <= record.suspended_at_millis.saturating_add(10_000)
                    && ended <= self.now_millis
                    && ledger.updated_at_millis <= ended
            })
        {
            return RestartDecision::Ask(AskReason::MissingEvidence);
        }
        let downtime = self.now_millis.checked_sub(record.suspended_at_millis);
        if !downtime
            .is_some_and(|duration| duration >= 0 && duration <= i64::from(self.policy.max_downtime_seconds) * 1000)
        {
            return RestartDecision::Ask(AskReason::Downtime);
        }
        if self.repo_fingerprint.is_none() || self.repo_fingerprint != record.repo_fingerprint.as_deref() {
            return RestartDecision::Ask(AskReason::RepositoryChanged);
        }
        if remaining_slots == 0 {
            return RestartDecision::Ask(AskReason::BatchLimit);
        }
        RestartDecision::Resume
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_incomplete_handoffs_need_confirmation(
        record: &SuspendRecord,
        evidence: &RestartEvidence<'_>,
        transcript: &TranscriptSnapshot,
    ) {
        for state in [
            None,
            Some(TurnState::Unknown),
            Some(TurnState::Blocked),
            Some(TurnState::Finished),
            Some(TurnState::Failed),
            Some(TurnState::Interrupted),
        ] {
            let mut incomplete = record.clone();
            incomplete.before_cancel = state.map(|state| TranscriptSnapshot {
                state,
                ..transcript.clone()
            });
            assert_eq!(
                RestartEvidence {
                    handoff: Some(&incomplete),
                    ..*evidence
                }
                .classify(1),
                RestartDecision::Ask(AskReason::MissingEvidence)
            );
        }
    }

    #[test]
    fn only_a_complete_clean_handoff_can_continue_automatically() {
        let transcript = TranscriptSnapshot {
            bytes: 10,
            tail_sha256: [1; 32],
            state: TurnState::Working,
        };
        let record = SuspendRecord {
            kind: PanelKind::Claude,
            before_cancel: Some(TranscriptSnapshot {
                bytes: 5,
                tail_sha256: [0; 32],
                state: TurnState::Working,
            }),
            panel_local_id: "panel".into(),
            session_id: "session".into(),
            prompt_id: "prompt".into(),
            generation: 1,
            suspended_at_millis: 1000,
            cwd: "/repo".into(),
            repo_fingerprint: Some("repo".into()),
            final_transcript: Some(transcript.clone()),
            cancelled_by_horizon: false,
        };
        let ledger = TurnLedger {
            session_id: "session".into(),
            prompt_id: Some("prompt".into()),
            state: TurnState::Working,
            generation: 1,
            ended_at_millis: Some(2000),
            ..TurnLedger::default()
        };
        let policy = ResumePolicy {
            enabled: true,
            ..ResumePolicy::default()
        };
        let mut evidence = RestartEvidence {
            kind: PanelKind::Claude,
            panel_local_id: "panel",
            session_id: "session",
            cwd: "/repo",
            policy: &policy,
            handoff: Some(&record),
            ledger: Some(&ledger),
            transcript: Some(&transcript),
            repo_fingerprint: Some("repo"),
            session_live_elsewhere: false,
            stale_live_session: false,
            now_millis: 3000,
        };
        assert_eq!(evidence.classify(1), RestartDecision::Resume);
        assert_incomplete_handoffs_need_confirmation(&record, &evidence, &transcript);
        assert_eq!(evidence.classify(0), RestartDecision::Ask(AskReason::BatchLimit));
        evidence.session_live_elsewhere = true;
        assert_eq!(evidence.classify(1), RestartDecision::NotResumable);
        evidence.session_live_elsewhere = false;
        evidence.repo_fingerprint = Some("changed");
        assert_eq!(evidence.classify(1), RestartDecision::Ask(AskReason::RepositoryChanged));
        evidence.repo_fingerprint = Some("repo");
        evidence.now_millis = 20_000_000;
        assert_eq!(evidence.classify(1), RestartDecision::Ask(AskReason::Downtime));
        evidence.now_millis = 3000;
        let mut unsupported = record.clone();
        unsupported.kind = PanelKind::Pi;
        evidence.kind = PanelKind::Pi;
        evidence.handoff = Some(&unsupported);
        assert_eq!(evidence.classify(1), RestartDecision::Ask(AskReason::MissingEvidence));
        evidence.kind = PanelKind::Claude;
        evidence.handoff = Some(&record);
        let mut failed = ledger.clone();
        failed.state = TurnState::Failed;
        let mut blocked = transcript.clone();
        blocked.state = TurnState::Blocked;
        let mut blocked_record = record.clone();
        blocked_record.final_transcript = Some(blocked.clone());
        evidence.handoff = Some(&blocked_record);
        evidence.transcript = Some(&blocked);
        evidence.ledger = Some(&failed);
        assert_eq!(evidence.classify(1), RestartDecision::NotResumable);
        evidence.transcript = Some(&transcript);
        evidence.ledger = Some(&ledger);
        evidence.handoff = None;
        assert_eq!(evidence.classify(1), RestartDecision::NotResumable);
        evidence.stale_live_session = true;
        assert_eq!(evidence.classify(1), RestartDecision::Ask(AskReason::UncleanShutdown));
        evidence.transcript = Some(&blocked);
        evidence.stale_live_session = false;
        assert_eq!(evidence.classify(1), RestartDecision::Ask(AskReason::WaitingForUser));
        evidence.ledger = Some(&failed);
        assert_eq!(evidence.classify(1), RestartDecision::NotResumable);
        evidence.transcript = Some(&transcript);
        assert_eq!(evidence.classify(1), RestartDecision::NotResumable);
    }
    #[test]
    fn a_horizon_interrupt_needs_the_same_final_turn_and_session() {
        let transcript = TranscriptSnapshot {
            bytes: 10,
            tail_sha256: [1; 32],
            state: TurnState::Interrupted,
        };
        let mut record = SuspendRecord {
            kind: PanelKind::Claude,
            before_cancel: Some(TranscriptSnapshot {
                bytes: 5,
                tail_sha256: [0; 32],
                state: TurnState::Working,
            }),
            panel_local_id: "panel".into(),
            session_id: "session".into(),
            prompt_id: "prompt".into(),
            generation: 1,
            suspended_at_millis: 1000,
            cwd: "/repo".into(),
            repo_fingerprint: Some("repo".into()),
            final_transcript: Some(transcript.clone()),
            cancelled_by_horizon: true,
        };
        let mut ledger = TurnLedger {
            session_id: "session".into(),
            prompt_id: Some("prompt".into()),
            state: TurnState::Interrupted,
            generation: 1,
            ended_at_millis: Some(2000),
            ..TurnLedger::default()
        };
        let policy = ResumePolicy {
            enabled: true,
            ..ResumePolicy::default()
        };
        let decision = |record: &SuspendRecord, ledger: &TurnLedger, transcript: &TranscriptSnapshot| {
            RestartEvidence {
                kind: PanelKind::Claude,
                panel_local_id: "panel",
                session_id: "session",
                cwd: "/repo",
                policy: &policy,
                handoff: Some(record),
                ledger: Some(ledger),
                transcript: Some(transcript),
                repo_fingerprint: Some("repo"),
                session_live_elsewhere: false,
                stale_live_session: false,
                now_millis: 3000,
            }
            .classify(1)
        };
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::Resume);
        record.kind = PanelKind::Pi;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        record.kind = PanelKind::Claude;
        let before = record.before_cancel.take();
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        record.before_cancel = Some(transcript.clone());
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        record.before_cancel = before;
        ledger.state = TurnState::Failed;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        ledger.state = TurnState::Interrupted;
        record.cancelled_by_horizon = false;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        record.cancelled_by_horizon = true;
        ledger.state = TurnState::Finished;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        ledger.state = TurnState::Interrupted;
        ledger.generation = 2;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        ledger.generation = 1;
        ledger.deliberate_exit = true;
        assert_eq!(decision(&record, &ledger, &transcript), RestartDecision::NotResumable);
        ledger.deliberate_exit = false;
        let mut advanced = transcript.clone();
        advanced.bytes += 1;
        assert_eq!(decision(&record, &ledger, &advanced), RestartDecision::NotResumable);
        let mut working = transcript.clone();
        working.state = TurnState::Working;
        record.final_transcript = Some(working.clone());
        record.before_cancel = None;
        assert_eq!(decision(&record, &ledger, &working), RestartDecision::NotResumable);
        record.before_cancel = Some(TranscriptSnapshot {
            bytes: 5,
            tail_sha256: [0; 32],
            state: TurnState::Working,
        });
        record.final_transcript = None;
        assert_eq!(
            decision(&record, &ledger, &transcript),
            RestartDecision::Ask(AskReason::MissingEvidence)
        );
    }
}
