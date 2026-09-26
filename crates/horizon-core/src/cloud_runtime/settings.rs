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
    /// Omitted unless Hetzner is configured, so existing settings keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hetzner: Option<Hetzner>,
}

/// A Hetzner Cloud project this machine may deploy CPU clouds into. The token
/// is project-wide, so it stays on this machine and never reaches a worker.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hetzner {
    pub token_file: PathBuf,
    /// Server types to try, in order, such as `cx43`.
    pub server_types: Vec<String>,
    /// Locations to try, in order, such as `hel1`.
    pub locations: Vec<String>,
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
        // A Hetzner cloud's chosen data centers are Hetzner locations, and they can
        // only narrow the locations this machine allows. A RunPod data center never
        // names a Hetzner location, so RunPod clouds leave the list unchanged.
        if let Some(hetzner) = settings.hetzner.as_mut() {
            let chosen: Vec<String> = placement
                .data_centers
                .iter()
                .filter(|location| hetzner.locations.contains(location))
                .cloned()
                .collect();
            if !chosen.is_empty() {
                hetzner.locations = chosen;
            }
        }
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
        if let Some(hetzner) = &self.hetzner {
            hetzner.validate()?;
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

impl Hetzner {
    /// # Errors
    /// Requires an absolute token path and at least one valid server type and location.
    pub fn validate(&self) -> Result<()> {
        if !self.token_file.is_absolute() {
            return Err(Error::Invalid(
                "Credential bindings must use absolute machine-local paths",
            ));
        }
        let names = |values: &[String]| {
            !values.is_empty()
                && values.iter().all(|value| {
                    !value.is_empty()
                        && value.len() <= 64
                        && value
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
        };
        if !names(&self.server_types) || !names(&self.locations) {
            return Err(Error::Invalid(
                "Hetzner settings need server types and locations such as cx43 and hel1",
            ));
        }
        Ok(())
    }

    /// # Errors
    /// Requires a private, readable token file.
    pub fn credential(&self) -> Result<Credential> {
        self.validate()?;
        validate_private_key_file(&self.token_file)?;
        Credential::new(std::fs::read_to_string(&self.token_file)?.trim().to_owned()).map_err(Error::from)
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
    fn hetzner_settings_are_optional_validated_and_narrowed_by_a_clouds_locations() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let absolute = |name: &str| root.path().join(name);
        let base = serde_json::json!({
            "runpod_key_file": absolute("key"), "ssh_identity_file": absolute("identity"),
            "docker_config": absolute("docker"), "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
        });
        std::fs::write(&path, base.to_string()).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert!(loaded.hetzner.is_none());
        assert!(
            serde_json::to_value(&loaded).unwrap().get("hetzner").is_none(),
            "settings without Hetzner keep their encoding"
        );
        let mut with_hetzner = base.clone();
        with_hetzner["hetzner"] = serde_json::json!({
            "token_file": absolute("hetzner-token"), "server_types": ["cx43", "cpx42"], "locations": ["hel1", "nbg1"],
        });
        std::fs::write(&path, with_hetzner.to_string()).unwrap();
        let any = crate::cloud_panel::Placement::default();
        assert_eq!(
            Settings::for_cloud(&path, &any).unwrap().hetzner.unwrap().locations,
            ["hel1", "nbg1"]
        );
        let nuremberg = crate::cloud_panel::Placement {
            data_centers: vec!["nbg1".into(), "fsn1".into()],
            ..Default::default()
        };
        assert_eq!(
            Settings::for_cloud(&path, &nuremberg)
                .unwrap()
                .hetzner
                .unwrap()
                .locations,
            ["nbg1"],
            "a placement narrows the allowed locations and cannot add one"
        );
        let runpod = crate::cloud_panel::Placement {
            data_centers: vec!["EU-RO-1".into()],
            ..Default::default()
        };
        let settings = Settings::for_cloud(&path, &runpod).unwrap();
        assert_eq!(settings.hetzner.as_ref().unwrap().locations, ["hel1", "nbg1"]);
        assert!(
            settings.validate().is_ok(),
            "a RunPod placement never invalidates Hetzner settings"
        );
        for (field, value) in [
            ("token_file", serde_json::json!("relative-token")),
            ("server_types", serde_json::json!([])),
            ("locations", serde_json::json!(["Hel 1"])),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut invalid = with_hetzner.clone();
            invalid["hetzner"][field] = value;
            std::fs::write(&path, invalid.to_string()).unwrap();
            assert!(Settings::load(&path).is_err(), "{field}");
        }
        let token = absolute("hetzner-token");
        std::fs::write(&token, "secret-token\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
            let hetzner = serde_json::from_value::<Hetzner>(with_hetzner["hetzner"].clone()).unwrap();
            assert!(hetzner.credential().is_err(), "a readable token file is refused");
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(hetzner.credential().is_ok());
        }
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
