use super::*;

const MIB: u64 = 1024 * 1024;

#[test]
fn git_sizes_are_read_in_bytes() {
    assert_eq!(received("12.00 MiB | 4.00 MiB/s"), Some(12 * MIB));
    assert_eq!(received("512.00 KiB | 256.00 KiB/s"), Some(512 * 1024));
    assert_eq!(received("1.50 GiB | 10.00 MiB/s"), Some(3 * 512 * MIB));
    assert_eq!(received("900 bytes | 300 bytes/s"), Some(900));
    assert_eq!(received("12.3 MiB"), Some(123 * MIB / 10), "before Git knows a speed");
    for unclear in ["", "fast", "12 parsecs", "-1 MiB", "1.234 MiB", ".5 MiB"] {
        assert_eq!(received(unclear), None, "{unclear}");
    }
}

#[test]
fn a_phase_ends_at_its_own_pace_and_receiving_counts_what_arrived() {
    let now = Instant::now();
    let mut snapshot = Snapshot::default();
    take(
        &mut snapshot,
        RECEIVING,
        "30.00 MiB | 10.00 MiB/s",
        Some(Duration::from_secs(8)),
        now,
    );
    assert_eq!(snapshot.receiving, 30 * MIB);
    assert_eq!(snapshot.ends, Some(now + Duration::from_secs(8)));
    // A phase that receives nothing leaves what arrived as it is.
    take(&mut snapshot, "Resolving deltas", "", None, now);
    assert_eq!(snapshot.receiving, 30 * MIB);
    assert_eq!(snapshot.ends, None, "too early to tell");
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
fn a_new_step_keeps_what_earlier_ones_received_and_when_the_clone_started() {
    let progress = Progress::default();
    super::super::announce(&progress, 1, false);
    let started = progress.lock().unwrap().started;
    assert!(started.is_some());
    update(&progress, |snapshot| {
        snapshot.expected = Some(100 * MIB);
        snapshot.received = 5 * MIB;
        snapshot.receiving = 7 * MIB;
        snapshot.ends = Some(Instant::now());
        snapshot.percent = Some(50);
    });
    super::super::announce(&progress, 2, false);
    let snapshot = progress.lock().unwrap().clone();
    assert_eq!(snapshot.step, 2);
    assert_eq!((snapshot.received, snapshot.receiving), (12 * MIB, 0));
    assert_eq!(snapshot.expected, Some(100 * MIB));
    assert_eq!(snapshot.started, started, "the clone's own start");
    assert_eq!((snapshot.ends, snapshot.percent), (None, None), "the phase starts over");
}

#[test]
fn deltas_are_judged_by_their_last_stretch_and_host_phases_not_at_all() {
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    // Resolving git/git's deltas: 60% in about 1 s, then about 4% a second.
    let mut seen = Seen::new(start);
    assert_eq!(seen.left(RESOLVING, 60, at(1.2)), None, "too early to tell");
    seen.left(RESOLVING, 64, at(2.2));
    seen.left(RESOLVING, 68, at(3.2));
    seen.left(RESOLVING, 72, at(4.2));
    let left = seen.left(RESOLVING, 76, at(5.2)).unwrap();
    // 12% over the last 3 s: 24% more takes about 6 s, where the pace since the start
    // would say about 1.6 s.
    assert!((5.5..6.5).contains(&left.as_secs_f64()), "{left:?}");
    // Receiving goes by its pace since it began.
    let mut receiving = Seen::new(start);
    assert_eq!(receiving.left(RECEIVING, 25, at(5.0)), Some(Duration::from_secs(15)));
    // A host's compressing jumps, so it tells nothing.
    let mut compressing = Seen::new(start);
    assert_eq!(compressing.left("Compressing objects", 30, at(5.0)), None);
}
