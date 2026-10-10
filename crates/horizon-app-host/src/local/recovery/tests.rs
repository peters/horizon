use super::*;
use crate::local::{confirm_receipt, confirm_receipt_with_reboot};
use horizon_app_runtime::journal::{Journal, Kind, Phase, execution::Workspace};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Fixture {
    root: tempfile::TempDir,
    workspace: Workspace,
    id: Uuid,
    path: PathBuf,
    directory: Directory,
    original: Vec<u8>,
}

fn fixture(kind: Kind, boot_id: Option<Uuid>, complete: bool, guardian_pid: u32) -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let journal = Arc::new(Journal::open(&path.join("private"), &crate::actor::tests::account()).unwrap());
    let workspace = Workspace::open(journal, Uuid::new_v4(), &path).unwrap();
    let id = workspace.start(kind, Duration::from_secs(60)).unwrap().id;
    workspace.journal().local_intent(workspace.owner(), id).unwrap();
    let operation = Uuid::new_v4();
    workspace
        .journal()
        .local_started(workspace.owner(), id, operation, guardian_pid)
        .unwrap();
    let directory = Directory::open(&path).unwrap();
    directory.create_child(&id.simple().to_string()).unwrap();
    let path = path.join(id.simple().to_string());
    let directory = Directory::open(&path).unwrap();
    let mut receipt = serde_json::json!({"operation":operation,"guardian_pid":guardian_pid,"child_pid":null,"complete":complete,"task":"synthetic-original"});
    if let Some(boot_id) = boot_id {
        receipt["boot_id"] = boot_id.to_string().into();
    }
    directory.save(&receipt).unwrap();
    let original = std::fs::read(path.join("process.json")).unwrap();
    Fixture {
        root,
        workspace,
        id,
        path,
        directory,
        original,
    }
}

impl Fixture {
    fn assert_original(&self) {
        assert_eq!(std::fs::read(self.path.join("process.json")).unwrap(), self.original);
    }
    fn status(&self) -> Phase {
        self.workspace
            .journal()
            .status(self.workspace.owner(), self.id)
            .unwrap()
            .phase
    }
    fn notes(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap().to_str().unwrap().starts_with("reboot-"))
            .collect()
    }
}

#[test]
fn recorded_reboot_releases_exact_local_journal_without_touching_reused_pid_or_receipt() {
    for kind in [Kind::Run, Kind::Tunnel] {
        let fixture = fixture(kind, Some(Uuid::new_v4()), false, std::process::id());
        confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).unwrap();
        assert_eq!(fixture.status(), Phase::Complete);
        fixture.assert_original();
        let notes = fixture.notes();
        assert_eq!(notes.len(), 1);
        let note: serde_json::Value = serde_json::from_slice(&std::fs::read(&notes[0]).unwrap()).unwrap();
        assert_eq!(note["resource"], fixture.id.to_string());
        assert_eq!(note["proof"], "recorded_earlier_boot");
        assert_eq!(note["current_boot_id"], boot::current().unwrap().to_string());
        assert!(std::fs::metadata(format!("/proc/{}", std::process::id())).is_ok());
    }
}

#[test]
fn same_boot_unknown_boot_and_mismatched_receipt_remain_held() {
    for boot_id in [boot::current(), None] {
        let fixture = fixture(Kind::Run, boot_id, false, std::process::id());
        assert!(confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).is_err());
        assert_eq!(fixture.status(), Phase::Uncertain);
        fixture.assert_original();
        assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
    }
    let fixture = fixture(Kind::Tunnel, Some(Uuid::new_v4()), false, std::process::id());
    let mut mismatch: serde_json::Value = serde_json::from_slice(&fixture.original).unwrap();
    mismatch["operation"] = Uuid::new_v4().to_string().into();
    fixture.directory.save(&mismatch).unwrap();
    assert!(confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).is_err());
    assert_eq!(fixture.status(), Phase::Uncertain);
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
}

#[test]
fn legacy_operator_confirmation_keeps_receipt_and_records_positive_attestation() {
    let fixture = fixture(Kind::Run, None, false, u32::MAX);
    let confirmation = RebootConfirmation::new(boot::current().unwrap(), vec![fixture.id]).unwrap();
    confirmation.validate(&fixture.workspace).unwrap();
    assert!(confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).is_err());
    confirm_receipt_with_reboot(&fixture.workspace, fixture.id, &fixture.path, Some(&confirmation)).unwrap();
    assert_eq!(fixture.status(), Phase::Complete);
    fixture.assert_original();
    let notes = fixture.notes();
    assert_eq!(notes.len(), 1);
    let note: serde_json::Value = serde_json::from_slice(&std::fs::read(&notes[0]).unwrap()).unwrap();
    assert_eq!(note["proof"], "operator_confirmed_legacy_boot");
}

#[test]
fn operator_confirmation_refuses_live_legacy_guardian_and_foreign_operation() {
    let fixture = fixture(Kind::Run, None, false, std::process::id());
    let confirmation = RebootConfirmation::new(boot::current().unwrap(), vec![fixture.id]).unwrap();
    assert!(confirm_receipt_with_reboot(&fixture.workspace, fixture.id, &fixture.path, Some(&confirmation)).is_err());
    assert_eq!(fixture.status(), Phase::Uncertain);
    fixture.assert_original();
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
    assert!(
        RebootConfirmation::new(boot::current().unwrap(), vec![Uuid::new_v4()])
            .unwrap()
            .validate(&fixture.workspace)
            .is_err()
    );
    for ids in [
        vec![],
        vec![fixture.id, fixture.id],
        vec![Uuid::nil()],
        vec![Uuid::new_v4(); 65],
    ] {
        assert!(RebootConfirmation::new(boot::current().unwrap(), ids).is_err());
    }
    assert!(RebootConfirmation::new(Uuid::new_v4(), vec![fixture.id]).is_err());
}

#[test]
fn completed_legacy_receipt_still_uses_original_cleanup_proof() {
    let fixture = fixture(Kind::Run, None, true, u32::MAX);
    confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).unwrap();
    assert_eq!(fixture.status(), Phase::Complete);
    fixture.assert_original();
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
}

#[test]
fn operator_confirmation_refuses_completed_local_operation() {
    let fixture = fixture(Kind::Run, None, true, u32::MAX);
    confirm_receipt(&fixture.workspace, fixture.id, &fixture.path).unwrap();
    let confirmation = RebootConfirmation::new(boot::current().unwrap(), vec![fixture.id]).unwrap();
    assert_eq!(confirmation.validate(&fixture.workspace), Err(Error::OperationInvalid));
    assert_eq!(fixture.status(), Phase::Complete);
    fixture.assert_original();
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
}

#[test]
fn confirmation_requires_all_pending_local_ids_before_any_state_change() {
    use crate::bootstrap::tests::state_snapshot;
    let fixture = fixture(Kind::Run, None, false, u32::MAX);
    let second = fixture.workspace.start(Kind::Tunnel, Duration::from_secs(60)).unwrap();
    assert_eq!(second.pending_resources, 0);
    let boot_id = boot::current().unwrap();
    let before = state_snapshot(fixture.root.path());
    for ids in [
        vec![fixture.id],
        vec![second.id],
        vec![fixture.id, second.id, Uuid::new_v4()],
    ] {
        let confirmation = RebootConfirmation::new(boot_id, ids).unwrap();
        assert!(confirmation.validate(&fixture.workspace).is_err());
        assert_eq!(state_snapshot(fixture.root.path()), before);
    }
    let confirmation = RebootConfirmation::new(boot_id, vec![second.id, fixture.id]).unwrap();
    confirmation.validate(&fixture.workspace).unwrap();
    assert_eq!(state_snapshot(fixture.root.path()), before);
    fixture.assert_original();
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
}

#[test]
fn unavailable_recovery_note_keeps_the_original_journal_resource_held() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture(Kind::Run, Some(Uuid::new_v4()), false, std::process::id());
    std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = confirm_receipt(&fixture.workspace, fixture.id, &fixture.path);
    std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fixture.status(), Phase::Uncertain);
    fixture.assert_original();
    assert_eq!(fixture.notes(), Vec::<PathBuf>::new());
}
