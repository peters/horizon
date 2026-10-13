//! The storage New cloud lets a person choose, the container disk and the workspace
//! volume's size and type, stays the worker's, as its provider and size do. Only the
//! rebuilt image must still fit on the worker's container disk.
use super::super::compatibility::{BUILT_IMAGE_TOO_LARGE, BYTES_PER_GB, IMAGE_TOO_LARGE};
use super::*;
use horizon_cloud::runpod::volumes::Tier;

/// Rebuilds at the commit the cloud was created at, whose profile is `committed`: the
/// worker is switched in place and keeps its bound profile and storage.
fn assert_rebuilt_where_it_is(fixture: &Fixture, committed: &Profile) {
    let before = fixture.state();
    assert_ne!(before.profile.storage, committed.storage);
    let mut script = Script::new(fixture);
    script.head.revision.clone_from(&before.revision);
    script.head.config = Some(config(committed));
    assert_eq!(rebuild_with(fixture, &script).unwrap(), Driven::Committed);
    assert_eq!(script.calls.borrow()[..3], ["head", "build", "replace Next"]);
    let after = fixture.state();
    assert_eq!(after.profile, before.profile);
    assert_eq!(after.spec.as_ref().unwrap().profile, before.profile);
    assert_eq!(after.spec.unwrap().image_digest, digest('b'));
    assert_eq!(after.operation, before.operation, "the same worker");
    // The same workspace volume, now recorded with the rebuilt image.
    assert_eq!(fixture.storage_image(), digest('b'));
}

#[test]
fn a_container_disk_chosen_in_new_cloud_is_not_a_change() {
    for chosen in [10, 60] {
        let fixture = Fixture::new();
        let committed = fixture.state().profile;
        fixture.bind_storage(|storage| storage.container_gb = chosen);
        assert_rebuilt_where_it_is(&fixture, &committed);
    }
}

#[test]
fn a_workspace_size_chosen_in_new_cloud_is_not_a_change() {
    // Smaller than the committed size, and larger.
    for chosen in [10, 60] {
        let fixture = Fixture::new();
        let committed = fixture.state().profile;
        fixture.bind_storage(|storage| storage.volume_gb = chosen);
        assert_rebuilt_where_it_is(&fixture, &committed);
    }
}

#[test]
fn a_workspace_type_chosen_in_new_cloud_is_not_a_change() {
    for (chosen, committed_tier) in [
        (Tier::HighPerformance, Tier::Standard),
        (Tier::Standard, Tier::HighPerformance),
    ] {
        let fixture = Fixture::new();
        let mut committed = fixture.state().profile;
        committed.storage.volume_tier = committed_tier;
        fixture.bind_storage(|storage| storage.volume_tier = chosen);
        assert_rebuilt_where_it_is(&fixture, &committed);
    }
}

/// `rebuild_with`, with the output lines it emits.
fn rebuild_reporting(fixture: &Fixture, script: &Script) -> (Result<Driven>, Vec<String>) {
    let output = RefCell::new(Vec::new());
    let emit = |event| {
        if let Event::Output(line) = event {
            output.borrow_mut().push(line);
        }
    };
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let recipes = committed_recipes(script, &state, "dev", &emit).unwrap();
    begin(script, &store, &mut state, recipes).unwrap();
    let driven = drive(script, &store, &mut state, &emit);
    (driven, output.into_inner())
}

#[test]
fn an_image_larger_than_the_bound_container_disk_is_refused_before_the_worker_changes() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let disk = u64::from(before.profile.storage.container_gb) * BYTES_PER_GB;
    // A larger committed container disk is a default for new clouds; it gives this worker no room.
    let mut committed = before.profile.clone();
    committed.storage.container_gb *= 10;
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&committed));
    script.unpacked = Some(disk + 1);
    let (driven, output) = rebuild_reporting(&fixture, &script);
    let error = driven.unwrap_err().to_string();
    assert_eq!(error, IMAGE_TOO_LARGE);
    assert!(error.contains("storage.container_gb"), "{error}");
    // Rounded up, so the image never reads as the disk's size.
    let sizes = "The rebuilt image is 20.1 GB unpacked; this cloud's container disk (storage.container_gb) is 20 GB";
    assert!(output.iter().any(|line| line == sizes), "{output:?}");
    // Nothing reached the worker, and nothing stays pending.
    assert_eq!(*script.calls.borrow(), ["head", "build"]);
    let after = fixture.state();
    assert!(after.image_replacement.is_none());
    assert_eq!(after.spec, before.spec);
    assert_eq!(after.stage, Stage::Ready);
    assert_eq!(fixture.storage_image(), digest('a'));
    // An image that fills the disk exactly fits.
    script.unpacked = Some(disk);
    script.calls.borrow_mut().clear();
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    assert_eq!(fixture.state().spec.unwrap().image_digest, digest('b'));
}

#[test]
fn an_image_fits_the_container_disk_chosen_in_new_cloud_not_the_committed_one() {
    let fixture = Fixture::new();
    let committed = fixture.state().profile;
    fixture.bind_storage(|storage| storage.container_gb = 40);
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&committed));
    script.unpacked = Some(30 * BYTES_PER_GB);
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
}

#[test]
fn a_built_image_journaled_unchecked_is_refused_and_can_be_cancelled_when_the_rebuild_continues() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let disk = u64::from(before.profile.storage.container_gb) * BYTES_PER_GB;
    // The image fits when it is built, and the rebuild stops once it is journaled as built.
    let mut script = Script::new(&fixture);
    script.fail_at.set(Some(Boundary::Built));
    assert!(matches!(rebuild_with(&fixture, &script), Err(Error::Invalid(CRASH))));
    assert!(matches!(
        fixture.state().image_replacement.unwrap().phase,
        ReplacementPhase::Built(_)
    ));
    // Continued with an image the disk cannot hold, as one an earlier Horizon version
    // journaled without the check: nothing reaches the worker, and the journal stays for
    // Cancel rebuild, which arms released hosted devices again.
    script.fail_at.set(None);
    script.unpacked = Some(disk + 1);
    script.calls.borrow_mut().clear();
    let error = recover(&fixture, &script, Recovery::Continue).unwrap_err().to_string();
    assert_eq!(error, BUILT_IMAGE_TOO_LARGE);
    assert_eq!(*script.calls.borrow(), ["verify"]);
    let after = fixture.state();
    assert!(!after.image_replacement.unwrap().requested());
    assert_eq!(after.spec, before.spec);
    assert_eq!(fixture.storage_image(), digest('a'));
    recover(&fixture, &script, Recovery::Cancel).unwrap();
    assert!(fixture.state().image_replacement.is_none());
}
