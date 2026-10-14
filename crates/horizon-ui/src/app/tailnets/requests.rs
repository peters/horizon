//! Volatile owner decisions. A queued workspace request never authorizes a write.
use horizon_core::browser::manifest::provider_usage::{CredentialOperation as Operation, CredentialRequest};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
};

const LIFETIME: i64 = 15 * 60 * 1000;
const LIMIT: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Target {
    pub root: PathBuf,
    pub id: String,
    pub name: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    PendingOwner,
    Running,
    Applied,
    Failed,
    Cancelled,
    Expired,
}
impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Self::PendingOwner => "pending_owner",
            Self::Running => "running",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}
struct Entry {
    actor: String,
    target: Target,
    action: Operation,
    expires: i64,
    retain_until: i64,
    phase: Phase,
    result: Option<Receiver<bool>>,
}
#[derive(Default)]
pub(in crate::app) struct Requests {
    entries: BTreeMap<String, Entry>,
}

impl Requests {
    pub(in crate::app) fn answer_existing(
        &mut self,
        actor: &str,
        request: &CredentialRequest,
        root: &Path,
        now: i64,
    ) -> Result<Value, &'static str> {
        if !matches!(request.operation, Operation::Status | Operation::Cancel) {
            return Err("tailnet_credential_invalid_request");
        }
        let target = self
            .entries
            .get(&request.request_id)
            .filter(|entry| {
                entry.actor == actor && entry.target.root.as_path() == root && entry.target.id == request.tailnet_id
            })
            .map(|entry| entry.target.clone())
            .ok_or("tailnet_credential_unknown_request")?;
        self.answer(actor, request, target, now)
    }

    pub(in crate::app) fn poll(&mut self, now: i64) {
        self.entries.retain(|_, entry| {
            if let Some(receiver) = &entry.result {
                match receiver.try_recv() {
                    Ok(ok) => {
                        entry.phase = if ok { Phase::Applied } else { Phase::Failed };
                        entry.result = None;
                        entry.retain_until = now.saturating_add(LIFETIME);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        entry.phase = Phase::Failed;
                        entry.result = None;
                        entry.retain_until = now.saturating_add(LIFETIME);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
            if entry.phase == Phase::PendingOwner && now >= entry.expires {
                entry.phase = Phase::Expired;
            }
            entry.phase == Phase::Running || now < entry.retain_until
        });
    }
    pub(in crate::app) fn answer(
        &mut self,
        actor: &str,
        request: &CredentialRequest,
        target: Target,
        now: i64,
    ) -> Result<Value, &'static str> {
        self.poll(now);
        if !request.valid() || target.id != request.tailnet_id {
            return Err("tailnet_credential_invalid_request");
        }
        if let Some(entry) = self.entries.get_mut(&request.request_id) {
            if entry.actor != actor || entry.target.root != target.root || entry.target.id != target.id {
                return Err("tailnet_credential_unknown_request");
            }
            match request.operation {
                Operation::Cancel if entry.phase == Phase::PendingOwner => entry.phase = Phase::Cancelled,
                Operation::Cancel if entry.phase == Phase::Running => {
                    return Err("tailnet_credential_running: the write cannot be cancelled");
                }
                Operation::Status | Operation::Cancel => {}
                operation @ Operation::Remove if operation == entry.action && entry.target == target => {}
                Operation::Remove => return Err("tailnet_credential_request_conflict"),
            }
            return Ok(receipt(&request.request_id, entry));
        }
        if matches!(request.operation, Operation::Status | Operation::Cancel) {
            return Err("tailnet_credential_unknown_request");
        }
        if self.entries.len() >= LIMIT {
            return Err("tailnet_credential_queue_full");
        }
        let entry = Entry {
            actor: actor.into(),
            target,
            action: request.operation,
            expires: now.saturating_add(LIFETIME),
            retain_until: now.saturating_add(LIFETIME).saturating_add(LIFETIME),
            phase: Phase::PendingOwner,
            result: None,
        };
        let answer = receipt(&request.request_id, &entry);
        self.entries.insert(request.request_id.clone(), entry);
        Ok(answer)
    }
    pub(super) fn pending(&self, target: &Target) -> Vec<(String, String, Operation)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.phase == Phase::PendingOwner && e.target == *target)
            .map(|(id, e)| (id.clone(), e.actor.clone(), e.action))
            .collect()
    }
    /// Called only after an explicit native owner click on the exact visible row.
    pub(super) fn approve(&mut self, id: &str, target: &Target, action: Operation, now: i64) -> Option<Sender<bool>> {
        self.poll(now);
        let entry = self.entries.get_mut(id)?;
        if entry.phase != Phase::PendingOwner || entry.target != *target || entry.action != action {
            return None;
        }
        let (sender, receiver) = channel();
        entry.phase = Phase::Running;
        entry.result = Some(receiver);
        Some(sender)
    }
    pub(super) fn reject(&mut self, id: &str, target: &Target) {
        if let Some(entry) = self.entries.get_mut(id)
            && entry.target == *target
            && entry.phase == Phase::PendingOwner
        {
            entry.phase = Phase::Cancelled;
        }
    }
}
fn receipt(id: &str, entry: &Entry) -> Value {
    json!({"request_id":id,"tailnet_id":entry.target.id,"tailnet_name":entry.target.name,"operation":entry.action,"state":entry.phase.as_str(),"expires_at_millis":entry.expires})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target() -> Target {
        Target {
            root: "synthetic-desktop".into(),
            id: "work".into(),
            name: "Work".into(),
        }
    }
    fn request(operation: Operation) -> CredentialRequest {
        CredentialRequest {
            operation,
            tailnet_id: "work".into(),
            request_id: "dff44c5a-d077-4eaf-8092-615849346347".into(),
        }
    }
    #[test]
    fn submission_is_only_pending_and_cannot_be_read_or_cancelled_by_another_actor() {
        let mut requests = Requests::default();
        let mut req = request(Operation::Remove);
        assert_eq!(
            requests.answer("actor-a", &req, target(), 0).unwrap()["state"],
            "pending_owner"
        );
        req.operation = Operation::Cancel;
        assert!(requests.answer("actor-b", &req, target(), 1).is_err());
        assert_eq!(requests.pending(&target()).len(), 1);
        let mut renamed = target();
        renamed.name = "Renamed".into();
        let status = request(Operation::Status);
        assert_eq!(
            requests.answer("actor-a", &status, renamed.clone(), 1).unwrap()["tailnet_name"],
            "Work"
        );
        assert!(
            requests
                .answer("actor-a", &request(Operation::Remove), renamed, 1)
                .is_err()
        );
        req.operation = Operation::Status;
        let mut other = target();
        other.root = "other-desktop".into();
        assert!(requests.answer("actor-a", &req, other, 1).is_err());
        assert_eq!(
            requests.answer("actor-a", &req, target(), 1).unwrap()["state"],
            "pending_owner"
        );
    }
    #[test]
    fn only_exact_unexpired_owner_choice_starts_and_write_result_is_not_inferred_from_kind() {
        let mut requests = Requests::default();
        let req = request(Operation::Remove);
        requests.answer("a", &req, target(), 0).unwrap();
        assert!(
            requests
                .approve(&req.request_id, &target(), Operation::Status, 1)
                .is_none()
        );
        let mut changed = target();
        changed.name = "Renamed".into();
        assert!(requests.approve(&req.request_id, &changed, req.operation, 1).is_none());
        let sender = requests.approve(&req.request_id, &target(), req.operation, 1).unwrap();
        let cancel = request(Operation::Cancel);
        assert!(requests.answer("a", &cancel, target(), 2).is_err());
        sender.send(false).unwrap();
        assert_eq!(
            requests.answer("a", &request(Operation::Status), target(), 3).unwrap()["state"],
            "failed"
        );
    }
    #[test]
    fn requests_are_bounded_and_disconnect_does_not_report_an_applied_transaction() {
        let mut requests = Requests::default();
        for number in 0..LIMIT {
            let mut req = request(Operation::Remove);
            req.request_id = format!("00000000-0000-0000-0000-{number:012x}");
            requests.answer("a", &req, target(), 0).unwrap();
        }
        assert!(requests.answer("a", &request(Operation::Remove), target(), 0).is_err());
        let req = CredentialRequest {
            request_id: "00000000-0000-0000-0000-000000000000".into(),
            ..request(Operation::Remove)
        };
        let sender = requests.approve(&req.request_id, &target(), req.operation, 1).unwrap();
        drop(sender);
        requests.poll(2);
        let status = CredentialRequest {
            operation: Operation::Status,
            ..req
        };
        assert_eq!(requests.answer("a", &status, target(), 3).unwrap()["state"], "failed");
    }

    #[test]
    fn a_running_operation_outlives_the_owner_view_and_only_its_result_marks_applied() {
        let mut requests = Requests::default();
        let req = request(Operation::Remove);
        requests.answer("a", &req, target(), 0).unwrap();
        let owner_view = target();
        let sender = requests
            .approve(&req.request_id, &owner_view, req.operation, 1)
            .unwrap();
        drop(owner_view);
        assert_eq!(
            requests
                .answer("a", &request(Operation::Status), target(), LIFETIME)
                .unwrap()["state"],
            "running"
        );
        sender.send(true).unwrap();
        assert_eq!(
            requests
                .answer("a", &request(Operation::Status), target(), LIFETIME + 1)
                .unwrap()["state"],
            "applied"
        );
        assert!(
            requests
                .approve(&req.request_id, &target(), req.operation, LIFETIME + 1)
                .is_none()
        );
    }

    #[test]
    fn a_late_write_result_remains_observable_for_a_bounded_completion_window() {
        for outcome in [Some(true), Some(false), None] {
            let mut requests = Requests::default();
            let req = request(Operation::Remove);
            requests.answer("a", &req, target(), 0).unwrap();
            let sender = requests.approve(&req.request_id, &target(), req.operation, 1).unwrap();
            let completion = 4 * LIFETIME;
            let status = request(Operation::Status);
            assert_eq!(
                requests.answer("a", &status, target(), completion).unwrap()["state"],
                "running"
            );
            if let Some(succeeded) = outcome {
                sender.send(succeeded).unwrap();
            } else {
                drop(sender);
            }
            let expected = if outcome == Some(true) { "applied" } else { "failed" };
            assert_eq!(
                requests.answer("a", &status, target(), completion + 1).unwrap()["state"],
                expected
            );
            assert_eq!(
                requests.answer("a", &status, target(), completion + LIFETIME).unwrap()["state"],
                expected
            );
            assert!(
                requests
                    .answer("a", &status, target(), completion + LIFETIME + 1)
                    .is_err()
            );
        }
    }

    #[test]
    fn terminal_receipts_need_the_original_actor_root_and_id_but_not_a_current_catalog_row() {
        let mut requests = Requests::default();
        let req = request(Operation::Remove);
        let original = target();
        requests.answer("a", &req, original.clone(), 0).unwrap();
        let sender = requests.approve(&req.request_id, &original, req.operation, 1).unwrap();
        sender.send(false).unwrap();
        let status = request(Operation::Status);
        assert_eq!(
            requests.answer_existing("a", &status, &original.root, 2).unwrap()["state"],
            "failed"
        );
        assert!(requests.answer_existing("b", &status, &original.root, 3).is_err());
        assert!(
            requests
                .answer_existing("a", &status, Path::new("other-root"), 3)
                .is_err()
        );
        let other_id = CredentialRequest {
            tailnet_id: "other-saved".into(),
            ..status
        };
        assert!(requests.answer_existing("a", &other_id, &original.root, 3).is_err());
        assert!(requests.answer_existing("a", &req, &original.root, 3).is_err());
    }

    #[test]
    fn cancellation_expiry_and_duplicate_ids_never_start_a_write() {
        for expired in [false, true] {
            let mut requests = Requests::default();
            let req = request(Operation::Remove);
            requests.answer("a", &req, target(), 0).unwrap();
            if !expired {
                requests.answer("a", &request(Operation::Cancel), target(), 1).unwrap();
            }
            assert!(
                requests
                    .approve(&req.request_id, &target(), req.operation, LIFETIME)
                    .is_none()
            );
            assert_eq!(
                requests.answer("a", &req, target(), LIFETIME).unwrap()["state"],
                if expired { "expired" } else { "cancelled" }
            );
            let mut renamed = target();
            renamed.name = "Another name".into();
            assert!(requests.answer("a", &req, renamed, LIFETIME).is_err());
        }
    }
}
