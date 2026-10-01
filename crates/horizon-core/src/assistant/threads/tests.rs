use super::*;

fn thread(session: &str, space: &str, updated_at: i64) -> Thread {
    Thread {
        session_id: session.to_string(),
        agent: PanelKind::Claude,
        title: String::new(),
        space: space.to_string(),
        cwd: None,
        updated_at,
    }
}

#[test]
fn a_new_session_is_added_once_and_a_known_one_only_gains_a_title() {
    let mut threads = Threads::default();
    assert!(threads.upsert(thread("a", "work", 1)));
    assert!(!threads.upsert(thread("a", "work", 2)), "same session, nothing new");

    let titled = Thread {
        title: "Fix the cloud tests".into(),
        ..thread("a", "work", 3)
    };
    assert!(threads.upsert(titled));
    assert_eq!(threads.get("a").map(|t| t.title.as_str()), Some("Fix the cloud tests"));
    assert_eq!(
        threads.get("a").map(|t| t.updated_at),
        Some(1),
        "adding a title is not use"
    );

    assert!(
        !threads.upsert(thread("a", "work", 4)),
        "an empty title never erases one"
    );
    assert_eq!(threads.get("a").map(|t| t.title.as_str()), Some("Fix the cloud tests"));
}

#[test]
fn threads_group_by_space_newest_first() {
    let mut threads = Threads::default();
    threads.upsert(thread("old-work", "work", 1));
    threads.upsert(thread("home", "home", 5));
    threads.upsert(thread("new-work", "work", 9));

    let groups = threads.by_space();

    let spaces: Vec<_> = groups.iter().map(|(space, _)| *space).collect();
    assert_eq!(spaces, ["work", "home"], "the space used most recently comes first");
    let work: Vec<_> = groups[0].1.iter().map(|t| t.session_id.as_str()).collect();
    assert_eq!(work, ["new-work", "old-work"]);
}

#[test]
fn touching_moves_a_thread_to_the_front_of_its_space() {
    let mut threads = Threads::default();
    threads.upsert(thread("a", "work", 1));
    threads.upsert(thread("b", "work", 2));
    assert!(threads.touch("a", 10));
    assert!(!threads.touch("a", 10));
    assert!(!threads.touch("missing", 10));
    assert_eq!(threads.by_space()[0].1[0].session_id, "a");
}

#[test]
fn forgetting_removes_only_the_named_thread() {
    let mut threads = Threads::default();
    threads.upsert(thread("a", "work", 1));
    threads.upsert(thread("b", "work", 2));
    assert!(threads.forget("a"));
    assert!(!threads.forget("a"));
    assert!(threads.get("a").is_none());
    assert!(threads.get("b").is_some());
}

#[test]
fn the_oldest_threads_are_dropped_past_the_limit() {
    let mut threads = Threads::default();
    for index in 0..MAX_THREADS + 5 {
        let used = i64::try_from(index).expect("small index");
        threads.upsert(thread(&format!("s{index}"), "work", used));
    }
    assert_eq!(threads.threads.len(), MAX_THREADS);
    assert!(threads.get("s0").is_none());
    assert!(threads.get(&format!("s{}", MAX_THREADS + 4)).is_some());
}

#[test]
fn threads_round_trip_and_a_corrupt_file_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let home = HorizonHome::from_root(dir.path().to_path_buf());
    let mut threads = Threads::default();
    threads.upsert(Thread {
        title: "Plan".into(),
        ..thread("a", "work", 1)
    });
    threads.save(&home).unwrap();
    assert_eq!(Threads::load(&home), threads);

    std::fs::write(dir.path().join("assistant/threads.json"), "not json").unwrap();
    assert!(Threads::load(&home).is_empty());
}

#[test]
fn untitled_threads_are_dropped_before_titled_ones() {
    let mut threads = Threads::default();
    threads.upsert(Thread {
        title: "Keep".into(),
        ..thread("titled-old", "work", 0)
    });
    for index in 1..=MAX_THREADS {
        threads.upsert(thread(
            &format!("empty{index}"),
            "work",
            i64::try_from(index).expect("small index"),
        ));
    }
    assert_eq!(threads.threads.len(), MAX_THREADS);
    assert!(
        threads.get("titled-old").is_some(),
        "the oldest thread has a title and survives"
    );
    assert!(threads.get("empty1").is_none(), "the oldest untitled thread goes first");
}
