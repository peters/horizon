//! Host-owned allocation records and their exact quota leases.
use std::collections::BTreeMap;

pub use super::manifest::recovery::RemoteAllocationSummary;
use super::remote_slots::SlotLease;
use horizon_browser::RemoteAllocation;

pub struct HeldRemoteAllocation {
    pub allocation: RemoteAllocation,
    pub provider: String,
    pub workspace: String,
    pub owner: String,
    pub lease: Option<SlotLease>,
}

#[derive(Default)]
pub struct RemoteAllocations {
    records: BTreeMap<String, HeldRemoteAllocation>,
    completed: std::collections::VecDeque<String>,
}

impl RemoteAllocations {
    pub fn insert(&mut self, record: HeldRemoteAllocation) {
        self.records.insert(record.allocation.reference().to_string(), record);
    }

    /// Refresh the exact panel instance's scope before its manifest disappears.
    pub fn update_scope(&mut self, allocation: &RemoteAllocation, workspace: &str, owner: Option<&str>) {
        if let Some(record) = self.records.get_mut(allocation.reference()) {
            record.workspace = workspace.to_string();
            record.owner = owner.unwrap_or_default().to_string();
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for (reference, record) in &mut self.records {
            if record.allocation.is_released() && !self.completed.contains(reference) {
                record.lease = None;
                self.completed.push_back(reference.clone());
                changed = true;
            }
        }
        while self.completed.len() > 128 {
            if let Some(reference) = self.completed.pop_front() {
                self.records.remove(&reference);
            }
        }
        changed
    }

    #[must_use]
    pub fn summaries(&mut self, scope: Option<(&str, &str)>) -> Vec<RemoteAllocationSummary> {
        let summaries = self
            .records
            .values()
            .filter_map(|record| {
                let status = match scope {
                    Some((owner, workspace)) => record.allocation.status_for(
                        super::manifest::host_instance(),
                        owner,
                        workspace,
                        record.owner == owner && record.workspace == workspace,
                    )?,
                    None => record.allocation.status(),
                };
                Some(RemoteAllocationSummary {
                    reference: record.allocation.reference().to_string(),
                    provider: record.provider.clone(),
                    status,
                    message: status.message().to_string(),
                })
            })
            .collect();
        // Release is monotonic: every Released snapshot must have relinquished
        // its cross-instance lease before either UI or MCP can publish it.
        self.poll();
        summaries
    }

    /// Unknown and unauthorized references intentionally have the same result.
    #[must_use]
    pub fn reconcile(&self, reference: &str, scope: Option<(&str, &str)>) -> bool {
        let Some(record) = self.records.get(reference) else {
            return false;
        };
        if let Some((owner, workspace)) = scope {
            record.allocation.reconcile_for(
                super::manifest::host_instance(),
                owner,
                workspace,
                record.owner == owner && record.workspace == workspace,
            )
        } else {
            record.allocation.reconcile();
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::RemoteRecoveryStatus;

    #[test]
    fn a_published_retired_allocation_requires_final_scope_and_history_is_bounded() {
        let mut records = RemoteAllocations::default();
        let allocation = RemoteAllocation::default();
        allocation.mark_published();
        allocation.cancel_before_launch();
        records.insert(HeldRemoteAllocation {
            allocation: allocation.clone(),
            provider: "grid".into(),
            workspace: "workspace".into(),
            owner: "owner".into(),
            lease: None,
        });
        assert!(records.summaries(Some(("owner", "workspace"))).is_empty());
        assert!(!records.reconcile(allocation.reference(), Some(("owner", "workspace"))));
        assert_eq!(records.summaries(None).len(), 1, "the host user can inspect it");
        for _ in 0..140 {
            let allocation = RemoteAllocation::default();
            allocation.cancel_before_launch();
            records.insert(HeldRemoteAllocation {
                allocation,
                provider: "grid".into(),
                workspace: "workspace".into(),
                owner: "owner".into(),
                lease: None,
            });
        }
        records.poll();
        assert_eq!(records.summaries(None).len(), 128);
    }

    #[test]
    fn recovery_is_owner_and_workspace_scoped_and_releases_only_its_lease() {
        let dir = tempfile::tempdir().expect("slots");
        let mut records = RemoteAllocations::default();
        let first = RemoteAllocation::default();
        let second = RemoteAllocation::default();
        for (allocation, owner) in [(&first, "actor-a"), (&second, "actor-b")] {
            records.insert(HeldRemoteAllocation {
                allocation: allocation.clone(),
                provider: "grid".into(),
                workspace: "workspace-a".into(),
                owner: owner.into(),
                lease: Some(super::super::remote_slots::acquire_slot(dir.path(), "quota", 2).expect("lease")),
            });
        }
        assert_eq!(records.summaries(Some(("actor-a", "workspace-a"))).len(), 1);
        assert!(records.summaries(Some(("actor-a", "workspace-b"))).is_empty());
        assert!(!records.reconcile(second.reference(), Some(("actor-a", "workspace-a"))));
        first.cancel_before_launch();
        let released = records.summaries(Some(("actor-a", "workspace-a")));
        assert_eq!(released[0].status, RemoteRecoveryStatus::Released);
        let _available = super::super::remote_slots::acquire_slot(dir.path(), "quota", 2)
            .expect("a published release makes its lease immediately available");
        assert!(matches!(
            super::super::remote_slots::acquire_slot(dir.path(), "quota", 2),
            Err(super::super::remote_slots::SlotError::Busy { .. })
        ));
        assert!(records.records[first.reference()].lease.is_none());
        assert!(records.records[second.reference()].lease.is_some());
        assert!(records.reconcile(first.reference(), Some(("actor-a", "workspace-a"))));
        assert_eq!(first.status(), RemoteRecoveryStatus::Released);
        assert!(!records.poll());
    }
}
