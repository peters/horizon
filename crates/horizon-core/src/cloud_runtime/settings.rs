//! Machine-local bindings. Repository configuration cannot select credentials.
mod azure;
pub use azure::Azure;

use super::{Error, Result};
use horizon_cloud::Credential;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// How many Docker hosts Cloud settings saves; each reading asks all of them at once.
/// A file edited by hand with more still loads, and a reading then reads none of them.
pub const MAX_DOCKER_HOSTS: usize = 16;

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
    /// Local image destinations scoped to a canonical checkout and named profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_publishing: Vec<super::publishing::Override>,
    /// Legacy in-process switch. Saved staging values no longer disable the completed flow.
    /// Kept for lower-level callers that explicitly construct a legacy request; never stored.
    #[serde(
        default = "Settings::portable_image_publishing_default",
        deserialize_with = "completed_portable_publishing",
        skip_serializing
    )]
    pub portable_image_publishing: bool,
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
    /// Claude's token from `claude setup-token`, given to the worker as
    /// `CLAUDE_CODE_OAUTH_TOKEN`; never together with the API key. Omitted until one is saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_token: Option<super::claude_token::ClaudeToken>,
    /// Explicit opt-in per local repository; never loaded from repository YAML.
    #[serde(default)]
    pub git_credentials: Vec<super::git_auth::Binding>,
    #[serde(default)]
    pub browserstack_credentials: Vec<super::browser_auth::Binding>,
    /// Omitted unless Hetzner is configured, so existing settings keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hetzner: Option<Hetzner>,
    /// Omitted unless Azure is configured, so existing settings keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure: Option<Azure>,
    /// Docker engines this machine may run clouds on, each under a unique name. Omitted
    /// until one is saved, so existing settings keep their encoding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docker_hosts: Vec<horizon_cloud::docker_host::Host>,
    /// New clouds are not offered the saved Docker hosts; clouds already on one keep
    /// working. Omitted while false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub docker_hosts_off: bool,
    /// The GitHub App of Connect GitHub. Omitted until it is connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<super::github::Settings>,
    /// The cloud's own placement, kept by [`Settings::for_cloud`] for providers
    /// that read it themselves. Never stored.
    #[serde(skip)]
    pub placement: Option<crate::cloud_panel::Placement>,
    /// The Codex home of this computer's `ChatGPT` login, beside the settings file. Set
    /// by [`Settings::load`]; never stored.
    #[serde(skip)]
    pub codex_home: Option<PathBuf>,
}

/// A Hetzner Cloud project this machine may deploy CPU clouds into. The token
/// is project-wide, so it stays on this machine and never reaches a worker.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hetzner {
    pub token_file: PathBuf,
    /// Fallback server types to try, in order, when no exact catalog worker is chosen.
    pub server_types: Vec<String>,
    /// Locations to try, in order, such as `hel1`.
    pub locations: Vec<String>,
    /// A read-only pull credential for a private worker image. It reaches the
    /// server's user data, which stays readable to the host for the server's
    /// life, so use a short-lived token scoped to the image repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_pull: Option<RegistryPull>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegistryPull {
    /// The registry host, such as `example.azurecr.io`.
    pub server: String,
    pub username: String,
    /// A private file holding the token or password.
    pub password_file: PathBuf,
}
impl RegistryPull {
    /// The host login this binding names.
    /// # Errors
    /// Requires a private, readable password file.
    pub fn login(&self) -> Result<horizon_cloud::host::RegistryLogin> {
        validate_private_key_file(&self.password_file)?;
        let read = read_secret(&self.password_file)?;
        let trimmed = std::str::from_utf8(&read)
            .map_err(|_| Error::Invalid("The registry pull password file must hold one printable token"))?
            .trim();
        // Checked here so a refused password is never moved into a buffer that
        // is dropped unwiped.
        if trimmed.is_empty() || trimmed.bytes().any(|b| b <= 32 || b >= 127) {
            return Err(Error::Invalid(
                "The registry pull password file must hold one printable token",
            ));
        }
        let password = Credential::new(trimmed.to_owned())?;
        Ok(horizon_cloud::host::RegistryLogin {
            server: self.server.clone(),
            username: self.username.clone(),
            password,
        })
    }

    /// Whether the registry host, username and password path can make a host login.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.password_file.is_absolute()
            && horizon_cloud::host::RegistryLogin::valid_identity(&self.server, &self.username)
    }
}

#[must_use]
pub fn default_agents() -> Vec<horizon_cloud::Agent> {
    vec![horizon_cloud::Agent::Codex, horizon_cloud::Agent::Claude]
}
fn completed_portable_publishing<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bool::deserialize(deserializer).map(|_| Settings::portable_image_publishing_default())
}

impl Settings {
    pub(in crate::cloud_runtime) const fn portable_image_publishing_default() -> bool {
        true
    }

    /// # Errors
    /// Rejects missing/malformed settings and relative binding paths.
    pub fn load(path: &Path) -> Result<Self> {
        let mut value: Self = serde_json::from_slice(&std::fs::read(path)?).map_err(|_| Error::Json)?;
        value.validate()?;
        value.codex_home = path.parent().map(super::codex_login::home);
        super::github::remember(value.github.is_some());
        Ok(value)
    }

    /// Whether New cloud and `cloud_offers` offer the saved Docker hosts.
    #[must_use]
    pub fn docker_hosts_on(&self) -> bool {
        !self.docker_hosts.is_empty() && !self.docker_hosts_off
    }

    /// The machine settings narrowed to one cloud's placement, so every attempt,
    /// retry and redeploy of that cloud asks for the data centers and GPU types
    /// chosen for it.
    /// # Errors
    /// As [`Settings::load`].
    pub fn for_cloud(path: &Path, placement: &crate::cloud_panel::Placement) -> Result<Self> {
        let mut settings = Self::load(path)?;
        placement.apply(&mut settings.data_centers, &mut settings.gpu_types);
        settings.placement = Some(placement.clone());
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
        if let Some(token) = &self.claude_token {
            if !token.file.is_absolute() {
                return Err(Error::Invalid(
                    "Agent credential bindings must use absolute machine-local paths",
                ));
            }
            if self.anthropic_api_key_file.is_some() {
                return Err(Error::Invalid("Give Claude an API key or a setup token, not both"));
            }
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
        let mut scopes = std::collections::BTreeSet::new();
        for choice in &self.image_publishing {
            choice.validate_scope()?;
            if !scopes.insert((&choice.repository, &choice.profile)) {
                return Err(Error::Invalid("Keep one image destination per checkout and profile"));
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for host in &self.docker_hosts {
            host.validate()?;
            if !names.insert(&host.name) {
                return Err(Error::Invalid("Give each saved Docker host its own name"));
            }
        }
        if let Some(hetzner) = &self.hetzner {
            hetzner.validate()?;
        }
        if let Some(azure) = &self.azure {
            azure.validate()?;
        }
        if let Some(github) = &self.github {
            github.validate()?;
        }
        Ok(())
    }
    /// The machine's Azure settings, which every Azure cloud needs.
    /// # Errors
    /// Refuses a machine without an `azure` section.
    pub fn azure_settings(&self) -> Result<&Azure> {
        self.azure.as_ref().ok_or(Error::Invalid(
            "Add an azure section to the cloud settings before deploying an Azure cloud",
        ))
    }
    /// Whether this machine has a `RunPod` API key; a machine may use Hetzner alone.
    #[must_use]
    pub fn runpod_configured(&self) -> bool {
        self.runpod_key_file.is_file()
    }
    /// # Errors
    /// Requires a private, readable API-key file.
    pub fn credential(&self) -> Result<Credential> {
        self.validate()?;
        if !self.runpod_configured() {
            return Err(Error::Invalid(RUNPOD_KEY_MISSING));
        }
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
        if !provider_names(&self.server_types) || !provider_names(&self.locations) {
            return Err(Error::Invalid(
                "Hetzner settings need server types and locations such as cx43 and hel1",
            ));
        }
        if self.registry_pull.as_ref().is_some_and(|pull| {
            !pull.password_file.is_absolute()
                || !horizon_cloud::host::RegistryLogin::valid_identity(&pull.server, &pull.username)
        }) {
            return Err(Error::Invalid(
                "The Hetzner registry pull needs a registry host such as example.azurecr.io, a username without ':' and an absolute password file",
            ));
        }
        Ok(())
    }

    /// The locations a Hetzner cloud may use: those its placement chose, in that
    /// order, or every allowed location when it chose none.
    /// # Errors
    /// Refuses a placement whose locations this machine does not allow, rather
    /// than moving the cloud somewhere it was not placed.
    pub fn locations_for(&self, placement: Option<&crate::cloud_panel::Placement>) -> Result<Vec<String>> {
        let Some(placement) = placement.filter(|placement| !placement.data_centers.is_empty()) else {
            return Ok(self.locations.clone());
        };
        let chosen: Vec<String> = placement
            .data_centers
            .iter()
            .filter(|location| self.locations.contains(location))
            .cloned()
            .collect();
        if chosen.is_empty() {
            return Err(Error::Invalid(
                "None of this cloud's chosen locations is allowed in the Hetzner settings",
            ));
        }
        Ok(chosen)
    }

    /// An exact catalog choice takes precedence over the fallback preferences.
    /// # Errors
    /// Rejects malformed server type names before any provider request.
    pub fn types_for(&self, placement: Option<&crate::cloud_panel::Placement>) -> Result<Vec<String>> {
        let Some(placement) = placement.filter(|placement| !placement.cpu_types.is_empty()) else {
            return Ok(self.server_types.clone());
        };
        if !provider_names(&placement.cpu_types) {
            return Err(Error::Invalid(
                "This cloud's chosen server type must be a valid Hetzner name",
            ));
        }
        Ok(placement.cpu_types.clone())
    }

    /// The pull login for the worker host, when a private image needs one.
    /// # Errors
    /// Requires a private, readable password file.
    pub fn registry_login(&self) -> Result<Option<horizon_cloud::host::RegistryLogin>> {
        self.registry_pull.as_ref().map(RegistryPull::login).transpose()
    }

    /// # Errors
    /// Requires a private, readable token file.
    pub fn credential(&self) -> Result<Credential> {
        self.with_token(|token| Credential::new(token.to_owned()).map_err(Error::from))
    }

    /// The credential with a fingerprint of the same token, from one read of the file:
    /// the first 16 hex digits of its SHA-256. The fingerprint tells whether a later
    /// token is still the one a server was made with, without keeping the token.
    /// # Errors
    /// Requires a private, readable token file.
    pub fn credential_with_fingerprint(&self) -> Result<(Credential, String)> {
        use sha2::{Digest, Sha256};
        self.with_token(|token| {
            let digest = Sha256::digest(token.as_bytes());
            let fingerprint = digest[..8].iter().fold(String::new(), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            });
            Ok((Credential::new(token.to_owned())?, fingerprint))
        })
    }

    fn with_token<T>(&self, use_token: impl FnOnce(&str) -> Result<T>) -> Result<T> {
        self.validate()?;
        validate_private_key_file(&self.token_file)?;
        // Checked while borrowed, so no copy of the token outlives this call
        // unwiped, even when it is refused.
        let read = read_secret(&self.token_file)?;
        let token = std::str::from_utf8(&read)
            .map_err(|_| Error::Invalid("The Hetzner token file must hold one printable token"))?
            .trim();
        if token.is_empty() || token.bytes().any(|b| b <= 32 || b >= 127) {
            return Err(Error::Invalid("The Hetzner token file must hold one printable token"));
        }
        use_token(token)
    }
}

fn provider_names(values: &[String]) -> bool {
    !values.is_empty()
        && values
            .iter()
            .all(|value| horizon_cloud::hetzner::cloud::valid_placement_name(value))
}

/// A secret file's bytes in a wiped buffer sized up front for the largest
/// accepted file, so neither a failed read nor a reallocation leaves a copy
/// behind. Decoding happens on the borrowed bytes.
fn read_secret(path: &Path) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    const LIMIT: u64 = 4096;
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(usize::try_from(LIMIT + 1).unwrap_or(4097)));
    std::fs::File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(Error::Invalid("Credential file is too large"));
    }
    Ok(bytes)
}

/// Why a `RunPod` request cannot be made on a machine set up for Hetzner alone.
pub const RUNPOD_KEY_MISSING: &str = "This machine has no RunPod API key; add one in cloud settings to use RunPod";

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

/// Checks that `path` is a private key file that deployment accepts.
/// # Errors
/// The file is missing, empty, too large, unreadable, or readable by others.
pub fn validate_private_key_file(path: &Path) -> Result<()> {
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
    // Sized for the whole read up front, so no reallocation frees an unwiped copy.
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(4097));
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
            cpu_types: Vec::new(),
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
        let settings = Settings::for_cloud(&path, &any).unwrap();
        let hetzner = settings.hetzner.as_ref().unwrap();
        assert_eq!(
            hetzner.locations_for(settings.placement.as_ref()).unwrap(),
            ["hel1", "nbg1"]
        );
        assert_eq!(hetzner.locations_for(None).unwrap(), ["hel1", "nbg1"]);
        let placed = |locations: &[&str]| crate::cloud_panel::Placement {
            data_centers: locations.iter().map(|&location| location.into()).collect(),
            ..Default::default()
        };
        let settings = Settings::for_cloud(&path, &placed(&["nbg1", "fsn1"])).unwrap();
        assert_eq!(
            settings
                .hetzner
                .as_ref()
                .unwrap()
                .locations_for(settings.placement.as_ref())
                .unwrap(),
            ["nbg1"],
            "a placement narrows the allowed locations and cannot add one"
        );
        let settings = Settings::for_cloud(&path, &placed(&["fsn1"])).unwrap();
        assert!(
            settings
                .hetzner
                .as_ref()
                .unwrap()
                .locations_for(settings.placement.as_ref())
                .is_err(),
            "a cloud placed only outside the allowed locations is refused, not moved"
        );
        assert!(
            serde_json::to_value(&settings).unwrap().get("placement").is_none(),
            "the placement is never stored"
        );
        let runpod = Settings::for_cloud(&path, &placed(&["EU-RO-1"])).unwrap();
        assert_eq!(runpod.data_centers, ["EU-RO-1"]);
        assert!(
            runpod.validate().is_ok(),
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
            std::fs::write(&token, b"secret-token\xff").unwrap();
            assert!(hetzner.credential().is_err(), "a token that is not text is refused");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_hetzner_registry_pull_is_optional_private_and_redacted() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let absolute = |name: &str| root.path().join(name);
        let mut pulling = serde_json::json!({
            "runpod_key_file": absolute("key"), "ssh_identity_file": absolute("identity"),
            "docker_config": absolute("docker"), "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"], "gpu_types": [],
            "hetzner": {"token_file": absolute("hetzner-token"), "server_types": ["cx43"], "locations": ["hel1"]},
        });
        std::fs::write(&path, pulling.to_string()).unwrap();
        let hetzner = Settings::load(&path).unwrap().hetzner.unwrap();
        assert!(
            hetzner.registry_login().unwrap().is_none(),
            "public images need no login"
        );
        let password = absolute("pull-token");
        std::fs::write(&password, "pull-secret\n").unwrap();
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
        pulling["hetzner"]["registry_pull"] =
            serde_json::json!({"server": "example.azurecr.io", "username": "pull", "password_file": password});
        std::fs::write(&path, pulling.to_string()).unwrap();
        let login = Settings::load(&path)
            .unwrap()
            .hetzner
            .unwrap()
            .registry_login()
            .unwrap()
            .unwrap();
        assert_eq!(
            (login.server.as_str(), login.username.as_str()),
            ("example.azurecr.io", "pull")
        );
        assert!(!format!("{login:?}").contains("pull-secret"));
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            Settings::load(&path)
                .unwrap()
                .hetzner
                .unwrap()
                .registry_login()
                .is_err()
        );
        for (field, value) in [
            ("server", "https://example.azurecr.io"),
            ("server", "Example.azurecr.io"),
            ("username", "pull:er"),
            ("username", ""),
        ] {
            let mut invalid = pulling.clone();
            invalid["hetzner"]["registry_pull"][field] = serde_json::json!(value);
            std::fs::write(&path, invalid.to_string()).unwrap();
            assert!(Settings::load(&path).is_err(), "{field}={value}");
        }
        pulling["hetzner"]["registry_pull"]["password_file"] = serde_json::json!("relative");
        std::fs::write(&path, pulling.to_string()).unwrap();
        assert!(Settings::load(&path).is_err());
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

    #[test]
    fn legacy_false_does_not_disable_completed_publishing_or_rewrite_bindings_on_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let token = serde_json::json!({"file":root.path().join("absent-agent-token"),"saved_at":0});
        let mut raw = serde_json::json!({
            "runpod_key_file":root.path().join("absent-compute"),
            "ssh_identity_file":root.path().join("absent-identity"),
            "docker_config":root.path().join("absent-docker"),
            "cpu_flavors":[],"gpu_types":[],"claude_token":token
        });
        for legacy in [Some(false), Some(true), None] {
            raw.as_object_mut().unwrap().remove("portable_image_publishing");
            if let Some(value) = legacy {
                raw["portable_image_publishing"] = value.into();
            }
            let before = serde_json::to_vec(&raw).unwrap();
            std::fs::write(&path, &before).unwrap();
            let loaded = Settings::load(&path).unwrap();
            assert!(
                loaded.portable_image_publishing,
                "old staging state must not disable the completed flow"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "reading never migrates machine settings in place"
            );
            let saved = serde_json::to_value(loaded).unwrap();
            assert!(saved.get("portable_image_publishing").is_none());
            assert_eq!(saved["claude_token"], raw["claude_token"]);
            assert!(!root.path().join("absent-agent-token").exists());
        }
        for invalid in [serde_json::Value::Null, "false".into(), 0.into()] {
            raw["portable_image_publishing"] = invalid;
            let before = serde_json::to_vec(&raw).unwrap();
            std::fs::write(&path, &before).unwrap();
            assert!(Settings::load(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert!(
            super::super::setup::load_settings(&root.path().join("fresh"))
                .unwrap()
                .portable_image_publishing
        );
        assert!(
            super::super::setup::Draft::load(&root.path().join("fresh-draft"))
                .unwrap()
                .settings
                .portable_image_publishing
        );
    }
}
