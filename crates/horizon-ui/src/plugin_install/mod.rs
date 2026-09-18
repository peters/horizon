use std::fs::{OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use horizon_core::browser::manifest;
use horizon_core::{HorizonHome, browser_mcp_executable, codex_home_dir, grok_home_dir, user_home_dir};

mod grok_mcp;
mod user_skills;
mod work_hooks;
use user_skills::{
    HORIZON_BROWSER_SKILL, HORIZON_DEVICE_SKILL, HORIZON_NOTIFY_SKILL, RETIRED_OFFLOAD_SKILL, SkillRootLease,
    bind_prepared_skill_root, bind_skill_roots, release_skill_roots, remove_horizon_skill_dir,
};

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
        relative_path: "skills/horizon-device/SKILL.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-device/SKILL.md"
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

const DEVICE_SKILL_FILES: &[EmbeddedFile] = &[EmbeddedFile {
    relative_path: "SKILL.md",
    content: include_str!(concat!(
        env!("OUT_DIR"),
        "/assets/plugins/codex/skills/horizon-device/SKILL.md"
    )),
}];

fn validate_device_skill(dir: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        let mut entries = std::fs::read_dir(dir)?;
        let entry = entries.next().transpose()?;
        if let Some(entry) = entry
            && entries.next().is_none()
            && entry.file_name() == "SKILL.md"
            && entry.file_type()?.is_file()
            && std::fs::read_to_string(entry.path())? == DEVICE_SKILL_FILES[0].content
        {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Grok device skill is not Horizon-owned; preserving existing content",
    ))
}

fn install_device_skill(dir: &Path) -> io::Result<()> {
    if dir.try_exists()? {
        return Ok(());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| io::Error::other("skill directory has no parent"))?;
    let staging = tempfile::Builder::new().prefix(".horizon-device-").tempdir_in(parent)?;
    sync_plugin_files(staging.path(), DEVICE_SKILL_FILES)?;
    std::fs::rename(staging.path(), dir)
}

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

static HELD_AGENT_PLUGIN_LEASE: Mutex<Option<AgentPluginHostLease>> = Mutex::new(None);

pub(crate) struct AgentPluginHostLease {
    host_dir: PathBuf,
    lock_path: PathBuf,
    lock_file: Option<std::fs::File>,
    skill_roots: Vec<SkillRootLease>,
}

impl AgentPluginHostLease {
    fn acquire(instance_dir: PathBuf) -> std::io::Result<Self> {
        let lock_path = agent_plugin_host_lock_path(&instance_dir)?;
        let plugin_root = instance_dir
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no parent"))?;
        std::fs::create_dir_all(plugin_root)?;
        let lock_file = open_lock_file(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
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
            skill_roots: Vec::new(),
        })
    }

    #[cfg(test)]
    fn bind_user_skills(&mut self, dirs: &[PathBuf]) -> io::Result<()> {
        self.bind_user_skills_with_cleanup(dirs, &[])
    }

    fn bind_user_skills_with_cleanup(&mut self, dirs: &[PathBuf], extra_cleanup: &[PathBuf]) -> io::Result<()> {
        let host_id = self
            .host_dir
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no name"))?;
        self.skill_roots = bind_skill_roots(host_id, dirs, extra_cleanup);
        Ok(())
    }

    fn bind_grok_device_skill(&mut self, grok_root: &Path, browser_bound: bool) {
        let Some(host_id) = self.host_dir.file_name() else {
            return;
        };
        let device_dir = grok_root.join("skills").join(HORIZON_DEVICE_SKILL);
        match bind_prepared_skill_root(host_id, device_dir.clone(), validate_device_skill, || {
            if browser_bound {
                validate_device_skill(&device_dir)?;
                install_device_skill(&device_dir)?;
            }
            Ok(())
        }) {
            Ok(device_lease) => self.skill_roots.push(device_lease),
            Err(error) => {
                tracing::warn!(path = %device_dir.display(), %error, "Grok device skill unavailable; preserving existing content");
            }
        }
    }

    fn covers_skill_dir(&self, skill_dir: &Path) -> bool {
        self.skill_roots.iter().any(|root| root.covers_skill_dir(skill_dir))
    }
}

impl Drop for AgentPluginHostLease {
    fn drop(&mut self) {
        release_skill_roots(&mut self.skill_roots);
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

pub(crate) struct AgentPluginHostGuard;

impl Drop for AgentPluginHostGuard {
    fn drop(&mut self) {
        release_held_agent_plugin_host();
    }
}

pub(crate) fn install_agent_plugins(horizon_home: &HorizonHome) -> AgentPluginHostGuard {
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
            return AgentPluginHostGuard;
        }
    };
    if let Err(error) = work_hooks::install(&horizon_home.agent_work_plugin_dir_for_host(manifest::host_instance())) {
        tracing::warn!(%error, "work resume hooks unavailable");
    }
    let user_skill_dirs = user_skill_lease_dirs(user_home.as_deref(), grok_home.as_deref(), codex_home.as_deref());
    let extra_cleanup = user_skill_cleanup_dirs(user_home.as_deref(), codex_home.as_deref());
    if let Err(error) = lease.bind_user_skills_with_cleanup(&user_skill_dirs, &extra_cleanup) {
        tracing::warn!(%error, "failed to bind Horizon skill root leases");
    }
    if let Some(root) = provider_home(grok_home.as_deref(), user_home.as_deref(), ".grok") {
        match grok_mcp::bind_browser_skill(&root, manifest::host_instance().as_ref()) {
            Ok(roots) => {
                lease.skill_roots.extend(roots);
                lease.bind_grok_device_skill(&root, true);
            }
            Err(error) => {
                tracing::warn!(%error, "Grok browser integration unavailable; preserving existing settings");
                lease.bind_grok_device_skill(&root, false);
            }
        }
    }
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

    *held_agent_plugin_lease() = Some(lease);
    AgentPluginHostGuard
}

pub(crate) fn release_held_agent_plugin_host() {
    drop(held_agent_plugin_lease().take());
}

pub(crate) fn exit_after_releasing_plugins(code: i32) -> ! {
    release_held_agent_plugin_host();
    std::process::exit(code);
}

fn held_agent_plugin_lease() -> std::sync::MutexGuard<'static, Option<AgentPluginHostLease>> {
    HELD_AGENT_PLUGIN_LEASE.lock().unwrap_or_else(PoisonError::into_inner)
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
        Some(lease),
    ) {
        Ok(updated_files) if updated_files > 0 => {
            tracing::info!(updated_files, "synced embedded Horizon agent plugins");
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("failed to sync embedded Horizon agent plugins: {error}"),
    }
}

fn agent_plugin_host_lock_path(instance_dir: &Path) -> io::Result<PathBuf> {
    let plugin_root = instance_dir
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no parent"))?;
    let host_name = instance_dir
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "agent plugin host directory has no name"))?;
    let mut name = host_name.to_os_string();
    name.push(".lock");
    Ok(plugin_root.join(name))
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
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
    mcp_command: &Path,
    lease: Option<&AgentPluginHostLease>,
) -> std::io::Result<usize> {
    let mut updated_files = 0usize;

    updated_files += sync_plugin_files(claude_plugin_dir, CLAUDE_PLUGIN_FILES)?;
    updated_files += usize::from(sync_file_if_changed(
        &claude_plugin_dir.join(".mcp.json"),
        &claude_mcp_config(mcp_command)?,
    )?);
    updated_files += sync_plugin_files(&horizon_home.codex_skill_dir(), NOTIFY_SKILL_FILES)?;
    updated_files += sync_plugin_files(&horizon_home.codex_browser_skill_dir(), BROWSER_SKILL_FILES)?;
    updated_files += sync_plugin_files(
        &horizon_home.codex_integrations_dir().join(HORIZON_DEVICE_SKILL),
        DEVICE_SKILL_FILES,
    )?;

    if let Some(home) = user_home {
        for skill_root in NOTIFY_SKILL_ROOTS {
            let dir = user_skill_dir(home, skill_root, HORIZON_NOTIFY_SKILL);
            if !skill_dir_is_leased(lease, &dir) {
                continue;
            }
            updated_files += sync_plugin_files(&dir, NOTIFY_SKILL_FILES)?;
        }
    }

    if let Some(grok_root) = provider_home(grok_home, user_home, ".grok") {
        let dir = grok_root.join("skills").join(HORIZON_NOTIFY_SKILL);
        if skill_dir_is_leased(lease, &dir) {
            updated_files += sync_plugin_files(&dir, NOTIFY_SKILL_FILES)?;
        }
        let browser_dir = grok_root.join("skills").join(HORIZON_BROWSER_SKILL);
        let device_dir = grok_root.join("skills").join(HORIZON_DEVICE_SKILL);
        if skill_dir_is_leased(lease, &browser_dir) {
            match grok_mcp::register(&grok_root) {
                Ok(changed) => {
                    updated_files += usize::from(changed);
                    updated_files += sync_plugin_files(&browser_dir, BROWSER_SKILL_FILES)?;
                    if skill_dir_is_leased(lease, &device_dir) {
                        updated_files += sync_plugin_files(&device_dir, DEVICE_SKILL_FILES)?;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Grok browser integration unavailable; preserving existing settings");
                }
            }
        }
    }
    if let Some(codex_root) = provider_home(codex_home, user_home, ".codex") {
        let notify_dir = codex_root.join("skills").join(HORIZON_NOTIFY_SKILL);
        let browser_dir = codex_root.join("skills").join(HORIZON_BROWSER_SKILL);
        let device_dir = codex_root.join("skills").join(HORIZON_DEVICE_SKILL);
        if skill_dir_is_leased(lease, &notify_dir) {
            updated_files += sync_plugin_files(&notify_dir, NOTIFY_SKILL_FILES)?;
        }
        if skill_dir_is_leased(lease, &browser_dir) {
            updated_files += sync_plugin_files(&browser_dir, BROWSER_SKILL_FILES)?;
        }
        if skill_dir_is_leased(lease, &device_dir) {
            updated_files += sync_plugin_files(&device_dir, DEVICE_SKILL_FILES)?;
        }
    }

    Ok(updated_files)
}

fn skill_dir_is_leased(lease: Option<&AgentPluginHostLease>, skill_dir: &Path) -> bool {
    lease.is_none_or(|lease| lease.covers_skill_dir(skill_dir))
}

fn user_skill_lease_dirs(
    user_home: Option<&Path>,
    grok_home: Option<&Path>,
    codex_home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = user_home {
        for skill_root in NOTIFY_SKILL_ROOTS {
            dirs.push(user_skill_dir(home, skill_root, HORIZON_NOTIFY_SKILL));
        }
    }
    if let Some(grok_root) = provider_home(grok_home, user_home, ".grok") {
        dirs.push(grok_root.join("skills").join(HORIZON_NOTIFY_SKILL));
    }
    if let Some(codex_root) = provider_home(codex_home, user_home, ".codex") {
        dirs.push(codex_root.join("skills").join(HORIZON_NOTIFY_SKILL));
        dirs.push(codex_root.join("skills").join(HORIZON_BROWSER_SKILL));
        dirs.push(codex_root.join("skills").join(HORIZON_DEVICE_SKILL));
    }
    dirs
}

fn user_skill_cleanup_dirs(user_home: Option<&Path>, codex_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = user_home.map(abandoned_user_skill_dirs).unwrap_or_default();
    if let Some(root) = provider_home(codex_home, user_home, ".codex") {
        dirs.push(root.join("skills").join(RETIRED_OFFLOAD_SKILL));
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
            "args": ["--browser-mcp"],
            // Per-server timeout is also Claude Code's idle-abort floor.
            // One hour of handoff wait plus 60s to deliver the result.
            "timeout": 3_660_000
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
mod retired_skill_tests;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use horizon_core::HorizonHome;

    use super::{
        AgentPluginHostLease, BROWSER_SKILL_FILES, CLAUDE_PLUGIN_FILES, DEVICE_SKILL_FILES, EmbeddedFile,
        HORIZON_BROWSER_SKILL, HORIZON_DEVICE_SKILL, HORIZON_NOTIFY_SKILL, NOTIFY_SKILL_FILES, NOTIFY_SKILL_ROOTS,
        abandoned_user_skill_dirs, agent_plugin_host_lock_path, install_agent_plugins_impl, open_lock_file,
        prune_stale_agent_plugin_hosts, sync_file_if_changed, sync_leased_user_skills, sync_plugin_files,
        user_skill_dir, user_skill_lease_dirs,
    };

    fn write_skill_dir(path: &Path, body: &str) {
        std::fs::create_dir_all(path).expect("skill dir");
        std::fs::write(path.join("SKILL.md"), body).expect("skill file");
    }

    fn write_blocked_file(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("blocked parent");
        }
        std::fs::write(path, "not a directory").expect("blocked file");
    }

    #[test]
    fn user_skill_lease_dirs_cover_private_homes_not_abandoned_broadcasts() {
        let home = Path::new("/tmp/horizon-user");
        let dirs = user_skill_lease_dirs(Some(home), None, None);
        let abandoned = abandoned_user_skill_dirs(home);

        assert!(dirs.contains(&home.join(".claude/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".gemini/antigravity-cli/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".codex/skills/horizon-notify")));
        assert!(dirs.contains(&home.join(".codex/skills/horizon-browser")));
        assert!(dirs.contains(&home.join(".codex/skills/horizon-device")));
        assert!(!dirs.contains(&home.join(".grok/skills/horizon-device")));
        assert!(!dirs.contains(&home.join(".agents/skills/horizon-notify")));
        assert!(!dirs.contains(&home.join(".agents/skills/horizon-browser")));
        assert!(!dirs.contains(&home.join(".agents/skills/horizon-device")));
        assert!(!dirs.contains(&home.join(".gemini/skills/horizon-notify")));
        assert!(!dirs.contains(&home.join(".kilocode/skills/horizon-browser")));
        assert!(!dirs.contains(&home.join(".kilocode/skills/horizon-device")));
        assert!(!dirs.contains(&home.join(".grok/skills/horizon-browser")));
        assert!(abandoned.contains(&home.join(".agents/skills/horizon-notify")));
        assert!(abandoned.contains(&home.join(".agents/skills/horizon-browser")));
        assert!(abandoned.contains(&home.join(".gemini/skills/horizon-notify")));
        assert!(abandoned.contains(&home.join(".kilocode/skills/horizon-browser")));
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

    fn assert_installed_skill(path: &Path, content: &str) {
        assert_eq!(
            std::fs::read_to_string(path).unwrap_or_else(|_| panic!("skill missing at {}", path.display())),
            content,
        );
    }

    fn assert_claude_plugin_skill(plugin_dir: &Path, relative_path: &str) {
        assert_eq!(
            std::fs::read_to_string(plugin_dir.join(relative_path)).expect("claude plugin skill"),
            CLAUDE_PLUGIN_FILES
                .iter()
                .find(|file| file.relative_path == relative_path)
                .expect("claude plugin file")
                .content,
        );
    }

    fn assert_browser_and_device_skill_split() {
        assert!(BROWSER_SKILL_FILES[0].content.contains("browser_visibility"));
        assert!(BROWSER_SKILL_FILES[0].content.contains("browser_network_watch"));
        assert!(
            !BROWSER_SKILL_FILES[0].content.contains("device_panel"),
            "browser skill must not own native VNC Device panel lifecycle"
        );
        assert!(DEVICE_SKILL_FILES[0].content.contains("device_panel"));
        assert!(DEVICE_SKILL_FILES[0].content.contains("native VNC"));
    }

    fn assert_skill_absent(path: &Path, reason: &str) {
        assert!(!path.exists(), "{reason}");
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
            None,
        )
        .expect("install plugins");

        assert!(updated > 0);
        for skill_root in NOTIFY_SKILL_ROOTS {
            assert_installed_skill(
                &user_skill_dir(&user_home, skill_root, HORIZON_NOTIFY_SKILL).join("SKILL.md"),
                NOTIFY_SKILL_FILES[0].content,
            );
        }
        assert_installed_skill(
            &user_home.join(".grok/skills/horizon-notify/SKILL.md"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_installed_skill(
            &user_home.join(".codex/skills/horizon-notify/SKILL.md"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_installed_skill(
            &user_home.join(".codex/skills/horizon-browser/SKILL.md"),
            BROWSER_SKILL_FILES[0].content,
        );
        assert_installed_skill(
            &user_home.join(".codex/skills/horizon-device/SKILL.md"),
            DEVICE_SKILL_FILES[0].content,
        );
        assert_installed_skill(
            &user_home.join(".grok/skills/horizon-device/SKILL.md"),
            DEVICE_SKILL_FILES[0].content,
        );
        assert_skill_absent(
            &user_home.join(".agents/skills/horizon-notify/SKILL.md"),
            "notify skill must not broadcast through ~/.agents/skills",
        );
        assert_skill_absent(
            &user_home.join(".agents/skills/horizon-browser/SKILL.md"),
            "browser MCP skill must not broadcast through ~/.agents/skills",
        );
        assert_skill_absent(
            &user_home.join(".gemini/skills/horizon-notify/SKILL.md"),
            "notify skill must use the Antigravity CLI home, not the Gemini CLI home",
        );
        assert_skill_absent(
            &user_home.join(".kilocode/skills/horizon-browser/SKILL.md"),
            "browser MCP skill must not be exported to agents without Horizon MCP injection",
        );
        assert_skill_absent(
            &user_home.join(".kilocode/skills/horizon-device/SKILL.md"),
            "device skill must not be exported to agents without Horizon MCP injection",
        );
        assert!(
            user_home.join(".grok/skills/horizon-browser/SKILL.md").exists(),
            "Grok receives the matching browser MCP skill"
        );
        assert_installed_skill(
            &horizon_home.codex_skill_dir().join("SKILL.md"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_claude_plugin_skill(&claude_plugin_dir, "skills/horizon-notify/SKILL.md");
        assert_claude_plugin_skill(&claude_plugin_dir, "skills/horizon-device/SKILL.md");
        assert_installed_skill(
            &horizon_home
                .codex_integrations_dir()
                .join(HORIZON_DEVICE_SKILL)
                .join("SKILL.md"),
            DEVICE_SKILL_FILES[0].content,
        );
        assert_browser_and_device_skill_split();
        let mcp_config = std::fs::read_to_string(claude_plugin_dir.join(".mcp.json"))
            .expect("Claude MCP config should be installed");
        assert!(mcp_config.contains("/opt/horizon"));
        assert!(mcp_config.contains("--browser-mcp"));
        assert!(mcp_config.contains("3660000"));
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

        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills(&super::user_skill_lease_dirs(Some(&user_home), None, None))
            .expect("bind user skills");
        sync_leased_user_skills(
            &lease,
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            None,
            None,
            Path::new("/opt/horizon"),
        );

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
            None,
        )
        .expect("install plugins");

        assert_eq!(
            std::fs::read_to_string(grok_home.join("skills/horizon-notify/SKILL.md"))
                .expect("notify skill should be installed under GROK_HOME"),
            NOTIFY_SKILL_FILES[0].content,
        );
        assert_eq!(
            std::fs::read_to_string(grok_home.join("skills/horizon-device/SKILL.md"))
                .expect("device skill should be installed under GROK_HOME"),
            DEVICE_SKILL_FILES[0].content,
        );
        assert!(
            !user_home.join(".grok/skills/horizon-notify/SKILL.md").exists(),
            "GROK_HOME must replace ~/.grok rather than writing both"
        );
        assert!(
            !user_home.join(".grok/skills/horizon-device/SKILL.md").exists(),
            "GROK_HOME must replace ~/.grok for the device skill rather than writing both"
        );
    }

    #[test]
    fn grok_device_skill_requires_browser_mcp_registration() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");
        let grok_root = user_home.join(".grok");
        let leftover = grok_root.join("skills").join(HORIZON_DEVICE_SKILL);
        write_skill_dir(&leftover, DEVICE_SKILL_FILES[0].content);
        let custom = grok_root.join("skills").join("custom-device");
        write_skill_dir(&custom, "user skill");
        let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills(&user_skill_lease_dirs(Some(&user_home), None, None))
            .expect("bind user skills");
        lease.bind_grok_device_skill(&grok_root, false);
        sync_leased_user_skills(
            &lease,
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            None,
            None,
            Path::new("/opt/horizon"),
        );

        assert!(user_home.join(".grok/skills/horizon-notify/SKILL.md").is_file());
        assert!(
            !user_home.join(".grok/skills/horizon-browser/SKILL.md").exists(),
            "Grok browser skill is leased only after MCP registration"
        );
        drop(lease);
        assert!(
            !leftover.exists(),
            "failed Grok browser registration must not retain a leftover Horizon-owned device skill"
        );
        assert!(
            custom.join("SKILL.md").is_file(),
            "unrelated Grok skills must be preserved"
        );
    }

    #[test]
    fn grok_preserves_a_user_managed_device_skill() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let grok_root = temp.path().join("user-home/.grok");
        let custom = grok_root.join("skills").join(HORIZON_DEVICE_SKILL);
        write_skill_dir(&custom, "user device skill");
        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease.bind_grok_device_skill(&grok_root, true);
        lease.bind_grok_device_skill(&grok_root, false);
        drop(lease);
        assert_eq!(
            std::fs::read_to_string(custom.join("SKILL.md")).expect("preserved skill"),
            "user device skill"
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
            None,
        )
        .expect("install first host plugin");
        install_agent_plugins_impl(
            &horizon_home,
            &second_plugin_dir,
            None,
            None,
            None,
            Path::new("/opt/horizon-b"),
            None,
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
        let skill_dir = temp.path().join("user-home/.grok/skills").join(HORIZON_NOTIFY_SKILL);
        write_skill_dir(&skill_dir, "leased");
        let browser = temp.path().join("user-home/.grok/skills").join(HORIZON_BROWSER_SKILL);
        write_skill_dir(&browser, "keep");
        let unrelated = temp.path().join("user-home/.grok/skills/custom-skill");
        write_skill_dir(&unrelated, "keep");

        let host_dir = horizon_home.agent_plugin_host_dir("host-a");
        let mut lease = AgentPluginHostLease::acquire(host_dir).expect("host lease");
        lease
            .bind_user_skills(std::slice::from_ref(&skill_dir))
            .expect("bind user skills");
        drop(lease);

        assert!(!skill_dir.exists());
        assert!(browser.join("SKILL.md").is_file());
        assert!(unrelated.join("SKILL.md").is_file());
        assert!(
            skill_dir
                .parent()
                .expect("skill parent")
                .join(".horizon-leases/.lock")
                .is_file(),
            "last host must keep the skill-root coordination lock"
        );
    }

    #[test]
    fn bind_user_skills_keeps_acquired_roots_when_a_later_root_fails() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let leftover = temp.path().join("codex-a/skills").join(HORIZON_NOTIFY_SKILL);
        write_skill_dir(&leftover, "stale");
        let blocked_parent = temp.path().join("blocked/skills");
        write_blocked_file(&blocked_parent);
        let blocked = blocked_parent.join(HORIZON_NOTIFY_SKILL);

        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills(&[leftover.clone(), blocked])
            .expect("partial bind must succeed");

        assert!(
            leftover.join("SKILL.md").is_file(),
            "writable roots stay leased when another root cannot be leased"
        );

        drop(lease);
        assert!(!leftover.exists());
        assert!(
            leftover
                .parent()
                .expect("skill parent")
                .join(".horizon-leases/.lock")
                .is_file(),
            "last host must keep the skill-root coordination lock"
        );
    }

    #[test]
    fn sync_skips_unleased_roots_without_blocking_writable_homes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let user_home = temp.path().join("user-home");
        let claude_plugin_dir = horizon_home.claude_plugin_dir_for_host("host-a");
        write_blocked_file(&user_home.join(".config/opencode/skills"));
        write_blocked_file(&user_home.join(".agents/skills"));

        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills(&user_skill_lease_dirs(Some(&user_home), None, None))
            .expect("bind writable roots");
        sync_leased_user_skills(
            &lease,
            &horizon_home,
            &claude_plugin_dir,
            Some(&user_home),
            None,
            None,
            Path::new("/opt/horizon"),
        );

        assert!(user_home.join(".claude/skills/horizon-notify/SKILL.md").is_file());
        assert!(
            user_home
                .join(".gemini/antigravity-cli/skills/horizon-notify/SKILL.md")
                .is_file()
        );
        assert!(user_home.join(".codex/skills/horizon-notify/SKILL.md").is_file());
        assert!(!user_home.join(".config/opencode/skills/horizon-notify").exists());
        assert!(!user_home.join(".agents/skills/horizon-notify").exists());
    }

    #[test]
    fn last_host_removes_abandoned_browser_on_a_notify_root() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let notify = temp
            .path()
            .join("user-home/.kilocode/skills")
            .join(HORIZON_NOTIFY_SKILL);
        let browser = temp
            .path()
            .join("user-home/.kilocode/skills")
            .join(HORIZON_BROWSER_SKILL);
        write_skill_dir(&notify, "leased");
        write_skill_dir(&browser, "abandoned");

        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills_with_cleanup(std::slice::from_ref(&notify), std::slice::from_ref(&browser))
            .expect("bind notify with abandoned browser cleanup");
        drop(lease);

        assert!(!notify.exists());
        assert!(!browser.exists());
    }

    #[test]
    fn last_host_for_a_skill_name_does_not_wait_on_a_sibling_skill() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let skills = temp.path().join("shared/skills");
        let notify = skills.join(HORIZON_NOTIFY_SKILL);
        let browser = skills.join(HORIZON_BROWSER_SKILL);
        write_skill_dir(&notify, "shared");
        write_skill_dir(&browser, "codex-only");

        let mut grok =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("grok-host")).expect("grok lease");
        let mut codex =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("codex-host")).expect("codex lease");
        grok.bind_user_skills(std::slice::from_ref(&notify))
            .expect("bind grok notify");
        codex
            .bind_user_skills(&[notify.clone(), browser.clone()])
            .expect("bind codex notify and browser");

        drop(codex);
        assert!(notify.join("SKILL.md").is_file());
        assert!(
            !browser.exists(),
            "last host for horizon-browser must remove it while notify remains leased"
        );

        drop(grok);
        assert!(!notify.exists());
    }

    #[test]
    fn last_host_removes_cleanup_only_abandoned_roots() {
        let temp = tempfile::tempdir().expect("temp dir");
        let horizon_home = HorizonHome::from_root(temp.path().join(".horizon"));
        let notify = temp.path().join("user-home/.claude/skills").join(HORIZON_NOTIFY_SKILL);
        let abandoned = temp.path().join("user-home/.agents/skills").join(HORIZON_NOTIFY_SKILL);
        write_skill_dir(&notify, "leased");
        write_skill_dir(&abandoned, "stale");

        let mut lease =
            AgentPluginHostLease::acquire(horizon_home.agent_plugin_host_dir("host-a")).expect("host lease");
        lease
            .bind_user_skills_with_cleanup(std::slice::from_ref(&notify), std::slice::from_ref(&abandoned))
            .expect("bind notify with abandoned cleanup-only root");
        drop(lease);

        assert!(!notify.exists());
        assert!(!abandoned.exists());
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
        first
            .bind_user_skills(std::slice::from_ref(&skill_dir))
            .expect("bind first skills");
        second
            .bind_user_skills(std::slice::from_ref(&skill_dir))
            .expect("bind second skills");

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
        first
            .bind_user_skills(std::slice::from_ref(&first_skill))
            .expect("bind first skills");
        second
            .bind_user_skills(std::slice::from_ref(&second_skill))
            .expect("bind second skills");

        drop(first);
        assert!(!first_skill.exists());
        assert!(second_skill.join("SKILL.md").is_file());

        drop(second);
        assert!(!second_skill.exists());
    }

    #[test]
    fn user_skill_leases_coordinate_through_the_target_skill_root() {
        let temp = tempfile::tempdir().expect("temp dir");
        let first_home = HorizonHome::from_root(temp.path().join("horizon-a"));
        let second_home = HorizonHome::from_root(temp.path().join("horizon-b"));
        let skill_dir = temp.path().join("codex/skills").join(HORIZON_NOTIFY_SKILL);
        write_skill_dir(&skill_dir, "shared");

        let mut first = AgentPluginHostLease::acquire(first_home.agent_plugin_host_dir("host-a")).expect("first lease");
        let mut second =
            AgentPluginHostLease::acquire(second_home.agent_plugin_host_dir("host-b")).expect("second lease");
        first
            .bind_user_skills(std::slice::from_ref(&skill_dir))
            .expect("bind first skills");
        second
            .bind_user_skills(std::slice::from_ref(&skill_dir))
            .expect("bind second skills");

        drop(first);
        assert!(skill_dir.join("SKILL.md").is_file());
        drop(second);
        assert!(!skill_dir.exists());
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
