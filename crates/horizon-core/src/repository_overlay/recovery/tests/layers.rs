use super::*;

#[test]
fn index_relationships_use_semantic_content_modes_and_exact_paths() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    for (left, right, expected) in [
        (b"base".as_slice(), b"base".as_slice(), None),
        (b"local", b"base", Some(ChangeRelation::LocalOnly)),
        (b"base", b"remote", Some(ChangeRelation::RemoteOnly)),
        (b"same", b"same", Some(ChangeRelation::SameChange)),
        (b"local", b"remote", Some(ChangeRelation::Divergent)),
    ] {
        let (a, ab) = file("item", left, false);
        let (b, bb) = file("item", right, false);
        let local = fixture.resolve(vec![a], vec![], vec![ab]);
        let remote = fixture.resolve(vec![b], vec![], vec![bb]);
        let comparison = compare_recovery(&local, &remote, || false).unwrap();
        assert_eq!(comparison.index().paths().first().map(|path| path.relation), expected);
    }
    let (mode, blob) = file("item", b"base", true);
    let local = fixture.resolve(vec![mode], vec![], vec![blob]);
    let remote = fixture.resolve(vec![remove("item")], vec![], vec![]);
    assert_eq!(
        compare_recovery(&local, &remote, || false).unwrap().index().paths()[0].relation,
        ChangeRelation::Divergent
    );
    let (a, ab) = file("Case", b"A", false);
    let (b, bb) = file("case", b"B", false);
    let local = fixture.resolve(vec![a], vec![], vec![ab]);
    let remote = fixture.resolve(vec![b], vec![], vec![bb]);
    let report = compare_recovery(&local, &remote, || false).unwrap();
    assert_eq!(
        report
            .index()
            .paths()
            .iter()
            .map(|p| (p.path, p.relation))
            .collect::<Vec<_>>(),
        [
            ("Case", ChangeRelation::LocalOnly),
            ("case", ChangeRelation::RemoteOnly)
        ]
    );
}

#[test]
fn unstaged_revert_is_relative_to_shared_index_not_git_base() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let (a, ab) = file("item", b"staged", false);
    let (b, bb) = file("item", b"staged", false);
    let (revert, rb) = file("item", b"base", false);
    let local = fixture.resolve(vec![a], vec![revert], vec![ab, rb]);
    let remote = fixture.resolve(vec![b], vec![], vec![bb]);
    let report = compare_recovery(&local, &remote, || false).unwrap();
    assert_eq!(report.index().paths()[0].relation, ChangeRelation::SameChange);
    assert_eq!(report.working_tree().paths()[0].relation, ChangeRelation::LocalOnly);
    assert!(report.working_tree().paths()[0].local.before.is_some());
    assert!(report.working_tree().paths()[0].local.after.is_some());
}

#[test]
fn working_layer_distinguishes_unilateral_equal_and_divergent_edits() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    for (left, right, expected) in [
        (Some(b"edit".as_slice()), None, ChangeRelation::LocalOnly),
        (None, Some(b"edit".as_slice()), ChangeRelation::RemoteOnly),
        (
            Some(b"edit".as_slice()),
            Some(b"edit".as_slice()),
            ChangeRelation::SameChange,
        ),
        (
            Some(b"left".as_slice()),
            Some(b"right".as_slice()),
            ChangeRelation::Divergent,
        ),
    ] {
        let make = |value: Option<&[u8]>| match value {
            Some(bytes) => {
                let (c, b) = file("item", bytes, false);
                fixture.resolve(vec![], vec![c], vec![b])
            }
            None => fixture.resolve(vec![], vec![], vec![]),
        };
        let local = make(left);
        let remote = make(right);
        let report = compare_recovery(&local, &remote, || false).unwrap();
        assert!(report.index().paths().is_empty());
        assert_eq!(report.working_tree().paths()[0].relation, expected);
    }
}

#[test]
fn different_index_baselines_remain_explicit_even_when_working_bytes_agree() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let (index, ib) = file("item", b"staged", false);
    let (working, wb) = file("item", b"base", false);
    let local = fixture.resolve(vec![index], vec![working], vec![ib, wb]);
    let remote = fixture.resolve(vec![], vec![], vec![]);
    let report = compare_recovery(&local, &remote, || false).unwrap();
    assert_eq!(report.index().paths()[0].relation, ChangeRelation::LocalOnly);
    assert_eq!(
        report.working_tree().paths()[0].relation,
        ChangeRelation::DifferentIndexBases
    );
}

#[test]
fn literal_link_divergence_and_two_sided_deletion_are_preserved() {
    let fixture = Fixture::new(&[("item", b"base", 0o100_644)]);
    let local = fixture.resolve(vec![link("item", "left")], vec![], vec![]);
    let remote = fixture.resolve(vec![link("item", "right")], vec![], vec![]);
    let report = compare_recovery(&local, &remote, || false).unwrap();
    assert_eq!(report.index().paths()[0].relation, ChangeRelation::Divergent);
    assert_eq!(
        report.index().paths()[0].local.after,
        Some(&NamespaceEntry::Symlink { target: "left".into() })
    );
    let local = fixture.resolve(vec![remove("item")], vec![], vec![]);
    let remote = fixture.resolve(vec![remove("item")], vec![], vec![]);
    assert_eq!(
        compare_recovery(&local, &remote, || false).unwrap().index().paths()[0].relation,
        ChangeRelation::SameChange
    );
}

#[test]
fn structural_conflicts_include_inherited_index_changes_but_not_unilateral_transitions() {
    let fixture = Fixture::new(&[("node/old", b"base", 0o100_644)]);
    let (leaf, lb) = file("node", b"leaf", false);
    let local = fixture.resolve(vec![remove("node/old"), leaf], vec![], vec![lb]);
    let remote = fixture.resolve(vec![], vec![], vec![]);
    let unilateral = compare_recovery(&local, &remote, || false).unwrap();
    assert!(unilateral.index().structural_conflicts().is_empty());
    assert!(unilateral.working_tree().structural_conflicts().is_empty());
    for staged in [false, true] {
        let (child, cb) = file("node/new", b"new", false);
        let changes = vec![child, file("node-other", b"new", false).0];
        let remote = if staged {
            fixture.resolve(changes, vec![], vec![cb])
        } else {
            fixture.resolve(vec![], changes, vec![cb])
        };
        for (a, b, side) in [
            (&local, &remote, ComparisonSide::Local),
            (&remote, &local, ComparisonSide::Remote),
        ] {
            let report = compare_recovery(a, b, || false).unwrap();
            assert_eq!(report.index().structural_conflicts().len(), usize::from(staged));
            let conflicts = report.working_tree().structural_conflicts();
            assert_eq!(conflicts.len(), 1);
            assert_eq!(
                (
                    conflicts[0].ancestor,
                    conflicts[0].descendant,
                    conflicts[0].ancestor_side
                ),
                ("node", "node/new", side)
            );
        }
    }
}
