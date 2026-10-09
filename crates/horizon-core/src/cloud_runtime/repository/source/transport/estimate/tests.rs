use super::*;

const MIB: u64 = 1024 * 1024;

#[test]
fn git_sizes_and_speeds_are_read_in_bytes() {
    assert_eq!(transfer("12.00 MiB | 4.00 MiB/s"), Some((12 * MIB, 4 * MIB)));
    assert_eq!(transfer("512.00 KiB | 256.00 KiB/s"), Some((512 * 1024, 256 * 1024)));
    assert_eq!(
        transfer("1.50 GiB | 10.00 MiB/s").map(|(bytes, _)| bytes),
        Some(3 * 512 * MIB)
    );
    assert_eq!(transfer("900 bytes | 300 bytes/s"), Some((900, 300)));
    assert_eq!(
        transfer("12.3 MiB | 1 MiB/s").map(|(bytes, _)| bytes),
        Some(123 * MIB / 10)
    );
    for unclear in [
        "",
        "12.00 MiB",
        "12.00 MiB | fast",
        "12 parsecs | 1 MiB/s",
        "-1 MiB | 1 MiB/s",
        "1.234 MiB | 1 MiB/s",
        ".5 MiB | 1 MiB/s",
    ] {
        assert_eq!(transfer(unclear), None, "{unclear}");
    }
}

#[test]
fn with_the_size_the_whole_clone_is_judged_from_what_arrived_and_the_speed() {
    let now = Instant::now();
    let mut snapshot = Snapshot {
        expected: Some(100 * MIB),
        received: 20 * MIB,
        ..Snapshot::default()
    };
    take(&mut snapshot, RECEIVING, "30.00 MiB | 10.00 MiB/s", None, now);
    assert_eq!(snapshot.receiving, 30 * MIB);
    assert!(snapshot.whole);
    // 100 - 20 - 30 = 50 MiB at 10 MiB/s.
    assert_eq!(snapshot.ends, Some(now + Duration::from_secs(5)));
    // A phase that receives nothing keeps counting down to the same end.
    let later = now + Duration::from_secs(2);
    take(
        &mut snapshot,
        "Resolving deltas",
        "",
        Some(Duration::from_secs(60)),
        later,
    );
    assert!(snapshot.whole);
    assert_eq!(snapshot.ends, Some(now + Duration::from_secs(5)));
    // Once it ran out, the step's own estimate is all there is.
    let past = now + Duration::from_secs(6);
    take(
        &mut snapshot,
        "Resolving deltas",
        "",
        Some(Duration::from_secs(3)),
        past,
    );
    assert!(!snapshot.whole);
    assert_eq!(snapshot.ends, Some(past + Duration::from_secs(3)));
}

#[test]
fn without_the_size_or_past_it_only_the_step_is_judged() {
    let now = Instant::now();
    let mut snapshot = Snapshot::default();
    take(
        &mut snapshot,
        RECEIVING,
        "30.00 MiB | 10.00 MiB/s",
        Some(Duration::from_secs(8)),
        now,
    );
    assert!(!snapshot.whole, "no size to go by");
    assert_eq!(snapshot.ends, Some(now + Duration::from_secs(8)));
    // A size the clone already went past says nothing more.
    snapshot.expected = Some(10 * MIB);
    take(&mut snapshot, RECEIVING, "30.00 MiB | 10.00 MiB/s", None, now);
    assert!(!snapshot.whole);
    assert_eq!(snapshot.ends, None);
}

#[test]
fn an_earlier_try_counts_what_its_objects_hold() {
    let temp = tempfile::tempdir().unwrap();
    let objects = temp.path().join(".git/objects");
    std::fs::create_dir_all(objects.join("pack")).unwrap();
    std::fs::create_dir_all(objects.join("ab")).unwrap();
    std::fs::write(objects.join("pack/pack-1.pack"), vec![0; 1000]).unwrap();
    std::fs::write(objects.join("ab/cdef"), vec![0; 24]).unwrap();
    assert_eq!(received_before(temp.path()), 1024);
    assert_eq!(received_before(&temp.path().join("missing")), 0);
}

#[test]
fn a_new_step_keeps_what_earlier_ones_received_and_the_whole_estimate() {
    let progress = Progress::default();
    let ends = Instant::now() + Duration::from_secs(30);
    update(&progress, |snapshot| {
        snapshot.expected = Some(100 * MIB);
        snapshot.received = 5 * MIB;
        snapshot.receiving = 7 * MIB;
        snapshot.ends = Some(ends);
        snapshot.whole = true;
        snapshot.percent = Some(50);
    });
    super::super::announce(&progress, 2, false);
    let snapshot = progress.lock().unwrap().clone();
    assert_eq!(snapshot.step, 2);
    assert_eq!((snapshot.received, snapshot.receiving), (12 * MIB, 0));
    assert_eq!(snapshot.expected, Some(100 * MIB));
    assert_eq!((snapshot.ends, snapshot.whole), (Some(ends), true));
    assert_eq!(snapshot.percent, None, "the step starts over");
}
