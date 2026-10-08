use super::super::{AgentPluginHostLease, install_agent_plugins_impl, user_skill_lease_dirs};
use super::*;

fn assert_files(dir: &Path, files: &[EmbeddedFile]) {
    for file in files {
        assert_eq!(
            std::fs::read_to_string(dir.join(file.relative_path)).expect("installed file"),
            file.content
        );
    }
}

#[test]
fn installs_complete_skills_into_bundles_and_custom_roots_idempotently() {
    let temp = tempfile::tempdir().expect("temp");
    let home = HorizonHome::from_root(temp.path().join("horizon"));
    let user = temp.path().join("user");
    let codex = temp.path().join("custom-client");
    let grok = temp.path().join("custom-provider");
    let plugin = home.claude_plugin_dir_for_host("host");
    let install = || {
        install_agent_plugins_impl(
            &home,
            &plugin,
            Some(&user),
            Some(&grok),
            Some(&codex),
            Path::new("/opt/horizon"),
            None,
        )
    };
    assert!(install().expect("install") > 0);
    for skill in MCP_SKILLS {
        for root in [
            plugin.join("skills"),
            home.codex_integrations_dir(),
            codex.join("skills"),
            grok.join("skills"),
        ] {
            assert_files(&root.join(skill.name), skill.files);
        }
        assert!(!user.join(".codex/skills").join(skill.name).exists());
    }
    assert_eq!(install().expect("repeat install"), 0);
    let skill = &MCP_SKILLS[0];
    let reference = &skill.files[1];
    std::fs::write(
        plugin.join("skills").join(skill.name).join(reference.relative_path),
        "old reference",
    )
    .expect("old bundle");
    assert_eq!(install().expect("update bundle"), 1);
    assert_files(&plugin.join("skills").join(skill.name), skill.files);
}

#[test]
fn preserves_custom_partial_and_symlinked_skill_trees() {
    let temp = tempfile::tempdir().expect("temp");
    let skill = &MCP_SKILLS[0];
    let dir = temp.path().join(skill.name);
    assert!(validate_mcp_skill(&dir).is_ok());
    sync_plugin_files(&dir, skill.files).expect("install");
    assert!(validate_mcp_skill(&dir).is_ok());
    std::fs::write(dir.join("notes.md"), "user notes").expect("custom file");
    assert!(validate_mcp_skill(&dir).is_err());
    std::fs::remove_file(dir.join("notes.md")).expect("remove fixture notes");
    std::fs::write(dir.join(skill.files[1].relative_path), "user reference").expect("custom reference");
    assert!(validate_mcp_skill(&dir).is_err());
    std::fs::remove_file(dir.join(skill.files[1].relative_path)).expect("partial fixture");
    assert!(validate_mcp_skill(&dir).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = temp.path().join("outside.md");
        std::fs::write(&outside, skill.files[1].content).expect("outside");
        symlink(&outside, dir.join(skill.files[1].relative_path)).expect("symlink");
        assert!(validate_mcp_skill(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(outside).expect("unchanged"),
            skill.files[1].content
        );
    }
}

#[test]
fn refuses_user_skill_leases_and_preserves_user_content_on_install_and_exit() {
    let temp = tempfile::tempdir().expect("temp");
    let home = HorizonHome::from_root(temp.path().join("horizon"));
    let user = temp.path().join("user");
    let skill = &MCP_SKILLS[0];
    let dir = user.join(".codex/skills").join(skill.name);
    std::fs::create_dir_all(&dir).expect("user dir");
    std::fs::write(dir.join("SKILL.md"), "user skill").expect("user skill");
    let mut lease = AgentPluginHostLease::acquire(home.agent_plugin_host_dir("host")).expect("host");
    lease
        .bind_user_skills(&user_skill_lease_dirs(Some(&user), None))
        .expect("leases");
    assert!(!lease.covers_skill_dir(&dir));
    install_agent_plugins_impl(
        &home,
        &home.claude_plugin_dir_for_host("host"),
        Some(&user),
        None,
        None,
        Path::new("/opt/horizon"),
        Some(&lease),
    )
    .expect("install");
    drop(lease);
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).expect("preserved"),
        "user skill"
    );
}

#[test]
fn complete_reference_trees_survive_until_last_host_and_preserve_later_edits() {
    let temp = tempfile::tempdir().expect("temp");
    let home = HorizonHome::from_root(temp.path().join("horizon"));
    let user = temp.path().join("user");
    let roots = user_skill_lease_dirs(Some(&user), None);
    let mut first = AgentPluginHostLease::acquire(home.agent_plugin_host_dir("first")).expect("first");
    first.bind_user_skills(&roots).expect("first leases");
    install_agent_plugins_impl(
        &home,
        &home.claude_plugin_dir_for_host("first"),
        Some(&user),
        None,
        None,
        Path::new("/opt/horizon"),
        Some(&first),
    )
    .expect("install");
    let mut second = AgentPluginHostLease::acquire(home.agent_plugin_host_dir("second")).expect("second");
    second.bind_user_skills(&roots).expect("second leases");
    drop(first);
    for skill in MCP_SKILLS {
        assert_files(&user.join(".codex/skills").join(skill.name), skill.files);
    }
    let changed = user.join(".codex/skills").join(MCP_SKILLS[0].name);
    std::fs::write(changed.join("notes.md"), "keep me").expect("user edit");
    drop(second);
    assert!(changed.join("notes.md").exists());
    for skill in &MCP_SKILLS[1..] {
        assert!(!user.join(".codex/skills").join(skill.name).exists());
    }
}

#[test]
fn clean_user_reference_versions_update_without_replacing_user_changes() {
    let temp = tempfile::tempdir().expect("temp");
    let dir = temp.path().join(MCP_SKILLS[0].name);
    let old = [
        EmbeddedFile {
            relative_path: "SKILL.md",
            content: "old skill",
        },
        EmbeddedFile {
            relative_path: "references/casting.md",
            content: "old reference",
        },
    ];
    super::super::owned_skills::sync(&dir, &old).expect("old install");
    validate_mcp_skill(&dir).expect("known prior version");
    super::super::owned_skills::sync(&dir, MCP_SKILLS[0].files).expect("upgrade");
    assert_files(&dir, MCP_SKILLS[0].files);
    assert_eq!(
        super::super::owned_skills::sync(&dir, MCP_SKILLS[0].files).expect("repeat"),
        0
    );
    std::fs::write(dir.join("references/casting.md"), "user reference").expect("user edit");
    assert!(super::super::owned_skills::sync(&dir, &old).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("references/casting.md")).expect("preserved"),
        "user reference"
    );
}

#[test]
fn interrupted_reference_upgrade_finishes_from_recorded_generations() {
    let temp = tempfile::tempdir().expect("temp");
    let dir = temp.path().join(MCP_SKILLS[0].name);
    let old = [
        EmbeddedFile {
            relative_path: "SKILL.md",
            content: "old skill",
        },
        EmbeddedFile {
            relative_path: "references/casting.md",
            content: "old reference",
        },
    ];
    super::super::owned_skills::sync(&dir, &old).expect("old install");
    let path = dir.join(".horizon-owned.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("record")).expect("JSON");
    record["pending"] = serde_json::json!(
        MCP_SKILLS[0]
            .files
            .iter()
            .map(|file| (file.relative_path, file.content))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    std::fs::write(&path, serde_json::to_string(&record).expect("record JSON")).expect("pending record");
    std::fs::write(dir.join("SKILL.md"), MCP_SKILLS[0].files[0].content).expect("first updated file");
    validate_mcp_skill(&dir).expect("recognized interrupted update");
    super::super::owned_skills::sync(&dir, MCP_SKILLS[0].files).expect("finish upgrade");
    assert_files(&dir, MCP_SKILLS[0].files);
    validate_mcp_skill(&dir).expect("complete updated tree");
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("record")).expect("JSON");
    assert!(record["pending"].is_null());
}

#[test]
fn unmanaged_or_invalid_grok_server_never_receives_additional_mcp_skills() {
    let temp = tempfile::tempdir().expect("temp");
    let home = HorizonHome::from_root(temp.path().join("horizon"));
    let user = temp.path().join("user");
    let grok = user.join(".grok");
    std::fs::create_dir_all(&grok).expect("provider");
    std::fs::write(
        grok.join("config.toml"),
        "[mcp_servers.horizon-browser]\ncommand='custom'\n",
    )
    .expect("custom server");
    install_agent_plugins_impl(
        &home,
        &home.claude_plugin_dir_for_host("host"),
        Some(&user),
        None,
        None,
        Path::new("/opt/horizon"),
        None,
    )
    .expect("other targets install");
    for skill in MCP_SKILLS {
        assert!(!grok.join("skills").join(skill.name).exists());
    }
}

#[test]
fn interrupted_retired_reference_removal_prunes_its_empty_directory() {
    let temp = tempfile::tempdir().expect("temp");
    let dir = temp.path().join(MCP_SKILLS[0].name);
    let old = [
        EmbeddedFile {
            relative_path: "SKILL.md",
            content: "old skill",
        },
        EmbeddedFile {
            relative_path: "legacy/help.md",
            content: "old reference",
        },
    ];
    super::super::owned_skills::sync(&dir, &old).expect("old install");
    let path = dir.join(".horizon-owned.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("record")).expect("JSON");
    record["pending"] = serde_json::json!(
        MCP_SKILLS[0]
            .files
            .iter()
            .map(|file| (file.relative_path, file.content))
            .collect::<std::collections::BTreeMap<_, _>>()
    );
    std::fs::write(&path, serde_json::to_string(&record).expect("record JSON")).expect("pending record");
    std::fs::remove_file(dir.join("legacy/help.md")).expect("retired file deleted before interruption");
    validate_mcp_skill(&dir).expect("recognized deletion interruption");
    super::super::owned_skills::sync(&dir, MCP_SKILLS[0].files).expect("finish upgrade");
    assert!(!dir.join("legacy").exists());
    assert_files(&dir, MCP_SKILLS[0].files);
    validate_mcp_skill(&dir).expect("no stale empty directories");
}

#[test]
fn private_integration_upgrade_removes_retired_unrecorded_references() {
    let temp = tempfile::tempdir().expect("temp");
    let home = HorizonHome::from_root(temp.path().join("horizon"));
    let plugin = home.claude_plugin_dir_for_host("host");
    let dirs = [
        home.codex_integrations_dir().join(MCP_SKILLS[0].name),
        plugin.join("skills").join(MCP_SKILLS[0].name),
    ];
    let old = [
        EmbeddedFile {
            relative_path: "SKILL.md",
            content: "old entry point",
        },
        EmbeddedFile {
            relative_path: "legacy/reference.md",
            content: "retired reference",
        },
    ];
    for dir in &dirs {
        sync_plugin_files(dir, &old).expect("prior unrecorded integration");
        assert!(!dir.join(".horizon-owned.json").exists());
    }
    let install = || sync_mcp_skills(&home, &plugin, None, None, None, None);
    install().expect("upgrade integration");
    for dir in &dirs {
        assert!(!dir.join("legacy").exists());
        assert_files(dir, MCP_SKILLS[0].files);
        validate_mcp_skill(dir).expect("exact upgraded tree");
    }
    assert_eq!(install().expect("repeat installation"), 0);
}

#[cfg(unix)]
#[test]
fn private_cache_sync_refuses_symlinks_before_writing_files() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("temp");
    let dir = temp.path().join("cache");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&dir).expect("cache directory");
    std::fs::create_dir_all(&outside).expect("outside directory");
    std::fs::write(dir.join("SKILL.md"), "old entry point").expect("old cache");
    std::fs::write(outside.join("notes.md"), "keep me").expect("outside file");
    symlink(&outside, dir.join("legacy")).expect("linked directory");
    assert!(sync_managed_skill_files(&dir, MCP_SKILLS[0].files).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).expect("unchanged cache"),
        "old entry point"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("notes.md")).expect("outside intact"),
        "keep me"
    );
    assert!(dir.join("legacy").is_symlink());
}
