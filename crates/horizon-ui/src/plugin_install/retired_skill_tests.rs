use std::path::Path;

use horizon_core::HorizonHome;

use super::{
    AgentMcpEntry, AgentPluginHostLease, RETIRED_MCP_SKILLS, RETIRED_OFFLOAD_SKILL, install_agent_plugins_impl,
    user_skill_cleanup_dirs, user_skill_lease_dirs,
};

fn write_skill(dir: &Path) {
    std::fs::create_dir_all(dir).expect("skill directory");
    std::fs::write(dir.join("SKILL.md"), "retired instructions").expect("skill file");
}

#[test]
fn retired_mcp_skills_are_cleanup_only_in_codex_and_grok_roots() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("home");
    let custom = temp.path().join("custom");
    for (user_home, override_home, codex_root, grok_root) in [
        (Some(home.as_path()), None, home.join(".codex"), home.join(".grok")),
        (
            Some(home.as_path()),
            Some(custom.as_path()),
            custom.clone(),
            custom.clone(),
        ),
        (None, Some(custom.as_path()), custom.clone(), custom.clone()),
    ] {
        let cleanup = user_skill_cleanup_dirs(user_home, override_home, override_home);
        let active = user_skill_lease_dirs(user_home, override_home);
        for skill in RETIRED_MCP_SKILLS {
            for root in [&codex_root, &grok_root] {
                let retired = root.join("skills").join(skill);
                assert!(cleanup.contains(&retired), "{} is cleaned up", retired.display());
                assert!(!active.contains(&retired), "{} is not installed", retired.display());
            }
        }
    }
}

#[test]
fn install_run_removes_retired_mcp_skills_from_horizon_owned_directories() {
    let temp = tempfile::tempdir().expect("temp directory");
    let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
    let user_home = temp.path().join("user-home");
    let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
    let integrations = horizon_home.codex_integrations_dir();
    for skill in RETIRED_MCP_SKILLS {
        write_skill(&claude_plugin_dir.join("skills").join(skill));
        write_skill(&integrations.join(skill));
    }
    let unrelated_plugin = claude_plugin_dir.join("skills/user-authored");
    let unrelated_cache = integrations.join("user-authored");
    write_skill(&unrelated_plugin);
    write_skill(&unrelated_cache);

    install_agent_plugins_impl(
        &horizon_home,
        &claude_plugin_dir,
        Some(&user_home),
        None,
        None,
        AgentMcpEntry {
            command: Path::new("/opt/horizon"),
        },
        None,
    )
    .expect("install plugins");

    for skill in RETIRED_MCP_SKILLS {
        let plugin_copy = claude_plugin_dir.join("skills").join(skill);
        assert!(!plugin_copy.exists(), "{} is removed", plugin_copy.display());
        let cache_copy = integrations.join(skill);
        assert!(!cache_copy.exists(), "{} is removed", cache_copy.display());
    }
    assert!(unrelated_plugin.join("SKILL.md").is_file());
    assert!(unrelated_cache.join("SKILL.md").is_file());
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
        let active = user_skill_lease_dirs(user_home, override_home);
        let cleanup = user_skill_cleanup_dirs(user_home, None, override_home);
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
    let cleanup = user_skill_cleanup_dirs(None, None, Some(&root));
    new.bind_user_skills_with_cleanup(&[], &cleanup).expect("cleanup lease");
    assert!(retired.join("SKILL.md").is_file());
    drop(new);
    assert!(retired.join("SKILL.md").is_file());
    drop(old);
    assert!(!retired.exists());
}

#[test]
fn retired_skill_is_removed_when_the_older_host_exits_before_the_cleanup_host() {
    let temp = tempfile::tempdir().expect("temp directory");
    let root = temp.path().join("custom");
    let retired = root.join("skills").join(RETIRED_OFFLOAD_SKILL);
    let mut old = AgentPluginHostLease::acquire(temp.path().join("hosts/old")).expect("old host");
    old.bind_user_skills(std::slice::from_ref(&retired))
        .expect("old skill lease");
    write_skill(&retired);
    let mut new = AgentPluginHostLease::acquire(temp.path().join("hosts/new")).expect("new host");
    let cleanup = user_skill_cleanup_dirs(None, None, Some(&root));
    new.bind_user_skills_with_cleanup(&[], &cleanup).expect("cleanup lease");
    assert!(retired.join("SKILL.md").is_file());
    drop(old);
    assert!(
        !retired.exists(),
        "the last installing host leaving removes the retired copy while the cleanup host runs on"
    );
    drop(new);
}
