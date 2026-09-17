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
    pub panel: Option<String>,
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

    pub fn attach_panel(&mut self, reference: &str, panel: String) {
        if let Some(record) = self.records.get_mut(reference) {
            record.panel = Some(panel);
        }
    }

    /// Refresh scope while a panel is live, before its manifest disappears.
    pub fn update_scope(&mut self, panel: &str, workspace: &str, owner: Option<&str>) {
        for record in self
            .records
            .values_mut()
            .filter(|record| record.panel.as_deref() == Some(panel))
        {
            record.workspace = workspace.to_string();
            record.owner = owner.unwrap_or_default().to_string();
        }
    }

    pub fn expect_workspace(&self, panel: &str, workspace: &str) {
        for record in self
            .records
            .values()
            .filter(|record| record.panel.as_deref() == Some(panel))
        {
            record.allocation.expect_workspace(workspace);
        }
    }

    pub fn confirm_scope(&self, panel: &str, confirmed: bool) {
        for record in self
            .records
            .values()
            .filter(|record| record.panel.as_deref() == Some(panel))
        {
            record.allocation.confirm_scope(confirmed);
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
    pub fn summaries(&self, scope: Option<(&str, &str)>) -> Vec<RemoteAllocationSummary> {
        self.records
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
            .collect()
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
            panel: None,
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
                panel: None,
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
                panel: None,
                lease: Some(super::super::remote_slots::acquire_slot(dir.path(), "quota", 2).expect("lease")),
            });
        }
        assert_eq!(records.summaries(Some(("actor-a", "workspace-a"))).len(), 1);
        assert!(records.summaries(Some(("actor-a", "workspace-b"))).is_empty());
        assert!(!records.reconcile(second.reference(), Some(("actor-a", "workspace-a"))));
        first.cancel_before_launch();
        assert!(records.poll());
        assert!(records.records[first.reference()].lease.is_none());
        assert!(records.records[second.reference()].lease.is_some());
        assert!(records.reconcile(first.reference(), Some(("actor-a", "workspace-a"))));
        assert_eq!(first.status(), RemoteRecoveryStatus::Released);
        assert!(!records.poll());
    }
}
