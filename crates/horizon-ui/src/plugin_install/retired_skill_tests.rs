use std::path::Path;

use super::{AgentPluginHostLease, RETIRED_OFFLOAD_SKILL, user_skill_cleanup_dirs, user_skill_lease_dirs};

fn write_skill(dir: &Path) {
    std::fs::create_dir_all(dir).expect("skill directory");
    std::fs::write(dir.join("SKILL.md"), "retired instructions").expect("skill file");
}

#[test]
fn retired_skill_is_cleanup_only_in_default_and_overridden_homes() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("home");
    let custom = temp.path().join("custom");
    for (user_home, override_home, root) in [
        (Some(home.as_path()), None, home.join(".codex")),
        (Some(home.as_path()), Some(custom.as_path()), custom.clone()),
        (None, Some(custom.as_path()), custom.clone()),
    ] {
        let retired = root.join("skills").join(RETIRED_OFFLOAD_SKILL);
        let active = user_skill_lease_dirs(user_home, None, override_home);
        let cleanup = user_skill_cleanup_dirs(user_home, override_home);
        assert!(cleanup.contains(&retired));
        assert!(!active.contains(&retired));
        write_skill(&retired);
        let stale = root.join("skills/.horizon-leases/horizon-offload/stale.live");
        std::fs::create_dir_all(stale.parent().expect("lease directory")).expect("stale lease directory");
        std::fs::write(&stale, "").expect("abandoned lease");
        let unrelated = root.join("skills/user-authored");
        write_skill(&unrelated);
        let mut lease = AgentPluginHostLease::acquire(temp.path().join("hosts/new")).expect("host lease");
        lease
            .bind_user_skills_with_cleanup(&active, &cleanup)
            .expect("skill leases");
        assert!(!retired.exists(), "abandoned skill is removed at startup");
        assert!(!stale.exists(), "stale lease is reclaimed");
        assert!(unrelated.join("SKILL.md").is_file());
        assert!(!lease.covers_skill_dir(&retired));
        drop(lease);
        assert!(!retired.exists());
    }
}

#[test]
fn retired_skill_cleanup_preserves_an_older_live_host() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = temp.path().join("custom");
    let retired = root.join("skills").join(RETIRED_OFFLOAD_SKILL);
    let mut old = AgentPluginHostLease::acquire(temp.path().join("hosts/old")).expect("old host");
    old.bind_user_skills(std::slice::from_ref(&retired))
        .expect("old skill lease");
    write_skill(&retired);
    let mut new = AgentPluginHostLease::acquire(temp.path().join("hosts/new")).expect("new host");
    let cleanup = user_skill_cleanup_dirs(None, Some(&root));
    new.bind_user_skills_with_cleanup(&[], &cleanup).expect("cleanup lease");
    assert!(retired.join("SKILL.md").is_file());
    drop(new);
    assert!(retired.join("SKILL.md").is_file());
    drop(old);
    assert!(!retired.exists());
}
