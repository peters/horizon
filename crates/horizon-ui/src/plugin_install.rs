use std::fs::{OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use horizon_core::browser::manifest;
use horizon_core::{HorizonHome, browser_mcp_executable};

struct EmbeddedFile {
    relative_path: &'static str,
    content: &'static str,
}

const CLAUDE_PLUGIN_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: ".claude-plugin/plugin.json",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/.claude-plugin/plugin.json"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-browser/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-browser/SKILL.md"
        )),
    },
    EmbeddedFile {
        relative_path: "skills/horizon-notify/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-notify/SKILL.md"
        )),
    },
];

const NOTIFY_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-notify/SKILL.md"
    )),
}];

const BROWSER_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-browser/SKILL.md"
    )),
}];

/// `$HOME`-relative skill roots that receive `horizon-notify`. This is the
/// title/notify skill every built-in agent needs. Claude also gets it through
/// the host plugin tree.
const NOTIFY_SKILL_ROOTS: &[&[&str]] = &[
    &[".agents", "skills"],
    &[".codex", "skills"],
    &[".claude", "skills"],
    &[".grok", "skills"],
    &[".config", "opencode", "skills"],
    &[".gemini", "skills"],
    &[".kilocode", "skills"],
    &[".pi", "agent", "skills"],
];

/// Browser MCP skill stays on the agents that already had Horizon browser
/// integration. Expanding it to Grok/Gemini/Pi/OpenCode is separate work.
const BROWSER_SKILL_ROOTS: &[&[&str]] = &[&[".agents", "skills"], &[".codex", "skills"], &[".kilocode", "skills"]];

pub(crate) struct AgentPluginHostLease {
    host_dir: PathBuf,
    lock_path: PathBuf,
    lock_file: Option<std::fs::File>,
}

impl AgentPluginHostLease {
    fn acquire(instance_dir: PathBuf) -> std::io::Result<Self> {
        let lock_path = agent_plugin_host_lock_path(&instance_dir)?;
        let plugin_root = instance_dir.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "agent plugin host directory has no parent",
            )
        })?;
        std::fs::create_dir_all(plugin_root)?;
        let lock_file = open_lock_file(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("agent plugin host is already active: {}", instance_dir.display()),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        if let Err(error) = std::fs::create_dir_all(&instance_dir) {
            drop(lock_file);
            let _ = std::fs::remove_file(&lock_path);
            return Err(error);
        }
        Ok(Self {
            host_dir: instance_dir,
            lock_path,
            lock_file: Some(lock_file),
        })
    }
}

impl Drop for AgentPluginHostLease {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.host_dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.host_dir.display(), %error, "failed to remove agent plugin host directory");
        }
        drop(self.lock_file.take());
        if let Err(error) = std::fs::remove_file(&self.lock_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.lock_path.display(), %error, "failed to remove agent plugin host lock");
        }
    }
}

pub(crate) fn install_agent_plugins(horizon_home: &HorizonHome) -> Option<AgentPluginHostLease> {
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let mcp_command = browser_mcp_executable().unwrap_or_else(|| PathBuf::from("horizon"));
    let host_dir = horizon_home.agent_plugin_host_dir(manifest::host_instance());
    let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host(manifest::host_instance());
    let lease = match AgentPluginHostLease::acquire(host_dir.clone()) {
        Ok(lease) => lease,
        Err(error) => {
            tracing::warn!(%error, "failed to acquire agent plugin host lease");
            return None;
        }
    };

    match install_agent_plugins_impl(horizon_home, &claude_plugin_dir, user_home.as_deref(), &mcp_command) {
        Ok(updated_files) if updated_files > 0 => {
            tracing::info!(updated_files, "synced embedded Horizon agent plugins");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("failed to sync embedded Horizon agent plugins: {error}"),
    }

    match prune_stale_agent_plugin_hosts(&host_dir) {
        Ok(pruned_hosts) if pruned_hosts > 0 => {
            tracing::info!(pruned_hosts, "pruned stale agent plugin hosts");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "failed to prune stale agent plugin hosts"),
    }

    Some(lease)
}

fn agent_plugin_host_lock_path(instance_dir: &Path) -> std::io::Result<PathBuf> {
    let plugin_root = instance_dir.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent plugin host directory has no parent",
        )
    })?;
    let host_name = instance_dir.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent plugin host directory has no name",
        )
    })?;
    let mut lock_name = host_name.to_os_string();
    lock_name.push(".lock");
    Ok(plugin_root.join(lock_name))
}

fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
}

fn prune_stale_agent_plugin_hosts(current_instance_dir: &Path) -> std::io::Result<usize> {
    let plugin_root = current_instance_dir.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent plugin host directory has no parent",
        )
    })?;
    let prune_lock = open_lock_file(&plugin_root.join(".prune.lock"))?;
    match prune_lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(0),
        Err(TryLockError::Error(error)) => return Err(error),
    }

    let mut pruned_hosts = 0usize;
    for entry in std::fs::read_dir(plugin_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.path() == current_instance_dir {
            continue;
        }
        let candidate_dir = entry.path();
        let host_lock_path = agent_plugin_host_lock_path(&candidate_dir)?;
        let host_lock = open_lock_file(&host_lock_path)?;
        match host_lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(error)) => return Err(error),
        }
        match std::fs::remove_dir_all(&candidate_dir) {
            Ok(()) => pruned_hosts += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        drop(host_lock);
        match std::fs::remove_file(host_lock_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(pruned_hosts)
}

fn install_agent_plugins_impl(
    horizon_home: &HorizonHome,
    claude_plugin_dir: &Path,
    user_home: Option<&Path>,
    mcp_command: &Path,
) -> std::io::Result<usize> {
    let mut updated_files = 0usize;

    updated_files += sync_plugin_files(claude_plugin_dir, CLAUDE_PLUGIN_FILES)?;
    updated_files += usize::from(sync_file_if_changed(
        &claude_plugin_dir.join(".mcp.json"),
        &claude_mcp_config(mcp_command)?,
    )?);
    updated_files += sync_plugin_files(&horizon_home.codex_skill_dir(), NOTIFY_SKILL_FILES)?;
    updated_files += sync_plugin_files(&horizon_home.codex_browser_skill_dir(), BROWSER_SKILL_FILES)?;

    if let Some(home) = user_home {
        for skill_root in NOTIFY_SKILL_ROOTS {
            updated_files +=
                sync_plugin_files(&user_skill_dir(home, skill_root, "horizon-notify"), NOTIFY_SKILL_FILES)?;
        }
        for skill_root in BROWSER_SKILL_ROOTS {
            updated_files += sync_plugin_files(
                &user_skill_dir(home, skill_root, "horizon-browser"),
                BROWSER_SKILL_FILES,
            )?;
        }
    }

    Ok(updated_files)
}

fn user_skill_dir(home: &Path, skill_root: &[&str], skill_name: &str) -> PathBuf {
    let mut dir = home.to_path_buf();
    for part in skill_root {
        dir.push(part);
    }
    dir.push(skill_name);
    dir
}

fn claude_mcp_config(command: &Path) -> std::io::Result<String> {
    let command = command.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Horizon executable path is not valid UTF-8",
        )
    })?;
    serde_json::to_string_pretty(&serde_json::json!({
        "horizon-browser": {
            "command": command,
            "args": ["--browser-mcp"]
        }
    }))
    .map_err(std::io::Error::other)
}

fn sync_plugin_files(base: &Path, files: &[EmbeddedFile]) -> std::io::Result<usize> {
    let mut updated_files = 0usize;

    for embedded_file in files {
        let path = base.join(embedded_file.relative_path);
        if sync_file_if_changed(&path, embedded_file.content)? {
            updated_files += 1;
        }
    }

    Ok(updated_files)
}

fn sync_file_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return Ok(false);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.flush()?;
    temp_file.persist(path).map_err(|error| error.error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use horizon_core::HorizonHome;

    use super::{
        AgentPluginHostLease, BROWSER_SKILL_FILES, BROWSER_SKILL_ROOTS, CLAUDE_PLUGIN_FILES, EmbeddedFile,
        NOTIFY_SKILL_FILES, NOTIFY_SKILL_ROOTS, agent_plugin_host_lock_path, install_agent_plugins_impl,
        open_lock_file, prune_stale_agent_plugin_hosts, sync_file_if_changed, sync_plugin_files, user_skill_dir,
    };

    #[test]
    fn sync_file_if_changed_writes_missing_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("skill").join("SKILL.md");

        let updated = sync_file_if_changed(&path, "version-1").expect("write file");

        assert!(updated);
        assert_eq!(std::fs::read_to_string(path).expect("read file"), "version-1");
    }

    #[test]
    fn sync_file_if_changed_skips_identical_content() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("SKILL.md");
        std::fs::write(&path, "same").expect("seed file");

        let updated = sync_file_if_changed(&path, "same").expect("sync file");

        assert!(!updated);
    }

    #[test]
    fn sync_plugin_files_reports_only_changed_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let files = [
            EmbeddedFile {
                relative_path: "a.txt",
                content: "alpha",
            },
            EmbeddedFile {
                relative_path: "nested/b.txt",
                content: "beta",
            },
        ];

        let first = sync_plugin_files(temp.path(), &files).expect("first sync");
        let second = sync_plugin_files(temp.path(), &files).expect("second sync");

        assert_eq!(first, 2);
        assert_eq!(second, 0);
    }

    #[test]
    fn install_agent_plugins_syncs_notify_skill_into_every_agent_home() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");
        let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");

        let updated = install_agent_plugins_impl(
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            Path::new("/opt/horizon"),
        )
        .expect("install plugins");

        assert!(updated > 0);
        for skill_root in NOTIFY_SKILL_ROOTS {
            let notify_path = user_skill_dir(&user_home, skill_root, "horizon-notify").join("SKILL.md");
            assert_eq!(
                std::fs::read_to_string(&notify_path)
                    .unwrap_or_else(|_| panic!("notify skill missing at {}", notify_path.display())),
                NOTIFY_SKILL_FILES[0].content,
            );
        }
        for skill_root in BROWSER_SKILL_ROOTS {
            let browser_path = user_skill_dir(&user_home, skill_root, "horizon-browser").join("SKILL.md");
            assert_eq!(
                std::fs::read_to_string(&browser_path)
                    .unwrap_or_else(|_| panic!("browser skill missing at {}", browser_path.display())),
                BROWSER_SKILL_FILES[0].content,
            );
        }
        assert!(
            !user_home.join(".grok/skills/horizon-browser/SKILL.md").exists(),
            "browser MCP skill must not be exported to agents without Horizon MCP injection"
        );
        assert_eq!(
            std::fs::read_to_string(horizon_home.codex_skill_dir().join("SKILL.md"))
                .expect("horizon codex integration should be synced"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(claude_plugin_dir.join("skills/horizon-notify/SKILL.md"))
                .expect("claude plugin notify skill should be installed"),
            CLAUDE_PLUGIN_FILES
                .iter()
                .find(|file| file.relative_path == "skills/horizon-notify/SKILL.md")
                .expect("claude notify file")
                .content,
        );
        assert!(BROWSER_SKILL_FILES[0].content.contains("browser_visibility"));
        assert!(BROWSER_SKILL_FILES[0].content.contains("browser_network_watch"));
        let mcp_config = std::fs::read_to_string(claude_plugin_dir.join(".mcp.json"))
            .expect("Claude MCP config should be installed");
        assert!(mcp_config.contains("/opt/horizon"));
        assert!(mcp_config.contains("--browser-mcp"));
    }

    #[test]
    fn install_agent_plugins_keeps_mcp_commands_isolated_per_horizon_host() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let first_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
        let second_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-b");

        install_agent_plugins_impl(&horizon_home, &first_plugin_dir, None, Path::new("/opt/horizon-a"))
            .expect("install first host plugin");
        install_agent_plugins_impl(&horizon_home, &second_plugin_dir, None, Path::new("/opt/horizon-b"))
            .expect("install second host plugin");

        let first_config = std::fs::read_to_string(first_plugin_dir.join(".mcp.json")).expect("first host config");
        let second_config = std::fs::read_to_string(second_plugin_dir.join(".mcp.json")).expect("second host config");
        assert!(first_config.contains("/opt/horizon-a"));
        assert!(!first_config.contains("/opt/horizon-b"));
        assert!(second_config.contains("/opt/horizon-b"));
        assert!(!second_config.contains("/opt/horizon-a"));
    }

    #[test]
    fn agent_plugin_host_lease_removes_its_directory_and_lock_on_drop() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let host_dir = horizon_home.agent_plugin_host_dir("host-a");
        let lock_path = agent_plugin_host_lock_path(&host_dir).expect("lock path");

        let lease = AgentPluginHostLease::acquire(host_dir.clone()).expect("host lease");
        assert!(host_dir.is_dir());
        assert!(lock_path.is_file());

        drop(lease);

        assert!(!host_dir.exists());
        assert!(!lock_path.exists());
    }

    #[test]
    fn prune_stale_agent_plugin_hosts_keeps_active_hosts() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let current_host_dir = horizon_home.agent_plugin_host_dir("current-host");
        let active_host_dir = horizon_home.agent_plugin_host_dir("active-host");
        let stale_host_dir = horizon_home.agent_plugin_host_dir("stale-host");
        let current_lease = AgentPluginHostLease::acquire(current_host_dir.clone()).expect("current lease");
        let active_lease = AgentPluginHostLease::acquire(active_host_dir.clone()).expect("active lease");
        std::fs::create_dir_all(stale_host_dir.join("claude-code")).expect("stale host directory");

        let pruned_hosts = prune_stale_agent_plugin_hosts(&current_host_dir).expect("prune stale hosts");

        assert_eq!(pruned_hosts, 1);
        assert!(current_host_dir.is_dir());
        assert!(active_host_dir.is_dir());
        assert!(!stale_host_dir.exists());

        drop(active_lease);
        drop(current_lease);
    }

    #[test]
    fn prune_stale_agent_plugin_hosts_defers_to_another_pruner() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let current_host_dir = horizon_home.agent_plugin_host_dir("current-host");
        let stale_host_dir = horizon_home.agent_plugin_host_dir("stale-host");
        let current_lease = AgentPluginHostLease::acquire(current_host_dir.clone()).expect("current lease");
        std::fs::create_dir_all(&stale_host_dir).expect("stale host directory");
        let prune_lock = open_lock_file(&horizon_home.agent_plugin_hosts_dir().join(".prune.lock"))
            .expect("prune coordination file");
        prune_lock.try_lock().expect("prune lock");

        let pruned_hosts = prune_stale_agent_plugin_hosts(&current_host_dir).expect("deferred prune");

        assert_eq!(pruned_hosts, 0);
        assert!(stale_host_dir.is_dir());

        drop(prune_lock);
        drop(current_lease);
    }
}
