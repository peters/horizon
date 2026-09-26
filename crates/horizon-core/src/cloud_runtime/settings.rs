//! Machine-local bindings. Repository configuration cannot select credentials.
use super::{Error, Result};
use horizon_cloud::Credential;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Starting selection for repository setup; existing profiles keep their own capabilities.
    #[serde(default = "default_agents")]
    pub default_agents: Vec<horizon_cloud::Agent>,
    pub runpod_key_file: PathBuf,
    pub ssh_identity_file: PathBuf,
    pub docker_config: PathBuf,
    #[serde(default)]
    pub docker_host: Option<String>,
    pub registry_pull_auth_id: Option<String>,
    #[serde(default)]
    pub registries: Option<super::registry::Config>,
    pub cpu_flavors: Vec<String>,
    pub gpu_types: Vec<String>,
    #[serde(default)]
    pub data_centers: Vec<String>,
    /// Optional explicit API authentication; otherwise the agent uses its normal login flow.
    #[serde(default)]
    pub anthropic_api_key_file: Option<PathBuf>,
    #[serde(default)]
    pub openai_api_key_file: Option<PathBuf>,
    #[serde(default)]
    pub anthropic_workspace_id: Option<String>,
    /// Explicit opt-in per local repository; never loaded from repository YAML.
    #[serde(default)]
    pub git_credentials: Vec<super::git_auth::Binding>,
    #[serde(default)]
    pub browserstack_credentials: Vec<super::browser_auth::Binding>,
}
#[must_use]
pub fn default_agents() -> Vec<horizon_cloud::Agent> {
    vec![horizon_cloud::Agent::Codex, horizon_cloud::Agent::Claude]
}
impl Settings {
    /// # Errors
    /// Rejects missing/malformed settings and relative binding paths.
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = serde_json::from_slice(&std::fs::read(path)?).map_err(|_| Error::Json)?;
        value.validate()?;
        Ok(value)
    }

    /// The machine settings narrowed to one cloud's placement, so every attempt,
    /// retry and redeploy of that cloud asks for the data centers and GPU types
    /// chosen for it.
    /// # Errors
    /// As [`Settings::load`].
    pub fn for_cloud(path: &Path, placement: &crate::cloud_panel::Placement) -> Result<Self> {
        let mut settings = Self::load(path)?;
        placement.apply(&mut settings.data_centers, &mut settings.gpu_types);
        Ok(settings)
    }
    /// # Errors
    /// Validates bindings supplied through either the file or Rust interface.
    pub fn validate(&self) -> Result<()> {
        if let Some(registries) = &self.registries {
            registries.validate()?;
        }
        if [&self.runpod_key_file, &self.ssh_identity_file, &self.docker_config]
            .iter()
            .any(|p| !p.is_absolute())
        {
            return Err(Error::Invalid(
                "Credential bindings must use absolute machine-local paths",
            ));
        }
        if [&self.anthropic_api_key_file, &self.openai_api_key_file]
            .into_iter()
            .flatten()
            .any(|p| !p.is_absolute())
        {
            return Err(Error::Invalid(
                "Agent credential bindings must use absolute machine-local paths",
            ));
        }
        if self.anthropic_workspace_id.as_ref().is_some_and(|id| {
            !id.starts_with("wrkspc_") || id.len() > 100 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            return Err(Error::Invalid("Invalid agent API workspace binding"));
        }
        for binding in &self.browserstack_credentials {
            binding.validate()?;
        }
        for binding in &self.git_credentials {
            binding.validate()?;
        }
        Ok(())
    }
    /// # Errors
    /// Requires a private, readable API-key file.
    pub fn credential(&self) -> Result<Credential> {
        self.validate()?;
        validate_private_key_file(&self.runpod_key_file)?;
        Credential::new(std::fs::read_to_string(&self.runpod_key_file)?.trim().to_owned()).map_err(Error::from)
    }
}

pub(super) fn validate_ssh_identity(path: &Path) -> Result<()> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > 64 * 1024 {
        return Err(Error::Invalid("Invalid SSH private-key file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("SSH private-key file must be private (0600)"));
        }
    }
    let mut first_byte = [0];
    std::fs::File::open(path)?.read_exact(&mut first_byte)?;
    Ok(())
}

pub(super) fn validate_private_key_file(path: &Path) -> Result<()> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("API-key file must be private (0600)"));
        }
    }
    if !meta.is_file() || meta.len() == 0 || meta.len() > 4096 {
        return Err(Error::Invalid("Invalid API-key file"));
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    std::fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 || bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::Invalid("API-key file must contain a nonempty credential"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_asks_only_for_the_data_centers_and_gpu_types_chosen_for_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let absolute = |name: &str| root.path().join(name);
        std::fs::write(
            &path,
            serde_json::json!({
                "runpod_key_file": absolute("key"), "ssh_identity_file": absolute("identity"),
                "docker_config": absolute("docker"), "registry_pull_auth_id": null,
                "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
            })
            .to_string(),
        )
        .unwrap();
        let any = crate::cloud_panel::Placement::default();
        assert_eq!(Settings::for_cloud(&path, &any).unwrap().data_centers, ["US-MO-2"]);
        let europe = crate::cloud_panel::Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
            gpu_types: vec!["NVIDIA RTX A5000".into()],
        };
        let settings = Settings::for_cloud(&path, &europe).unwrap();
        assert_eq!(settings.data_centers, ["EU-RO-1"]);
        assert_eq!(settings.gpu_types, ["NVIDIA RTX A5000"]);
    }

    #[test]
    fn ssh_identity_requires_a_readable_nonempty_private_file() {
        let root = tempfile::tempdir().unwrap();
        let key = root.path().join("identity");
        std::fs::write(root.path().join("identity.pub"), "ssh-ed25519 synthetic-public-key").unwrap();
        assert!(validate_ssh_identity(&key).is_err());
        assert!(validate_ssh_identity(root.path()).is_err());
        let file = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        assert!(validate_ssh_identity(file.path()).is_err());
        std::fs::write(file.path(), "synthetic-private-identity").unwrap();
        assert!(validate_ssh_identity(file.path()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(validate_ssh_identity(file.path()).is_err());
        }
    }
}
