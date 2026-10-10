//! Reboot proof never rewrites a guardian receipt or signals a possibly reused process ID.
use super::Receipt;
use horizon_app_process::{boot, storage::Directory};
use horizon_app_runtime::{Error, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

/// Trusted operator attestation for exact legacy local records. Never read from app or MCP input.
/// The operator must independently confirm that these resources predate an actual host reboot.
pub struct RebootConfirmation {
    boot_id: Uuid,
    operations: BTreeSet<Uuid>,
}

impl RebootConfirmation {
    /// # Errors
    /// Requires the current Linux kernel boot identity and 1..=64 distinct nonzero operation IDs.
    pub fn new(boot_id: Uuid, operations: Vec<Uuid>) -> Result<Self> {
        let count = operations.len();
        let operations: BTreeSet<_> = operations.into_iter().collect();
        if boot_id.is_nil()
            || boot::current() != Some(boot_id)
            || !(1..=64).contains(&count)
            || operations.len() != count
            || operations.contains(&Uuid::nil())
        {
            return Err(Error::ReconciliationRequired);
        }
        Ok(Self { boot_id, operations })
    }

    #[cfg(unix)]
    pub(crate) fn validate(&self, workspace: &horizon_app_runtime::journal::execution::Workspace) -> Result<()> {
        use horizon_app_runtime::journal::{Kind, Phase};
        if boot::current() != Some(self.boot_id) {
            return Err(Error::ReconciliationRequired);
        }
        for id in &self.operations {
            let record = workspace.journal().status(workspace.owner(), *id)?;
            if !matches!(record.kind, Kind::Run | Kind::Tunnel) || record.phase == Phase::Complete {
                return Err(Error::OperationInvalid);
            }
        }
        let pending = workspace
            .journal()
            .pending(workspace.owner())?
            .into_iter()
            .filter(|record| matches!(record.kind, Kind::Run | Kind::Tunnel))
            .map(|record| record.id)
            .collect::<BTreeSet<_>>();
        if self.operations != pending {
            return Err(Error::OperationInvalid);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Proof {
    RecordedEarlierBoot,
    OperatorConfirmedLegacyBoot,
}

#[derive(Serialize)]
struct Record {
    version: u32,
    resource: Uuid,
    receipt_operation: Uuid,
    guardian_pid: u32,
    child_pid: Option<u32>,
    recorded_boot_id: Option<Uuid>,
    current_boot_id: Uuid,
    proof: Proof,
}

fn reboot_proof(
    id: Uuid,
    receipt: &Receipt,
    current: Option<Uuid>,
    confirmation: Option<&RebootConfirmation>,
    absent: impl Fn(u32) -> bool,
) -> Result<(Uuid, Proof)> {
    let current = current.filter(|id| !id.is_nil()).ok_or(Error::ReconciliationRequired)?;
    if receipt.operation.is_nil() || receipt.guardian_pid == 0 || receipt.child_pid == Some(0) {
        return Err(Error::ReconciliationRequired);
    }
    match receipt.boot_id {
        Some(previous) if !previous.is_nil() && previous != current => Ok((current, Proof::RecordedEarlierBoot)),
        None if confirmation
            .is_some_and(|confirmed| confirmed.boot_id == current && confirmed.operations.contains(&id))
            && absent(receipt.guardian_pid)
            && receipt.child_pid.is_none_or(absent) =>
        {
            Ok((current, Proof::OperatorConfirmedLegacyBoot))
        }
        _ => Err(Error::ReconciliationRequired),
    }
}

fn pid_absent(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        matches!(std::fs::metadata(format!("/proc/{pid}")), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

pub(super) fn confirm_reboot(
    directory: &Directory,
    id: Uuid,
    receipt: &Receipt,
    confirmation: Option<&RebootConfirmation>,
) -> Result<()> {
    let (current_boot_id, proof) = reboot_proof(id, receipt, boot::current(), confirmation, pid_absent)?;
    let record = Record {
        version: 1,
        resource: id,
        receipt_operation: receipt.operation,
        guardian_pid: receipt.guardian_pid,
        child_pid: receipt.child_pid,
        recorded_boot_id: receipt.boot_id,
        current_boot_id,
        proof,
    };
    let mut note = directory
        .new_file(&format!("reboot-{}.json", Uuid::new_v4().simple()))
        .map_err(|_| Error::ReconciliationRequired)?;
    serde_json::to_writer(&mut note, &record).map_err(|_| Error::ReconciliationRequired)?;
    note.sync_all().map_err(|_| Error::ReconciliationRequired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(boot_id: Option<Uuid>) -> Receipt {
        Receipt {
            operation: Uuid::new_v4(),
            guardian_pid: 11,
            child_pid: Some(12),
            boot_id,
            complete: false,
        }
    }

    #[test]
    fn earlier_boot_is_positive_proof_even_when_pids_are_reused() {
        let current = Uuid::new_v4();
        assert_eq!(
            reboot_proof(
                Uuid::new_v4(),
                &receipt(Some(Uuid::new_v4())),
                Some(current),
                None,
                |_| false
            ),
            Ok((current, Proof::RecordedEarlierBoot))
        );
        for previous in [Some(current), Some(Uuid::nil()), None] {
            assert_eq!(
                reboot_proof(Uuid::new_v4(), &receipt(previous), Some(current), None, |_| true),
                Err(Error::ReconciliationRequired)
            );
        }
        assert_eq!(
            reboot_proof(Uuid::new_v4(), &receipt(Some(Uuid::new_v4())), None, None, |_| true),
            Err(Error::ReconciliationRequired)
        );
    }

    #[test]
    fn legacy_confirmation_is_exact_and_never_overrides_same_boot_or_live_pid() {
        let id = Uuid::new_v4();
        let current = Uuid::new_v4();
        let confirmed = RebootConfirmation {
            boot_id: current,
            operations: [id].into_iter().collect(),
        };
        let legacy = receipt(None);
        assert_eq!(
            reboot_proof(id, &legacy, Some(current), Some(&confirmed), |_| true),
            Ok((current, Proof::OperatorConfirmedLegacyBoot))
        );
        for (resource, value, now) in [
            (Uuid::new_v4(), &legacy, Some(current)),
            (id, &legacy, Some(Uuid::new_v4())),
            (id, &legacy, None),
            (id, &receipt(Some(current)), Some(current)),
        ] {
            assert_eq!(
                reboot_proof(resource, value, now, Some(&confirmed), |_| true),
                Err(Error::ReconciliationRequired)
            );
        }
        for present in [11, 12] {
            assert_eq!(
                reboot_proof(id, &legacy, Some(current), Some(&confirmed), |pid| pid != present),
                Err(Error::ReconciliationRequired)
            );
        }
    }

    #[test]
    fn invalid_receipt_identity_never_proves_reboot() {
        let current = Uuid::new_v4();
        let mut invalid = receipt(Some(Uuid::new_v4()));
        invalid.operation = Uuid::nil();
        assert!(reboot_proof(Uuid::new_v4(), &invalid, Some(current), None, |_| true).is_err());
        invalid.operation = Uuid::new_v4();
        invalid.guardian_pid = 0;
        assert!(reboot_proof(Uuid::new_v4(), &invalid, Some(current), None, |_| true).is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "recovery/tests.rs"]
mod integration_tests;
