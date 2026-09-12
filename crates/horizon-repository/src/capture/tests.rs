use super::*;

fn enrollment() -> Enrollment {
    serde_json::from_value(serde_json::json!({"version":1,
        "preparation":{"version":1,"workspace_local_id":"fixture",
            "runtime_id":"00000000-0000-4000-8000-000000000001",
            "source":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":"work/one"},
            "work_branch":"work/one"},
        "selected":["source.txt"],"retained_volume_attested":true}))
    .unwrap()
}

#[test]
fn binding_is_explicit_and_rejects_excluded_or_ambiguous_selection() {
    let original = enrollment().binding().unwrap();
    for paths in [
        vec![],
        vec![".env"],
        vec![".ssh/key"],
        vec!["../other"],
        vec!["a", "a"],
        vec!["node_modules/a"],
        vec!["a"; 129],
    ] {
        let mut request = enrollment();
        request.selected = paths.into_iter().map(str::to_owned).collect();
        assert!(request.binding().is_err());
    }
    let mut request = enrollment();
    request.retained_volume_attested = false;
    assert!(request.binding().is_err());
    request.retained_volume_attested = true;
    request.selected.push("second.txt".to_owned());
    assert_ne!(request.binding().unwrap(), original);
    request = enrollment();
    request.preparation.workspace_local_id = "another".to_owned();
    assert_ne!(request.binding().unwrap(), original);
}

#[test]
fn reordered_selection_keeps_identity_but_changed_paths_do_not() {
    let mut request = enrollment();
    request.selected = vec!["z.txt".into(), "a.txt".into(), "source.txt".into()];
    let original = request.binding().unwrap();
    request.selected.reverse();
    assert_eq!(request.binding().unwrap(), original);
    request.selected.sort();
    assert_eq!(request.binding().unwrap(), original);
    request.selected[0] = "changed.txt".into();
    assert_ne!(request.binding().unwrap(), original);
}

#[test]
fn rejection_is_bounded_redacted_and_never_acknowledges_capture() {
    for plan in [false, true] {
        for bytes in [
            b"private malformed bytes".to_vec(),
            vec![b' '; usize::try_from(REQUEST_LIMIT).unwrap() + 1],
            b"{}{}".to_vec(),
            b"{\"enrollment\":{},\"available_bytes\":0}".to_vec(),
        ] {
            let mut output = vec![];
            assert_eq!(
                run(plan, &mut bytes.as_slice(), &mut output, &mut std::io::sink()),
                ExitCode::from(2)
            );
            let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(value["reason"], "invalid");
            for field in ["binding", "manifest", "record_sha256", "record_bytes"] {
                assert!(value[field].is_null());
            }
            assert!(!String::from_utf8(output).unwrap().contains("private"));
        }
    }
    assert_eq!(
        run(
            true,
            &mut b"{}".as_slice(),
            &mut [0; 0].as_mut_slice(),
            &mut std::io::sink()
        ),
        ExitCode::from(3)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn encoded_existing_bundle_needs_no_new_quota_but_missing_and_invalid_are_distinct() {
    use horizon_core::repository_overlay::{RepositoryOverlayPlan, bundle::VerifiedOverlayBlob};
    let blob = VerifiedOverlayBlob::new(b"protected literal bytes".to_vec()).unwrap();
    let change = OverlayChange::new(
        "selected.txt".into(),
        OverlayContent::File {
            sha256: blob.sha256().clone(),
            bytes: blob.bytes().len() as u64,
            executable: false,
        },
    )
    .unwrap();
    let plan = RepositoryOverlayPlan::new(enrollment().preparation.source, [], [change]).unwrap();
    let bundle = RepositoryOverlayBundle::new(plan, [blob]).unwrap();
    let encoded = codec::encode(&bundle).unwrap();
    let decoded = codec::decode(&encoded).unwrap();
    assert_eq!(decoded, bundle);
    assert!(!needs_space(Ok(decoded), &bundle).unwrap());
    assert!(needs_space(Err(BundleStoreError::Missing), &bundle).unwrap());
    assert!(matches!(
        needs_space(Err(BundleStoreError::Conflict), &bundle),
        Err(Reason::Storage)
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn named_capacity_counts_complete_slots_and_never_admits_partial_or_flat_records() {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    };
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "horizon-capture-capacity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let root = Fixture(path);
    let owner = root.0.metadata().unwrap().uid();
    assert_eq!(available(&root.0, owner).unwrap(), CAPACITY);
    for index in 0..8 {
        let slot = root.0.join(format!("{index:064x}"));
        fs::create_dir(&slot).unwrap();
        fs::set_permissions(&slot, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(available(&root.0, owner), Err(Reason::Storage)));
        let file = fs::File::create(slot.join("record.hzov")).unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o600)).unwrap();
        file.set_len(CAPACITY / 8).unwrap();
    }
    assert_eq!(available(&root.0, owner).unwrap(), 0);
    let flat = root.0.join(format!("{}.hzov", "a".repeat(64)));
    fs::write(flat, b"not the selected layout").unwrap();
    assert!(matches!(available(&root.0, owner), Err(Reason::Storage)));
}
