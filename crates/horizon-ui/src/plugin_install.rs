use std::ffi::OsStr;
use std::fs::{OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use horizon_core::browser::manifest;
use horizon_core::{HorizonHome, browser_mcp_executable, codex_home_dir, grok_home_dir, user_home_dir};

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

const HORIZON_NOTIFY_SKILL: &str = "horizon-notify";
const HORIZON_BROWSER_SKILL: &str = "horizon-browser";

/// `$HOME`-relative skill roots that receive `horizon-notify` for the life of
/// this Horizon process. Claude also gets it through the host plugin tree.
/// Shared `~/.agents/skills` is not a broadcast target: each agent gets its
/// private home so the skill is not visible to CLIs that never receive Horizon MCP.
const NOTIFY_SKILL_ROOTS: &[&[&str]] = &[
    &[".claude", "skills"],
    &[".config", "opencode", "skills"],
    &[".gemini", "antigravity-cli", "skills"],
    &[".kilocode", "skills"],
    &[".pi", "agent", "skills"],
];

/// Leftover Horizon-owned notify dirs from older installers. Removed on start
/// and when the last Horizon host exits.
const ABANDONED_NOTIFY_SKILL_ROOTS: &[&[&str]] = &[&[".agents", "skills"], &[".gemini", "skills"]];

/// Leftover Horizon-owned browser dirs from older installers that taught MCP
/// workflows to agents this process does not wire to the browser server.
const ABANDONED_BROWSER_SKILL_ROOTS: &[&[&str]] =
    &[&[".agents", "skills"], &[".gemini", "skills"], &[".kilocode", "skills"]];

pub(crate) struct AgentPluginHostLease {
    host_dir: PathBuf,
    lock_path: PathBuf,
    lock_file: Option<std::fs::File>,
    user_skill_dirs: Vec<PathBuf>,
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
            user_skill_dirs: Vec::new(),
        })
    }

    fn release_user_skills_if_last_host(&self) {
        let Some(plugin_root) = self.host_dir.parent() else {
            return;
        };
        let _lock_file = match lock_user_skills(plugin_root) {
            Ok(lock_file) => lock_file,
            Err(error) => {
                tracing::warn!(%error, "failed to lock user-skill cleanup");
                return;
            }
        };
        if let Err(error) = persist_user_skill_manifest(self) {
            tracing::warn!(%error, "failed to persist Horizon skill lease paths");
        }
        match another_agent_plugin_host_is_live(plugin_root, &self.lock_path) {
            Ok(true) => {}
            Ok(false) => {
                let mut dirs = self.user_skill_dirs.clone();
                match leased_skill_dirs_from_manifests(plugin_root) {
                    Ok(mut leased) => dirs.append(&mut leased),
                    Err(error) => tracing::warn!(%error, "failed to read Horizon skill lease manifests"),
                }
                for dir in dirs {
                    remove_horizon_skill_dir(&dir);
                }
                remove_user_skill_manifests(plugin_root);
            }
            Err(error) => tracing::warn!(%error, "failed to inspect agent plugin hosts for skill cleanup"),
        }
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
        self.release_user_skills_if_last_host();
    }
}

pub(crate) fn install_agent_plugins(horizon_home: &HorizonHome) -> Option<AgentPluginHostLease> {
    let user_home = user_home_dir();
    let grok_home = grok_home_dir();
    let codex_home = codex_home_dir();
    let mcp_command = browser_mcp_executable().unwrap_or_else(|| PathBuf::from("horizon"));
    let host_dir = horizon_home.agent_plugin_host_dir(manifest::host_instance());
    let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host(manifest::host_instance());
    let mut lease = match AgentPluginHostLease::acquire(host_dir.clone()) {
        Ok(lease) => lease,
        Err(error) => {
            tracing::warn!(%error, "failed to acquire agent plugin host lease");
            return None;
        }
    };
    lease.user_skill_dirs = user_skill_cleanup_dirs(user_home.as_deref(), grok_home.as_deref(), codex_home.as_deref());
    sync_leased_user_skills(
        &lease,
        horizon_home,
        &claude_plugin_dir,
        user_home.as_deref(),
        grok_home.as_deref(),
        codex_home.as_deref(),
        &mcp_command,
    );

    match prune_stale_agent_plugin_hosts(&host_dir) {
        Ok(pruned_hosts) if pruned_hosts > 0 => {
            tracing::info!(pruned_hosts, "pruned stale agent plugin hosts");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "failed to prune stale agent plugin hosts"),
    }

    Some(lease)
}

fn sync_leased_user_skills(
    lease: &AgentPluginHostLease,
    horizon_home: &HorizonHome,
    claude_plugin_dir: &Path,
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
    mcp_command: &Path,
) {
    let Some(plugin_root) = lease.host_dir.parent() else {
        tracing::warn!("agent plugin host directory has no parent");
        return;
    };
    let _guard = match lock_user_skills(plugin_root) {
        Ok(guard) => guard,
        Err(error) => {
            tracing::warn!(%error, "failed to lock user-skill installation");
            return;
        }
    };
    if let Err(error) = persist_user_skill_manifest(lease) {
        tracing::warn!(%error, "failed to persist Horizon skill lease paths");
    }
    if let Some(home) = user_home {
        for dir in abandoned_user_skill_dirs(home) {
            remove_horizon_skill_dir(&dir);
        }
    }
    match install_agent_plugins_impl(
        horizon_home,
        claude_plugin_dir,
        user_home,
        grok_home,
        codex_home,
        mcp_command,
    ) {
        Ok(updated_files) if updated_files > 0 => {
            tracing::info!(updated_files, "synced embedded Horizon agent plugins");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("failed to sync embedded Horizon agent plugins: {error}"),
    }
}

fn agent_plugin_host_lock_path(instance_dir: &Path) -> io::Result<PathBuf> {
    host_sidecar_path(instance_dir, ".lock")
}

fn user_skill_manifest_path(instance_dir: &Path) -> io::Result<PathBuf> {
    host_sidecar_path(instance_dir, ".user-skills")
}

fn host_sidecar_path(instance_dir: &Path, suffix: &str) -> io::Result<PathBuf> {
    let plugin_root = instance_dir
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no parent"))?;
    let host_name = instance_dir
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no name"))?;
    let mut name = host_name.to_os_string();
    name.push(suffix);
    Ok(plugin_root.join(name))
}

fn lock_user_skills(plugin_root: &Path) -> io::Result<std::fs::File> {
    let lock_file = open_lock_file(&plugin_root.join(".user-skills.lock"))?;
    lock_file.lock()?;
    Ok(lock_file)
}

fn persist_user_skill_manifest(lease: &AgentPluginHostLease) -> io::Result<()> {
    let path = user_skill_manifest_path(&lease.host_dir)?;
    let encoded = serde_json::to_string(
        &lease
            .user_skill_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    )
    .map_err(io::Error::other)?;
    sync_file_if_changed(&path, &encoded)?;
    Ok(())
}

fn leased_skill_dirs_from_manifests(plugin_root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(plugin_root)? {
        let entry = entry?;
        if !is_user_skill_manifest(entry.file_name().as_os_str()) {
            continue;
        }
        match read_user_skill_manifest(&entry.path()) {
            Ok(mut leased) => dirs.append(&mut leased),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    path = %entry.path().display(),
                    %error,
                    "failed to read Horizon skill lease manifest"
                );
            }
        }
    }
    Ok(dirs)
}

fn read_user_skill_manifest(path: &Path) -> io::Result<Vec<PathBuf>> {
    let encoded = std::fs::read_to_string(path)?;
    let paths: Vec<String> = serde_json::from_str(&encoded).map_err(io::Error::other)?;
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

fn remove_user_skill_manifests(plugin_root: &Path) {
    let entries = match std::fs::read_dir(plugin_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(path = %plugin_root.display(), %error, "failed to list Horizon skill lease manifests");
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, "failed to read Horizon skill lease manifest entry");
                continue;
            }
        };
        if !is_user_skill_manifest(entry.file_name().as_os_str()) {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path())
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %entry.path().display(),
                %error,
                "failed to remove Horizon skill lease manifest"
            );
        }
    }
}

fn is_user_skill_manifest(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    !name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("user-skills"))
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

fn another_agent_plugin_host_is_live(plugin_root: &Path, current_lock_path: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(plugin_root)? {
        let entry = entry?;
        let path = entry.path();
        if path == current_lock_path || !is_host_lock_file(entry.file_name().as_os_str()) {
            continue;
        }
        let host_lock = open_lock_file(&path)?;
        match host_lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(true),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(false)
}

fn is_host_lock_file(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    !name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lock"))
}

fn install_agent_plugins_impl(
    horizon_home: &HorizonHome,
    claude_plugin_dir: &Path,
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
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
            updated_files += sync_plugin_files(
                &user_skill_dir(home, skill_root, HORIZON_NOTIFY_SKILL),
                NOTIFY_SKILL_FILES,
            )?;
        }
    }

    if let Some(grok_root) = provider_home(grok_home, user_home, ".grok") {
        updated_files += sync_plugin_files(&grok_root.join("skills").join(HORIZON_NOTIFY_SKILL), NOTIFY_SKILL_FILES)?;
    }
    if let Some(codex_root) = provider_home(codex_home, user_home, ".codex") {
        updated_files += sync_plugin_files(
            &codex_root.join("skills").join(HORIZON_NOTIFY_SKILL),
            NOTIFY_SKILL_FILES,
        )?;
        updated_files += sync_plugin_files(
            &codex_root.join("skills").join(HORIZON_BROWSER_SKILL),
            BROWSER_SKILL_FILES,
        )?;
    }

    Ok(updated_files)
}

fn user_skill_cleanup_dirs(
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = user_home {
        for skill_root in NOTIFY_SKILL_ROOTS {
            dirs.push(user_skill_dir(home, skill_root, HORIZON_NOTIFY_SKILL));
        }
        dirs.extend(abandoned_user_skill_dirs(home));
    }
    if let Some(grok_root) = provider_home(grok_home, user_home, ".grok") {
        dirs.push(grok_root.join("skills").join(HORIZON_NOTIFY_SKILL));
    }
    if let Some(codex_root) = provider_home(codex_home, user_home, ".codex") {
        dirs.push(codex_root.join("skills").join(HORIZON_NOTIFY_SKILL));
        dirs.push(codex_root.join("skills").join(HORIZON_BROWSER_SKILL));
    }
    dirs
}

fn abandoned_user_skill_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for skill_root in ABANDONED_NOTIFY_SKILL_ROOTS {
        dirs.push(user_skill_dir(home, skill_root, HORIZON_NOTIFY_SKILL));
    }
    for skill_root in ABANDONED_BROWSER_SKILL_ROOTS {
        dirs.push(user_skill_dir(home, skill_root, HORIZON_BROWSER_SKILL));
    }
    dirs
}

fn is_horizon_skill_dir_name(name: &OsStr) -> bool {
    name == HORIZON_NOTIFY_SKILL || name == HORIZON_BROWSER_SKILL
}

fn remove_horizon_skill_dir(path: &Path) {
    let Some(name) = path.file_name() else {
        return;
    };
    if !is_horizon_skill_dir_name(name) {
        return;
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "failed to inspect Horizon skill directory");
            return;
        }
    };
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "failed to remove Horizon skill directory");
    }
}

fn provider_home(override_home: Option<&Path>, user_home: Option<&Path>, default_dir: &str) -> Option<PathBuf> {
    override_home
        .map(Path::to_path_buf)
        .or_else(|| user_home.map(|home| home.join(default_dir)))
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
        AgentPluginHostLease, BROWSER_SKILL_FILES, CLAUDE_PLUGIN_FILES, EmbeddedFile, HORIZON_BROWSER_SKILL,
        HORIZON_NOTIFY_SKILL, NOTIFY_SKILL_FILES, NOTIFY_SKILL_ROOTS, abandoned_user_skill_dirs,
        agent_plugin_host_lock_path, install_agent_plugins_impl, open_lock_file, prune_stale_agent_plugin_hosts,
        sync_file_if_changed, sync_plugin_files, user_skill_cleanup_dirs, user_skill_dir,
    };

    fn write_skill_dir(path: &Path, body: &str) {
        std::fs::create_dir_all(path).expect("skill dir");
        std::fs::write(path.join("SKILL.md"), body).expect("skill file");
    }

    #[test]
    fn user_skill_cleanup_dirs_cover_private_homes_and_abandoned_broadcasts() {
        let home = Path::new("/tmp/horizon-user");
        let dirs = user_skill_cleanup_dirs(Some(home), None, None);

        assert!(dirs.contains(&home.join(".claude/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".gemini/antigravity-cli/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".codex/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".codex/skills/horizon-browser")));
        assert!(dirs.contains(&home.join(".agents/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".agents/skills/horizon-browser")));
        assert!(dirs.contains(&home.join(".gemini/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".kilocode/skills/horizon-browser")));
        assert!(!dirs.contains(&home.join(".grok/skills/horizon-browser")));
    }

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
            None,
            None,
            Path::new("/opt/horizon"),
        )
        .expect("install plugins");

        assert!(updated > 0);
        for skill_root in NOTIFY_SKILL_ROOTS {
            let notify_path = user_skill_dir(&user_home, skill_root, HORIZON_NOTIFY_SKILL).join("SKILL.md");
            assert_eq!(
                std::fs::read_to_string(&notify_path)
                    .unwrap_or_else(|_| panic!("notify skill missing at {}", notify_path.display())),
                NOTIFY_SKILL_FILES[0].content,
            );
        }
        assert_eq!(
            std::fs::read_to_string(user_home.join(".gemini/antigravity-cli/skills/horizon-notify/SKILL.md"))
                .expect("antigravity notify skill"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join(".grok/skills/horizon-notify/SKILL.md"))
                .expect("grok skill should fall back to ~/.grok"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join(".codex/skills/horizon-notify/SKILL.md"))
                .expect("codex skill should fall back to ~/.codex"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join(".codex/skills/horizon-browser/SKILL.md"))
                .expect("codex browser skill should fall back to ~/.codex"),
            BROWSER_SKILL_FILES[0].content,
        );
        assert!(
            !user_home.join(".agents/skills/horizon-notify/SKILL.md").exists(),
            "notify skill must not broadcast through ~/.agents/skills"
        );
        assert!(
            !user_home.join(".agents/skills/horizon-browser/SKILL.md").exists(),
            "browser MCP skill must not broadcast through ~/.agents/skills"
        );
        assert!(
            !user_home.join(".gemini/skills/horizon-notify/SKILL.md").exists(),
            "notify skill must use the Antigravity CLI home, not the Gemini CLI home"
        );
        assert!(
            !user_home.join(".kilocode/skills/horizon-browser/SKILL.md").exists(),
            "browser MCP skill must not be exported to agents without Horizon MCP injection"
        );
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
    fn install_agent_plugins_removes_abandoned_broadcast_skills() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");
        let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
        for dir in abandoned_user_skill_dirs(&user_home) {
            write_skill_dir(&dir, "stale");
        }

        for dir in abandoned_user_skill_dirs(&user_home) {
            super::remove_horizon_skill_dir(&dir);
        }
        install_agent_plugins_impl(
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            None,
            None,
            Path::new("/opt/horizon"),
        )
        .expect("install plugins");

        for dir in abandoned_user_skill_dirs(&user_home) {
            assert!(!dir.exists(), "abandoned skill should be removed: {}", dir.display());
        }
        assert!(
            user_home
                .join(".gemini/antigravity-cli/skills/horizon-notify/SKILL.md")
                .is_file()
        );
        assert!(user_home.join(".kilocode/skills/horizon-notify/SKILL.md").is_file());
    }

    #[test]
    fn install_agent_plugins_honors_grok_home_override() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");
        let grok_home = temp.path().join("custom-grok");
        let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");

        install_agent_plugins_impl(
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            Some(&grok_home),
            None,
            Path::new("/opt/horizon"),
        )
        .expect("install plugins");

        assert_eq!(
            std::fs::read_to_string(grok_home.join("skills/horizon-notify/SKILL.md"))
                .expect("notify skill should be installed under GROK_HOME"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert!(
            !user_home.join(".grok/skills/horizon-notify/SKILL.md").exists(),
            "GROK_HOME must replace ~/.grok rather than writing both"
        );
    }

    #[test]
    fn install_agent_plugins_keeps_mcp_commands_isolated_per_horizon_host() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let first_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
        let second_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-b");

        install_agent_plugins_impl(
            &horizon_home,
            &first_plugin_dir,
            None,
            None,
            None,
            Path::new("/opt/horizon-a"),
        )
        .expect("install first host plugin");
        install_agent_plugins_impl(
            &horizon_home,
            &second_plugin_dir,
            None,
            None,
            None,
            Path::new("/opt/horizon-b"),
        )
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
    fn last_agent_plugin_host_lease_removes_user_skills() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let skill_dir = temp.path().join("user-home/.codex/skills").join(HORIZON_NOTIFY_SKILL);
        write_skill_dir(&skill_dir, "leased");
        let unrelated = temp.path().join("user-home/.codex/skills/custom-skill");
        write_skill_dir(&unrelated, "keep");

        let host_dir = horizon_home.agent_plugin_host_dir("host-a");
        let mut lease = AgentPluginHostLease::acquire(host_dir).expect("host lease");
        lease.user_skill_dirs = vec![skill_dir.clone()];
        drop(lease);

        assert!(!skill_dir.exists());
        assert!(unrelated.join("SKILL.md").is_file());
    }

    #[test]
    fn agent_plugin_host_lease_keeps_user_skills_while_another_host_is_live() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let skill_dir = temp.path().join("user-home/.codex/skills").join(HORIZON_BROWSER_SKILL);
        write_skill_dir(&skill_dir, "shared");

        let first_dir = horizon_home.agent_plugin_host_dir("host-a");
        let second_dir = horizon_home.agent_plugin_host_dir("host-b");
        let mut first = AgentPluginHostLease::acquire(first_dir).expect("first lease");
        let mut second = AgentPluginHostLease::acquire(second_dir).expect("second lease");
        first.user_skill_dirs = vec![skill_dir.clone()];
        second.user_skill_dirs = vec![skill_dir.clone()];

        drop(first);
        assert!(skill_dir.join("SKILL.md").is_file());

        drop(second);
        assert!(!skill_dir.exists());
    }

    #[test]
    fn last_host_removes_other_hosts_custom_skill_homes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let first_skill = temp.path().join("codex-a/skills").join(HORIZON_NOTIFY_SKILL);
        let second_skill = temp.path().join("codex-b/skills").join(HORIZON_BROWSER_SKILL);
        write_skill_dir(&first_skill, "host-a");
        write_skill_dir(&second_skill, "host-b");

        let mut first =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("first lease");
        let mut second =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-b")).expect("second lease");
        first.user_skill_dirs = vec![first_skill.clone()];
        second.user_skill_dirs = vec![second_skill.clone()];

        drop(first);
        assert!(first_skill.join("SKILL.md").is_file());
        assert!(second_skill.join("SKILL.md").is_file());

        drop(second);
        assert!(!first_skill.exists());
        assert!(!second_skill.exists());
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
